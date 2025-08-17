use std::{sync::Arc, time::Duration};

use futures::{SinkExt, StreamExt, stream};
use octocrab::{Octocrab, models::issues::Issue, params};
use tokio::{spawn, time::sleep};

use futures::channel::mpsc::channel;

use crate::{
    login_cx::LoginContext,
    model::{Pr, Repo},
    sort::sort,
};

pub fn scrape_github_for_user(login_context: Arc<LoginContext>) -> impl StreamExt<Item = Pr> {
    stream::iter(login_context.repos.clone())
        // for each repo
        .map({
            let login_context = login_context.clone();
            move |repo| {
                // all assigned issues
                assigned_issues(repo.repo.clone(), login_context.clone())
                    // and all own issues
                    .chain(own_issues(repo.repo.clone(), login_context.clone()))
                    .zip(stream::repeat(repo))
            }
        })
        // flattened
        .flatten()
        // only the issues that are actually PRs
        .filter_map({
            let login_context = login_context.clone();
            move |(issue, repo)| {
                let login_context = login_context.clone();

                async move {
                    if issue.pull_request.is_none() {
                        return None;
                    }
                    match login_context
                        .octocrab
                        .pulls(&repo.owner, &repo.name)
                        .get(issue.number)
                        .await
                    {
                        Ok(pr) => Some((issue, repo, pr)),
                        Err(e) => {
                            tracing::error!("error getting PR: {e}");
                            None
                        }
                    }
                }
            }
        })
        // sort them into our own data structures
        .then(move |(issue, repo, pr)| {
            let login_context = login_context.clone();
            async move { sort(&login_context, &repo, &issue, &pr).await }
        })
}

enum IssueKind {
    Own(String),
    Assigned(String),
}

fn own_issues(repo: Repo, login_context: Arc<LoginContext>) -> impl StreamExt<Item = Issue> {
    read_paginated_issues(login_context.octocrab.clone(), repo, {
        IssueKind::Own(login_context.username.clone())
    })
}

fn assigned_issues(repo: Repo, login_context: Arc<LoginContext>) -> impl StreamExt<Item = Issue> {
    read_paginated_issues(
        login_context.octocrab.clone(),
        repo,
        IssueKind::Assigned(login_context.username.clone()),
    )
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

// async fn process_pr(
//     config: Arc<LoginContext>,
//     repo: Repo,
//     bors: Option<Arc<SetOnce<AllBorsInfo>>>,
//     issue: Issue,
//     own_pr: bool,
// ) -> color_eyre::Result<Option<Pr>> {
//     tracing::debug!("processing {}/{} {}", repo.owner, repo.name, issue.number);
//     let Some(_) = issue.pull_request else {
//         return Ok(None);
//     };

//     let Some(body) = issue.body else {
//         return Ok(None);
//     };

//     let pr = pr_info(&config, &repo, issue.number).await?;

//     let comments_cell = OnceCell::new();
//     let get_comments = {
//         let repo = repo.clone();
//         || comments_cell.get_or_try_init(|| comments(config.clone(), repo, issue.number))
//     };

//     let bors = if let Some(bors) = bors
//         && let all_bors_info = bors.wait().await
//         && let Some(bors_info) = all_bors_info.prs.get(&issue.number)
//     {
//         Some((bors_info.clone(), all_bors_info.rollups.clone()))
//     } else {
//         None
//     };

//     let local_config = config.clone();
//     let shared_status = async || -> color_eyre::Result<Option<SharedStatus>> {
//         if issue
//             .labels
//             .iter()
//             .any(|i| i.name == "S-waiting-on-concerns")
//         {
//             return Ok(Some(SharedStatus::FcpConcerns));
//         }

//         if issue
//             .labels
//             .iter()
//             .any(|i| i.name == "final-comment-period")
//         {
//             let comments = get_comments().await?;

//             let mut fcp_start = None;

//             for i in comments {
//                 if i.user.login == "rfcbot"
//                     && i.body.as_ref().is_some_and(|i| {
//                         i.contains("This is now entering its final comment period")
//                     })
//                 {
//                     fcp_start =
//                         Some(jiff::Timestamp::from_second(i.created_at.timestamp()).unwrap());
//                 }
//             }

//             if let Some(start) = fcp_start {
//                 tracing::debug!("fcp start at {start}");
//                 return Ok(Some(SharedStatus::Fcp(FcpStatus { start })));
//             }
//         }

//         // if issue.labels.iter().any(|i| i.name == "S-waiting-on-crater") {
//         //     if let Some(status) = local_config
//         //         .state
//         //         .crater_info
//         //         .get()
//         //         .await
//         //         .get(&issue.number)
//         //     {
//         //         return Ok(Some(SharedStatus::Crater(CraterInfo {
//         //             status: status.clone(),
//         //         })));
//         //     }
//         // }

//         if issue.labels.iter().any(|i| i.name == "S-blocked") {
//             return Ok(Some(SharedStatus::Blocked));
//         }

//         Ok(None)
//     };

//     let status = if let Some((bors, rollups)) = &bors
//         && (bors.status == BorsStatus::Approved
//             || bors.status == BorsStatus::Pending
//             || issue.labels.iter().any(|i| i.name == "S-waiting-on-bors"))
//     {
//         let mut rollup_status = RollupStatus::InQueue {
//             position: bors.position_in_queue,
//         };

//         if bors.running {
//             rollup_status = RollupStatus::Running;
//         } else {
//             for (idx, rollup) in rollups.iter().enumerate() {
//                 if rollup.pr_numbers.contains(&issue.number) {
//                     rollup_status = if rollup.running {
//                         RollupStatus::InRunningRollup
//                     } else if idx == 0 {
//                         RollupStatus::InNextRollup {
//                             position: rollup.position_in_queue,
//                         }
//                     } else {
//                         RollupStatus::InRollup { nth_rollup: idx }
//                     };

//                     break;
//                 }
//             }
//         }

//         PrStatus::Queued(QueuedStatus {
//             // TODO: make this the bors approver
//             approvers: issue.assignees.iter().map(|i| i.clone()).collect(),
//             author: issue.user,
//             rollup_setting: bors.rollup_setting.clone(),
//             rollup_status,
//         })
//     } else if own_pr {
//         // creator
//         PrStatus::Own(OwnPr {
//             status: if let Some(s) = shared_status().await? {
//                 OwnPrStatus::Shared(s)
//             } else if issue.labels.iter().any(|i| i.name == "S-waiting-on-review") {
//                 OwnPrStatus::WaitingForReview
//             } else {
//                 OwnPrStatus::Pending
//             },
//             reviewers: issue.assignees.iter().map(|i| i.clone()).collect(),
//         })
//     } else {
//         // revieiwer
//         PrStatus::Review(PrReview {
//             status: if let Some(s) = shared_status().await? {
//                 PrReviewStatus::Shared(s)
//             } else if issue.labels.iter().any(|i| i.name == "S-waiting-on-review") {
//                 PrReviewStatus::Review
//             } else {
//                 PrReviewStatus::Author
//             },
//             author: issue.user,
//         })
//     };

//     Ok(Some(Pr {
//         repo: repo,
//         number: issue.number,
//         title: issue.title,
//         description: body,
//         link: issue.html_url,
//         perf_runs: Vec::new(),
//         crater_runs: Vec::new(),
//         associated_issues: Vec::new(),
//         created: jiff::Timestamp::from_second(issue.created_at.timestamp()).unwrap(),
//         draft: pr.draft.is_some_and(|i| i),
//         status,
//         ci_status: match (
//             pr.mergeable,
//             pr.mergeable_state,
//             bors.as_ref().map(|i| &i.0),
//         ) {
//             _ if pr.draft.is_some_and(|i| i) => CiStatus::Draft,
//             (Some(_), Some(MergeableState::Behind | MergeableState::Dirty), _) => {
//                 CiStatus::Conflicted
//             }

//             (
//                 _,
//                 _,
//                 Some(BorsInfo {
//                     status: BorsStatus::Approved,
//                     ..
//                 }),
//             ) => CiStatus::Good,
//             (
//                 _,
//                 _,
//                 Some(BorsInfo {
//                     status: BorsStatus::Error,
//                     ..
//                 }),
//             ) => CiStatus::Bad,
//             (
//                 _,
//                 _,
//                 Some(BorsInfo {
//                     status: BorsStatus::Failure,
//                     ..
//                 }),
//             ) => CiStatus::Bad,
//             (
//                 _,
//                 _,
//                 Some(BorsInfo {
//                     status: BorsStatus::Pending,
//                     ..
//                 }),
//             ) => CiStatus::Running,
//             (
//                 _,
//                 _,
//                 Some(BorsInfo {
//                     status: BorsStatus::Success,
//                     ..
//                 }),
//             ) => CiStatus::Good,
//             (
//                 _,
//                 _,
//                 Some(BorsInfo {
//                     status: BorsStatus::None,
//                     ..
//                 }),
//             ) => CiStatus::Unknown,

//             // github: super unreliable
//             (None, _, _) => CiStatus::Running,
//             (Some(true), None, _) => CiStatus::Good,
//             (Some(_), Some(s), _) => match s {
//                 MergeableState::Behind => CiStatus::Conflicted,
//                 MergeableState::Dirty => CiStatus::Conflicted,
//                 MergeableState::Blocked => CiStatus::Unknown,
//                 MergeableState::Clean => CiStatus::Good,
//                 MergeableState::Draft => CiStatus::Draft,
//                 MergeableState::HasHooks => CiStatus::Good,
//                 MergeableState::Unknown => CiStatus::Unknown,
//                 MergeableState::Unstable => CiStatus::Good,
//                 _ => todo!(),
//             },
//             _ => CiStatus::Unknown,
//         },
//     }))
// }
