use std::{env, net::SocketAddr};

use axum::{
    extract::{ws::WebSocketUpgrade, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

use crate::{auth, browser_ws, runner_ws, state::AppState};

pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:7777";
pub const DEFAULT_DEV_RUNNER_TOKEN: &str = "dev-runner-token";

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/auth/password/login", post(auth_login))
        .route("/api/auth/password/logout", post(auth_logout))
        .route("/api/auth/me", get(auth_me))
        .route("/api/runner/ws", get(runner_ws::handler))
        .route("/api/browser/ws", get(browser_ws_authenticated))
        .with_state(state)
}

pub async fn serve() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    let bind_addr = env::var("AGENTER_BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_owned());
    let bind_addr: SocketAddr = bind_addr.parse()?;
    let runner_token = env::var("AGENTER_DEV_RUNNER_TOKEN")
        .unwrap_or_else(|_| DEFAULT_DEV_RUNNER_TOKEN.to_owned());

    let state = match (
        env::var("AGENTER_BOOTSTRAP_ADMIN_EMAIL"),
        env::var("AGENTER_BOOTSTRAP_ADMIN_PASSWORD"),
    ) {
        (Ok(email), Ok(password)) => {
            AppState::new_with_bootstrap_admin(runner_token, email, password)?
        }
        _ => AppState::new(runner_token),
    };

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(%bind_addr, "agenter control plane listening");
    axum::serve(listener, app(state)).await?;

    Ok(())
}

async fn healthz() -> &'static str {
    "ok"
}

#[derive(Debug, Deserialize)]
struct PasswordLoginRequest {
    email: String,
    password: String,
}

async fn auth_login(
    State(state): State<AppState>,
    Json(request): Json<PasswordLoginRequest>,
) -> Response {
    let Some(token) = state
        .login_password(&request.email, &request.password)
        .await
    else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    (
        StatusCode::OK,
        [(axum::http::header::SET_COOKIE, auth::session_cookie(&token))],
        Json(serde_json::json!({ "ok": true })),
    )
        .into_response()
}

async fn auth_logout(State(state): State<AppState>, headers: HeaderMap) -> Response {
    if let Some(token) = auth::session_token_from_headers(&headers) {
        state.logout(token).await;
    }

    (
        StatusCode::NO_CONTENT,
        [(
            axum::http::header::SET_COOKIE,
            auth::expired_session_cookie(),
        )],
    )
        .into_response()
}

async fn auth_me(State(state): State<AppState>, headers: HeaderMap) -> Response {
    let Some(user) = authenticated_user_from_headers(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };

    Json(user).into_response()
}

async fn browser_ws_authenticated(
    ws: WebSocketUpgrade,
    State(state): State<AppState>,
    headers: HeaderMap,
) -> Response {
    if authenticated_user_from_headers(&state, &headers)
        .await
        .is_none()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    browser_ws::handler(ws, State(state)).await
}

async fn authenticated_user_from_headers(
    state: &AppState,
    headers: &HeaderMap,
) -> Option<auth::AuthenticatedUser> {
    let token = auth::session_token_from_headers(headers)?;
    state.authenticated_user(token).await
}

#[cfg(test)]
mod tests {
    use agenter_core::{
        AgentCapabilities, AgentMessageDeltaEvent, AgentProviderId, AppEvent, RunnerId, WorkspaceId,
    };
    use agenter_protocol::{
        browser::{BrowserClientMessage, BrowserServerMessage, SubscribeSession},
        runner::{
            AgentEvent, AgentProviderAdvertisement, RunnerCapabilities, RunnerClientMessage,
            RunnerEvent, RunnerEventEnvelope, RunnerHello, RunnerServerMessage, PROTOCOL_VERSION,
        },
        RequestId,
    };
    use axum::{
        body::{to_bytes, Body},
        http::{header, Request, StatusCode},
    };
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::{
        connect_async,
        tungstenite::{client::IntoClientRequest, Message},
    };
    use tower::ServiceExt;

    use super::*;
    use crate::runner_ws::smoke_session_id;

    #[tokio::test]
    async fn smoke_routes_runner_events_to_subscribed_browser() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
        )
        .expect("create bootstrap admin");
        let browser_session_token = state
            .login_password("admin@example.test", "correct horse battery staple")
            .await
            .expect("login bootstrap admin");
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read listener address");
        tokio::spawn(async move {
            axum::serve(listener, app(state)).await.expect("serve app");
        });

        let (runner_socket, _) = connect_async(format!("ws://{addr}/api/runner/ws"))
            .await
            .expect("connect runner");
        let (mut runner_sender, mut runner_receiver) = runner_socket.split();
        runner_sender
            .send(Message::Text(
                serde_json::to_string(&RunnerClientMessage::Hello(fake_hello()))
                    .expect("serialize hello")
                    .into(),
            ))
            .await
            .expect("send runner hello");

        let runner_command = runner_receiver
            .next()
            .await
            .expect("runner command frame")
            .expect("runner command websocket result");
        let Message::Text(runner_command) = runner_command else {
            panic!("expected text runner command");
        };
        let RunnerServerMessage::Command(command) =
            serde_json::from_str::<RunnerServerMessage>(&runner_command)
                .expect("decode runner command")
        else {
            panic!("expected runner command");
        };

        let mut browser_request = format!("ws://{addr}/api/browser/ws")
            .into_client_request()
            .expect("build browser websocket request");
        browser_request.headers_mut().insert(
            header::COOKIE,
            format!("{}={browser_session_token}", auth::SESSION_COOKIE_NAME)
                .parse()
                .expect("cookie header"),
        );
        let (browser_socket, _) = connect_async(browser_request)
            .await
            .expect("connect browser");
        let (mut browser_sender, mut browser_receiver) = browser_socket.split();
        browser_sender
            .send(Message::Text(
                serde_json::to_string(&BrowserClientMessage::SubscribeSession(SubscribeSession {
                    request_id: Some(RequestId::from("sub-1")),
                    session_id: smoke_session_id(),
                }))
                .expect("serialize subscribe")
                .into(),
            ))
            .await
            .expect("send browser subscription");

        let browser_ack = browser_receiver
            .next()
            .await
            .expect("browser ack frame")
            .expect("browser ack websocket result");
        assert!(matches!(
            serde_json::from_str::<BrowserServerMessage>(browser_ack.to_text().unwrap()),
            Ok(BrowserServerMessage::Ack(_))
        ));

        runner_sender
            .send(Message::Text(
                serde_json::to_string(&RunnerClientMessage::Event(RunnerEventEnvelope {
                    request_id: Some(command.request_id),
                    event: RunnerEvent::AgentEvent(AgentEvent {
                        session_id: smoke_session_id(),
                        event: AppEvent::AgentMessageDelta(AgentMessageDeltaEvent {
                            session_id: smoke_session_id(),
                            message_id: "agent-1".to_owned(),
                            delta: "hello browser".to_owned(),
                            provider_payload: None,
                        }),
                    }),
                }))
                .expect("serialize runner event")
                .into(),
            ))
            .await
            .expect("send runner event");

        loop {
            let browser_event = browser_receiver
                .next()
                .await
                .expect("browser event frame")
                .expect("browser event websocket result");
            let BrowserServerMessage::Event(event) =
                serde_json::from_str::<BrowserServerMessage>(browser_event.to_text().unwrap())
                    .expect("decode browser event")
            else {
                panic!("expected browser app event");
            };

            if matches!(event.event, AppEvent::AgentMessageDelta(_)) {
                break;
            }
        }
    }

    #[tokio::test]
    async fn auth_me_rejects_missing_session_cookie() {
        let response = app(AppState::new("dev-token".to_owned()))
            .oneshot(
                Request::builder()
                    .uri("/api/auth/me")
                    .body(Body::empty())
                    .expect("build request"),
            )
            .await
            .expect("route request");

        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }

    #[tokio::test]
    async fn browser_websocket_rejects_missing_session_cookie() {
        let state = AppState::new("dev-token".to_owned());
        let listener = TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind test listener");
        let addr = listener.local_addr().expect("read listener address");
        tokio::spawn(async move {
            axum::serve(listener, app(state)).await.expect("serve app");
        });

        let connection = connect_async(format!("ws://{addr}/api/browser/ws")).await;

        assert!(connection.is_err(), "browser websocket should require auth");
    }

    #[tokio::test]
    async fn password_login_sets_session_cookie_and_me_returns_user() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
        )
        .expect("create bootstrap admin");
        let app = app(state);

        let response = app
            .clone()
            .oneshot(
                Request::builder()
                    .method("POST")
                    .uri("/api/auth/password/login")
                    .header(header::CONTENT_TYPE, "application/json")
                    .body(Body::from(
                        serde_json::json!({
                            "email": "admin@example.test",
                            "password": "correct horse battery staple"
                        })
                        .to_string(),
                    ))
                    .expect("build login request"),
            )
            .await
            .expect("route login request");

        assert_eq!(response.status(), StatusCode::OK);
        let cookie = response
            .headers()
            .get(header::SET_COOKIE)
            .expect("login sets cookie")
            .to_str()
            .expect("cookie is ascii")
            .to_owned();
        assert!(cookie.contains("agenter_session="));
        assert!(cookie.contains("HttpOnly"));

        let me_response = app
            .oneshot(
                Request::builder()
                    .uri("/api/auth/me")
                    .header(header::COOKIE, cookie)
                    .body(Body::empty())
                    .expect("build me request"),
            )
            .await
            .expect("route me request");

        assert_eq!(me_response.status(), StatusCode::OK);
        let body = to_bytes(me_response.into_body(), usize::MAX)
            .await
            .expect("read me body");
        let body: serde_json::Value = serde_json::from_slice(&body).expect("json response");
        assert_eq!(body["email"], "admin@example.test");
    }

    fn fake_hello() -> RunnerHello {
        let runner_id = RunnerId::nil();
        RunnerHello {
            runner_id,
            protocol_version: PROTOCOL_VERSION.to_owned(),
            token: "dev-token".to_owned(),
            capabilities: RunnerCapabilities {
                agent_providers: vec![AgentProviderAdvertisement {
                    provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                    capabilities: AgentCapabilities {
                        streaming: true,
                        ..AgentCapabilities::default()
                    },
                }],
                transports: vec!["fake".to_owned()],
                workspace_discovery: false,
            },
            workspaces: vec![agenter_core::WorkspaceRef {
                workspace_id: WorkspaceId::nil(),
                runner_id,
                path: "/tmp/agenter-fake".to_owned(),
                display_name: Some("fake".to_owned()),
            }],
        }
    }
}
