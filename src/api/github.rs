use std::{future::ready, sync::Arc, time::Duration};

use color_eyre::eyre::{Context, ContextCompat};
use futures::{SinkExt, StreamExt, stream};
use octocrab::{
    Octocrab,
    models::{
        issues::Issue,
        pulls::{MergeableState, PullRequest},
    },
    params,
};
use reqwest::StatusCode;
use serde::Deserialize;
use tokio::{spawn, time::sleep};

use futures::channel::mpsc::channel;
use url::Url;

use crate::{
    login_cx::LoginContext,
    model::{self, Author, IssueOrPr, Repo},
    sort::{PredeterminedCategory, convert_author, sort},
};

pub enum PrSource {
    Mentioned,
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
) -> impl StreamExt<Item = IssueOrPr> {
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
                    // and all subscribed issues
                    .chain(subscribed_issues(
                        repo.repo.clone(),
                        username.clone(),
                        login_context.clone(),
                    ))
                    .zip(stream::repeat(repo))
            }
        })
        // flattened
        .flatten()
        // get their PR object from github
        .map({
            let login_context = login_context.clone();
            move |((issue, source), repo)| {
                let login_context = login_context.clone();

                async move {
                    if issue.pull_request.is_some() {
                        match source {
                            PrSource::Mentioned => {
                                Some((issue, repo, PredeterminedCategory::Mentioned))
                            }
                            PrSource::Direct => {
                                match get_pr_with_related_issues(
                                    &login_context.octocrab,
                                    repo.repo.clone(),
                                    issue.number,
                                )
                                .await
                                {
                                    Ok(pr) => Some((issue, repo, PredeterminedCategory::Pr(pr))),
                                    Err(e) => {
                                        tracing::error!("error getting PR: {e}");
                                        None
                                    }
                                }
                            }
                        }
                    } else {
                        Some((issue, repo, PredeterminedCategory::Issue))
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

#[derive(Clone, Debug)]
pub struct RawPullRequestData {
    pub draft: bool,
    pub mergeable: Option<bool>,
    pub mergeable_state: MergeableState,
    pub issues: Vec<model::Issue>,
}

pub async fn get_pr_with_related_issues(
    octocrab: &Octocrab,
    repo: Repo,
    pr_number: u64,
) -> color_eyre::Result<RawPullRequestData> {
    let res: serde_json::Value = octocrab
        .graphql(&serde_json::json!({ "query": format!("
        {{
            resource(url: \"https://github.com/{}/{}/pull/{pr_number}\") {{
                ... on PullRequest {{
                    isDraft
                    mergeable
                    mergeStateStatus
                    closingIssuesReferences(first: 100) {{
                        nodes {{
                            number
                            title
                            body
                            author {{
                                login
                                avatarUrl
                                url
                            }}
                            createdAt
                            url
                        }}
                    }}
                }}
            }}
        }}", repo.owner, repo.name)
        }))
        .await?;

    let deserialize = || -> Option<RawPullRequestData> {
        let resource = res.get("data")?.get("resource")?;

        let draft = resource.get("isDraft")?.as_bool()?;
        let mergeable = match resource.get("mergeable")?.as_str()? {
            "CONFLICTING" => Some(false),
            "MERGEABLE" => Some(true),
            _ => None,
        };
        let mergeable_state = match resource.get("mergeStateStatus")?.as_str()? {
            "DIRTY" => MergeableState::Dirty,
            "UNKNOWN" => MergeableState::Unknown,
            "BLOCKED" => MergeableState::Blocked,
            "BEHIND" => MergeableState::Behind,
            "DRAFT" => MergeableState::Draft,
            "UNSTABLE" => MergeableState::Unstable,
            "HAS_HOOKS" => MergeableState::HasHooks,
            "CLEAN" => MergeableState::Clean,
            _ => MergeableState::Unknown,
        };

        let mut issues = Vec::new();
        for i in resource
            .get("closingIssuesReferences")?
            .get("nodes")?
            .as_array()?
        {
            let author = i.get("author")?;
            let title = i.get("title")?.as_str()?.to_string();
            let description = Some(i.get("body")?.as_str()?.to_string());
            let number = i.get("number")?.as_u64()?;
            let link = Url::parse(i.get("url")?.as_str()?).ok()?;
            let created = i.get("createdAt")?.as_str()?.parse().ok()?;

            let name = author.get("login")?.as_str()?.to_string();
            let avatar_url = Url::parse(author.get("avatarUrl")?.as_str()?).ok()?;
            let profile_url = Url::parse(author.get("url")?.as_str()?).ok()?;

            issues.push(model::Issue {
                repo: repo.clone(),
                title,
                description,
                number,
                link,
                author: model::Author {
                    name,
                    avatar_url,
                    profile_url,
                },
                assigned: Vec::new(),
                me_assigned: false,
                created,
            });
        }

        Some(RawPullRequestData {
            draft,
            mergeable,
            mergeable_state,
            issues,
        })
    };

    Ok(deserialize().context(format!("deserialize: {res:?}"))?)
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
    Mentioned(String),
}

fn subscribed_issues(
    repo: Repo,
    username: String,
    login_context: Arc<LoginContext>,
) -> impl StreamExt<Item = (Issue, PrSource)> {
    read_paginated_issues(
        login_context.octocrab.clone(),
        repo,
        IssueKind::Mentioned(username),
    )
    .map(|i| (i, PrSource::Mentioned))
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
                IssueKind::Mentioned(username) => list.mentioned(username.as_str()),
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
