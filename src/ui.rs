use std::sync::Arc;

use axum::extract::State;
use maud::{DOCTYPE, Markup, PreEscaped, html};
use octocrab::models::Author;
use tokio::sync::Mutex;

use crate::{
    AppState, REFRESH_RATE,
    model::{Pr, PrBoxKind},
};

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

pub async fn home(State(state): State<Arc<Mutex<AppState>>>) -> Markup {
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
                    (render_pr_box(state.clone(), PrBoxKind::WorkReady).await)
                    (render_pr_box(state.clone(), PrBoxKind::TodoReview).await)
                    (render_pr_box(state.clone(), PrBoxKind::Stalled).await)
                    (render_pr_box(state.clone(), PrBoxKind::Queue).await)
                    (render_pr_box(state.clone(), PrBoxKind::Other).await)
                }
            }
        }
    }
}

pub async fn render_pr_box(state: Arc<Mutex<AppState>>, kind: PrBoxKind) -> Markup {
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
                    (render_pr(&pr).await)
                }
            }
        }
    }
}

pub async fn render_pr(pr: &Pr) -> Markup {
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

                @if let Some(s) = pr.badge() {
                    div class="badge" {
                        (s)
                    }
                }
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
