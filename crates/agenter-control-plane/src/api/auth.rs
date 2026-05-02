use axum::{
    extract::{Path, State},
    http::HeaderMap,
    response::Response,
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};

use super::services::auth::AuthService;

#[derive(Debug, Deserialize)]
pub(super) struct PasswordLoginRequest {
    pub(super) email: String,
    pub(super) password: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct OidcCallbackRequest {
    pub(super) state: String,
    pub(super) subject: String,
    pub(super) email: String,
    pub(super) display_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct OidcLoginResponse {
    pub(super) provider_id: String,
    pub(super) state: String,
    pub(super) nonce: String,
    pub(super) authorization_url: String,
}

#[derive(Debug, Deserialize)]
pub(super) struct CreateLinkCodeRequest {
    pub(super) connector_id: String,
    pub(super) external_account_id: String,
    pub(super) display_name: Option<String>,
}

#[derive(Debug, Serialize)]
pub(super) struct CreateLinkCodeResponse {
    pub(super) code: String,
    pub(super) expires_at: chrono::DateTime<chrono::Utc>,
}

pub(super) fn router() -> Router<crate::state::AppState> {
    Router::new()
        .route("/password/login", post(auth_login))
        .route("/password/logout", post(auth_logout))
        .route("/me", get(auth_me))
        .route("/oidc/{provider_id}/login", get(oidc_login))
        .route("/oidc/{provider_id}/callback", post(oidc_callback))
        .route("/link-codes", post(create_link_code))
        .route("/link/{code}", post(consume_link_code))
}

pub(super) async fn auth_login(
    State(state): State<crate::state::AppState>,
    Json(request): Json<PasswordLoginRequest>,
) -> Response {
    AuthService::login(&state, request.email, request.password).await
}

pub(super) async fn auth_logout(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
) -> Response {
    AuthService::logout(&state, headers).await
}

pub(super) async fn auth_me(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
) -> Response {
    AuthService::me(&state, headers).await
}

pub(super) async fn oidc_login(
    State(state): State<crate::state::AppState>,
    Path(provider_id): Path<String>,
) -> Response {
    AuthService::oidc_login(&state, provider_id).await
}

pub(super) async fn oidc_callback(
    State(state): State<crate::state::AppState>,
    Path(provider_id): Path<String>,
    Json(request): Json<OidcCallbackRequest>,
) -> Response {
    AuthService::oidc_callback(
        &state,
        provider_id,
        request.state,
        request.subject,
        request.email,
        request.display_name,
    )
    .await
}

pub(super) async fn create_link_code(
    State(state): State<crate::state::AppState>,
    Json(request): Json<CreateLinkCodeRequest>,
) -> Response {
    AuthService::create_link_code(
        &state,
        request.connector_id,
        request.external_account_id,
        request.display_name,
    )
    .await
}

pub(super) async fn consume_link_code(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Path(code): Path<String>,
) -> Response {
    AuthService::consume_link_code(&state, headers, code).await
}
