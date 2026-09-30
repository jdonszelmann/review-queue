use std::collections::HashMap;

use color_eyre::eyre::Context;
use serde::Deserialize;
use url::Url;

#[derive(Debug, Clone, PartialEq, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum BorsStatus {
    Closed,
    Draft,
    Merged,
    Open,
    #[serde(untagged)]
    Other(String),
}

#[derive(Debug, Clone, Deserialize)]
#[allow(unused)]
pub struct BorsApiPr {
    pub number: Option<u64>,
    pub author: Option<String>,
    pub approver: Option<String>,
    pub status: BorsStatus,
    pub priority: Option<u64>,
    pub title: String,

    #[serde(flatten)]
    other: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone)]
pub struct BorsPr {
    pub number: u64,
    pub author: String,
    pub approver: String,
    pub status: BorsStatus,
    pub priority: u64,
    pub title: String,

    pub position_in_queue: usize,
    pub running: bool,
    pub url: Url,
}

#[derive(Debug, Clone, Default)]
pub struct BorsQueue {
    pub items: Vec<BorsPr>,
}

impl BorsQueue {
    pub fn for_pr(&self, pr_number: u64) -> Option<&BorsPr> {
        self.items.iter().find(|i| i.number == pr_number)
    }
}

pub async fn get_bors_info(url: Url) -> color_eyre::Result<BorsQueue> {
    tracing::info!("requesting bors");

    let response = reqwest::get(url.clone()).await.context("get bors info")?;
    let items: Vec<BorsApiPr> = response.json().await.context("body")?;

    println!("{:?}", items.iter().map(|i| &i.status).collect::<Vec<_>>());
    Ok(BorsQueue {
        items: items
            .into_iter()
            .enumerate()
            .filter_map(|(idx, api)| {
                let number = api.number?;
                Some(BorsPr {
                    position_in_queue: idx + 1,
                    running: idx == 0,
                    url: Url::parse(&format!(
                        "https://github.com/rust-lang/rust/issues/{number}",
                    ))
                    .unwrap(),
                    number,
                    author: api.author.unwrap_or_default(),
                    approver: api.approver.unwrap_or_default(),
                    status: api.status,
                    priority: api.priority.unwrap_or(0),
                    title: api.title,
                })
            })
            .collect(),
    })
}
