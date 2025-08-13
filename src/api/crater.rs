use std::collections::HashMap;

use color_eyre::eyre::Context;
use jiff::Timestamp;
use scraper::{ElementRef, Html, Selector};

use crate::model::CraterStatus;

pub async fn get_crater_queue() -> color_eyre::Result<HashMap<u64, CraterStatus>> {
    let url = "https://crater.rust-lang.org/";
    // tracing::info!("reading crater page at {url}");

    let mut results = HashMap::new();
    let response = reqwest::get(url).await?;
    let body = response.text().await.context("body")?;

    let document = Html::parse_document(&body);
    let mut number_in_queue = 0;

    let row_selector = Selector::parse("table.list tbody tr").unwrap();
    for row in document.select(&row_selector) {
        let children = row
            .children()
            .filter_map(ElementRef::wrap)
            .collect::<Vec<_>>();

        let number = children[0].text().collect::<String>();
        let status = children[5].text().collect::<String>();

        // header row
        if number == "Name" {
            continue;
        }

        let Ok(number) = number.trim().trim_start_matches("pr-").parse::<u64>() else {
            tracing::error!("parse PR number: {}", number.trim().trim_end_matches("pr-"));
            continue;
        };

        let status = match status.trim() {
            x if x.starts_with("Running") => CraterStatus::Running {
                expected_end: Timestamp::now(),
            },
            "Generating report" => CraterStatus::GeneratingReport,
            "Queued" => {
                number_in_queue += 1;
                CraterStatus::Queued {
                    num_before: number_in_queue,
                }
            }
            other => {
                tracing::error!("weird status: '{other}'");
                continue;
            }
        };

        results.insert(number, status);
    }
    Ok(results)
}
