use crate::{
    api::repositories::AuthRepository,
    auth::{
        expired_session_cookie_with_policy, session_cookie_with_policy, session_token_from_headers,
        AuthenticatedUser,
    },
    state::{AppState, BROWSER_AUTH_SESSION_TTL},
};
use axum::{
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::{Duration, Utc};
use uuid::Uuid;

use crate::api::auth::{CreateLinkCodeResponse, OidcLoginResponse};

pub struct AuthService;

impl AuthService {
    fn auth_repo(state: &AppState) -> AuthRepository<'_> {
        AuthRepository::new(state)
    }

    pub async fn login(state: &AppState, email: String, password: String) -> Response {
        tracing::debug!(email = %email, "password login requested");
        let Some(token) = state.login_password(&email, &password).await else {
            tracing::warn!(email = %email, "password login rejected");
            return StatusCode::UNAUTHORIZED.into_response();
        };
        tracing::info!(email = %email, "password login accepted");

        (
            StatusCode::OK,
            [(
                axum::http::header::SET_COOKIE,
                session_cookie_with_policy(
                    &token,
                    state.cookie_security(),
                    Some(BROWSER_AUTH_SESSION_TTL),
                ),
            )],
            axum::Json(serde_json::json!({ "ok": true })),
        )
            .into_response()
    }

    pub async fn logout(state: &AppState, headers: HeaderMap) -> Response {
        tracing::debug!("logout requested");
        if let Some(token) = session_token_from_headers(&headers) {
            state.logout(token).await;
            tracing::info!("session logged out");
        }

        (
            StatusCode::NO_CONTENT,
            [(
                axum::http::header::SET_COOKIE,
                expired_session_cookie_with_policy(state.cookie_security()),
            )],
        )
            .into_response()
    }

    pub async fn me(state: &AppState, headers: HeaderMap) -> Response {
        let Some(user) = authenticate_user(state, &headers).await else {
            tracing::debug!("auth me rejected missing or invalid session");
            return StatusCode::UNAUTHORIZED.into_response();
        };
        tracing::debug!(user_id = %user.user_id, "auth me accepted");
        axum::Json(user).into_response()
    }

    pub async fn oidc_login(state: &AppState, provider_id: String) -> Response {
        tracing::debug!(%provider_id, "oidc login requested");
        let repo = Self::auth_repo(state);
        let Ok(Some(provider)) = repo.find_oidc_provider(&provider_id).await else {
            let has_db = state.db_pool().is_some();
            if has_db {
                tracing::warn!(%provider_id, "oidc login requested without provider");
                return StatusCode::NOT_FOUND.into_response();
            }
            tracing::warn!(%provider_id, "oidc login requested without database");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        };

        let state_token = Uuid::new_v4().to_string();
        let nonce = Uuid::new_v4().to_string();
        let expires_at = Utc::now() + Duration::minutes(10);

        if repo
            .create_oidc_login_state(
                &state_token,
                &provider.provider_id,
                &nonce,
                None,
                Some("/"),
                expires_at,
            )
            .await
            .is_err()
        {
            tracing::error!(%provider_id, "failed to create oidc login state");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        }

        axum::Json(OidcLoginResponse {
            provider_id: provider.provider_id.clone(),
            state: state_token.clone(),
            nonce: nonce.clone(),
            authorization_url: oidc_authorization_url(&provider, &state_token, &nonce),
        })
        .into_response()
    }

    #[allow(clippy::too_many_arguments)]
    pub async fn oidc_callback(
        state: &AppState,
        provider_id: String,
        request_state: String,
        subject: String,
        email: String,
        display_name: Option<String>,
    ) -> Response {
        if !Self::unsafe_dev_oidc_callback_enabled() {
            tracing::warn!(%provider_id, "unsafe development oidc callback rejected because flag is disabled");
            return StatusCode::NOT_IMPLEMENTED.into_response();
        }

        if state.db_pool().is_none() {
            tracing::warn!(%provider_id, "oidc callback requested without database");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }

        let repo = Self::auth_repo(state);
        let Ok(Some(login_state)) = repo
            .consume_oidc_login_state(&provider_id, &request_state, Utc::now())
            .await
        else {
            tracing::warn!(%provider_id, "oidc callback state rejected");
            return StatusCode::UNAUTHORIZED.into_response();
        };

        if login_state.provider_id != provider_id {
            tracing::warn!(%provider_id, login_state_provider_id = %login_state.provider_id, "oidc callback provider mismatch");
            return StatusCode::UNAUTHORIZED.into_response();
        }

        let Ok(user) = repo
            .upsert_oidc_identity(&provider_id, &subject, &email, display_name.as_deref())
            .await
        else {
            tracing::error!(%provider_id, "failed to bind oidc identity");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };

        let token = state
            .create_authenticated_session(AuthenticatedUser {
                user_id: user.user_id,
                email: user.email,
                display_name: user.display_name,
            })
            .await;

        (
            StatusCode::OK,
            [(
                axum::http::header::SET_COOKIE,
                session_cookie_with_policy(
                    &token,
                    state.cookie_security(),
                    Some(BROWSER_AUTH_SESSION_TTL),
                ),
            )],
            axum::Json(serde_json::json!({ "ok": true })),
        )
            .into_response()
    }

    pub async fn create_link_code(
        state: &AppState,
        connector_id: String,
        external_account_id: String,
        display_name: Option<String>,
    ) -> Response {
        if !Self::unsafe_dev_link_code_creation_enabled() {
            tracing::warn!(
                "unsafe development link-code creation rejected because flag is disabled"
            );
            return StatusCode::NOT_IMPLEMENTED.into_response();
        }
        if state.db_pool().is_none() {
            tracing::warn!("link-code creation requested without database");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }

        let code = Uuid::new_v4().to_string();
        let expires_at = Utc::now() + Duration::minutes(15);

        let Ok(link_code) = Self::auth_repo(state)
            .create_connector_link_code(
                &code,
                &connector_id,
                &external_account_id,
                display_name.as_deref(),
                expires_at,
            )
            .await
        else {
            tracing::error!(connector_id = %connector_id, "failed to create connector link code");
            return StatusCode::INTERNAL_SERVER_ERROR.into_response();
        };

        (
            StatusCode::CREATED,
            axum::Json(CreateLinkCodeResponse {
                code: link_code.code,
                expires_at: link_code.expires_at,
            }),
        )
            .into_response()
    }

    pub async fn consume_link_code(state: &AppState, headers: HeaderMap, code: String) -> Response {
        let Some(user) = authenticate_user(state, &headers).await else {
            tracing::debug!("link-code consume rejected missing or invalid session");
            return StatusCode::UNAUTHORIZED.into_response();
        };

        if state.db_pool().is_none() {
            tracing::warn!(user_id = %user.user_id, "link-code consume requested without database");
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }

        let repo = Self::auth_repo(state);
        let account = match repo
            .consume_connector_link_code(&code, user.user_id, Utc::now())
            .await
        {
            Ok(Some(account)) => account,
            Ok(None) => {
                tracing::warn!(user_id = %user.user_id, "link-code consume rejected");
                return StatusCode::NOT_FOUND.into_response();
            }
            Err(error) => {
                tracing::error!(user_id = %user.user_id, %error, "failed to consume connector link code");
                return StatusCode::INTERNAL_SERVER_ERROR.into_response();
            }
        };

        tracing::info!(
            user_id = %user.user_id,
            connector_id = %account.connector_id,
            "connector account linked"
        );
        axum::Json(serde_json::json!({
            "connector_id": account.connector_id,
            "external_account_id": account.external_account_id,
            "display_name": account.display_name,
        }))
        .into_response()
    }

    pub fn unsafe_dev_oidc_callback_enabled() -> bool {
        env_flag_enabled("AGENTER_UNSAFE_DEV_OIDC_CALLBACK")
    }

    pub fn unsafe_dev_link_code_creation_enabled() -> bool {
        env_flag_enabled("AGENTER_UNSAFE_DEV_LINK_CODE_CREATION")
    }
}

async fn authenticate_user(state: &AppState, headers: &HeaderMap) -> Option<AuthenticatedUser> {
    let token = session_token_from_headers(headers)?;
    state.authenticated_user(token).await
}

fn oidc_authorization_url(
    provider: &agenter_db::models::OidcProvider,
    state: &str,
    nonce: &str,
) -> String {
    format!(
        "{}/authorize?client_id={}&response_type=code&scope={}&state={}&nonce={}",
        provider.issuer_url.trim_end_matches('/'),
        provider.client_id,
        provider.scopes.join("%20"),
        state,
        nonce
    )
}

fn env_flag_enabled(name: &str) -> bool {
    std::env::var(name).is_ok_and(|value| matches!(value.as_str(), "1" | "true" | "TRUE" | "True"))
}
