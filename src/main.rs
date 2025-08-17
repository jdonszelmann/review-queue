use axum::{
    Router,
    routing::{any, get},
};
use color_eyre::eyre::Context;
use futures::StreamExt;
use octocrab::Octocrab;
use rust_query::{Database, IntoExpr, Update};
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::PathBuf;
use std::{env, sync::Arc, time::Duration};
use tokio::sync::{Mutex, OnceCell, RwLock};
use tower_http::services::ServeDir;

use crate::{
    api::{
        Cache,
        bors::{BorsQueue, get_bors_info},
        github::scrape_github_for_user,
        rollup::find_rollups,
    },
    model::{CraterStatus, Pr, RepoInfo},
};
use crate::{
    api::{crater::get_crater_queue, rfcbot::get_fcp_info},
    db::Schema,
};
use crate::{
    api::{rfcbot::FcpInfoAll, rollup::RollupQueue},
    db::Issue,
    model::Repo,
};
use crate::{db::User, login_cx::LoginContext};

mod api;
mod db;
mod login_cx;
mod model;
mod pages;
mod sort;

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

    bors_info: Mutex<HashMap<Repo, Cache<'static, BorsQueue>>>,
    rollup_info: Mutex<HashMap<Repo, Cache<'static, RollupQueue, Octocrab>>>,

    crater_info: Cache<'static, HashMap<u64, CraterStatus>>,
    fcp_info: Cache<'static, FcpInfoAll>,

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
        .get_or_init(async || {
            let pr_stream = scrape_github_for_user(config.clone());
            let prs: Vec<_> = pr_stream.collect().await;

            update_prs_database(&prs, config.clone()).await;

            prs
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
                    tracing::info!("reloading crater info");
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
            fcp_info: Cache::new(
                async || {
                    tracing::info!("reloading fcp info");
                    match get_fcp_info().await {
                        Ok(i) => i,
                        Err(e) => {
                            tracing::error!("crater error: {e}");
                            Default::default()
                        }
                    }
                },
                Duration::from_secs(30),
            ),
            bors_info: Mutex::new(HashMap::new()),
            rollup_info: Mutex::new(HashMap::new()),
        }
    }

    pub async fn bors_info(&self, repo: RepoInfo) -> Arc<BorsQueue> {
        let RepoInfo {
            repo,
            bors_queue_url: Some(url),
        } = repo
        else {
            return Arc::new(Default::default());
        };

        self.bors_info
            .lock()
            .await
            .entry(repo.clone())
            .or_insert_with(move || {
                let repo = repo.clone();
                let url = url.clone();
                Cache::new(
                    move || {
                        let repo = repo.clone();
                        let url = url.clone();
                        async move {
                            tracing::info!("reloading bors info for {repo}");
                            match get_bors_info(url.clone()).await {
                                Ok(i) => i,
                                Err(e) => {
                                    tracing::error!("bors queue error: {e}");
                                    Default::default()
                                }
                            }
                        }
                    },
                    Duration::from_secs(30),
                )
            })
            .get()
            .await
    }

    pub async fn rollup_info(
        self: Arc<Self>,
        repo: RepoInfo,
        octocrab: Octocrab,
    ) -> Arc<RollupQueue> {
        let this = self.clone();

        self.rollup_info
            .lock()
            .await
            .entry(repo.repo.clone())
            .or_insert_with(move || {
                let repo = repo.clone();
                let this = this.clone();
                Cache::new_with_param(
                    move |octocrab: Octocrab| {
                        let repo = repo.clone();
                        let octocrab = octocrab.clone();
                        let this = this.clone();
                        async move {
                            tracing::info!("reloading rollup info for {}", repo.repo);

                            let bors_queue = this.bors_info(repo.clone()).await;

                            match find_rollups(octocrab, repo.repo, &*bors_queue).await {
                                Ok(i) => i,
                                Err(e) => {
                                    tracing::error!("bors queue error: {e}");
                                    Default::default()
                                }
                            }
                        }
                    },
                    Duration::from_secs(60),
                )
            })
            .get_with_param(octocrab)
            .await
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
