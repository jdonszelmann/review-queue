use std::sync::Arc;

use axum::{
    body::Body,
    extract::State,
    response::{IntoResponse, Redirect, Response},
};
use maud::{Markup, html};

use crate::{AppState, auth::ExtractLoginContext, queue_page::page_template};

#[axum::debug_handler]
pub async fn home_page(
    State(state): State<Arc<AppState>>,
    ExtractLoginContext(config): ExtractLoginContext,
) -> impl IntoResponse {
    tracing::info!("{config:?}");
    if config.is_some() {
        return Redirect::to("/queue").into_response();
    }

    page_template(html! {
        main class="home" {
            div class="login" {
                h1 {
                    "Review Queue"
                }

                p {
                    "A dashboard with all PRs you're associated with as an author or reviewer, and their statusses"
                }

                p {
                    "by Jana Dönszelmann"
                }

                div class="button-container" {
                    a href="/auth/github/login" {"Login with GitHub"}
                }
            }
        }
    }).into_response()
}
