use std::collections::HashMap;

use color_eyre::eyre::Context;
use scraper::{ElementRef, Html, Selector};
use url::Url;

#[derive(Debug, Clone, PartialEq)]
pub enum BorsStatus {
    None,
    Approved,
    Pending,
    Failure,
    Error,
    Success,
    Other(String),
}

#[derive(Debug, Clone)]
pub enum RollupSetting {
    Never,
    Always,
    Iffy,
    Unset,
}

#[derive(Debug, Clone)]
pub struct BorsInfo {
    approver: String,
    pub status: BorsStatus,
    pub mergeable: bool,
    pub rollup_setting: RollupSetting,
    pub priority: u64,
}

pub async fn get_bors_queue(url: Url) -> color_eyre::Result<HashMap<u64, BorsInfo>> {
    tracing::info!("reading bors page at {url}");
    let mut results = HashMap::new();
    let response = reqwest::get(url).await?;
    let body = response.text().await.context("body")?;

    let document = Html::parse_document(&body);

    let row_selector = Selector::parse("#queue tbody tr").unwrap();
    for row in document.select(&row_selector) {
        let children = row
            .children()
            .filter_map(ElementRef::wrap)
            .collect::<Vec<_>>();

        let number = children[2].text().collect::<String>();
        let status = children[3].text().collect::<String>();
        let mergeable = children[4].text().collect::<String>();
        let approver = children[8].text().collect::<String>();
        let priority = children[9].text().collect::<String>();
        let rollup = children[10].text().collect::<String>();

        let Ok(number) = number.trim().parse::<u64>() else {
            tracing::error!("parse PR number");
            continue;
        };

        let status = match status.trim() {
            "" => BorsStatus::None,
            "error" => BorsStatus::Error,
            "failure" => BorsStatus::Failure,
            "approved" => BorsStatus::Approved,
            "pending" => BorsStatus::Pending,
            other => BorsStatus::Other(other.to_string()),
        };

        let mergeable = match mergeable.trim() {
            "" => continue,
            "yes" => true,
            "no" => false,
            other => {
                tracing::error!("weird mergeable status: {other}");
                continue;
            }
        };

        let rollup_status = match rollup.trim() {
            "" => RollupSetting::Unset,
            "never" => RollupSetting::Never,
            "always" => RollupSetting::Always,
            "iffy" => RollupSetting::Iffy,
            other => {
                tracing::error!("weird rollup status: {other}");
                continue;
            }
        };

        let Ok(priority) = priority.trim().parse::<u64>() else {
            tracing::error!("parse priority: {}", priority.trim());
            continue;
        };

        let info = BorsInfo {
            approver: approver.trim().to_string(),
            status,
            mergeable,
            rollup_setting: rollup_status,
            priority,
        };
        results.insert(number, info);
    }

    Ok(results)
}
