use std::{collections::HashMap, sync::Arc, time::Duration};

use color_eyre::eyre::Context;
use octocrab::{
    Octocrab, Page,
    models::{
        self,
        issues::{Comment, Issue},
        pulls::{MergeableState, PullRequest},
    },
    params,
};
use tokio::{
    join, spawn,
    sync::{
        OnceCell, SetOnce,
        mpsc::{Sender, channel},
    },
    time::sleep,
};

use crate::{
    Config,
    api::{
        bors::{BorsInfo, BorsStatus, get_bors_queue},
        crater::get_crater_queue,
    },
    model::{
        CiStatus, CraterInfo, CraterStatus, FcpStatus, LoginContext, OwnPr, OwnPrStatus, Pr,
        PrReview, PrReviewStatus, PrStatus, QueuedStatus, Repo, SharedStatus,
    },
};

pub async fn get_prs(config: Arc<LoginContext>) -> color_eyre::Result<Vec<Pr>> {
    tracing::info!("logging in");
    sleep(Duration::from_secs(1)).await;

    let (tx, mut rx) = channel(16);

    spawn(async move {
        if let Err(e) = process_repos(config, tx.clone()).await {
            tx.send(Err(e)).await.unwrap();
        }
    });

    let mut res = Vec::new();

    while let Some(i) = rx.recv().await {
        match i {
            Ok(i) => {
                res.push(i);
            }
            Err(e) => {
                tracing::error!("{e}")
            }
        }
    }

    Ok(res)
}

async fn process_repos(
    config: Arc<LoginContext>,
    tx: Sender<color_eyre::Result<Pr>>,
) -> color_eyre::Result<()> {
    for repo in config.repos.clone() {
        let bors = if let Some(bors_url) = repo.bors_queue_url {
            let shared = Arc::new(SetOnce::new());

            let inner = shared.clone();
            spawn(async move {
                match get_bors_queue(bors_url).await {
                    Ok(i) => {
                        inner.set(i).unwrap();
                    }
                    Err(e) => {
                        tracing::error!("{e}");
                        inner.set(HashMap::new()).unwrap();
                    }
                }
            });

            Some(shared)
        } else {
            None
        };

        let crater = Arc::new(SetOnce::new());
        let inner = crater.clone();
        spawn(async move {
            match get_crater_queue().await {
                Ok(i) => {
                    inner.set(i).unwrap();
                }
                Err(e) => {
                    tracing::error!("{e}");
                    inner.set(HashMap::new()).unwrap();
                }
            }
        });

        tracing::info!("getting prs and issues for {}/{}", repo.owner, repo.name);

        let author = process_issues(
            config.clone(),
            async || {
                config
                    .octocrab
                    .clone()
                    .issues(&repo.owner, &repo.name)
                    .list()
                    .state(params::State::Open)
                    .creator(&config.username)
                    .per_page(50)
                    .send()
                    .await
                    .context("author issues")
            },
            bors.clone(),
            crater.clone(),
            tx.clone(),
            true,
        );

        let reviewer = process_issues(
            config.clone(),
            async || {
                config
                    .octocrab
                    .clone()
                    .issues(&repo.owner, &repo.name)
                    .list()
                    .state(params::State::Open)
                    .assignee(config.username.as_str())
                    .per_page(50)
                    .send()
                    .await
                    .context("author issues")
            },
            bors.clone(),
            crater.clone(),
            tx.clone(),
            false,
        );

        let (a, b) = join! {
            author, reviewer
        };

        a?;
        b?;
    }

    Ok(())
}

async fn process_issues<F: Future<Output = color_eyre::Result<Page<Issue>>>>(
    config: Arc<LoginContext>,
    issues: impl Fn() -> F,
    bors: Option<Arc<SetOnce<HashMap<u64, BorsInfo>>>>,
    crater: Arc<SetOnce<HashMap<u64, CraterStatus>>>,
    tx: Sender<color_eyre::Result<Pr>>,
    own_pr: bool,
) -> color_eyre::Result<()> {
    for repo in config.repos.clone() {
        tracing::info!("getting prs and issues for {}/{}", repo.owner, repo.name);

        let mut ctr = 0;
        let mut page = loop {
            let page = issues().await?;

            if page.total_count.is_none() && page.items.is_empty() {
                // let rate_limit = octocrab.ratelimit().get().await.context("rate limit")?;
                tracing::info!("waiting...");
                ctr += 1;
                sleep(Duration::from_millis(50)).await;

                if ctr == 20 {
                    return Ok(());
                }

                continue;
            }

            break page;
        };

        // Go through every page of issues. Warning: There's no rate limiting so
        // be careful.
        let mut n = 1;

        loop {
            tracing::info!("page {n}");
            n += 1;

            let next = page.next.clone();

            for issue in page {
                let tx = tx.clone();
                let repo = repo.clone();
                let bors = bors.clone();
                let crater = crater.clone();
                let config = config.clone();
                spawn(async move {
                    match process_pr(config, repo, bors, crater, issue, own_pr).await {
                        Ok(Some(i)) => {
                            tx.send(Ok(i)).await.unwrap();
                        }
                        Ok(None) => {}
                        Err(e) => {
                            tx.send(Err(e)).await.unwrap();
                        }
                    }
                });
            }
            page = match config.octocrab.get_page::<Issue>(&next).await? {
                Some(next_page) => next_page,
                None => break,
            }
        }
    }

    Ok(())
}

async fn comments(
    config: Arc<LoginContext>,
    repo: Repo,
    issue_number: u64,
) -> color_eyre::Result<Vec<Comment>> {
    tracing::info!(
        "getting comments for {}/{}#{}",
        repo.owner,
        repo.name,
        issue_number
    );

    let mut res = Vec::new();

    let mut page = loop {
        let page = config
            .octocrab
            .issues(&repo.owner, &repo.name)
            .list_comments(issue_number)
            .per_page(100)
            .send()
            .await?;

        if page.total_count.is_none() && page.items.is_empty() {
            tracing::info!("waiting...");
            sleep(Duration::from_millis(50)).await;
            continue;
        }

        break page;
    };

    // Go through every page of issues. Warning: There's no rate limiting so
    // be careful.
    let mut n = 1;

    loop {
        tracing::info!("page {n}");
        n += 1;

        let next = page.next.clone();

        for comment in page {
            res.push(comment);
        }
        page = match config
            .octocrab
            .get_page::<models::issues::Comment>(&next)
            .await?
        {
            Some(next_page) => next_page,
            None => break,
        }
    }

    Ok(res)
}

async fn pr_info(
    config: &Arc<LoginContext>,
    repo: &Repo,
    pr_number: u64,
) -> color_eyre::Result<PullRequest> {
    let pr = config
        .octocrab
        .pulls(&repo.owner, &repo.name)
        .get(pr_number)
        .await
        .context("get PR")?;

    Ok(pr)
}

async fn process_pr(
    config: Arc<LoginContext>,
    repo: Repo,
    bors: Option<Arc<SetOnce<HashMap<u64, BorsInfo>>>>,
    crater: Arc<SetOnce<HashMap<u64, CraterStatus>>>,
    issue: Issue,
    own_pr: bool,
) -> color_eyre::Result<Option<Pr>> {
    tracing::info!("processing {}/{} {}", repo.owner, repo.name, issue.number);
    let Some(_) = issue.pull_request else {
        return Ok(None);
    };

    let Some(body) = issue.body else {
        return Ok(None);
    };

    let pr = pr_info(&config, &repo, issue.number).await?;

    let comments_cell = OnceCell::new();
    let get_comments = {
        let repo = repo.clone();
        || comments_cell.get_or_try_init(|| comments(config, repo, issue.number))
    };

    let bors = if let Some(bors) = bors
        && let Some(bors_info) = bors.wait().await.get(&issue.number)
    {
        Some(bors_info.clone())
    } else {
        None
    };

    let shared_status = async || -> color_eyre::Result<Option<SharedStatus>> {
        if issue
            .labels
            .iter()
            .any(|i| i.name == "final-comment-period")
        {
            let comments = get_comments().await?;

            let mut fcp_start = None;

            for i in comments {
                if i.user.login == "rfcbot"
                    && i.body.as_ref().is_some_and(|i| {
                        i.contains("This is now entering its final comment period")
                    })
                {
                    fcp_start =
                        Some(jiff::Timestamp::from_second(i.created_at.timestamp()).unwrap());
                }
            }

            if let Some(start) = fcp_start {
                tracing::info!("fcp start at {start}");
                return Ok(Some(SharedStatus::Fcp(FcpStatus { start })));
            }
        }

        if issue.labels.iter().any(|i| i.name == "S-waiting-on-crater") {
            if let Some(status) = crater.wait().await.get(&issue.number) {
                return Ok(Some(SharedStatus::Crater(CraterInfo {
                    status: status.clone(),
                })));
            }
        }

        if issue.labels.iter().any(|i| i.name == "S-blocked") {
            return Ok(Some(SharedStatus::Blocked));
        }

        Ok(None)
    };

    let status = if let Some(bors) = &bors
        && (bors.status == BorsStatus::Approved || bors.status == BorsStatus::Pending)
    {
        PrStatus::Queued(QueuedStatus {
            // TODO: make this the bors approver
            approvers: issue.assignees.iter().map(|i| i.clone()).collect(),
            author: issue.user,
            rollup_setting: bors.rollup_setting.clone(),
        })
    } else if own_pr {
        // creator
        PrStatus::Own(OwnPr {
            status: if let Some(s) = shared_status().await? {
                OwnPrStatus::Shared(s)
            } else if issue.labels.iter().any(|i| i.name == "S-waiting-on-review") {
                OwnPrStatus::WaitingForReview
            } else {
                OwnPrStatus::Pending
            },
            reviewers: issue.assignees.iter().map(|i| i.clone()).collect(),
        })
    } else {
        // revieiwer
        PrStatus::Review(PrReview {
            status: if let Some(s) = shared_status().await? {
                PrReviewStatus::Shared(s)
            } else if issue.labels.iter().any(|i| i.name == "S-waiting-on-review") {
                PrReviewStatus::Review
            } else {
                PrReviewStatus::Author
            },
            author: issue.user,
        })
    };

    Ok(Some(Pr {
        repo: repo,
        number: issue.number,
        title: issue.title,
        description: body,
        link: issue.html_url,
        perf_runs: Vec::new(),
        crater_runs: Vec::new(),
        associated_issues: Vec::new(),
        draft: pr.draft.is_some_and(|i| i),
        status,
        ci_state: format!(
            "{:?}",
            pr.mergeable_state
                .clone()
                .unwrap_or(MergeableState::Unknown)
        ),
        ci_status: match (pr.mergeable, pr.mergeable_state, bors) {
            _ if pr.draft.is_some_and(|i| i) => CiStatus::Draft,
            (Some(_), Some(MergeableState::Behind | MergeableState::Dirty), _) => {
                CiStatus::Conflicted
            }

            (
                _,
                _,
                Some(BorsInfo {
                    status: BorsStatus::Approved,
                    ..
                }),
            ) => CiStatus::Good,
            (
                _,
                _,
                Some(BorsInfo {
                    status: BorsStatus::Error,
                    ..
                }),
            ) => CiStatus::Bad,
            (
                _,
                _,
                Some(BorsInfo {
                    status: BorsStatus::Failure,
                    ..
                }),
            ) => CiStatus::Bad,
            (
                _,
                _,
                Some(BorsInfo {
                    status: BorsStatus::Pending,
                    ..
                }),
            ) => CiStatus::Running,
            (
                _,
                _,
                Some(BorsInfo {
                    status: BorsStatus::Success,
                    ..
                }),
            ) => CiStatus::Good,
            (
                _,
                _,
                Some(BorsInfo {
                    status: BorsStatus::None,
                    ..
                }),
            ) => CiStatus::Unknown,

            // github: super unreliable
            (None, _, _) => CiStatus::Running,
            (Some(true), None, _) => CiStatus::Good,
            (Some(_), Some(s), _) => match s {
                MergeableState::Behind => CiStatus::Conflicted,
                MergeableState::Dirty => CiStatus::Conflicted,
                MergeableState::Blocked => CiStatus::Unknown,
                MergeableState::Clean => CiStatus::Good,
                MergeableState::Draft => CiStatus::Draft,
                MergeableState::HasHooks => CiStatus::Good,
                MergeableState::Unknown => CiStatus::Unknown,
                MergeableState::Unstable => CiStatus::Good,
                _ => todo!(),
            },
            _ => CiStatus::Unknown,
        },
    }))
}
