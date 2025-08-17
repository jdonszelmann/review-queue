#![allow(dead_code)]

use std::sync::Arc;

use octocrab::Octocrab;

use crate::{AppState, model::RepoInfo};

#[derive(Clone, Debug)]
pub struct LoginContext {
    pub username: String,
    pub repos: Vec<RepoInfo>,
    pub octocrab: Octocrab,
    pub state: Arc<AppState>,
}
