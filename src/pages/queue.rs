use std::{collections::BTreeMap, iter};

use axum::{
    extract::{
        WebSocketUpgrade,
        ws::{Message, Utf8Bytes},
    },
    response::{IntoResponse, Redirect, Response},
};
use futures_util::{sink::SinkExt, stream::StreamExt};
use jiff::{Span, SpanRound, Timestamp, Unit};
use maud::{DOCTYPE, Markup, PreEscaped, Render, html};
use tokio::{select, spawn, time::sleep};

use crate::{
    REFRESH_RATE, get_and_update_state, get_state_instantly,
    model::{
        Author, CiStatus, CraterStatus, Pr, PrStatus, QueueStatus, QueuedInfo, RollupSetting,
        WaitingReason,
    },
    pages::auth::ExtractLoginContext,
};

const CHECKMARK: PreEscaped<&str> = PreEscaped(
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 640" fill="currentColor"><!--!Font Awesome Free v7.0.0 by @fontawesome - https://fontawesome.com License - https://fontawesome.com/license/free Copyright 2025 Fonticons, Inc.--><path d="M530.8 134.1C545.1 144.5 548.3 164.5 537.9 178.8L281.9 530.8C276.4 538.4 267.9 543.1 258.5 543.9C249.1 544.7 240 541.2 233.4 534.6L105.4 406.6C92.9 394.1 92.9 373.8 105.4 361.3C117.9 348.8 138.2 348.8 150.7 361.3L252.2 462.8L486.2 141.1C496.6 126.8 516.6 123.6 530.9 134z"/></svg>"#,
);
const WARN: PreEscaped<&str> = PreEscaped(
    r#"<svg focusable="false" class="warn" viewBox="0 0 16 16" width="16" height="16" fill="currentColor" display="inline-block" overflow="visible" style="vertical-align: text-bottom;" fill="none"><path d="M6.457 1.047c.659-1.234 2.427-1.234 3.086 0l6.082 11.378A1.75 1.75 0 0 1 14.082 15H1.918a1.75 1.75 0 0 1-1.543-2.575ZM8 5a.75.75 0 0 0-.75.75v2.5a.75.75 0 0 0 1.5 0v-2.5A.75.75 0 0 0 8 5Zm1 6a1 1 0 1 0-2 0 1 1 0 0 0 2 0Z"></path></svg>"#,
);
const CROSS: PreEscaped<&str> = PreEscaped(
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 640" fill="currentColor"><!--!Font Awesome Free v7.0.0 by @fontawesome - https://fontawesome.com License - https://fontawesome.com/license/free Copyright 2025 Fonticons, Inc.--><path d="M183.1 137.4C170.6 124.9 150.3 124.9 137.8 137.4C125.3 149.9 125.3 170.2 137.8 182.7L275.2 320L137.9 457.4C125.4 469.9 125.4 490.2 137.9 502.7C150.4 515.2 170.7 515.2 183.2 502.7L320.5 365.3L457.9 502.6C470.4 515.1 490.7 515.1 503.2 502.6C515.7 490.1 515.7 469.8 503.2 457.3L365.8 320L503.1 182.6C515.6 170.1 515.6 149.8 503.1 137.3C490.6 124.8 470.3 124.8 457.8 137.3L320.5 274.7L183.1 137.4z"/></svg>"#,
);
const PROGRESS: PreEscaped<&str> = PreEscaped(
    r#"<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 640 640" fill="currentColor"><!--!Font Awesome Free v7.0.0 by @fontawesome - https://fontawesome.com License - https://fontawesome.com/license/free Copyright 2025 Fonticons, Inc.--><path d="M320 64C461.4 64 576 178.6 576 320C576 461.4 461.4 576 320 576C178.6 576 64 461.4 64 320C64 178.6 178.6 64 320 64zM296 184L296 320C296 328 300 335.5 306.7 340L402.7 404C413.7 411.4 428.6 408.4 436 397.3C443.4 386.2 440.4 371.4 429.3 364L344 307.2L344 184C344 170.7 333.3 160 320 160C306.7 160 296 170.7 296 184z"/></svg>"#,
);

pub fn field(label: impl Render, value: impl Render) -> Markup {
    html! {
        div class="field" {
            span class="label" { (label) }
            div class="value" {
                (value)
            }
        }
    }
}

pub fn page_template(body: Markup) -> Markup {
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
            }
            body {
                (body)
            }
        }
    }
}

pub async fn queue_ws(
    ExtractLoginContext(config): ExtractLoginContext,
    ws: WebSocketUpgrade,
) -> Response {
    let Some(config) = config else {
        return Redirect::to("/").into_response();
    };

    ws.on_upgrade(async move |socket| {
        let (mut send, mut recv) = socket.split();

        select! {
            _ = async {
                while let Some(_) = recv.next().await {}
            } => {}
            _ = async {
                loop {
                    let prs = spawn(get_and_update_state(config.clone())).await.unwrap();
                    let page = queue_page_main(&prs);
                    let _ = send.send(Message::Text(Utf8Bytes::from(&page.into_string()))).await;
                    sleep(REFRESH_RATE).await;
                }
            } => {}
        };
    })
    .into_response()
}

pub async fn queue_page(ExtractLoginContext(config): ExtractLoginContext) -> Response {
    let Some(config) = config else {
        return Redirect::to("/").into_response();
    };

    let prs = get_state_instantly(config.clone()).await;

    let ws_url = format!(
        "{}/queue/ws",
        config.state.config.host.replace("http", "ws") //http -> ws, https -> wss
    );

    page_template(html! {
        nav {
            div class="backend-status" {
                span {"last refreshed: "} span id="refresh-time" {"never (the first refresh can take a few seconds)"}
            }

            div class="divider" {}

            div class="logout" {
                a href="/logout" {
                    "logout"
                }
            }
        }

        (queue_page_main(&prs))

        script {
            (PreEscaped(format!(r#"
                const socket = new WebSocket("{ws_url}");
                socket.addEventListener("message", (event) => {{
                    console.log("replacing main");
                    document.getElementById("main").outerHTML = event.data;

                    const d = new Date();
                    const n = d.toLocaleTimeString();
                    document.getElementById("refresh-time").innerText = n;
                }});
            "#)))
        }
    })
    .into_response()
}

fn queue_page_main(prs: &[Pr]) -> Markup {
    html! {
        main id="main" {
            (render_pr_box(ReadyPrBox(prs)))
            (render_pr_box(ReviewPrBox(prs)))
            (render_pr_box(BlockedPrBox(prs)))
            (render_pr_box(QueuedPrBox(prs)))
            (render_pr_box(DraftPrBox(prs)))
        }
    }
}

trait PrBox {
    type SortKey: Ord + Copy;

    fn title(&self) -> impl AsRef<str>;
    fn render(&self, res: &mut Vec<(Markup, Self::SortKey)>);
}

struct ReadyPrBox<'a>(&'a [Pr]);

impl<'a> PrBox for ReadyPrBox<'a> {
    type SortKey = &'a Timestamp;

    fn title(&self) -> impl AsRef<str> {
        "Ready to work on"
    }

    fn render(&self, res: &mut Vec<(Markup, &'a Timestamp)>) {
        for i in self.0 {
            let PrStatus::Ready {} = i.status else {
                continue;
            };

            res.push((
                pr_skeleton(
                    i,
                    i.reviewers.iter().map(Field::Reviewer),
                    vec![Badge::CiStatus(&i.ci_status)],
                ),
                &i.created,
            ));
        }
    }
}

struct ReviewPrBox<'a>(&'a [Pr]);

impl<'a> PrBox for ReviewPrBox<'a> {
    type SortKey = &'a Timestamp;

    fn title(&self) -> impl AsRef<str> {
        "Waiting for me to review"
    }

    fn render(&self, res: &mut Vec<(Markup, &'a Timestamp)>) {
        for i in self.0 {
            let PrStatus::Review { other_reviewers } = &i.status else {
                continue;
            };

            res.push((
                pr_skeleton(
                    i,
                    iter::once(Field::Author(&i.author))
                        .chain(other_reviewers.iter().map(Field::OtherReviewer)),
                    vec![Badge::CiStatus(&i.ci_status)],
                ),
                &i.created,
            ));
        }
    }
}

struct BlockedPrBox<'a>(&'a [Pr]);

impl<'a> PrBox for BlockedPrBox<'a> {
    type SortKey = &'a Timestamp;

    fn title(&self) -> impl AsRef<str> {
        "Waiting"
    }

    fn render(&self, res: &mut Vec<(Markup, &'a Timestamp)>) {
        for i in self.0 {
            let PrStatus::Waiting { wait_reason } = &i.status else {
                continue;
            };

            res.push((
                pr_skeleton(
                    i,
                    iter::once(Field::Author(&i.author))
                        // TODO: only other reviewers?
                        .chain(i.reviewers.iter().map(Field::Reviewer)),
                    vec![
                        Badge::WaitingReason(wait_reason),
                        Badge::CiStatus(&i.ci_status),
                    ],
                ),
                &i.created,
            ));
        }
    }
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Copy)]
pub enum RollupPosition {
    Next(usize),
    Nth(usize),
}

#[derive(Ord, PartialOrd, Eq, PartialEq, Clone, Copy)]
enum QueuedSortKey<'a> {
    Rollup(RollupPosition),
    Normal(usize),
    Other(&'a Timestamp),
}

struct QueuedPrBox<'a>(&'a [Pr]);

impl<'a> PrBox for QueuedPrBox<'a> {
    type SortKey = QueuedSortKey<'a>;

    fn title(&self) -> impl AsRef<str> {
        "Queued"
    }

    fn render(&self, res: &mut Vec<(Markup, Self::SortKey)>) {
        let mut rollups = BTreeMap::<RollupPosition, (Vec<(Markup, _)>, _, _)>::new();

        for i in self.0 {
            let PrStatus::Queued(QueuedInfo {
                approvers,
                rollup_setting,
                queue_status,
            }) = &i.status
            else {
                continue;
            };

            // TODO: draft should store whether it's yours or someone elses
            // if someone elses, show author
            let skeleton = pr_skeleton(
                i,
                iter::once(Field::Author(&i.author)).chain(approvers.iter().map(Field::Approver)),
                vec![
                    Badge::RollupSetting(rollup_setting),
                    Badge::QueueStatus(queue_status),
                    Badge::CiStatus(&i.ci_status),
                ],
            );

            match queue_status {
                QueueStatus::Unknown => res.push((skeleton, QueuedSortKey::Other(&i.created))),
                QueueStatus::InQueue { position } => {
                    res.push((skeleton, QueuedSortKey::Normal(*position)))
                }
                QueueStatus::Running => res.push((skeleton, QueuedSortKey::Normal(0))),
                QueueStatus::InNextRollup {
                    position,
                    pr_link,
                    pr_number,
                } => rollups
                    .entry(RollupPosition::Next(*position))
                    .or_insert((Vec::new(), pr_link.clone(), *pr_number))
                    .0
                    .push((skeleton, &i.created)),
                QueueStatus::InRollup {
                    nth_rollup,
                    pr_link,
                    pr_number,
                } => rollups
                    .entry(RollupPosition::Nth(*nth_rollup))
                    .or_insert((Vec::new(), pr_link.clone(), *pr_number))
                    .0
                    .push((skeleton, &i.created)),
                QueueStatus::InRunningRollup { pr_link, pr_number } => rollups
                    .entry(RollupPosition::Next(0))
                    .or_insert((Vec::new(), pr_link.clone(), *pr_number))
                    .0
                    .push((skeleton, &i.created)),
            }
        }

        for (pos, (mut group, pr_link, pr_number)) in rollups {
            group.sort_by_key(|(_, i)| *i);
            res.push((
                html! {
                    div class="rollup" style=(format!("--num-prs-in-rollup: {};", group.len())) {
                        h4 {a href=(pr_link) target="_blank" rel="noopener noreferrer" {"Rollup #" (pr_number)}}
                        div class="contents" {
                            @for (pr, _) in group {
                                (pr)
                            }
                        }
                    }
                },
                QueuedSortKey::Rollup(pos),
            ));
        }
    }
}

struct DraftPrBox<'a>(&'a [Pr]);

impl<'a> PrBox for DraftPrBox<'a> {
    type SortKey = &'a Timestamp;

    fn title(&self) -> impl AsRef<str> {
        "Drafts"
    }

    fn render(&self, res: &mut Vec<(Markup, &'a Timestamp)>) {
        for i in self.0 {
            let PrStatus::Draft {} = &i.status else {
                continue;
            };

            res.push((
                // TODO: draft should store whether it's yours or someone elses
                // if someone elses, show author
                pr_skeleton(i, iter::once(Field::Author(&i.author)), vec![]),
                &i.created,
            ));
        }
    }
}

fn render_pr_box(pr_box: impl PrBox) -> Markup {
    let mut res = Vec::new();
    pr_box.render(&mut res);
    res.sort_by_key(|(_, i)| *i);

    html! {
        div class="prbox" {
            h1 { (pr_box.title().as_ref()) }
            div class="prs" {
                @for (pr, _) in res {
                    (pr)
                }
            }
        }
    }
}

impl Render for RollupSetting {
    fn render(&self) -> Markup {
        match self {
            RollupSetting::Never => html! {"rollup=never"},
            RollupSetting::Always => html! {},
            RollupSetting::Iffy => html! {"rollup=iffy"},
            RollupSetting::Unset => html! {},
        }
    }
}

struct Ordinal(usize);
impl Render for Ordinal {
    fn render(&self) -> Markup {
        let number = self.0;
        match number % 10 {
            1 => html! {(format!("{number}st"))},
            2 => html! {(format!("{number}nd"))},
            3 => html! {(format!("{number}rd"))},
            _ => html! {(format!("{number}th"))},
        }
    }
}

impl Render for QueueStatus {
    fn render(&self) -> Markup {
        match self {
            QueueStatus::InQueue { position } => html! {span {(Ordinal(*position)) " in queue"}},
            QueueStatus::Running => html! {span {"running"}},
            QueueStatus::InRollup { nth_rollup: 0, .. } => html! {"in next rollup"},
            QueueStatus::InRollup { nth_rollup, .. } => {
                html! {"in "(Ordinal(nth_rollup - 1))" rollup"}
            }
            QueueStatus::InRunningRollup { .. } => html! {span {"in running rollup"}},
            QueueStatus::InNextRollup { position, .. } => {
                html! {span {"rollup " (Ordinal(*position))" in queue"}}
            }
            QueueStatus::Unknown => html! {},
        }
    }
}

pub enum Badge<'a> {
    CiStatus(&'a CiStatus),
    WaitingReason(&'a WaitingReason),
    RollupSetting(&'a RollupSetting),
    QueueStatus(&'a QueueStatus),
}

impl Render for Badge<'_> {
    fn render(&self) -> Markup {
        fn maybe_badge(m: impl Render) -> Markup {
            let r = m.render();
            if r.clone().into_string().is_empty() {
                html! {}
            } else {
                html! {
                    div class="status-badge" {
                        (r)
                    }
                }
            }
        }

        match self {
            Badge::CiStatus(ci_status) => ci_status.render(),
            Badge::WaitingReason(waiting_reason) => maybe_badge(waiting_reason),
            Badge::RollupSetting(rollup_setting) => maybe_badge(rollup_setting),
            Badge::QueueStatus(queue_status) => maybe_badge(queue_status),
        }
    }
}

impl Render for Author {
    fn render(&self) -> Markup {
        html! {
            a class="author" href=(self.profile_url)
                target="_blank" rel="noopener noreferrer"
            {
                img class="avatar" src=(self.avatar_url) alt=(format!("{}'s profile picture", self.name))
                span class="name" {(self.name)}
            }
        }
    }
}

impl Render for CiStatus {
    fn render(&self) -> Markup {
        match self {
            CiStatus::Conflicted => html! {
                div class="ci-status conflict" title=("conflicted") { (WARN) "conflict" }
            },
            CiStatus::Good => html! {
                div class="ci-status good" title=("passing") { (CHECKMARK) }
            },
            CiStatus::Running => html! {
                div class="ci-status progress" title=("in progress") { (PROGRESS) }
            },
            CiStatus::Bad => html! {
                div class="ci-status bad" title=("failing") { (CROSS) }
            },
            CiStatus::Unknown => html! {},
            CiStatus::Draft => html! {},
        }
    }
}

impl Render for WaitingReason {
    fn render(&self) -> Markup {
        match self {
            WaitingReason::Blocked => html! {
                "blocked"
            },
            WaitingReason::TryBuild() => html! {},
            WaitingReason::PerfRun() => html! {},
            WaitingReason::CraterRun(crater_status) => match crater_status {
                CraterStatus::Unknown => html! {
                    "running crater, context unknown (bug?)"
                },
                CraterStatus::Queued { num_before } => html! {
                    (format!("in crater queue ({} queued before this)", num_before))
                },
                CraterStatus::GeneratingReport => html! {
                    "generating crater report"
                },
                CraterStatus::Running { expected_end } => {
                    let duration = expected_end.duration_since(Timestamp::now());
                    let span = Span::try_from(duration).unwrap();

                    let options = SpanRound::new()
                        .largest(Unit::Week)
                        .smallest(Unit::Hour)
                        .days_are_24_hours();

                    html! {
                        (format!("crater experiment done in {:#}", span.round(options).unwrap()))
                    }
                }
            },
            WaitingReason::Fcp(fcp_status) => {
                // TODO: FCP concerns
                let duration = fcp_status.ends_on().duration_since(Timestamp::now());
                let span = Span::try_from(duration).unwrap();

                let options = SpanRound::new()
                    .largest(Unit::Week)
                    .smallest(Unit::Hour)
                    .days_are_24_hours();

                html! {
                    span {(format!("FCP ends in {:#}", span.round(options).unwrap()))}
                }
            }
            WaitingReason::Author => html! {
                "Waiting for author"
            },
            WaitingReason::Review => html! {
                "Waiting for review"
            },
            WaitingReason::Unknown => html! {},
        }
    }
}

pub enum Field<'a> {
    Reviewer(&'a Author),
    Author(&'a Author),
    Approver(&'a Author),
    OtherReviewer(&'a Author),
}

impl Render for Field<'_> {
    fn render(&self) -> Markup {
        match self {
            Field::Reviewer(author) => field("Reviewer", author),
            Field::Author(author) => field("Author", author),
            Field::OtherReviewer(author) => field("Other reviewer", author),
            // TODO: should be bors approver
            Field::Approver(author) => field("Approver", author),
        }
    }
}

fn pr_skeleton<'a>(
    pr: &Pr,
    fields: impl IntoIterator<Item = Field<'a>>,
    badges: impl IntoIterator<Item = Badge<'a>>,
) -> Markup {
    html! {
        div class="pr" {
            h2 class="title" { a target="_blank" rel="noopener noreferrer" href=(pr.link) {
                (pr.title)
            }}

            a class="pr-link" target="_blank" rel="noopener noreferrer" href=(pr.link) {
                (pr.repo) "#" (pr.number)
            }

            div class="fields" {
                @for field in fields {
                    (field)
                }

                div class="badges" {
                    @for badge in badges {
                        (badge)
                    }
                }
            }
        }
    }
}
