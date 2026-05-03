use crate::state::SessionRegistration;
use agenter_core::WorkspaceId;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::Deserialize;

#[derive(Debug, Deserialize)]
pub(super) struct CreateSessionRequest {
    pub(super) workspace_id: agenter_core::WorkspaceId,
    pub(super) provider_id: agenter_core::AgentProviderId,
    pub(super) title: Option<String>,
    /// Optional first user message to seed the new session with. When present,
    /// the control plane dispatches it to the runner immediately after
    /// registration, mirroring Codex TUI's "Clear context and implement"
    /// handoff which spawns a fresh thread carrying the prior plan content.
    #[serde(default)]
    pub(super) initial_message: Option<String>,
    /// Optional turn-settings override applied to the seed message and
    /// persisted as the new session's sticky settings.
    #[serde(default)]
    pub(super) settings_override: Option<agenter_core::AgentTurnSettings>,
}

#[derive(Debug, Deserialize)]
pub(super) struct SendMessageRequest {
    pub(super) content: String,
    #[serde(default)]
    pub(super) command_id: Option<agenter_core::CommandId>,
    #[serde(default)]
    pub(super) idempotency_key: Option<String>,
    /// Atomic per-turn settings override. When present we persist these
    /// settings as the session's sticky configuration BEFORE forwarding the
    /// runner command, so the model sees the new collaboration mode on this
    /// turn and every subsequent turn until the user changes it again. This
    /// mirrors Codex TUI's `SubmitUserMessageWithMode` event and lets the
    /// browser implement the "Implement plan" handoff in one round-trip.
    #[serde(default)]
    pub(super) settings_override: Option<agenter_core::AgentTurnSettings>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UpdateSessionRequest {
    pub(super) title: Option<String>,
}

#[derive(Debug, Deserialize)]
pub(super) struct UpdateSessionSettingsRequest {
    #[serde(default)]
    pub(super) command_id: Option<agenter_core::CommandId>,
    #[serde(default)]
    pub(super) idempotency_key: Option<String>,
    #[serde(flatten)]
    pub(super) settings: agenter_core::AgentTurnSettings,
}

pub(super) fn router() -> Router<crate::state::AppState> {
    Router::new()
        .route("/", get(list_sessions).post(create_session))
        .route("/{session_id}", get(get_session).patch(update_session))
        .route("/{session_id}/messages", post(send_session_message))
        .route("/{session_id}/agent-options", get(session_agent_options))
        .route(
            "/{session_id}/settings",
            get(get_session_settings).patch(update_session_settings),
        )
        .route("/{session_id}/history", get(session_history))
}

pub(super) async fn list_sessions(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
) -> Response {
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!("list sessions rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let sessions = state.list_sessions(user.user_id).await;
    tracing::debug!(user_id = %user.user_id, session_count = sessions.len(), "listed sessions");
    Json(sessions).into_response()
}

pub(super) async fn get_session(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
) -> Response {
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%session_id, "get session rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let Some(session) = state.session(user.user_id, session_id).await else {
        tracing::warn!(user_id = %user.user_id, %session_id, "session not found or forbidden");
        return StatusCode::NOT_FOUND.into_response();
    };

    Json(session.info()).into_response()
}

pub(super) async fn update_session(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
    Json(request): Json<UpdateSessionRequest>,
) -> Response {
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%session_id, "update session rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let title = request.title.and_then(|title| {
        let trimmed = title.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    });
    let Some(session) = state
        .update_session_title(user.user_id, session_id, title)
        .await
    else {
        tracing::warn!(user_id = %user.user_id, %session_id, "session update not found or forbidden");
        return StatusCode::NOT_FOUND.into_response();
    };

    Json(session).into_response()
}

pub(super) async fn create_session(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Json(request): Json<CreateSessionRequest>,
) -> Response {
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!("create session rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let workspace_id = request.workspace_id;
    let provider_id = request.provider_id;

    let Some((runner_id, workspace)) = state
        .resolve_runner_workspace(workspace_id, &provider_id)
        .await
    else {
        tracing::warn!(
            user_id = %user.user_id,
            %workspace_id,
            provider_id = %provider_id,
            "session creation failed: no connected runner workspace/provider match"
        );
        return StatusCode::NOT_FOUND.into_response();
    };
    let session_id = agenter_core::SessionId::new();
    let initial_message = request.initial_message.and_then(|content| {
        let trimmed = content.trim();
        (!trimmed.is_empty()).then(|| trimmed.to_owned())
    });
    let settings_override = request.settings_override;
    let request_id = agenter_protocol::RequestId::from(uuid::Uuid::new_v4().to_string());
    let title = request.title;
    let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::CreateSession(
                agenter_protocol::runner::CreateSessionCommand {
                    session_id,
                    workspace: workspace.clone(),
                    provider_id: provider_id.clone(),
                    initial_input: None,
                },
            ),
        },
    ));
    let outcome = match state
        .send_runner_command_and_wait(
            runner_id,
            request_id,
            command,
            super::RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(outcome) => outcome,
        Err(error) => {
            tracing::warn!(
                user_id = %user.user_id,
                %session_id,
                %runner_id,
                ?error,
                "session creation failed while waiting for runner"
            );
            return super::runner_wait_error_status(error).into_response();
        }
    };
    let external_session_id = match outcome {
        agenter_protocol::runner::RunnerResponseOutcome::Ok {
            result:
                agenter_protocol::runner::RunnerCommandResult::SessionCreated {
                    session_id: returned_session_id,
                    external_session_id,
                },
        } if returned_session_id == session_id => external_session_id,
        agenter_protocol::runner::RunnerResponseOutcome::Error { error } => {
            tracing::warn!(
                user_id = %user.user_id,
                %session_id,
                %runner_id,
                code = %error.code,
                message = %error.message,
                "runner rejected session creation"
            );
            return StatusCode::BAD_GATEWAY.into_response();
        }
        other => {
            tracing::warn!(
                user_id = %user.user_id,
                %session_id,
                %runner_id,
                ?other,
                "runner returned unexpected create-session response"
            );
            return StatusCode::BAD_GATEWAY.into_response();
        }
    };

    let session = state
        .register_session(SessionRegistration {
            session_id,
            owner_user_id: user.user_id,
            runner_id,
            workspace,
            provider_id,
            title,
            external_session_id: Some(external_session_id),
            turn_settings: settings_override.clone(),
            usage: None,
        })
        .await;
    tracing::info!(
        user_id = %user.user_id,
        session_id = %session.session_id,
        runner_id = %session.runner_id,
        workspace_id = %session.workspace.workspace_id,
        provider_id = %session.provider_id,
        "session created"
    );

    let info = session.info();
    state
        .publish_event(
            session.session_id,
            agenter_core::AppEvent::SessionStarted(info.clone()),
        )
        .await;

    if let Some(content) = initial_message {
        if let Err(error) = dispatch_user_message(
            &state,
            DispatchUserMessage {
                user_id: user.user_id,
                session_id: session.session_id,
                runner_id: session.runner_id,
                provider_id: session.provider_id.clone(),
                external_session_id: session.external_session_id.clone(),
                content,
                settings: settings_override,
            },
        )
        .await
        {
            tracing::warn!(
                user_id = %user.user_id,
                session_id = %session.session_id,
                ?error,
                "initial session message dispatch failed"
            );
        }
    }

    (StatusCode::CREATED, Json(info)).into_response()
}

/// Internal helper used by both `send_session_message` and `create_session`'s
/// initial-message path. Performs the `AgentSendInput` round-trip with the
/// given (already-resolved) settings, publishes the user-message event, and
/// emits status events on success/failure.
#[derive(Debug)]
#[allow(dead_code)]
struct DispatchError {
    code: String,
    message: String,
}

struct DispatchUserMessage {
    user_id: agenter_core::UserId,
    session_id: agenter_core::SessionId,
    runner_id: agenter_core::RunnerId,
    provider_id: agenter_core::AgentProviderId,
    external_session_id: Option<String>,
    content: String,
    settings: Option<agenter_core::AgentTurnSettings>,
}

async fn dispatch_user_message(
    state: &crate::state::AppState,
    request: DispatchUserMessage,
) -> Result<(), DispatchError> {
    let session_id = request.session_id;
    let user_message = agenter_core::UserMessageEvent {
        session_id,
        message_id: Some(uuid::Uuid::new_v4().to_string()),
        author_user_id: Some(request.user_id),
        content: request.content,
    };
    state
        .publish_event(
            session_id,
            agenter_core::AppEvent::UserMessage(user_message.clone()),
        )
        .await;
    super::publish_session_status_event(
        state,
        session_id,
        agenter_core::SessionStatus::Running,
        Some("Message dispatched to runner.".to_owned()),
    )
    .await;

    let request_id = agenter_protocol::RequestId::from(uuid::Uuid::new_v4().to_string());
    let message = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::AgentSendInput(
                agenter_protocol::runner::AgentInputCommand {
                    session_id,
                    provider_id: Some(request.provider_id),
                    external_session_id: request.external_session_id,
                    settings: request.settings,
                    input: agenter_protocol::runner::AgentInput::UserMessage {
                        payload: user_message,
                    },
                },
            ),
        },
    ));

    match state
        .send_runner_command_and_wait(
            request.runner_id,
            request_id.clone(),
            message,
            super::RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok {
            result: agenter_protocol::runner::RunnerCommandResult::Accepted,
        }) => Ok(()),
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            super::publish_runner_error_event(
                state,
                session_id,
                "send_session_message",
                Some(&request_id),
                &error.code,
                &error.message,
            )
            .await;
            super::publish_session_status_event(
                state,
                session_id,
                agenter_core::SessionStatus::Failed,
                Some(error.message.clone()),
            )
            .await;
            Err(DispatchError {
                code: error.code,
                message: error.message,
            })
        }
        Ok(other) => {
            let detail = format!("unexpected runner response: {other:?}");
            super::publish_runner_error_event(
                state,
                session_id,
                "send_session_message",
                Some(&request_id),
                "unexpected_runner_response",
                &detail,
            )
            .await;
            super::publish_session_status_event(
                state,
                session_id,
                agenter_core::SessionStatus::Failed,
                Some(detail.clone()),
            )
            .await;
            Err(DispatchError {
                code: "unexpected_runner_response".to_owned(),
                message: detail,
            })
        }
        Err(error) => {
            let code = super::runner_wait_error_code(error);
            let message = super::runner_wait_error_message(error);
            super::publish_runner_error_event(
                state,
                session_id,
                "send_session_message",
                Some(&request_id),
                code,
                message,
            )
            .await;
            super::publish_session_status_event(
                state,
                session_id,
                agenter_core::SessionStatus::Failed,
                Some(message.to_owned()),
            )
            .await;
            Err(DispatchError {
                code: code.to_owned(),
                message: message.to_owned(),
            })
        }
    }
}

pub(super) async fn refresh_workspace_provider_sessions(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Path((workspace_id, provider_id)): Path<(WorkspaceId, String)>,
) -> Response {
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%workspace_id, provider_id, "session refresh rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let provider_id = agenter_core::AgentProviderId::from(provider_id);
    let Some((runner_id, workspace)) = state
        .resolve_runner_workspace(workspace_id, &provider_id)
        .await
    else {
        tracing::warn!(
            user_id = %user.user_id,
            %workspace_id,
            provider_id = %provider_id,
            "session refresh failed: no connected runner workspace/provider match"
        );
        return StatusCode::NOT_FOUND.into_response();
    };

    let request_id = agenter_protocol::RequestId::from(uuid::Uuid::new_v4().to_string());
    let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::RefreshSessions(
                agenter_protocol::runner::RefreshSessionsCommand {
                    workspace,
                    provider_id,
                },
            ),
        },
    ));
    match state
        .send_runner_command_and_wait(
            runner_id,
            request_id.clone(),
            command,
            super::RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok { .. }) => {
            let summary = state
                .take_refresh_summary(&request_id)
                .await
                .unwrap_or_default();
            Json(summary).into_response()
        }
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            tracing::warn!(
                user_id = %user.user_id,
                %workspace_id,
                code = %error.code,
                message = %error.message,
                "session refresh failed in runner"
            );
            (StatusCode::BAD_GATEWAY, Json(error)).into_response()
        }
        Err(error) => super::runner_wait_error_status(error).into_response(),
    }
}

pub(super) async fn send_session_message(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
    Json(request): Json<SendMessageRequest>,
) -> Response {
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%session_id, "send session message rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let Some(session) = state.session(user.user_id, session_id).await else {
        tracing::warn!(user_id = %user.user_id, %session_id, "send session message rejected session not found");
        return StatusCode::NOT_FOUND.into_response();
    };
    let content = request.content.trim();
    if content.is_empty() {
        tracing::debug!(user_id = %user.user_id, %session_id, "send session message rejected empty content");
        return StatusCode::BAD_REQUEST.into_response();
    }
    let settings_override = request.settings_override.clone();
    let command = agenter_core::UniversalCommandEnvelope {
        command_id: request
            .command_id
            .unwrap_or_else(agenter_core::CommandId::new),
        idempotency_key: request.idempotency_key.unwrap_or_else(|| {
            format!("legacy:send_message:{session_id}:{}", uuid::Uuid::new_v4())
        }),
        session_id: Some(session_id),
        turn_id: None,
        command: agenter_core::UniversalCommand::StartTurn {
            input: agenter_core::UserInput::Text {
                text: content.to_owned(),
            },
            settings: settings_override.clone(),
        },
    };
    if let Some(response) =
        super::command_replay_response(state.begin_universal_command(&command).await)
    {
        return response;
    }
    let settings = if let Some(override_settings) = settings_override {
        state
            .update_session_turn_settings(user.user_id, session_id, override_settings)
            .await
    } else {
        state.session_turn_settings(user.user_id, session_id).await
    };
    let user_message = agenter_core::UserMessageEvent {
        session_id,
        message_id: Some(uuid::Uuid::new_v4().to_string()),
        author_user_id: Some(user.user_id),
        content: content.to_owned(),
    };
    state
        .publish_event(
            session_id,
            agenter_core::AppEvent::UserMessage(user_message.clone()),
        )
        .await;
    super::publish_session_status_event(
        &state,
        session_id,
        agenter_core::SessionStatus::Running,
        Some("Message dispatched to runner.".to_owned()),
    )
    .await;

    let message = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: agenter_protocol::RequestId::from(uuid::Uuid::new_v4().to_string()),
            command: agenter_protocol::runner::RunnerCommand::AgentSendInput(
                agenter_protocol::runner::AgentInputCommand {
                    session_id,
                    provider_id: Some(session.provider_id),
                    external_session_id: session.external_session_id,
                    settings,
                    input: agenter_protocol::runner::AgentInput::UserMessage {
                        payload: user_message,
                    },
                },
            ),
        },
    ));
    let request_id = match &message {
        agenter_protocol::runner::RunnerServerMessage::Command(command) => {
            command.request_id.clone()
        }
        _ => unreachable!("constructed runner command"),
    };

    match state
        .send_runner_command_and_wait(
            session.runner_id,
            request_id.clone(),
            message,
            super::RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok {
            result: agenter_protocol::runner::RunnerCommandResult::Accepted,
        }) => {
            tracing::info!(
                user_id = %user.user_id,
                %session_id,
                runner_id = %session.runner_id,
                content_len = content.len(),
                "session message accepted by runner channel"
            );
            let response = super::command_response(StatusCode::ACCEPTED, None);
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &command,
                crate::state::UniversalCommandIdempotencyStatus::Succeeded,
                response,
            )
            .await
            {
                return response;
            }
            StatusCode::ACCEPTED.into_response()
        }
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            tracing::warn!(
                user_id = %user.user_id,
                %session_id,
                runner_id = %session.runner_id,
                code = %error.code,
                message = %error.message,
                "session message rejected by runner"
            );
            super::publish_runner_error_event(
                &state,
                session_id,
                "send_session_message",
                Some(&request_id),
                &error.code,
                &error.message,
            )
            .await;
            super::publish_session_status_event(
                &state,
                session_id,
                agenter_core::SessionStatus::Failed,
                Some(error.message.clone()),
            )
            .await;
            let body = serde_json::to_value(&error).ok();
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &command,
                crate::state::UniversalCommandIdempotencyStatus::Failed,
                super::command_response(StatusCode::BAD_GATEWAY, body.clone()),
            )
            .await
            {
                return response;
            }
            (StatusCode::BAD_GATEWAY, Json(error)).into_response()
        }
        Ok(other) => {
            tracing::warn!(
                user_id = %user.user_id,
                %session_id,
                runner_id = %session.runner_id,
                ?other,
                "session message received unexpected runner response"
            );
            let detail = format!("unexpected runner response: {other:?}");
            super::publish_runner_error_event(
                &state,
                session_id,
                "send_session_message",
                Some(&request_id),
                "unexpected_runner_response",
                &detail,
            )
            .await;
            super::publish_session_status_event(
                &state,
                session_id,
                agenter_core::SessionStatus::Failed,
                Some(detail.clone()),
            )
            .await;
            let error = agenter_protocol::runner::RunnerError {
                code: "unexpected_runner_response".to_owned(),
                message: detail,
            };
            let body = serde_json::to_value(&error).ok();
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &command,
                crate::state::UniversalCommandIdempotencyStatus::Failed,
                super::command_response(StatusCode::BAD_GATEWAY, body.clone()),
            )
            .await
            {
                return response;
            }
            (StatusCode::BAD_GATEWAY, Json(error)).into_response()
        }
        Err(error) => {
            tracing::warn!(
                user_id = %user.user_id,
                %session_id,
                runner_id = %session.runner_id,
                ?error,
                "session message failed because runner is unavailable"
            );
            let status = super::runner_wait_error_status(error);
            let code = super::runner_wait_error_code(error);
            let message = super::runner_wait_error_message(error);
            super::publish_runner_error_event(
                &state,
                session_id,
                "send_session_message",
                Some(&request_id),
                code,
                message,
            )
            .await;
            super::publish_session_status_event(
                &state,
                session_id,
                agenter_core::SessionStatus::Failed,
                Some(message.to_owned()),
            )
            .await;
            let error = agenter_protocol::runner::RunnerError {
                code: code.to_owned(),
                message: message.to_owned(),
            };
            let body = serde_json::to_value(&error).ok();
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &command,
                crate::state::UniversalCommandIdempotencyStatus::Failed,
                super::command_response(status, body.clone()),
            )
            .await
            {
                return response;
            }
            (status, Json(error)).into_response()
        }
    }
}

pub(super) async fn session_agent_options(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
) -> Response {
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(session) = state.session(user.user_id, session_id).await else {
        return StatusCode::NOT_FOUND.into_response();
    };

    let request_id = agenter_protocol::RequestId::from(uuid::Uuid::new_v4().to_string());
    let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::GetAgentOptions(
                agenter_protocol::runner::GetAgentOptionsCommand {
                    session_id,
                    provider_id: session.provider_id,
                },
            ),
        },
    ));

    match state
        .send_runner_command_and_wait(
            session.runner_id,
            request_id,
            command,
            super::RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok {
            result: agenter_protocol::runner::RunnerCommandResult::AgentOptions { options },
        }) => Json(options).into_response(),
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            tracing::warn!(
                %session_id,
                code = %error.code,
                message = %error.message,
                "runner could not load agent options"
            );
            Json(agenter_core::AgentOptions::default()).into_response()
        }
        Ok(other) => {
            tracing::warn!(%session_id, ?other, "unexpected runner options response");
            Json(agenter_core::AgentOptions::default()).into_response()
        }
        Err(error) => {
            tracing::warn!(%session_id, ?error, "agent options unavailable");
            Json(agenter_core::AgentOptions::default()).into_response()
        }
    }
}

pub(super) async fn get_session_settings(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
) -> Response {
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !state.can_access_session(user.user_id, session_id).await {
        return StatusCode::NOT_FOUND.into_response();
    }
    Json(
        state
            .session_turn_settings(user.user_id, session_id)
            .await
            .unwrap_or_default(),
    )
    .into_response()
}

pub(super) async fn update_session_settings(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
    Json(request): Json<UpdateSessionSettingsRequest>,
) -> Response {
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let command = agenter_core::UniversalCommandEnvelope {
        command_id: request
            .command_id
            .unwrap_or_else(agenter_core::CommandId::new),
        idempotency_key: request.idempotency_key.unwrap_or_else(|| {
            format!(
                "legacy:update_settings:{session_id}:{}",
                uuid::Uuid::new_v4()
            )
        }),
        session_id: Some(session_id),
        turn_id: None,
        command: agenter_core::UniversalCommand::SetTurnSettings {
            settings: request.settings.clone(),
        },
    };
    if let Some(response) =
        super::command_replay_response(state.begin_universal_command(&command).await)
    {
        return response;
    }
    let Some(settings) = state
        .update_session_turn_settings(user.user_id, session_id, request.settings)
        .await
    else {
        if let Some(response) = super::finish_command_or_error_response(
            &state,
            &command,
            crate::state::UniversalCommandIdempotencyStatus::Failed,
            super::command_response(StatusCode::NOT_FOUND, None),
        )
        .await
        {
            return response;
        }
        return StatusCode::NOT_FOUND.into_response();
    };
    let body = serde_json::to_value(&settings).ok();
    if let Some(response) = super::finish_command_or_error_response(
        &state,
        &command,
        crate::state::UniversalCommandIdempotencyStatus::Succeeded,
        super::command_response(StatusCode::OK, body.clone()),
    )
    .await
    {
        return response;
    }
    Json(settings).into_response()
}

pub(super) async fn session_history(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
) -> Response {
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%session_id, "session history rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };

    let Some(history) = state.session_history(user.user_id, session_id).await else {
        tracing::warn!(user_id = %user.user_id, %session_id, "session history rejected session not found");
        return StatusCode::NOT_FOUND.into_response();
    };

    tracing::debug!(user_id = %user.user_id, %session_id, event_count = history.len(), "returned session history");
    Json(history).into_response()
}
