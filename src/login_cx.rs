#![allow(dead_code)]

use std::sync::Arc;

use octocrab::Octocrab;
use rust_query::Update;
use tokio::{sync::Mutex, task::spawn_blocking};

use crate::{AppState, db::User, model::RepoInfo};

#[derive(Debug)]
pub struct LoginContext {
    pub base_username: String,
    pub current_username: Mutex<String>,

    pub repos: Vec<RepoInfo>,
    pub octocrab: Octocrab,
    pub state: Arc<AppState>,
}

impl LoginContext {
    pub async fn change_username(&self, new_username: String) {
        let mut current = self.current_username.lock().await;
        *current = new_username.clone();
        // keep the lock alive until the db is updated

        let state = self.state.clone();
        let base_username = self.base_username.clone();

        spawn_blocking(move || {
            state.db.transaction_mut_ok(|txn| {
                if let Some(user) = txn.query_one(User::unique(base_username)) {
                    txn.update_ok(
                        user,
                        User {
                            current_username: Update::set(new_username),
                            ..Default::default()
                        },
                    );
                }
            });
        })
        .await
        .unwrap();
    }

    pub async fn username(&self) -> String {
        self.current_username.lock().await.clone()
    }
}
