use std::{future::ready, sync::Arc, time::Duration};

use color_eyre::eyre::Context;
use futures::{SinkExt, StreamExt, stream};
use octocrab::{
    Octocrab,
    models::{issues::Issue, pulls::PullRequest},
    params,
};
use reqwest::StatusCode;
use tokio::{spawn, time::sleep};

use futures::channel::mpsc::channel;
use url::Url;

use crate::{
    login_cx::LoginContext,
    model::{Author, Pr, Repo},
    sort::{PredeterminedCategory, convert_author, sort},
};

pub enum PrSource {
    Subscribed,
    Direct,
}

async fn try_username_suggestions(
    login_context: Arc<LoginContext>,
    current: String,
) -> color_eyre::Result<Vec<Author>> {
    if current.len() > 3 {
        let url = format!("https://github.com/{current}");
        let res = reqwest::get(&url).await.context("reqwest")?;

        return Ok(if res.status() == StatusCode::OK {
            vec![Author {
                name: current,
                id: 0,
                avatar_url: Url::parse(&format!("{url}.png")).unwrap(),
                profile_url: Url::parse(&url).unwrap(),
            }]
        } else {
            vec![]
        });
    }

    let res = login_context
        .octocrab
        .search()
        .users(&format!("{current} type:user"))
        .per_page(10)
        .send()
        .await
        .context("search request")?;

    Ok(res.items.iter().map(convert_author).collect())
}

pub async fn username_suggestions(
    login_context: Arc<LoginContext>,
    current: String,
) -> Vec<Author> {
    match try_username_suggestions(login_context, current).await {
        Ok(i) => i,
        Err(e) => {
            tracing::error!("error getting username suggestions:\n{e:#}");
            Default::default()
        }
    }
}

pub fn scrape_github_for_user(
    login_context: Arc<LoginContext>,
    username: String,
) -> impl StreamExt<Item = Pr> {
    stream::iter(login_context.repos.clone())
        // for each repo
        .map({
            let login_context = login_context.clone();
            let username = username.clone();
            move |repo| {
                // all assigned issues
                assigned_issues(repo.repo.clone(), username.clone(), login_context.clone())
                    // and all own issues
                    .chain(own_issues(
                        repo.repo.clone(),
                        username.clone(),
                        login_context.clone(),
                    ))
                    // .chain(subscribed_issues(repo.repo.clone(), login_context.clone()))
                    .zip(stream::repeat(repo))
            }
        })
        // flattened
        .flatten()
        // only the issues that are actually PRs
        .filter(|((issue, _), _)| ready(issue.pull_request.is_some()))
        // get their PR object from github
        .map({
            let login_context = login_context.clone();
            move |((issue, source), repo)| {
                let login_context = login_context.clone();

                async move {
                    match source {
                        PrSource::Subscribed => {
                            Some((issue, repo, PredeterminedCategory::Subscribed))
                        }
                        PrSource::Direct => {
                            match get_pr(&login_context.octocrab, repo.repo.clone(), issue.number)
                                .await
                            {
                                Ok(pr) => Some((issue, repo, PredeterminedCategory::None(pr))),
                                Err(e) => {
                                    tracing::error!("error getting PR: {e}");
                                    None
                                }
                            }
                        }
                    }
                }
            }
        })
        // paralellized
        .buffer_unordered(100)
        // filter out the ones where we couldn't get a PR object from github
        .filter_map(|i| ready(i))
        // sort them into our own data structures
        .map(move |(issue, repo, predetermined_category)| {
            let login_context = login_context.clone();
            let username = username.clone();
            async move {
                sort(
                    &login_context,
                    username,
                    &repo,
                    &issue,
                    predetermined_category,
                )
                .await
            }
        })
        .buffer_unordered(100)
        .filter_map(|i| ready(i))
}

pub async fn get_pr(
    octocrab: &Octocrab,
    repo: Repo,
    pr_number: u64,
) -> Result<PullRequest, octocrab::Error> {
    octocrab.pulls(&repo.owner, &repo.name).get(pr_number).await
}

enum IssueKind {
    Own(String),
    Assigned(String),
    Subscribed,
}

fn subscribed_issues(
    repo: Repo,
    login_context: Arc<LoginContext>,
) -> impl StreamExt<Item = (Issue, PrSource)> {
    read_paginated_issues(login_context.octocrab.clone(), repo, IssueKind::Subscribed)
        .map(|i| (i, PrSource::Subscribed))
}

fn own_issues(
    repo: Repo,
    username: String,
    login_context: Arc<LoginContext>,
) -> impl StreamExt<Item = (Issue, PrSource)> {
    read_paginated_issues(
        login_context.octocrab.clone(),
        repo,
        IssueKind::Own(username),
    )
    .map(|i| (i, PrSource::Direct))
}

fn assigned_issues(
    repo: Repo,
    username: String,
    login_context: Arc<LoginContext>,
) -> impl StreamExt<Item = (Issue, PrSource)> {
    read_paginated_issues(
        login_context.octocrab.clone(),
        repo,
        IssueKind::Assigned(username),
    )
    .map(|i| (i, PrSource::Direct))
}

fn read_paginated_issues(
    octocrab: Octocrab,
    repo: Repo,
    issue_kind: IssueKind,
) -> impl StreamExt<Item = Issue>
where
{
    let (mut tx, rx) = channel::<Issue>(0);

    spawn(async move {
        let mut ctr = 0;
        let mut initial_page = loop {
            let list = octocrab.issues(repo.owner.clone(), repo.name.clone());
            let list = list.list().state(params::State::Open).per_page(100);
            let list = match &issue_kind {
                IssueKind::Own(username) => list.creator(username),
                IssueKind::Assigned(username) => list.assignee(username.as_str()),
                IssueKind::Subscribed => list.filter("subscribed"),
            };

            let page = match list.send().await {
                Ok(i) => i,
                Err(e) => {
                    tracing::error!("{e}");
                    return;
                }
            };

            if page.total_count.is_none() && page.items.is_empty() {
                // let rate_limit = octocrab.ratelimit().get().await.context("rate limit")?;
                tracing::debug!("waiting...");
                ctr += 1;
                sleep(Duration::from_millis(50)).await;

                if ctr == 20 {
                    tracing::error!("no issues after trying 20 times");
                    return;
                }

                continue;
            }

            break page;
        };

        loop {
            let next = initial_page.next.clone();

            tx.send_all(&mut stream::iter(initial_page.items).map(Result::Ok))
                .await
                .unwrap();

            initial_page = match octocrab.get_page::<Issue>(&next).await {
                Ok(Some(next_page)) => next_page,
                Ok(None) => break,
                Err(e) => {
                    tracing::error!("error getting next page: {e}");
                    break;
                }
            }
        }
    });

    rx
}
