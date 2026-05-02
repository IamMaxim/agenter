use axum::http::StatusCode;
use axum::{
    extract::{ws::WebSocketUpgrade, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};

pub(super) async fn browser_ws_authenticated(
    ws: WebSocketUpgrade,
    state: State<crate::state::AppState>,
    headers: HeaderMap,
) -> Response {
    let state = state.0;
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    crate::browser_ws::handler(ws, State(state), user.user_id).await
}
