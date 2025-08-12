use axum::{Router, routing::get};
use color_eyre::eyre::Context;
use jiff::Timestamp;
use std::{env, sync::Arc, time::Duration};
use tokio::sync::Mutex;
use tokio::{spawn, time::sleep};
use tower_http::services::ServeDir;
use url::Url;

use crate::api::github::get_prs;
use crate::model::{BackendStatus, Config, Pr, Repo};
use crate::ui::home;

mod api;
mod model;
mod ui;

const REFRESH_RATE: Duration = Duration::from_secs(60);

struct AppState {
    config: Config,
    prs: Vec<Pr>,
    status: BackendStatus,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            prs: Vec::new(),
            status: BackendStatus::Uninitialized,
        }
    }
}

async fn update_state(state: Arc<Mutex<AppState>>) {
    loop {
        let config = {
            let mut state = state.lock().await;
            state.status = BackendStatus::Refreshing;
            state.config.clone()
        };

        match get_prs(config).await {
            Err(e) => tracing::error!("{e}"),
            Ok(prs) => {
                let mut state = state.lock().await;
                state.status = BackendStatus::Idle {
                    last_refresh: Timestamp::now(),
                };
                state.prs = prs;
            }
        }

        sleep(REFRESH_RATE).await;
    }
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    dotenvy::dotenv().context("get dotenv")?;
    tracing_subscriber::fmt::init();

    let config = Config {
        username: "jdonszelmann".to_string(),
        // username: "BoxyUwU".to_string(),
        token: env::var("GITHUB_TOKEN").context("get `GITHUB_TOKEN` envvar")?,
        repos: vec![Repo {
            owner: "rust-lang".to_string(),
            name: "rust".to_string(),
            bors_queue_url: Some(Url::parse("https://bors.rust-lang.org/queue/rust").unwrap()),
        }],
    };

    let state = Arc::new(Mutex::new(AppState::new(config)));

    spawn(update_state(state.clone()));

    // for i in res {
    //     let name = format!("{}/{}", i.repo.owner, i.repo.name);
    //     println!("{name:<10}#{} {}", i.number, i.title);
    // }

    // build our application with a single route
    let app = Router::new()
        .route("/", get(home))
        .with_state(state)
        .nest_service("/assets/", ServeDir::new("assets"));
    let address = "0.0.0.0:3000";

    let listener = tokio::net::TcpListener::bind(address).await.unwrap();

    tracing::info!("listening on http://{address}");

    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();

    Ok(())
}
