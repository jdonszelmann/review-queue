use std::{collections::HashMap, sync::Arc};

use color_eyre::eyre::Context;
use scraper::{ElementRef, Html, Selector};
use url::Url;

use crate::{
    api::github::pr_info,
    model::{LoginContext, Repo},
};

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
#[allow(unused)]
pub struct BorsInfo {
    pub approver: String,
    pub status: BorsStatus,
    pub mergeable: bool,
    pub rollup_setting: RollupSetting,
    pub priority: u64,
    pub title: String,
    pub position_in_queue: usize,
    pub running: bool,
}

#[derive(Debug, Clone)]
#[allow(unused)]
pub struct Rollup {
    pub pr_number: u64,
    pub running: bool,
    pub position_in_queue: usize,
    pub pr_numbers: Vec<u64>,
}

#[derive(Debug, Clone, Default)]
pub struct AllBorsInfo {
    pub prs: HashMap<u64, BorsInfo>,
    pub rollups: Vec<Rollup>,
}

pub async fn get_bors_queue(
    config: Arc<LoginContext>,
    repo: Repo,
    url: Url,
) -> color_eyre::Result<AllBorsInfo> {
    // tracing::info!("reading bors page at {url}");

    let mut pr_numbers = Vec::new();
    let mut prs = HashMap::new();

    let response = reqwest::get(url).await?;
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

            if title.starts_with("Rollup of") {
                pr_numbers.push((number, position_in_queue));
            }

            let info = BorsInfo {
                approver: approver.trim().to_string(),
                status,
                mergeable,
                rollup_setting: rollup_status,
                priority,
                title: title.trim().to_string(),
                position_in_queue: position_in_queue,
                running: position_in_queue == 1,
            };
            prs.insert(number, info);
        }
    }

    let mut rollups = Vec::new();
    for (number, position_in_queue) in pr_numbers {
        rollups.extend(
            process_rollup_pr(config.clone(), repo.clone(), number, position_in_queue).await?,
        );
    }

    Ok(AllBorsInfo { prs, rollups })
}

pub async fn process_rollup_pr(
    config: Arc<LoginContext>,
    repo: Repo,
    number: u64,
    position_in_queue: usize,
) -> color_eyre::Result<Option<Rollup>> {
    let pr = pr_info(&config, &repo, number).await?;

    let Some(body) = pr.body else {
        tracing::error!("no body");
        return Ok(None);
    };

    let mut pr_numbers = Vec::new();

    for i in body.lines() {
        if let Some(line) = i.trim().strip_prefix("- ")
            && let Some((_repo, rest)) = line.split_once("#")
            && let Some((number, _description)) = rest.split_once(" ")
            && let Ok(n) = number.parse()
        {
            pr_numbers.push(n);
        }
    }

    Ok(Some(Rollup {
        pr_numbers,
        running: position_in_queue == 1,
        position_in_queue,
        pr_number: number,
    }))
}
