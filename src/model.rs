use std::{fmt::Display, ops::Deref};

use jiff::{SignedDuration, Timestamp};
use serde::Serialize;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct Repo {
    pub owner: String,
    pub name: String,
}

impl Display for Repo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}/{}", self.owner, self.name)
    }
}

#[derive(Clone, Debug)]
pub struct RepoInfo {
    pub repo: Repo,
    pub bors_queue_url: Option<Url>,
}

impl Deref for RepoInfo {
    type Target = Repo;

    fn deref(&self) -> &Self::Target {
        &self.repo
    }
}

#[derive(Clone, Debug)]
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

#[derive(Clone, Debug, Serialize)]
pub struct Author {
    pub name: String,
    pub id: u64,
    pub avatar_url: Url,
    pub profile_url: Url,
}

#[derive(Clone, Debug)]
pub enum QueueStatus {
    Unknown,
    InQueue {
        position: usize,
    },
    Running,
    InNextRollup {
        position: usize,
        pr_link: Url,
        pr_number: u64,
        rollup_size: usize,
    },
    InRollup {
        nth_rollup: usize,
        pr_link: Url,
        pr_number: u64,
        rollup_size: usize,
    },
    InRunningRollup {
        pr_link: Url,
        pr_number: u64,
        rollup_size: usize,
    },
}

#[derive(Clone, Debug)]
pub struct Pr {
    pub repo: Repo,
    pub title: String,
    pub description: Option<String>,
    pub number: u64,
    pub link: Url,

    pub author: Author,
    pub reviewers: Vec<Author>,

    pub status: PrStatus,

    pub ci_status: CiStatus,

    pub created: Timestamp,
}

impl Pr {
    pub fn is_rollup_pr(&self) -> bool {
        self.title.starts_with("Rollup of") && self.title.ends_with("pull requests")
    }
}

#[derive(Clone, Debug)]
pub enum CiStatus {
    Conflicted,
    Good,
    Running,
    Bad,
    Unknown,
    Draft,
}

#[derive(Clone, Debug)]
pub enum PrStatus {
    /// Ready for yourself to work on
    Ready {},
    /// Ready for review work
    Review {
        other_reviewers: Vec<Author>,
    },
    /// Waiting for some reason
    Waiting {
        wait_reason: WaitingReason,
    },
    /// Approved & Queued
    Queued(QueuedInfo),
    Draft {},
    Subscribed,
}

#[derive(Debug, Clone, Default)]
pub enum RollupSetting {
    Never,
    Always,
    Iffy,
    #[default]
    Unset,
}

#[derive(Clone, Debug)]
pub struct QueuedInfo {
    pub approvers: Vec<Author>,
    pub rollup_setting: RollupSetting,
    pub queue_status: QueueStatus,
}

#[derive(Clone, Debug)]
pub enum CraterStatus {
    Unknown,
    Queued { num_before: usize },
    Running { expected_end: Timestamp },
    GeneratingReport,
}

#[derive(Clone, Debug)]
pub enum WaitingReason {
    Author,
    /// Generic S-blocked
    Blocked,

    /// It's your PR, waiting for the reviewer
    Review,

    Fcp(FcpStatus),
    CraterRun(CraterStatus),

    TryBuild(),
    PerfRun(),

    /// weird
    Unknown,
}
