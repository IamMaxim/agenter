use std::{env, net::SocketAddr};

use axum::{routing::get, Router};

use crate::{browser_ws, runner_ws, state::AppState};

pub const DEFAULT_BIND_ADDR: &str = "127.0.0.1:7777";
pub const DEFAULT_DEV_RUNNER_TOKEN: &str = "dev-runner-token";

pub fn app(state: AppState) -> Router {
    Router::new()
        .route("/healthz", get(healthz))
        .route("/api/runner/ws", get(runner_ws::handler))
        .route("/api/browser/ws", get(browser_ws::handler))
        .with_state(state)
}

pub async fn serve() -> anyhow::Result<()> {
    let _ = tracing_subscriber::fmt::try_init();

    let bind_addr = env::var("AGENTER_BIND_ADDR").unwrap_or_else(|_| DEFAULT_BIND_ADDR.to_owned());
    let bind_addr: SocketAddr = bind_addr.parse()?;
    let runner_token = env::var("AGENTER_DEV_RUNNER_TOKEN")
        .unwrap_or_else(|_| DEFAULT_DEV_RUNNER_TOKEN.to_owned());

    let listener = tokio::net::TcpListener::bind(bind_addr).await?;
    tracing::info!(%bind_addr, "agenter control plane listening");
    axum::serve(listener, app(AppState::new(runner_token))).await?;

    Ok(())
}

async fn healthz() -> &'static str {
    "ok"
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
    use futures_util::{SinkExt, StreamExt};
    use tokio::net::TcpListener;
    use tokio_tungstenite::{connect_async, tungstenite::Message};

    use super::*;
    use crate::runner_ws::smoke_session_id;

    #[tokio::test]
    async fn smoke_routes_runner_events_to_subscribed_browser() {
        let state = AppState::new("dev-token".to_owned());
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

        let (browser_socket, _) = connect_async(format!("ws://{addr}/api/browser/ws"))
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

        assert!(matches!(event.event, AppEvent::AgentMessageDelta(_)));
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
