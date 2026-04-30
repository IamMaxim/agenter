use agenter_core::{AppEvent, SessionId, SessionInfo, SessionStatus, UserId};
use agenter_protocol::runner::{
    AgentEvent, AgentInput, AgentInputCommand, RunnerClientMessage, RunnerCommand,
    RunnerCommandEnvelope, RunnerEvent, RunnerServerMessage, PROTOCOL_VERSION,
};
use agenter_protocol::RequestId;
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State,
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use uuid::Uuid;

use crate::state::AppState;

const SMOKE_REQUEST_ID: &str = "smoke-input-1";

pub fn smoke_session_id() -> SessionId {
    SessionId::from_uuid(Uuid::from_u128(0x11111111111111111111111111111111))
}

pub async fn handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(socket: WebSocket, state: AppState) {
    let (mut sender, mut receiver) = socket.split();
    let Some(first) = receiver.next().await else {
        return;
    };

    let Ok(Message::Text(text)) = first else {
        return;
    };

    let Ok(RunnerClientMessage::Hello(hello)) = serde_json::from_str::<RunnerClientMessage>(&text)
    else {
        return;
    };

    if hello.protocol_version != PROTOCOL_VERSION || !state.is_runner_token_valid(&hello.token) {
        return;
    }

    let runner = state
        .register_runner(
            hello.runner_id,
            hello.capabilities.clone(),
            hello.workspaces.clone(),
        )
        .await;
    let Some(workspace) = runner.workspaces.first().cloned() else {
        return;
    };
    let Some(provider) = runner.capabilities.agent_providers.first().cloned() else {
        return;
    };
    let session_id = smoke_session_id();
    let session = state
        .create_session(
            session_id,
            runner.runner_id,
            workspace.clone(),
            provider.provider_id.clone(),
        )
        .await;
    state
        .publish_event(
            session_id,
            AppEvent::SessionStarted(SessionInfo {
                session_id: session.session_id,
                owner_user_id: UserId::nil(),
                runner_id: session.runner_id,
                workspace_id: session.workspace.workspace_id,
                provider_id: session.provider_id.clone(),
                status: SessionStatus::Running,
                external_session_id: None,
                title: Some(format!(
                    "Smoke session on {}",
                    session
                        .workspace
                        .display_name
                        .as_deref()
                        .unwrap_or(&session.workspace.path)
                )),
            }),
        )
        .await;
    let command = RunnerServerMessage::Command(Box::new(RunnerCommandEnvelope {
        request_id: RequestId::from(SMOKE_REQUEST_ID),
        command: RunnerCommand::AgentSendInput(AgentInputCommand {
            session_id,
            external_session_id: None,
            input: AgentInput::Text {
                text: "hello from control plane".to_owned(),
            },
        }),
    }));

    if send_server_message(&mut sender, command).await.is_err() {
        return;
    }

    while let Some(message) = receiver.next().await {
        match message {
            Ok(Message::Text(text)) => {
                if let Ok(RunnerClientMessage::Event(envelope)) =
                    serde_json::from_str::<RunnerClientMessage>(&text)
                {
                    if let RunnerEvent::AgentEvent(AgentEvent { session_id, event }) =
                        envelope.event
                    {
                        if app_event_session_id(&event) != Some(session_id) {
                            tracing::warn!(
                                %session_id,
                                "runner event envelope session_id did not match embedded event"
                            );
                            continue;
                        }
                        state.publish_event(session_id, event).await;
                    }
                }
            }
            Ok(Message::Close(_)) | Err(_) => return,
            Ok(_) => {}
        }
    }
}

async fn send_server_message(
    sender: &mut futures_util::stream::SplitSink<WebSocket, Message>,
    message: RunnerServerMessage,
) -> Result<(), axum::Error> {
    let json = serde_json::to_string(&message).expect("runner server message serializes");
    sender.send(Message::Text(json.into())).await
}

fn app_event_session_id(event: &AppEvent) -> Option<SessionId> {
    match event {
        AppEvent::SessionStarted(info) => Some(info.session_id),
        AppEvent::SessionStatusChanged(event) => Some(event.session_id),
        AppEvent::UserMessage(event) => Some(event.session_id),
        AppEvent::AgentMessageDelta(event) => Some(event.session_id),
        AppEvent::AgentMessageCompleted(event) => Some(event.session_id),
        AppEvent::PlanUpdated(event) => Some(event.session_id),
        AppEvent::ToolStarted(event)
        | AppEvent::ToolUpdated(event)
        | AppEvent::ToolCompleted(event) => Some(event.session_id),
        AppEvent::CommandStarted(event) => Some(event.session_id),
        AppEvent::CommandOutputDelta(event) => Some(event.session_id),
        AppEvent::CommandCompleted(event) => Some(event.session_id),
        AppEvent::FileChangeProposed(event)
        | AppEvent::FileChangeApplied(event)
        | AppEvent::FileChangeRejected(event) => Some(event.session_id),
        AppEvent::ApprovalRequested(event) => Some(event.session_id),
        AppEvent::ApprovalResolved(event) => Some(event.session_id),
        AppEvent::Error(event) => event.session_id,
    }
}
