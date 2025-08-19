use std::sync::Arc;

use octocrab::models::{
    issues::Issue,
    pulls::{MergeableState, PullRequest},
};

use crate::{
    api::bors::{BorsPr, BorsQueue, BorsStatus},
    login_cx::LoginContext,
    model::{
        Author, CiStatus, CraterStatus, Pr, PrStatus, QueueStatus, QueuedInfo, RepoInfo,
        WaitingReason,
    },
};

fn label(issue: &Issue, label: impl AsRef<str>) -> bool {
    issue.labels.iter().any(|i| i.name == label.as_ref())
}

async fn sort_waiting(
    login_context: &LoginContext,
    issue: &Issue,
    pr: &PullRequest,
    bors_for_pr: Option<&BorsPr>,
) -> WaitingReason {
    if label(issue, "S-waiting-on-author") {
        WaitingReason::Author
    } else if label(issue, "S-blocked") {
        WaitingReason::Blocked
    } else if label(issue, "S-waiting-on-review") {
        WaitingReason::Review
    } else if label(issue, "S-final-comment-period") || label(issue, "S-waiting-on-concerns") {
        WaitingReason::Fcp(todo!())
    } else if label(issue, "S-waiting-on-crater") {
        let crater_info = login_context.state.crater_info.get().await;

        let Some(crater_status) = crater_info.get(&issue.number) else {
            return WaitingReason::CraterRun(CraterStatus::Unknown);
            // return Ok(Some(SharedStatus::Crater(CraterInfo {
            // status: status.clone(),
            // })));
        };

        WaitingReason::CraterRun(crater_status.clone())
    } else {
        tracing::error!(
            "no clue why we're waiting... {} {}",
            issue.number,
            issue.title
        );
        WaitingReason::Unknown
    }
}

async fn sort_queued(
    login_context: &LoginContext,
    repo: &RepoInfo,
    issue: &Issue,
    bors_for_pr: Option<&BorsPr>,
) -> QueuedInfo {
    let rollup_status = if let Some(bors) = bors_for_pr {
        let mut rollup_status = QueueStatus::InQueue {
            position: bors.position_in_queue,
        };

        if bors.running {
            rollup_status = QueueStatus::Running;
        } else {
            for (idx, rollup) in login_context
                .state
                .clone()
                .rollup_info(repo.clone(), login_context.octocrab.clone())
                .await
                .rollups
                .iter()
                .enumerate()
            {
                if !matches!(
                    rollup.status,
                    BorsStatus::Pending | BorsStatus::Success | BorsStatus::Approved
                ) {
                    continue;
                }

                if rollup.pr_numbers.contains(&issue.number) {
                    rollup_status = if rollup.running {
                        QueueStatus::InRunningRollup {
                            pr_link: rollup.pr_link.clone(),
                            pr_number: rollup.pr_number,
                            rollup_size: rollup.pr_numbers.len(),
                        }
                    } else if idx == 0 {
                        QueueStatus::InNextRollup {
                            position: rollup.position_in_queue,
                            pr_link: rollup.pr_link.clone(),
                            pr_number: rollup.pr_number,
                            rollup_size: rollup.pr_numbers.len(),
                        }
                    } else {
                        QueueStatus::InRollup {
                            nth_rollup: idx,
                            pr_link: rollup.pr_link.clone(),
                            pr_number: rollup.pr_number,
                            rollup_size: rollup.pr_numbers.len(),
                        }
                    };

                    break;
                }
            }
        }

        rollup_status
    } else {
        tracing::warn!("bors was none for {}#{}", repo.repo, issue.number);
        QueueStatus::Unknown
    };

    QueuedInfo {
        // TODO: make this the bors approver
        approvers: issue.assignees.iter().map(convert_author).collect(),
        rollup_setting: bors_for_pr
            .map(|i| i.rollup_setting.clone())
            .unwrap_or_default(),
        queue_status: rollup_status,
    }
}

async fn sort_status(
    login_context: &LoginContext,
    username: String,
    repo: &RepoInfo,
    issue: &Issue,
    pr: &PullRequest,
    bors_for_repo: &Arc<BorsQueue>,
) -> PrStatus {
    let bors_for_pr = bors_for_repo.for_pr(issue.number);

    let res = if pr.draft.is_some_and(|i| i) {
        PrStatus::Draft {}
    } else if
    // you're assigned for review
    issue.assignees.iter().any(|i| i.login == username)
        // and it's waiting for review
        && label(issue, "S-waiting-on-review")
    {
        PrStatus::Review {
            other_reviewers: issue
                .assignees
                .iter()
                .filter(|i| i.login != username)
                .map(convert_author)
                .collect(),
        }
    } else if
    // you're the creator of the PR
    issue.user.login == username
        // and it's waiting for the author
        && label(issue, "S-waiting-on-author")
    {
        PrStatus::Ready {}
    } else if
    // if it's waiting for bors, it could be for:
    // - a try build
    // - it's in the queue
    // TODO: try build detection
    label(issue, "S-waiting-on-bors")
        || bors_for_pr
            .is_some_and(|b| matches!(b.status, BorsStatus::Approved | BorsStatus::Pending))
    {
        PrStatus::Queued(sort_queued(login_context, repo, issue, bors_for_pr).await)
    } else {
        // the PR must be waiting for some reason. There are many reasons though...
        PrStatus::Waiting {
            wait_reason: sort_waiting(login_context, issue, pr, bors_for_pr).await,
        }
    };

    res
}

fn ci_status(issue: &Issue, pr: &PullRequest, bors_for_repo: &Arc<BorsQueue>) -> CiStatus {
    let bors_for_pr = bors_for_repo.for_pr(issue.number);

    match (pr.mergeable, &pr.mergeable_state, bors_for_pr) {
        _ if pr.draft.is_some_and(|i| i) => CiStatus::Draft,
        (Some(_), Some(MergeableState::Behind | MergeableState::Dirty), _) => CiStatus::Conflicted,

        (
            _,
            _,
            Some(BorsPr {
                status: BorsStatus::Approved,
                ..
            }),
        ) => CiStatus::Good,
        (
            _,
            _,
            Some(BorsPr {
                status: BorsStatus::Error,
                ..
            }),
        ) => CiStatus::Bad,
        (
            _,
            _,
            Some(BorsPr {
                status: BorsStatus::Failure,
                ..
            }),
        ) => CiStatus::Bad,
        (
            _,
            _,
            Some(BorsPr {
                status: BorsStatus::Pending,
                ..
            }),
        ) => CiStatus::Running,
        (
            _,
            _,
            Some(BorsPr {
                status: BorsStatus::Success,
                ..
            }),
        ) => CiStatus::Good,
        (
            _,
            _,
            Some(BorsPr {
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
    }
}

#[derive(Clone, Debug)]
pub enum PredeterminedCategory {
    Subscribed,
    None(PullRequest),
}

pub async fn sort(
    login_context: &LoginContext,
    username: String,
    repo: &RepoInfo,
    issue: &Issue,
    predetermined_category: PredeterminedCategory,
) -> Option<Pr> {
    tracing::info!("sorting PR {}#{} {}", repo.repo, issue.number, issue.title);
    let bors_for_repo = login_context.state.bors_info(repo.clone()).await;

    // no subscribed issues when impersonating
    if let PredeterminedCategory::Subscribed = predetermined_category
        && login_context.base_username != username
    {
        return None;
    }

    Some(Pr {
        repo: repo.repo.clone(),
        title: issue.title.clone(),
        description: issue.body.clone(),
        number: issue.number,
        link: issue.html_url.clone(),
        author: convert_author(&issue.user),
        reviewers: issue.assignees.iter().map(convert_author).collect(),
        status: match &predetermined_category {
            PredeterminedCategory::None(pr) => {
                sort_status(login_context, username, repo, issue, pr, &bors_for_repo).await
            }
            PredeterminedCategory::Subscribed => PrStatus::Subscribed,
        },
        ci_status: match &predetermined_category {
            PredeterminedCategory::None(pr) => ci_status(issue, pr, &bors_for_repo),
            PredeterminedCategory::Subscribed => CiStatus::Unknown,
        },

        created: jiff::Timestamp::from_second(issue.created_at.timestamp()).unwrap(),
    })
}

pub fn convert_author(author: &octocrab::models::Author) -> Author {
    Author {
        name: author.login.clone(),
        id: *author.id,
        avatar_url: author.avatar_url.clone(),
        profile_url: author.html_url.clone(),
    }
}
