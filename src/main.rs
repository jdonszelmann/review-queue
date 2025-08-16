use axum::{
    Router,
    routing::{any, get},
};
use color_eyre::eyre::Context;
use rust_query::{Database, IntoExpr, Update};
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::PathBuf;
use std::{env, sync::Arc, time::Duration};
use tokio::sync::{OnceCell, RwLock};
use tower_http::services::ServeDir;

use crate::db::Issue;
use crate::{api::crater::get_crater_queue, db::Schema};
use crate::{
    api::{Cache, github::scrape_pr_data},
    model::CraterStatus,
};
use crate::{
    db::User,
    model::{LoginContext, Pr},
};

mod api;
mod db;
mod model;
mod pages;

const REFRESH_RATE: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct Config {
    pub assets_dir: String,
    pub db_path: String,
    pub host: String,
    pub oauth_client_id: String,
    pub oauth_client_secret: String,
}

#[derive(Default)]
struct UserState {
    prs: OnceCell<Vec<Pr>>,
    old: Vec<Pr>,
}

struct AppState {
    db: Database<Schema>,
    config: Config,

    crater_info: Cache<'static, HashMap<u64, CraterStatus>>,

    users_prs_by_username: RwLock<HashMap<String, UserState>>,
}

async fn get_state_instantly(config: Arc<LoginContext>) -> Vec<Pr> {
    let state = config.state.users_prs_by_username.read().await;

    state
        .get(&config.username)
        .map(|i| i.prs.get().cloned().unwrap_or_else(|| i.old.clone()))
        .unwrap_or_default()
}

async fn update_prs_database(prs: &[Pr], config: Arc<LoginContext>) {
    config.state.db.transaction_mut_ok(|txn| {
        let user_row = txn
            .query_one(User::unique(&config.username))
            .expect("logged in");

        txn.update_ok(
            user_row,
            User {
                sequence_number: Update::add(1),
                ..Default::default()
            },
        );

        // Make an `Expr` from the `TableRow` so that we can get an `Expr` for the `sequence_number`.
        let user = user_row.into_expr();
        for pr in prs {
            let res = txn.insert(db::Issue {
                number: pr.number as i64,
                user: user_row,
                last_seen_sequence_number: &user.sequence_number,
            });

            if let Err(existing_row) = res {
                txn.update_ok(
                    existing_row,
                    Issue {
                        last_seen_sequence_number: Update::set(&user.sequence_number),
                        ..Default::default()
                    },
                );
            }
        }
    });
}

async fn get_and_update_state(config: Arc<LoginContext>) -> Vec<Pr> {
    tracing::info!("refreshing for user {}", config.username);

    {
        let mut state = config.state.users_prs_by_username.write().await;
        let data = state.entry(config.username.clone()).or_default();
        data.old = data.prs.take().unwrap_or_default();
    };

    let state = config.state.users_prs_by_username.read().await;
    let user_state = state.get(&config.username).expect("just inserted");

    user_state
        .prs
        .get_or_init(async || match scrape_pr_data(config.clone()).await {
            Err(e) => {
                tracing::error!("{e}");
                Vec::new()
            }
            Ok(prs) => {
                update_prs_database(&prs, config.clone()).await;
                prs
            }
        })
        .await
        .clone()
}

impl Debug for AppState {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("AppState").finish()
    }
}

impl AppState {
    pub fn new(db: Database<Schema>, config: Config) -> Self {
        Self {
            db,
            users_prs_by_username: RwLock::new(HashMap::new()),
            config,
            crater_info: Cache::new(
                async || {
                    tracing::info!("reloading crater");
                    match get_crater_queue().await {
                        Ok(i) => i,
                        Err(e) => {
                            tracing::error!("crater error: {e}");
                            Default::default()
                        }
                    }
                },
                Duration::from_secs(60 * 10),
            ),
        }
    }
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    dotenvy::dotenv().ok();
    tracing_subscriber::fmt::init();

    let config = Config {
        db_path: env::var("DB_PATH").context("get `DB_PATH` envvar")?,
        assets_dir: env::var("ASSETS_DIR").unwrap_or("./assets".to_string()),
        host: env::var("HOST").context("get `HOST` envvar")?,
        oauth_client_id: env::var("OAUTH_CLIENT_ID").context("get `OAUTH_CLIENT_ID` envvar")?,
        oauth_client_secret: env::var("OAUTH_CLIENT_SECRET")
            .context("get `OAUTH_CLIENT_SECRET` envvar")?,
    };

    let db = db::migrate(PathBuf::from(config.db_path.clone()));

    // build our application with a single route
    let app = Router::new()
        // auth routes
        .route("/auth/github/login", get(pages::auth::login))
        .route("/auth/github/callback", get(pages::auth::callback))
        .route("/logout", get(pages::auth::logout))
        // home page
        .route("/", get(pages::home::home_page))
        // queue page
        .route("/queue", get(pages::queue::queue_page))
        .route("/queue/ws", any(pages::queue::queue_ws))
        // rest
        .with_state(Arc::new(AppState::new(db, config.clone())))
        .nest_service("/assets/", ServeDir::new(config.assets_dir.clone()));

    let address = "0.0.0.0:3000";

    let listener = tokio::net::TcpListener::bind(address).await.unwrap();

    tracing::info!("listening on http://{address}");

    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();

    Ok(())
}
