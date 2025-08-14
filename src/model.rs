#![allow(dead_code)]

use std::sync::Arc;

use jiff::{SignedDuration, Span, SpanRound, Timestamp, Unit};
use maud::{Markup, html};
use octocrab::{Octocrab, models::Author};
use url::Url;

use crate::{AppState, api::bors::RollupSetting};

#[derive(Clone, Debug)]
pub struct LoginContext {
    pub username: String,
    pub repos: Vec<Repo>,
    pub octocrab: Octocrab,
    pub state: Arc<AppState>,
}

#[derive(Clone, Debug)]
pub struct Repo {
    pub owner: String,
    pub name: String,
    pub bors_queue_url: Option<Url>,
}

#[derive(Clone)]
pub enum PerfStatus {
    Queued,
    Running,
}

#[derive(Clone)]
pub struct FcpStatus {
    pub start: Timestamp,
}

impl FcpStatus {
    pub fn ends_on(&self) -> Timestamp {
        self.start
            .checked_add(SignedDuration::from_hours(24 * 10))
            .unwrap()
    }
}

#[derive(Clone, Debug)]
pub enum CraterStatus {
    Queued { num_before: usize },
    Running { expected_end: Timestamp },
    GeneratingReport,
}

#[derive(Clone)]
pub struct CraterInfo {
    pub status: CraterStatus,
}

#[derive(Clone)]
pub enum SharedStatus {
    FcpConcerns,
    Try,
    Perf(PerfStatus),
    Crater(CraterInfo),
    Fcp(FcpStatus),
    Blocked,
}

impl SharedStatus {
    pub fn sort(&self) -> PrSortCategory {
        match self {
            SharedStatus::Try => PrSortCategory::Stalled,
            SharedStatus::Perf(..) => PrSortCategory::Stalled,
            SharedStatus::Crater(..) => PrSortCategory::Stalled,
            SharedStatus::Fcp(..) => PrSortCategory::Stalled,
            SharedStatus::Blocked => PrSortCategory::Stalled,
            SharedStatus::FcpConcerns => PrSortCategory::Stalled,
        }
    }

    pub fn stalled(&self) -> Option<Markup> {
        match self {
            SharedStatus::Blocked => Some(html! {
                "blocked"
            }),
            Self::FcpConcerns => Some(html! {
                "FCP concerns"
            }),
            SharedStatus::Try => todo!(),
            SharedStatus::Perf(..) => todo!(),
            SharedStatus::Crater(crater_status) => match crater_status.status {
                CraterStatus::Queued { num_before } => Some(html! {
                    (format!("in crater queue ({} queued before this)", num_before))
                }),
                CraterStatus::GeneratingReport => Some(html! {
                    "generating crater report"
                }),
                CraterStatus::Running { expected_end } => {
                    let duration = expected_end.duration_since(Timestamp::now());
                    let span = Span::try_from(duration).unwrap();

                    let options = SpanRound::new()
                        .largest(Unit::Week)
                        .smallest(Unit::Hour)
                        .days_are_24_hours();

                    Some(html! {
                        (format!("crater experiment done in {:#}", span.round(options).unwrap()))
                    })
                }
            },
            SharedStatus::Fcp(fcp_status) => {
                let duration = fcp_status.ends_on().duration_since(Timestamp::now());
                let span = Span::try_from(duration).unwrap();

                let options = SpanRound::new()
                    .largest(Unit::Week)
                    .smallest(Unit::Hour)
                    .days_are_24_hours();

                Some(html! {
                    span {(format!("FCP ends in {:#}", span.round(options).unwrap()))}
                })
            }
        }
    }
}

#[derive(Clone)]
pub enum PrReviewStatus {
    Author,
    Review,
    Shared(SharedStatus),
}

#[derive(Clone)]
pub struct PrReview {
    pub status: PrReviewStatus,
    pub author: Author,
}

#[derive(Clone)]
pub enum OwnPrStatus {
    WaitingForReview,
    /// Ready for me to do more work on
    Pending,
    Shared(SharedStatus),
}

#[derive(Clone)]
pub struct OwnPr {
    pub status: OwnPrStatus,
    pub reviewers: Vec<Author>,
}

#[derive(Clone)]
pub enum RollupStatus {
    InQueue { position: usize },
    Running,
    InNextRollup { position: usize },
    InRollup { nth_rollup: usize },
    InRunningRollup,
}

#[derive(Clone)]
pub struct QueuedStatus {
    pub author: Author,
    pub approvers: Vec<Author>,
    pub rollup_setting: RollupSetting,
    pub rollup_status: RollupStatus,
}

#[derive(Clone)]
pub enum PrStatus {
    Review(PrReview),
    Own(OwnPr),
    Queued(QueuedStatus),
}

#[derive(Clone)]
pub struct PastPerfRun {}
#[derive(Clone)]
pub struct PastCraterRun {}

#[derive(Clone)]
pub enum CiStatus {
    Conflicted,
    Good,
    Running,
    Bad,
    Unknown,
    Draft,
}

#[derive(Clone)]
pub struct Pr {
    pub repo: Repo,
    pub number: u64,

    pub status: PrStatus,

    pub ci_state: String,
    pub ci_status: CiStatus,

    pub title: String,
    pub description: String,

    pub link: Url,

    pub perf_runs: Vec<PastPerfRun>,
    pub crater_runs: Vec<PastPerfRun>,

    pub associated_issues: Vec<()>,
    pub draft: bool,

    pub created: Timestamp,
}

#[derive(PartialEq)]
pub enum PrSortCategory {
    Draft,
    Stalled,
    WorkReady,
    TodoReview,
    Queue,
    Other,
}
