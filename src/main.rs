use axum::response::Redirect;
use axum::{Router, routing::get};
use color_eyre::eyre::Context;
use jiff::Timestamp;
use rust_query::Database;
use std::collections::HashMap;
use std::fmt::Debug;
use std::path::PathBuf;
use std::{env, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio::{spawn, time::sleep};
use tower_http::services::ServeDir;
use url::Url;

use crate::api::github::get_prs;
use crate::db::Schema;
use crate::model::{BackendStatus, LoginContext, Pr, Repo};

mod api;
mod auth;
mod db;
mod home_page;
mod model;
mod queue_page;

const REFRESH_RATE: Duration = Duration::from_secs(60);

#[derive(Clone)]
pub struct Config {
    pub db_path: String,
    pub host: String,
    pub oauth_client_id: String,
    pub oauth_client_secret: String,
}

#[derive(Default)]
struct UserState {
    prs: Vec<Pr>,
    status: BackendStatus,
}

struct AppState {
    db: Database<Schema>,
    config: Config,

    users_prs_by_username: Mutex<HashMap<String, UserState>>,
}

async fn get_and_update_state(config: Arc<LoginContext>) -> Vec<Pr> {
    let prs = {
        let mut state = config.state.users_prs_by_username.lock().await;
        let data = state.entry(config.username.clone()).or_default();

        match data.status {
            BackendStatus::Idle { last_refresh }
                if Timestamp::now().duration_since(last_refresh).unsigned_abs() > REFRESH_RATE => {}
            BackendStatus::Uninitialized => {}
            BackendStatus::Refreshing | BackendStatus::Idle { .. } => return data.prs.clone(),
        }

        data.status = BackendStatus::Refreshing;
        data.prs.clone()
    };

    tracing::info!("refreshing for user {}", config.username);

    spawn(async move {
        match get_prs(config.clone()).await {
            Err(e) => tracing::error!("{e}"),
            Ok(prs) => {
                let mut state = config.state.users_prs_by_username.lock().await;
                let data = state.entry(config.username.clone()).or_default();

                data.status = BackendStatus::Idle {
                    last_refresh: Timestamp::now(),
                };
                data.prs = prs;
                tracing::info!("done refreshing for user {}", config.username);
            }
        }
    });

    prs
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
            users_prs_by_username: Mutex::new(HashMap::new()),
            config,
        }
    }
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    dotenvy::dotenv().context("get dotenv")?;
    tracing_subscriber::fmt::init();

    let config = Config {
        db_path: env::var("DB_PATH").context("get `DB_PATH` envvar")?,
        host: env::var("HOST").context("get `HOST` envvar")?,
        oauth_client_id: env::var("OAUTH_CLIENT_ID").context("get `OAUTH_CLIENT_ID` envvar")?,
        oauth_client_secret: env::var("OAUTH_CLIENT_SECRET")
            .context("get `OAUTH_CLIENT_SECRET` envvar")?,
    };

    let db = db::migrate(PathBuf::from(config.db_path.clone()));

    // build our application with a single route
    let app = Router::new()
        .route("/", get(home_page::home_page))
        .route("/auth/github/login", get(auth::login))
        .route("/auth/github/callback", get(auth::callback))
        .route("/queue", get(queue_page::queue_page))
        .with_state(Arc::new(AppState::new(db, config)))
        .nest_service("/assets/", ServeDir::new("assets"));

    let address = "0.0.0.0:3000";

    let listener = tokio::net::TcpListener::bind(address).await.unwrap();

    tracing::info!("listening on http://{address}");

    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();

    Ok(())
}
