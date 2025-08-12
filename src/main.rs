use axum::extract::State;
use axum::{Router, routing::get};
use color_eyre::eyre::Context;
use jiff::{SignedDuration, Span, SpanRound, Timestamp, Unit};
use maud::{DOCTYPE, Markup, PreEscaped, Render, html};
use octocrab::models::Author;
use octocrab::models::issues::Comment;
use octocrab::{
    Octocrab, Page,
    models::{self, issues::Issue},
    params,
};
use scraper::{ElementRef, Html, Selector};
use std::{collections::HashMap, env, sync::Arc, time::Duration};
use tokio::sync::{Mutex, OnceCell};
use tokio::{
    join, spawn,
    sync::{
        SetOnce,
        mpsc::{Sender, channel},
    },
    time::sleep,
};
use tower_http::services::ServeDir;
use url::Url;

#[derive(Clone)]
struct Repo {
    owner: String,
    name: String,
    bors_queue_url: Option<Url>,
}

#[derive(Clone)]
struct Config {
    username: String,
    token: String,

    repos: Vec<Repo>,
}

#[derive(Clone)]
pub enum PerfStatus {
    Queued,
    Running,
}

#[derive(Clone)]
pub struct FcpStatus {
    start: Timestamp,
}

impl FcpStatus {
    pub fn ends_on(&self) -> Timestamp {
        self.start
            .checked_add(SignedDuration::from_hours(24 * 10))
            .unwrap()
    }
}

#[derive(Clone)]
pub enum SharedStatus {
    Try,
    Perf(PerfStatus),
    Crater,
    Fcp(FcpStatus),
}

impl SharedStatus {
    pub fn sort(&self) -> PrBoxKind {
        match self {
            SharedStatus::Try => PrBoxKind::Stalled,
            SharedStatus::Perf(perf_status) => PrBoxKind::Stalled,
            SharedStatus::Crater => PrBoxKind::Stalled,
            SharedStatus::Fcp(fcp_status) => PrBoxKind::Stalled,
        }
    }

    pub fn stalled(&self) -> Option<Markup> {
        match self {
            SharedStatus::Try => todo!(),
            SharedStatus::Perf(perf_status) => todo!(),
            SharedStatus::Crater => todo!(),
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
    status: PrReviewStatus,
    author: Author,
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
    status: OwnPrStatus,
    reviewers: Vec<Author>,
    wip: bool,
}

#[derive(Clone)]
pub enum RollupStatus {
    Pending,
    Running,
}

#[derive(Clone)]
pub struct QueuedStatus {
    author: Author,
    approvers: Vec<Author>,
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
struct Pr {
    repo: Repo,
    number: u64,

    status: PrStatus,

    title: String,
    description: String,

    link: Url,

    perf_runs: Vec<PastPerfRun>,
    crater_runs: Vec<PastPerfRun>,

    associated_issues: Vec<()>,
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
            Some(field("author", Author(&r.author)))
        } else {
            None
        }
    }

    pub fn reviewers(&self) -> Option<Markup> {
        if let PrStatus::Own(r) = &self.status {
            Some(html! {
                @for r in &r.reviewers {
                    (field("reviewer", Author(r)))
                }
            })
        } else if let PrStatus::Queued(r) = &self.status {
            Some(html! {
                (field("author", Author(&r.author)))
                @for r in &r.approvers {
                    (field("approver", Author(r)))
                }
            })
        } else {
            None
        }
    }

    pub fn stalled(&self) -> Option<Markup> {
        match &self.status {
            PrStatus::Review(pr_review) => match &pr_review.status {
                PrReviewStatus::Author => Some(html! {
                    span{"waiting for author"}
                }),
                PrReviewStatus::Review => None,
                PrReviewStatus::Shared(shared_status) => shared_status.stalled(),
            },
            PrStatus::Own(own_pr) => match &own_pr.status {
                OwnPrStatus::Conflicted => None,
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

fn field(label: impl AsRef<str>, value: Markup) -> Markup {
    html! {
        div class="field" {
            span class="label" { (label.as_ref()) }
            div class="value" {
                (value)
            }
        }
    }
}

const REFRESH_RATE: Duration = Duration::from_secs(60);

enum BackendStatus {
    Idle { last_refresh: Timestamp },
    Refreshing,
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

struct AppState {
    config: Config,
    prs: Vec<Pr>,
    status: BackendStatus,
}

impl AppState {
    pub fn new(config: Config) -> Self {
        Self {
            config,
            prs: Vec::new(),
            status: BackendStatus::Uninitialized,
        }
    }
}

async fn home(State(state): State<Arc<Mutex<AppState>>>) -> Markup {
    html! {
        (DOCTYPE)
        html {
            head {
                (PreEscaped(r#"
                <title>Review Queue</title>
                <link rel="stylesheet" href="/assets/style.css">
                <meta name="viewport" content="width=device-width, initial-scale=1.0" />

                <link rel="preconnect" href="https://fonts.googleapis.com">
                <link rel="preconnect" href="https://fonts.gstatic.com" crossorigin>
                <link href="https://fonts.googleapis.com/css2?family=Noto+Sans:ital,wght@0,100..900;1,100..900&display=swap" rel="stylesheet">
                "#))
                script {
                    (PreEscaped(format!(r#"
                        setTimeout(() => {{
                            window.location.reload();
                        }}, {})
                    "#, REFRESH_RATE.as_millis() / 4)))
                }
            }
            body {
                nav {
                    "backend status: " (state.lock().await.status)
                }
                main {
                    (PrBox(state.clone(), PrBoxKind::WorkReady).await)
                    (PrBox(state.clone(), PrBoxKind::TodoReview).await)
                    (PrBox(state.clone(), PrBoxKind::Stalled).await)
                    (PrBox(state.clone(), PrBoxKind::Queue).await)
                    (PrBox(state.clone(), PrBoxKind::Other).await)
                }
            }
        }
    }
}

#[derive(PartialEq)]
enum PrBoxKind {
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

async fn PrBox(state: Arc<Mutex<AppState>>, kind: PrBoxKind) -> Markup {
    html! {
        div class="prbox" {
            h1 { (kind) }
            div class="prs" {
                @for pr in {
                    let guard = state.lock().await;
                    let mut prs = guard.prs.iter().filter(|pr| pr.sort() == kind).cloned().collect::<Vec<_>>();
                    drop(guard);
                    prs.sort_by_cached_key(|pr| pr.title.clone());
                    prs
                } {
                    (Pr(&pr).await)
                }
            }
        }
    }
}

async fn Pr(pr: &Pr) -> Markup {
    html! {
        div class="pr" {
            h2 class="title" { a target="_blank" rel="noopener noreferrer" href=(pr.link) { (pr.title)}}

            div class="fields" {
                @if let Some(a) = pr.author() {
                    (a)
                }
                @if let Some(r) = pr.reviewers() {
                    (r)
                }

                @if let Some(s) = pr.stalled() {
                    div class="badge" {
                        (s)
                    }
                }
            }
        }
    }
}

fn Author(author: &Author) -> Markup {
    html! {
        a class="author" href=(author.url)
            target="_blank" rel="noopener noreferrer"
        {
            img class="avatar" src=(author.avatar_url) alt=(format!("{}'s profile picture", author.login))
            span class="name" {(author.login)}
        }
    }
}

async fn update_state(state: Arc<Mutex<AppState>>) {
    loop {
        let config = {
            let mut state = state.lock().await;
            state.status = BackendStatus::Refreshing;
            state.config.clone()
        };

        match get_prs(config).await {
            Err(e) => tracing::error!("{e}"),
            Ok(prs) => {
                let mut state = state.lock().await;
                state.status = BackendStatus::Idle {
                    last_refresh: Timestamp::now(),
                };
                state.prs = prs;
            }
        }

        sleep(REFRESH_RATE).await;
    }
}

#[tokio::main]
async fn main() -> color_eyre::Result<()> {
    color_eyre::install()?;
    dotenvy::dotenv().context("get dotenv")?;
    tracing_subscriber::fmt::init();

    let config = Config {
        username: "jdonszelmann".to_string(),
        token: env::var("GITHUB_TOKEN").context("get `GITHUB_TOKEN` envvar")?,
        repos: vec![Repo {
            owner: "rust-lang".to_string(),
            name: "rust".to_string(),
            bors_queue_url: Some(Url::parse("https://bors.rust-lang.org/queue/rust").unwrap()),
        }],
    };

    let state = Arc::new(Mutex::new(AppState::new(config)));

    spawn(update_state(state.clone()));

    // for i in res {
    //     let name = format!("{}/{}", i.repo.owner, i.repo.name);
    //     println!("{name:<10}#{} {}", i.number, i.title);
    // }

    // build our application with a single route
    let app = Router::new()
        .route("/", get(home))
        .with_state(state)
        .nest_service("/assets/", ServeDir::new("assets"));
    let address = "0.0.0.0:3000";

    let listener = tokio::net::TcpListener::bind(address).await.unwrap();

    tracing::info!("listening on http://{address}");

    axum::serve(listener, app.into_make_service())
        .await
        .unwrap();

    Ok(())
}

async fn get_prs(config: Config) -> color_eyre::Result<Vec<Pr>> {
    tracing::info!("logging in");
    let octocrab = Octocrab::builder()
        .personal_token(config.token.as_str())
        .build()
        .context("build octocrab")?;

    sleep(Duration::from_secs(1)).await;

    let (tx, mut rx) = channel(16);

    spawn(async move {
        if let Err(e) = process_repos(config, octocrab, tx.clone()).await {
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
    config: Config,
    octocrab: Octocrab,
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

        tracing::info!("getting prs and issues for {}/{}", repo.owner, repo.name);

        let author = process_issues(
            config.clone(),
            octocrab.clone(),
            async || {
                octocrab
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
            tx.clone(),
            true,
        );

        let reviewer = process_issues(
            config.clone(),
            octocrab.clone(),
            async || {
                octocrab
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
    config: Config,
    octocrab: Octocrab,
    issues: impl Fn() -> F,
    bors: Option<Arc<SetOnce<HashMap<u64, BorsInfo>>>>,
    tx: Sender<color_eyre::Result<Pr>>,
    own_pr: bool,
) -> color_eyre::Result<()> {
    for repo in config.repos {
        tracing::info!("getting prs and issues for {}/{}", repo.owner, repo.name);

        let mut page = loop {
            let page = issues().await?;

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

            for issue in page {
                let tx = tx.clone();
                let octocrab = octocrab.clone();
                let repo = repo.clone();
                let bors = bors.clone();
                spawn(async move {
                    match process_pr(octocrab, repo, bors, issue, own_pr).await {
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
            page = match octocrab.get_page::<models::issues::Issue>(&next).await? {
                Some(next_page) => next_page,
                None => break,
            }
        }
    }

    Ok(())
}

async fn comments(
    octocrab: Octocrab,
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
        let page = octocrab
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
        page = match octocrab.get_page::<models::issues::Comment>(&next).await? {
            Some(next_page) => next_page,
            None => break,
        }
    }

    Ok(res)
}

async fn process_pr(
    octocrab: Octocrab,
    repo: Repo,
    bors: Option<Arc<SetOnce<HashMap<u64, BorsInfo>>>>,
    issue: Issue,
    own_pr: bool,
) -> color_eyre::Result<Option<Pr>> {
    tracing::info!("processing {}/{} {}", repo.owner, repo.name, issue.number);
    let Some(pull_request) = issue.pull_request else {
        return Ok(None);
    };

    let Some(body) = issue.body else {
        return Ok(None);
    };

    let comments_cell = OnceCell::new();
    let get_comments = {
        let repo = repo.clone();
        || comments_cell.get_or_try_init(|| comments(octocrab, repo, issue.number))
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

        Ok(None)
    };

    let status = if let Some(bors) = &bors
        && (bors.status == BorsStatus::Approved || bors.status == BorsStatus::Pending)
    {
        PrStatus::Queued(QueuedStatus {
            // TODO: make this the bors approver
            approvers: issue.assignees.iter().map(|i| i.clone()).collect(),
            author: issue.user,
        })
    } else if own_pr {
        // creator
        PrStatus::Own(OwnPr {
            status: if let Some(s) = shared_status().await? {
                OwnPrStatus::Shared(s)
            } else if let Some(bors) = &bors
                && !bors.mergeable
            {
                OwnPrStatus::Conflicted
            } else if issue.labels.iter().any(|i| i.name == "S-waiting-on-review") {
                OwnPrStatus::WaitingForReview
            } else {
                OwnPrStatus::Pending
            },
            reviewers: issue.assignees.iter().map(|i| i.clone()).collect(),
            wip: false,
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
        status,
    }))
}

#[derive(Debug, Clone, PartialEq)]
enum BorsStatus {
    None,
    Approved,
    Pending,
    Failure,
    Error,
    Success,
    Other(String),
}

#[derive(Debug, Clone)]
enum RollupSetting {
    Never,
    Always,
    Iffy,
    Unset,
}

#[derive(Debug, Clone)]
struct BorsInfo {
    approver: String,
    status: BorsStatus,
    mergeable: bool,
    rollup_status: RollupSetting,
    priority: u64,
}

async fn get_bors_queue(url: Url) -> color_eyre::Result<HashMap<u64, BorsInfo>> {
    tracing::info!("reading bors page at {url}");
    let mut results = HashMap::new();
    let response = reqwest::get(url).await?;
    let body = response.text().await.context("body")?;

    let document = Html::parse_document(&body);

    let row_selector = Selector::parse("#queue tbody tr").unwrap();
    for row in document.select(&row_selector) {
        let children = row
            .children()
            .filter_map(ElementRef::wrap)
            .collect::<Vec<_>>();

        let number = children[2].text().collect::<String>();
        let status = children[3].text().collect::<String>();
        let mergeable = children[4].text().collect::<String>();
        let approver = children[8].text().collect::<String>();
        let priority = children[9].text().collect::<String>();
        let rollup = children[10].text().collect::<String>();

        let Ok(number) = number.trim().parse::<u64>() else {
            tracing::error!("parse PR number");
            continue;
        };

        let status = match status.trim() {
            "" => BorsStatus::None,
            "error" => BorsStatus::Error,
            "failure" => BorsStatus::Failure,
            "approved" => BorsStatus::Approved,
            "pending" => BorsStatus::Pending,
            other => BorsStatus::Other(other.to_string()),
        };

        let mergeable = match mergeable.trim() {
            "" => continue,
            "yes" => true,
            "no" => false,
            other => {
                tracing::error!("weird mergeable status: {other}");
                continue;
            }
        };

        let rollup_status = match rollup.trim() {
            "" => RollupSetting::Unset,
            "never" => RollupSetting::Never,
            "always" => RollupSetting::Always,
            "iffy" => RollupSetting::Iffy,
            other => {
                tracing::error!("weird rollup status: {other}");
                continue;
            }
        };

        let Ok(priority) = priority.trim().parse::<u64>() else {
            tracing::error!("parse priority: {}", priority.trim());
            continue;
        };

        let info = BorsInfo {
            approver: approver.trim().to_string(),
            status,
            mergeable,
            rollup_status,
            priority,
        };
        results.insert(number, info);
    }

    Ok(results)
}
