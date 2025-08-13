use std::sync::Arc;

use jiff::{SignedDuration, Span, SpanRound, Timestamp, Unit};
use maud::{Markup, Render, html};
use octocrab::{Octocrab, models::Author};
use url::Url;

use crate::{
    AppState,
    queue_page::{field, render_author},
};

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
    Try,
    Perf(PerfStatus),
    Crater(CraterInfo),
    Fcp(FcpStatus),
}

impl SharedStatus {
    pub fn sort(&self) -> PrBoxKind {
        match self {
            SharedStatus::Try => PrBoxKind::Stalled,
            SharedStatus::Perf(..) => PrBoxKind::Stalled,
            SharedStatus::Crater(..) => PrBoxKind::Stalled,
            SharedStatus::Fcp(..) => PrBoxKind::Stalled,
        }
    }

    pub fn stalled(&self) -> Option<Markup> {
        match self {
            SharedStatus::Try => todo!(),
            SharedStatus::Perf(perf_status) => todo!(),
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
                    span {(format!("fcp ends in {:#}", span.round(options).unwrap()))}
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
    Conflicted,
    WaitingForReview,
    /// Ready for me to do more work on
    Pending,
    Shared(SharedStatus),
}

#[derive(Clone)]
pub struct OwnPr {
    pub status: OwnPrStatus,
    pub reviewers: Vec<Author>,
    pub wip: bool,
}

#[derive(Clone)]
pub enum RollupStatus {
    Pending,
    Running,
}

#[derive(Clone)]
pub struct QueuedStatus {
    pub author: Author,
    pub approvers: Vec<Author>,
}

#[derive(Clone)]
pub enum PrStatus {
    Review(PrReview),
    Own(OwnPr),
    Rollup(RollupStatus),
    Queued(QueuedStatus),
}

#[derive(Clone)]
pub struct PastPerfRun {}
#[derive(Clone)]
pub struct PastCraterRun {}

#[derive(Clone)]
pub struct Pr {
    pub repo: Repo,
    pub number: u64,

    pub status: PrStatus,

    pub title: String,
    pub description: String,

    pub link: Url,

    pub perf_runs: Vec<PastPerfRun>,
    pub crater_runs: Vec<PastPerfRun>,

    pub associated_issues: Vec<()>,
}

impl Pr {
    pub fn sort(&self) -> PrBoxKind {
        match &self.status {
            PrStatus::Review(pr_review) => match &pr_review.status {
                PrReviewStatus::Author => PrBoxKind::Stalled,
                PrReviewStatus::Review => PrBoxKind::TodoReview,
                PrReviewStatus::Shared(shared_status) => shared_status.sort(),
            },
            PrStatus::Own(own_pr) => match &own_pr.status {
                OwnPrStatus::Conflicted => PrBoxKind::WorkReady,
                OwnPrStatus::WaitingForReview => PrBoxKind::Stalled,
                OwnPrStatus::Pending => PrBoxKind::WorkReady,
                OwnPrStatus::Shared(shared_status) => shared_status.sort(),
            },
            PrStatus::Rollup(rollup_status) => todo!(),
            PrStatus::Queued(queued_status) => PrBoxKind::Queue,
        }
    }

    pub fn author(&self) -> Option<Markup> {
        if let PrStatus::Review(r) = &self.status {
            Some(field("author", render_author(&r.author)))
        } else {
            None
        }
    }

    pub fn reviewers(&self) -> Option<Markup> {
        if let PrStatus::Own(r) = &self.status {
            Some(html! {
                @for r in &r.reviewers {
                    (field("reviewer", render_author(r)))
                }
            })
        } else if let PrStatus::Queued(r) = &self.status {
            Some(html! {
                (field("author", render_author(&r.author)))
                @for r in &r.approvers {
                    (field("approver", render_author(r)))
                }
            })
        } else {
            None
        }
    }

    pub fn badge(&self) -> Option<Markup> {
        match &self.status {
            PrStatus::Review(pr_review) => match &pr_review.status {
                PrReviewStatus::Author => Some(html! {
                    span{"waiting for author"}
                }),
                PrReviewStatus::Review => None,
                PrReviewStatus::Shared(shared_status) => shared_status.stalled(),
            },
            PrStatus::Own(own_pr) => match &own_pr.status {
                OwnPrStatus::Conflicted => Some(html! {
                    span {"conflicts"}
                }),
                OwnPrStatus::WaitingForReview => Some(html! {
                    span {"waiting for review"}
                }),
                OwnPrStatus::Pending => None,
                OwnPrStatus::Shared(shared_status) => shared_status.stalled(),
            },
            PrStatus::Rollup(rollup_status) => None,
            PrStatus::Queued(queued_status) => None,
        }
    }
}

#[derive(Default)]
pub enum BackendStatus {
    Idle {
        last_refresh: Timestamp,
    },
    Refreshing,
    #[default]
    Uninitialized,
}

impl Render for BackendStatus {
    fn render(&self) -> Markup {
        match self {
            BackendStatus::Idle { last_refresh } => {
                let last_refresh = last_refresh.strftime("%H:%M (%Q)").to_string();
                html! {span {"Idle (last refresh: " (last_refresh) ")"}}
            }
            BackendStatus::Refreshing => html! {span {"refreshing..."}},
            BackendStatus::Uninitialized => html! {span {"uninitialized"}},
        }
    }
}

#[derive(PartialEq)]
pub enum PrBoxKind {
    Stalled,
    WorkReady,
    TodoReview,
    Queue,
    Other,
}

impl Render for PrBoxKind {
    fn render(&self) -> Markup {
        match self {
            PrBoxKind::Stalled => html! {span {"Waiting"}},
            PrBoxKind::TodoReview => html! {span {"Waiting for my review"}},
            PrBoxKind::Other => html! {span {"Other"}},
            PrBoxKind::Queue => html! {span {"In the bors queue"}},
            PrBoxKind::WorkReady => html! {span {"Ready to work on"}},
        }
    }
}
