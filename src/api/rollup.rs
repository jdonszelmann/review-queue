use color_eyre::eyre::Context;
use octocrab::Octocrab;

use crate::{
    api::{bors::BorsQueue, github::get_pr},
    model::Repo,
};

#[derive(Debug, Clone)]
#[allow(unused)]
pub struct Rollup {
    pub pr_number: u64,
    pub running: bool,
    pub position_in_queue: usize,
    pub pr_numbers: Vec<u64>,
}

#[derive(Default, Debug, Clone)]
pub struct RollupQueue {
    pub rollups: Vec<Rollup>,
}

pub async fn find_rollups(
    octocrab: Octocrab,
    repo: Repo,
    bors_queue: &BorsQueue, // pr_number: u64,
) -> color_eyre::Result<RollupQueue> {
    let mut res = RollupQueue::default();

    for pr in &bors_queue.items {
        if !pr.title.starts_with("Rollup of") {
            continue;
        }

        let gh_pr = get_pr(&octocrab, repo.clone(), pr.pr_number)
            .await
            .context("get PR")?;

        let Some(body) = gh_pr.body else {
            tracing::error!("no body");
            continue;
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

        res.rollups.push(Rollup {
            pr_number: pr.pr_number,
            running: pr.position_in_queue == 1,
            position_in_queue: pr.position_in_queue,
            pr_numbers,
        });
    }

    Ok(res)
}
