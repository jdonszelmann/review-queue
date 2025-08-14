use maud::{Markup, Render, html};

use crate::{
    api::bors::RollupSetting,
    model::{
        CiStatus, OwnPrStatus, Pr, PrReviewStatus, PrSortCategory, PrStatus, QueuedStatus,
        RollupStatus,
    },
    pages::queue::{field, render_author},
};

pub mod auth;
pub mod home;
pub mod queue;

impl Render for CiStatus {
    fn render(&self) -> Markup {
        match self {
            CiStatus::Conflicted => html! {"conflicted"},
            CiStatus::Good => html! {"passing"},
            CiStatus::Running => html! {"in progress"},
            CiStatus::Bad => html! {"failing"},
            CiStatus::Unknown => html! {"unknown"},
            CiStatus::Draft => html! {"draft"},
        }
    }
}

impl Render for PrSortCategory {
    fn render(&self) -> Markup {
        match self {
            PrSortCategory::Draft => html! {span {"Draft"}},
            PrSortCategory::Stalled => html! {span {"Waiting"}},
            PrSortCategory::TodoReview => html! {span {"Waiting for my review"}},
            PrSortCategory::Other => html! {span {"Other"}},
            PrSortCategory::Queue => html! {span {"In the bors queue"}},
            PrSortCategory::WorkReady => html! {span {"Ready to work on"}},
        }
    }
}

impl Pr {
    pub fn sort(&self) -> PrSortCategory {
        if self.draft {
            return PrSortCategory::Draft;
        }

        match &self.status {
            PrStatus::Review(pr_review) => match &pr_review.status {
                PrReviewStatus::Author => PrSortCategory::Stalled,
                PrReviewStatus::Review => PrSortCategory::TodoReview,
                PrReviewStatus::Shared(shared_status) => shared_status.sort(),
            },
            PrStatus::Own(own_pr) => match &own_pr.status {
                OwnPrStatus::WaitingForReview => PrSortCategory::Stalled,
                OwnPrStatus::Pending => PrSortCategory::WorkReady,
                OwnPrStatus::Shared(shared_status) => shared_status.sort(),
            },
            PrStatus::Queued(_) => PrSortCategory::Queue,
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

    pub fn rollup(&self) -> Option<Markup> {
        if let PrStatus::Queued(r) = &self.status {
            match r.rollup_status {
                RollupStatus::InQueue { position: 1 } => Some(html! {span {"1st in queue"}}),
                RollupStatus::InQueue { position: 2 } => Some(html! {span {"2nd in queue"}}),
                RollupStatus::InQueue { position: 3 } => Some(html! {span {"3rd in queue"}}),
                RollupStatus::InQueue { position } => Some(html! {span {(position) "th in queue"}}),
                RollupStatus::Running => Some(html! {span {"running"}}),
                RollupStatus::InRollup { nth_rollup: 0 } => Some(html! {"in next rollup"}),
                RollupStatus::InRollup { nth_rollup: 1 } => Some(html! {"in 2nd rollup"}),
                RollupStatus::InRollup { nth_rollup: 2 } => Some(html! {"in 3rd rollup"}),
                RollupStatus::InRollup { nth_rollup } => Some(html! {"in "(nth_rollup)"th rollup"}),
                RollupStatus::InRunningRollup => Some(html! {span {"in running rollup"}}),
                RollupStatus::InNextRollup { position: 1 } => {
                    Some(html! {span {"rollup 1st in queue"}})
                }
                RollupStatus::InNextRollup { position: 2 } => {
                    Some(html! {span {"rollup 2nd in queue"}})
                }
                RollupStatus::InNextRollup { position: 3 } => {
                    Some(html! {span {"rollup 3rd in queue"}})
                }
                RollupStatus::InNextRollup { position } => {
                    Some(html! {span {"rollup " (position)"th in queue"}})
                }
            }
        } else {
            None
        }
    }

    pub fn badge(&self) -> Vec<Markup> {
        let mut res = Vec::new();

        match &self.status {
            PrStatus::Review(pr_review) => match &pr_review.status {
                PrReviewStatus::Author => res.push(html! {
                    span{"waiting for author"}
                }),
                PrReviewStatus::Review => {}
                PrReviewStatus::Shared(shared_status) => res.extend(shared_status.stalled()),
            },
            PrStatus::Own(own_pr) => match &own_pr.status {
                OwnPrStatus::WaitingForReview => res.push(html! {
                    span {"waiting for review"}
                }),
                OwnPrStatus::Pending => {}
                OwnPrStatus::Shared(shared_status) => res.extend(shared_status.stalled()),
            },
            PrStatus::Queued(QueuedStatus {
                rollup_setting: RollupSetting::Iffy,
                ..
            }) => res.push(html! { span{"rollup=iffy"} }),
            PrStatus::Queued(QueuedStatus {
                rollup_setting: RollupSetting::Never,
                ..
            }) => res.push(html! { span{"rollup=never"} }),
            PrStatus::Queued(..) => {}
        }

        res
    }
}
