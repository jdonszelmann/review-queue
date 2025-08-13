use std::sync::Arc;

use axum::{
    extract::{
        State, WebSocketUpgrade,
        ws::{Message, Utf8Bytes},
    },
    response::{IntoResponse, Redirect, Response},
};
use futures_util::{
    sink::SinkExt,
    stream::{SplitSink, SplitStream, StreamExt},
};
use maud::{DOCTYPE, Markup, PreEscaped, html};
use octocrab::models::Author;
use tokio::{select, spawn, time::sleep};

use crate::{
    AppState, REFRESH_RATE,
    auth::ExtractLoginContext,
    get_and_update_state, get_state_instantly,
    model::{BackendStatus, CiStatus, LoginContext, Pr, PrBoxKind},
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

pub fn field(label: impl AsRef<str>, value: Markup) -> Markup {
    html! {
        div class="field" {
            span class="label" { (label.as_ref()) }
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
                    let page = queue_page_main(&prs).await;
                    let _ = send.send(Message::Text(Utf8Bytes::from(&page.into_string()))).await;
                    sleep(REFRESH_RATE).await;
                }
            } => {}
        };
    })
    .into_response()
}

pub async fn queue_page_main(prs: &[Pr]) -> Markup {
    html! {
        main id="main" {
            (render_pr_box(prs, PrBoxKind::WorkReady).await)
            (render_pr_box(prs, PrBoxKind::TodoReview).await)
            (render_pr_box(prs, PrBoxKind::Stalled).await)
            (render_pr_box(prs, PrBoxKind::Queue).await)
            (render_pr_box(prs, PrBoxKind::Draft).await)
            (render_pr_box(prs, PrBoxKind::Other).await)
        }
    }
}

pub async fn queue_page(ExtractLoginContext(config): ExtractLoginContext) -> Response {
    tracing::info!("{config:?}");
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

        (queue_page_main(&prs).await)

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

pub async fn render_pr_box(prs: &[Pr], kind: PrBoxKind) -> Markup {
    let mut prs = prs
        .iter()
        .filter(|pr| pr.sort() == kind)
        .cloned()
        .collect::<Vec<_>>();

    if prs.is_empty() {
        return html! {};
    }

    prs.sort_by_cached_key(|pr| pr.title.clone());

    html! {
        div class="prbox" {
            h1 { (kind) }
            div class="prs" {
                @for pr in prs {
                    (render_pr(&pr).await)
                }
            }
        }
    }
}

pub fn render_badges(pr: &Pr) -> Markup {
    let mut badges = Vec::new();

    if let Some(badge) = pr.badge() {
        badges.push(html! {
            div class="status-badge" {
                (badge)
            }
        })
    }

    match pr.ci_status {
        CiStatus::Conflicted => badges.push(html! {
            div class="ci-status conflict" title=(pr.ci_status) { (WARN) "conflict" }
        }),
        CiStatus::Good => badges.push(html! {
            div class="ci-status good" title=(pr.ci_status) { (CHECKMARK) }
        }),
        CiStatus::Running => badges.push(html! {
            div class="ci-status progress" title=(pr.ci_status) { (PROGRESS) }
        }),
        CiStatus::Bad => badges.push(html! {
            div class="ci-status bad" title=(pr.ci_status) { (CROSS) }
        }),
        CiStatus::Unknown => {}
        CiStatus::Draft => {}
    }

    if badges.is_empty() {
        html!()
    } else {
        html! {
            div class="badges" {
                @for badge in badges {
                    (badge)
                }
            }
        }
    }
}

pub async fn render_pr(pr: &Pr) -> Markup {
    html! {
        div class="pr" {
            h2 class="title" { a target="_blank" rel="noopener noreferrer" href=(pr.link) {
                (pr.title)
            }}

            a class="pr-link" href=(pr.link) {
                (pr.repo.owner) "/" (pr.repo.name) "#" (pr.number)
            }

            div class="fields" {
                @if let Some(a) = pr.author() {
                    (a)
                }
                @if let Some(r) = pr.reviewers() {
                    (r)
                }

                (render_badges(pr))

                // (pr.ci_state)
            }
        }
    }
}

pub fn render_author(author: &Author) -> Markup {
    html! {
        a class="author" href=(author.url)
            target="_blank" rel="noopener noreferrer"
        {
            img class="avatar" src=(author.avatar_url) alt=(format!("{}'s profile picture", author.login))
            span class="name" {(author.login)}
        }
    }
}
