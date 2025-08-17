use std::{collections::HashMap, sync::Arc};

use crate::{
    db::{MacroRoot, User},
    login_cx::LoginContext,
    model::{Repo, RepoInfo},
    pages::queue::page_template,
};
use axum::{
    RequestPartsExt,
    extract::{FromRequestParts, Query, State},
    http::request::Parts,
    response::{IntoResponse, Redirect, Response},
};
use axum_extra::extract::{
    CookieJar,
    cookie::{Cookie, Expiration, SameSite},
};
use maud::{PreEscaped, html};
use oauth2::{
    AuthUrl, AuthorizationCode, ClientId, ClientSecret, CsrfToken, PkceCodeChallenge,
    PkceCodeVerifier, TokenResponse, TokenUrl, basic::BasicClient,
};
use octocrab::Octocrab;
use reqwest::{Client, StatusCode};
use rust_query::FromExpr;
use tokio::task::spawn_blocking;
use url::Url;

use crate::{AppState, db::OauthState};

pub struct LoginError;

impl IntoResponse for LoginError {
    fn into_response(self) -> axum::response::Response {
        (
            // set status code
            StatusCode::INTERNAL_SERVER_ERROR,
            // and finally the body
            "internal server error",
        )
            .into_response()
    }
}

macro_rules! get_client {
    ($config: expr) => {
        BasicClient::new(ClientId::new($config.oauth_client_id.clone()))
            .set_client_secret(ClientSecret::new($config.oauth_client_secret.clone()))
            .set_auth_uri(
                AuthUrl::new("https://github.com/login/oauth/authorize".to_string()).unwrap(),
            )
            .set_token_uri(
                TokenUrl::new("https://github.com/login/oauth/access_token".to_string())
                    .expect("Invalid token endpoint URL"),
            )
            .set_redirect_uri(
                oauth2::RedirectUrl::new(format!("{}/auth/github/callback", $config.host)).unwrap(),
            )
    };
}

pub async fn logout(State(_): State<Arc<AppState>>, jar: CookieJar) -> impl IntoResponse {
    (jar.remove("github-token"), Redirect::to("/"))
}

pub async fn login(
    State(state): State<Arc<AppState>>,
    Query(mut params): Query<HashMap<String, String>>,
) -> Result<Redirect, LoginError> {
    let client = get_client!(state.config);

    let return_url = params
        .remove("return_url")
        .unwrap_or_else(|| "/queue".to_string());

    let (pkce_code_challenge, pkce_code_verifier) = PkceCodeChallenge::new_random_sha256();

    let (mut authorize_url, csrf_state) = client
        .authorize_url(oauth2::CsrfToken::new_random)
        .add_scope(oauth2::Scope::new("user:email".to_string()))
        .add_scope(oauth2::Scope::new("read:user".to_string()))
        .add_scope(oauth2::Scope::new("read:org".to_string()))
        .add_scope(oauth2::Scope::new("public_repo".to_string()))
        .set_pkce_challenge(pkce_code_challenge)
        .url();
    authorize_url
        .query_pairs_mut()
        .append_pair("prompt", "consent");

    state.db.transaction_mut_ok(|txn| {
        txn.insert(OauthState {
            csrf: csrf_state.secret(),
            pkcs: pkce_code_verifier.secret(),
            return_url,
        })
        .expect("csrf is expected to be unique every time");
    });

    Ok(Redirect::to(authorize_url.as_str()))
}

pub async fn callback(
    State(state): State<Arc<AppState>>,
    Query(mut params): Query<HashMap<String, String>>,
    jar: CookieJar,
) -> Result<impl IntoResponse, LoginError> {
    let state_param = CsrfToken::new(params.remove("state").ok_or_else(|| {
        tracing::error!("oauth response without state");
        LoginError
    })?);
    let code_param = AuthorizationCode::new(params.remove("code").ok_or_else(|| {
        tracing::error!("oauth response without code");
        LoginError
    })?);

    let Some(res) = state.db.transaction_mut_ok(|txn| {
        // get the row (id)
        let info = txn.query_one(OauthState::unique(state_param.secret()));

        if let Some(info) = info {
            // query the pkcs and return url
            let data: OauthState!(pkcs, return_url) = txn.query_one(FromExpr::from_expr(info));
            // delete the record
            txn.downgrade().delete(info).unwrap();
            Some(data)
        } else {
            None
        }
    }) else {
        tracing::error!("database");
        return Err(LoginError);
    };

    let pkce_code_verifier = PkceCodeVerifier::new(res.pkcs);

    let client = get_client!(state.config);

    let token_response = match client
        .exchange_code(code_param)
        .set_pkce_verifier(pkce_code_verifier)
        .request_async(&Client::new())
        .await
    {
        Ok(i) => i,
        Err(e) => {
            tracing::error!("{e}");
            return Err(LoginError);
        }
    };
    let access_token = token_response.access_token().secret();

    let mut cookie = Cookie::new("github-token", access_token.clone());
    cookie.set_same_site(SameSite::Strict);
    cookie.set_path("/");
    cookie.set_http_only(true);
    cookie.set_expires(Expiration::Session);
    cookie.set_secure(true);

    Ok((
        jar.add(cookie),
        page_template(html! {
            script {
                (PreEscaped(format!("window.location = '{}'", res.return_url)))
            }
        }),
    ))
}

pub struct ExtractLoginContext(pub Option<Arc<LoginContext>>);

impl FromRequestParts<Arc<AppState>> for ExtractLoginContext {
    type Rejection = Response;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        let jar = parts
            .extract::<CookieJar>()
            .await
            .map_err(|err| err.into_response())?;

        let Some(token) = jar.get("github-token") else {
            tracing::debug!("no cookies: {jar:?}");
            return Ok(Self(None));
        };

        let octocrab = Octocrab::builder()
            .personal_token(token.value())
            .build()
            .map_err(|e| {
                tracing::error!("{e}");
                LoginError.into_response()
            })?;

        let user: octocrab::models::SimpleUser =
            octocrab.get("/user", None::<&()>).await.map_err(|e| {
                tracing::error!("{e}");
                LoginError.into_response()
            })?;

        spawn_blocking({
            let user = user.clone();
            let state = state.clone();
            move || {
                state.db.transaction_mut_ok(|txn| {
                    txn.find_or_insert(User {
                        username: user.login.clone(),
                        refresh_rate_seconds: 2 * 60,
                        sequence_number: 0,
                    });
                });
            }
        })
        .await
        .unwrap();

        Ok(Self(Some(Arc::new(LoginContext {
            octocrab,
            username: user.login,
            repos: vec![RepoInfo {
                repo: Repo {
                    owner: "rust-lang".to_string(),
                    name: "rust".to_string(),
                },
                bors_queue_url: Some(Url::parse("https://bors.rust-lang.org/queue/rust").unwrap()),
            }],
            state: state.clone(),
        }))))
    }
}
