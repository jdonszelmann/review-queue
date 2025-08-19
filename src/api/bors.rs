use color_eyre::eyre::Context;
use scraper::{ElementRef, Html, Selector};
use url::Url;

use crate::model::RollupSetting;

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
#[allow(unused)]
pub struct BorsPr {
    pub pr_number: u64,
    pub approver: String,
    pub status: BorsStatus,
    pub mergeable: bool,
    pub rollup_setting: RollupSetting,
    pub priority: u64,
    pub title: String,
    pub position_in_queue: usize,
    pub running: bool,
}

#[derive(Debug, Clone, Default)]
pub struct BorsQueue {
    pub items: Vec<BorsPr>,
}

impl BorsQueue {
    pub fn for_pr(&self, pr_number: u64) -> Option<&BorsPr> {
        self.items.iter().find(|i| i.pr_number == pr_number)
    }
}

pub async fn get_bors_info(url: Url) -> color_eyre::Result<BorsQueue> {
    tracing::info!("requesting bors");

    let mut prs = Vec::new();

    let response = reqwest::get(url).await.context("get bors info")?;
    let body = response.text().await.context("body")?;

    {
        let document = Html::parse_document(&body);

        let mut position_in_queue = 0;

        let row_selector = Selector::parse("#queue tbody tr").unwrap();
        for row in document.select(&row_selector) {
            position_in_queue += 1;

            let children = row
                .children()
                .filter_map(ElementRef::wrap)
                .collect::<Vec<_>>();

            let number = children[2].text().collect::<String>();
            let status = children[3].text().collect::<String>();
            let mergeable = children[4].text().collect::<String>();
            let title = children[5].text().collect::<String>();
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
                "success" => BorsStatus::Success,
                other => BorsStatus::Other(other.to_string()),
            };

            let mergeable = match mergeable.trim() {
                "" => {
                    tracing::warn!("mergable empty");
                    true
                }
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

            let res = BorsPr {
                pr_number: number,
                approver: approver.trim().to_string(),
                status,
                mergeable,
                rollup_setting: rollup_status,
                priority,
                title: title.trim().to_string(),
                position_in_queue: position_in_queue,
                running: position_in_queue == 1,
            };

            prs.push(res);
        }
    }

    Ok(BorsQueue { items: prs })
}
