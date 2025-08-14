use std::sync::Arc;

use axum::{
    extract::State,
    response::{IntoResponse, Redirect},
};
use maud::html;

use crate::{AppState, pages::auth::ExtractLoginContext, pages::queue::page_template};

pub async fn home_page(
    State(..): State<Arc<AppState>>,
    ExtractLoginContext(config): ExtractLoginContext,
) -> impl IntoResponse {
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
