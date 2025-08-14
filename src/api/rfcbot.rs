use std::collections::HashMap;

use color_eyre::eyre::Context;
use jiff::civil::DateTime;
use serde::Deserialize;

#[derive(Deserialize)]
pub struct Issue {
    pub number: u64,
    pub repository: String,

    #[serde(flatten)]
    pub other: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
pub struct GitHubUser {
    pub id: i32,
    pub login: String,

    #[serde(flatten)]
    pub other: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
pub struct FcpProposal {
    pub disposition: String,
    pub fcp_start: Option<DateTime>,
    pub fcp_closed: bool,

    #[serde(flatten)]
    pub other: HashMap<String, serde_json::Value>,
}

#[derive(Deserialize)]
pub struct DeserializedFcpWithInfo {
    pub fcp: FcpProposal,
    pub reviews: Vec<(GitHubUser, bool)>,
    // (Concern name, comment registering it, and user leaving it)
    pub concerns: Vec<(String, serde_json::Value, GitHubUser)>,
    pub issue: Issue,

    #[serde(flatten)]
    pub other: HashMap<String, serde_json::Value>,
}

pub struct FcpInfo {
    pub proposal: FcpProposal,
    pub reviews: Vec<(GitHubUser, bool)>,
    pub concerns: Vec<(String, GitHubUser)>,
}

pub type FcpInfoAll = HashMap<u64, FcpInfo>;

const URL: &str = "https://crater.rust-lang.org/";

pub async fn get_fcp_info() -> color_eyre::Result<FcpInfoAll> {
    let response = reqwest::get(URL).await?;
    let body: Vec<DeserializedFcpWithInfo> = response.json().await.context("body")?;

    Ok(body
        .into_iter()
        .map(|fcp| {
            (
                fcp.issue.number,
                FcpInfo {
                    proposal: fcp.fcp,
                    reviews: fcp.reviews,
                    concerns: fcp.concerns.into_iter().map(|i| (i.0, i.2)).collect(),
                },
            )
        })
        .collect())
}
