use crate::state::SessionRegistration;
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::get,
    Json, Router,
};
use serde_json::Value;

pub(super) fn router() -> Router<crate::state::AppState> {
    Router::new().route(
        "/{session_id}/slash-commands",
        get(list_slash_commands).post(execute_slash_command),
    )
}

pub(super) async fn list_slash_commands(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
) -> Response {
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%session_id, "slash command list rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(session) = state.session(user.user_id, session_id).await else {
        tracing::warn!(user_id = %user.user_id, %session_id, "slash command list rejected session not found");
        return StatusCode::NOT_FOUND.into_response();
    };

    let mut commands = local_slash_commands(&session.provider_id);
    let request_id = agenter_protocol::RequestId::from(uuid::Uuid::new_v4().to_string());
    let message = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::ListProviderCommands(
                agenter_protocol::runner::ListProviderCommandsCommand {
                    session_id,
                    provider_id: session.provider_id.clone(),
                },
            ),
        },
    ));
    match state
        .send_runner_command_and_wait(
            session.runner_id,
            request_id,
            message,
            super::RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok {
            result:
                agenter_protocol::runner::RunnerCommandResult::ProviderCommands {
                    commands: provider_commands,
                },
        }) => commands.extend(provider_commands),
        Ok(other) => {
            tracing::warn!(%session_id, ?other, "runner returned unexpected slash command manifest response");
        }
        Err(error) => {
            tracing::debug!(%session_id, ?error, "provider slash command manifest unavailable");
        }
    }
    let mut seen = std::collections::HashSet::new();
    commands.retain(|command| seen.insert(command.id.clone()));

    Json(commands).into_response()
}

pub(super) async fn execute_slash_command(
    State(state): State<crate::state::AppState>,
    headers: HeaderMap,
    Path(session_id): Path<agenter_core::SessionId>,
    Json(request): Json<agenter_core::SlashCommandRequest>,
) -> Response {
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%session_id, "slash command rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let Some(session) = state.session(user.user_id, session_id).await else {
        tracing::warn!(user_id = %user.user_id, %session_id, "slash command rejected session not found");
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(definition) = local_slash_commands(&session.provider_id)
        .into_iter()
        .find(|command| command.id == request.command_id)
    else {
        return (
            StatusCode::BAD_REQUEST,
            Json(agenter_core::SlashCommandResult {
                accepted: false,
                message: format!("Unknown slash command `{}`.", request.raw_input),
                session: None,
                provider_payload: None,
            }),
        )
            .into_response();
    };
    if matches!(
        definition.danger_level,
        agenter_core::SlashCommandDangerLevel::Confirm
            | agenter_core::SlashCommandDangerLevel::Dangerous
    ) && !request.confirmed
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(agenter_core::SlashCommandResult {
                accepted: false,
                message: format!("Command /{} requires confirmation.", definition.name),
                session: None,
                provider_payload: None,
            }),
        )
            .into_response();
    }
    publish_slash_user_echo(&state, user.user_id, session.session_id, &request).await;

    if request.command_id.starts_with("local.") {
        return execute_local_slash_command(state, user.user_id, session, request, definition)
            .await;
    }
    if request.command_id == "runner.interrupt" {
        return execute_runner_interrupt_slash_command(state, session, request, definition).await;
    }
    if !request
        .command_id
        .starts_with(&format!("{}.", session.provider_id.as_str()))
    {
        return (
            StatusCode::BAD_REQUEST,
            Json(agenter_core::SlashCommandResult {
                accepted: false,
                message: format!("Unknown slash command `{}`.", request.raw_input),
                session: None,
                provider_payload: None,
            }),
        )
            .into_response();
    }

    execute_provider_slash_command(state, user.user_id, session, request, definition).await
}

async fn execute_local_slash_command(
    state: crate::state::AppState,
    user_id: agenter_core::UserId,
    session: crate::state::RegisteredSession,
    request: agenter_core::SlashCommandRequest,
    definition: agenter_core::SlashCommandDefinition,
) -> Response {
    let session_id = session.session_id;
    match request.command_id.as_str() {
        "local.title" => {
            let title = request
                .arguments
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(str::to_owned);
            let Some(session) = state
                .update_session_title(user_id, session.session_id, title)
                .await
            else {
                return StatusCode::NOT_FOUND.into_response();
            };
            let result = agenter_core::SlashCommandResult {
                accepted: true,
                message: "Session title updated.".to_owned(),
                session: Some(session),
                provider_payload: None,
            };
            slash_result_response(
                &state,
                session_id,
                &request,
                &definition,
                StatusCode::OK,
                result,
            )
            .await
        }
        "local.model" | "local.mode" | "local.reasoning" => {
            let mut settings = state
                .session_turn_settings(user_id, session.session_id)
                .await
                .unwrap_or_default();
            match request.command_id.as_str() {
                "local.model" => {
                    settings.model = request
                        .arguments
                        .get("model")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned);
                }
                "local.mode" => {
                    settings.collaboration_mode = request
                        .arguments
                        .get("mode")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned);
                }
                "local.reasoning" => {
                    settings.reasoning_effort = request
                        .arguments
                        .get("effort")
                        .and_then(Value::as_str)
                        .and_then(agent_reasoning_effort_from_str);
                }
                _ => {}
            }
            let Some(settings) = state
                .update_session_turn_settings(user_id, session.session_id, settings)
                .await
            else {
                return StatusCode::NOT_FOUND.into_response();
            };
            let result = agenter_core::SlashCommandResult {
                accepted: true,
                message: "Session settings updated.".to_owned(),
                session: None,
                provider_payload: Some(serde_json::to_value(settings).unwrap_or_default()),
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::OK,
                result,
            )
            .await
        }
        "local.help" => {
            let result = agenter_core::SlashCommandResult {
                accepted: true,
                message: "Type / to browse commands. Provider commands are marked with their provider and dangerous commands require confirmation.".to_owned(),
                session: None,
                provider_payload: None,
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::OK,
                result,
            )
            .await
        }
        "local.new" => {
            let title = request
                .arguments
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(str::to_owned);
            create_session_from_slash(state, user_id, session, request, definition, title).await
        }
        "local.refresh" => {
            refresh_workspace_provider_sessions_for_user(
                state,
                user_id,
                session.workspace.workspace_id,
                session.provider_id,
                Some((session.session_id, request, definition)),
            )
            .await
        }
        _ => (
            StatusCode::BAD_REQUEST,
            Json(agenter_core::SlashCommandResult {
                accepted: false,
                message: format!("Unknown slash command `{}`.", request.raw_input),
                session: None,
                provider_payload: None,
            }),
        )
            .into_response(),
    }
}

async fn create_session_from_slash(
    state: crate::state::AppState,
    user_id: agenter_core::UserId,
    source: crate::state::RegisteredSession,
    request: agenter_core::SlashCommandRequest,
    definition: agenter_core::SlashCommandDefinition,
    title: Option<String>,
) -> Response {
    let source_session_id = source.session_id;
    let session_id = agenter_core::SessionId::new();
    let external_session_id = {
        let request_id = agenter_protocol::RequestId::from(uuid::Uuid::new_v4().to_string());
        let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
            agenter_protocol::runner::RunnerCommandEnvelope {
                request_id: request_id.clone(),
                command: agenter_protocol::runner::RunnerCommand::CreateSession(
                    agenter_protocol::runner::CreateSessionCommand {
                        session_id,
                        workspace: source.workspace.clone(),
                        provider_id: source.provider_id.clone(),
                        initial_input: None,
                    },
                ),
            },
        ));
        match state
            .send_runner_command_and_wait(
                source.runner_id,
                request_id.clone(),
                command,
                super::RUNNER_COMMAND_RESPONSE_TIMEOUT,
            )
            .await
        {
            Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok {
                result:
                    agenter_protocol::runner::RunnerCommandResult::SessionCreated {
                        session_id: returned_session_id,
                        external_session_id,
                    },
            }) if returned_session_id == session_id => Some(external_session_id),
            Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
                let result = agenter_core::SlashCommandResult {
                    accepted: false,
                    message: error.message.clone(),
                    session: None,
                    provider_payload: Some(serde_json::json!({ "code": error.code })),
                };
                return slash_result_response(
                    &state,
                    source_session_id,
                    &request,
                    &definition,
                    StatusCode::BAD_GATEWAY,
                    result,
                )
                .await;
            }
            Ok(other) => {
                tracing::warn!(
                    ?other,
                    "unexpected runner create-session response for slash /new"
                );
                let result = agenter_core::SlashCommandResult {
                    accepted: false,
                    message: format!("Unexpected runner response: {other:?}"),
                    session: None,
                    provider_payload: None,
                };
                return slash_result_response(
                    &state,
                    source_session_id,
                    &request,
                    &definition,
                    StatusCode::BAD_GATEWAY,
                    result,
                )
                .await;
            }
            Err(error) => {
                let result = agenter_core::SlashCommandResult {
                    accepted: false,
                    message: super::runner_wait_error_message(error).to_owned(),
                    session: None,
                    provider_payload: Some(serde_json::json!({
                        "code": super::runner_wait_error_code(error),
                    })),
                };
                return slash_result_response(
                    &state,
                    source_session_id,
                    &request,
                    &definition,
                    super::runner_wait_error_status(error),
                    result,
                )
                .await;
            }
        }
    };

    let session = state
        .register_session(SessionRegistration {
            session_id,
            owner_user_id: user_id,
            runner_id: source.runner_id,
            workspace: source.workspace,
            provider_id: source.provider_id,
            title,
            external_session_id,
            turn_settings: source.turn_settings,
            usage: source.usage,
        })
        .await;
    let info = session.info();
    state
        .publish_event(
            info.session_id,
            agenter_core::AppEvent::SessionStarted(info.clone()),
        )
        .await;
    let result = agenter_core::SlashCommandResult {
        accepted: true,
        message: "New session created.".to_owned(),
        session: Some(info),
        provider_payload: None,
    };
    slash_result_response(
        &state,
        source_session_id,
        &request,
        &definition,
        StatusCode::OK,
        result,
    )
    .await
}

async fn execute_runner_interrupt_slash_command(
    state: crate::state::AppState,
    session: crate::state::RegisteredSession,
    request: agenter_core::SlashCommandRequest,
    definition: agenter_core::SlashCommandDefinition,
) -> Response {
    let request_id = agenter_protocol::RequestId::from(uuid::Uuid::new_v4().to_string());
    let message = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::InterruptSession {
                session_id: session.session_id,
            },
        },
    ));
    match state
        .send_runner_command_and_wait(
            session.runner_id,
            request_id,
            message,
            super::RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok {
            result: agenter_protocol::runner::RunnerCommandResult::Accepted,
        }) => {
            let result = agenter_core::SlashCommandResult {
                accepted: true,
                message: "Interrupt requested.".to_owned(),
                session: None,
                provider_payload: None,
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::OK,
                result,
            )
            .await
        }
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: error.message.clone(),
                session: None,
                provider_payload: Some(serde_json::json!({ "code": error.code })),
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::BAD_GATEWAY,
                result,
            )
            .await
        }
        Ok(other) => {
            tracing::warn!(session_id = %session.session_id, ?other, "runner returned unexpected interrupt slash response");
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: format!("Unexpected runner response: {other:?}"),
                session: None,
                provider_payload: None,
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::BAD_GATEWAY,
                result,
            )
            .await
        }
        Err(error) => {
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: super::runner_wait_error_message(error).to_owned(),
                session: None,
                provider_payload: Some(serde_json::json!({
                    "code": super::runner_wait_error_code(error),
                })),
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                super::runner_wait_error_status(error),
                result,
            )
            .await
        }
    }
}

async fn execute_provider_slash_command(
    state: crate::state::AppState,
    user_id: agenter_core::UserId,
    session: crate::state::RegisteredSession,
    request: agenter_core::SlashCommandRequest,
    definition: agenter_core::SlashCommandDefinition,
) -> Response {
    let provider_definition = provider_slash_commands(&session.provider_id)
        .into_iter()
        .find(|command| command.id == request.command_id);
    if provider_definition.is_none() {
        return (
            StatusCode::BAD_REQUEST,
            Json(agenter_core::SlashCommandResult {
                accepted: false,
                message: format!("Unknown slash command `{}`.", request.raw_input),
                session: None,
                provider_payload: None,
            }),
        )
            .into_response();
    }

    let request_id = agenter_protocol::RequestId::from(uuid::Uuid::new_v4().to_string());
    let command_id = request.command_id.clone();
    let command_request = request.clone();
    let message = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::ExecuteProviderCommand(
                agenter_protocol::runner::ProviderCommandExecutionCommand {
                    session_id: session.session_id,
                    external_session_id: session.external_session_id.clone(),
                    provider_id: session.provider_id.clone(),
                    command: command_request,
                },
            ),
        },
    ));
    match state
        .send_runner_command_and_wait(
            session.runner_id,
            request_id,
            message,
            super::RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok {
            result:
                agenter_protocol::runner::RunnerCommandResult::ProviderCommandExecuted { mut result },
        }) => {
            if command_id == "codex.fork" {
                if let Some(forked) = register_forked_session_from_provider_result(
                    &state,
                    user_id,
                    &session,
                    result.provider_payload.as_ref(),
                )
                .await
                {
                    result.session = Some(forked);
                }
            }
            if let Some(status) = provider_slash_session_status(&command_id) {
                if let Some(updated) = state
                    .update_session_status(user_id, session.session_id, status.clone())
                    .await
                {
                    state
                        .publish_event(
                            session.session_id,
                            agenter_core::AppEvent::SessionStatusChanged(
                                agenter_core::SessionStatusChangedEvent {
                                    session_id: session.session_id,
                                    status,
                                    reason: Some(format!(
                                        "Updated by /{}.",
                                        command_id
                                            .strip_prefix("codex.")
                                            .unwrap_or(command_id.as_str())
                                    )),
                                },
                            ),
                        )
                        .await;
                    result.session = Some(updated);
                }
            }
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::OK,
                result,
            )
            .await
        }
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: error.message.clone(),
                session: None,
                provider_payload: Some(serde_json::json!({ "code": error.code })),
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::BAD_GATEWAY,
                result,
            )
            .await
        }
        Ok(other) => {
            tracing::warn!(?other, "runner returned unexpected provider slash response");
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: format!("Unexpected runner response: {other:?}"),
                session: None,
                provider_payload: None,
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                StatusCode::BAD_GATEWAY,
                result,
            )
            .await
        }
        Err(error) => {
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: super::runner_wait_error_message(error).to_owned(),
                session: None,
                provider_payload: Some(serde_json::json!({
                    "code": super::runner_wait_error_code(error),
                })),
            };
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                super::runner_wait_error_status(error),
                result,
            )
            .await
        }
    }
}

async fn publish_slash_user_echo(
    state: &crate::state::AppState,
    user_id: agenter_core::UserId,
    session_id: agenter_core::SessionId,
    request: &agenter_core::SlashCommandRequest,
) {
    state
        .publish_event(
            session_id,
            agenter_core::AppEvent::UserMessage(agenter_core::UserMessageEvent {
                session_id,
                message_id: Some(uuid::Uuid::new_v4().to_string()),
                author_user_id: Some(user_id),
                content: request.raw_input.clone(),
            }),
        )
        .await;
}

async fn slash_result_response(
    state: &crate::state::AppState,
    session_id: agenter_core::SessionId,
    request: &agenter_core::SlashCommandRequest,
    definition: &agenter_core::SlashCommandDefinition,
    status: StatusCode,
    result: agenter_core::SlashCommandResult,
) -> Response {
    publish_slash_result_event(state, session_id, request, definition, &result).await;
    (status, Json(result)).into_response()
}

async fn publish_slash_result_event(
    state: &crate::state::AppState,
    session_id: agenter_core::SessionId,
    request: &agenter_core::SlashCommandRequest,
    definition: &agenter_core::SlashCommandDefinition,
    result: &agenter_core::SlashCommandResult,
) {
    state
        .publish_event(
            session_id,
            agenter_core::AppEvent::ProviderEvent(agenter_core::ProviderEvent {
                session_id,
                provider_id: definition
                    .provider_id
                    .clone()
                    .unwrap_or_else(|| agenter_core::AgentProviderId::from("local")),
                event_id: Some(format!(
                    "slash-{}-{}",
                    request.command_id.replace('.', "-"),
                    uuid::Uuid::new_v4()
                )),
                method: "slash_command_result".to_owned(),
                category: "slash_command".to_owned(),
                title: format!("/{}", definition.name),
                detail: Some(result.message.clone()),
                status: Some(
                    if result.accepted {
                        "accepted"
                    } else {
                        "rejected"
                    }
                    .to_owned(),
                ),
                provider_payload: Some(serde_json::json!({
                    "command_id": request.command_id,
                    "raw_input": request.raw_input,
                    "target": definition.target,
                    "danger_level": definition.danger_level,
                    "arguments": request.arguments,
                    "accepted": result.accepted,
                    "message": result.message,
                    "provider_payload": result.provider_payload,
                })),
            }),
        )
        .await;
}

async fn register_forked_session_from_provider_result(
    state: &crate::state::AppState,
    user_id: agenter_core::UserId,
    source: &crate::state::RegisteredSession,
    provider_payload: Option<&serde_json::Value>,
) -> Option<agenter_core::SessionInfo> {
    let external_session_id = provider_payload
        .and_then(|payload| {
            string_pointer(
                payload,
                &[
                    "/result/thread/id",
                    "/result/threadId",
                    "/thread/id",
                    "/threadId",
                ],
            )
        })?
        .to_owned();
    let session = state
        .register_session(SessionRegistration {
            session_id: agenter_core::SessionId::new(),
            owner_user_id: user_id,
            runner_id: source.runner_id,
            workspace: source.workspace.clone(),
            provider_id: source.provider_id.clone(),
            title: Some(format!(
                "Fork of {}",
                source.title.as_deref().unwrap_or("session")
            )),
            external_session_id: Some(external_session_id),
            turn_settings: source.turn_settings.clone(),
            usage: source.usage.clone(),
        })
        .await;
    let info = session.info();
    state
        .publish_event(
            info.session_id,
            agenter_core::AppEvent::SessionStarted(info.clone()),
        )
        .await;
    Some(info)
}

fn provider_slash_session_status(command_id: &str) -> Option<agenter_core::SessionStatus> {
    match command_id {
        "codex.archive" => Some(agenter_core::SessionStatus::Archived),
        "codex.unarchive" => Some(agenter_core::SessionStatus::Running),
        _ => None,
    }
}

async fn refresh_workspace_provider_sessions_for_user(
    state: crate::state::AppState,
    _user_id: agenter_core::UserId,
    workspace_id: agenter_core::WorkspaceId,
    provider_id: agenter_core::AgentProviderId,
    slash_context: Option<(
        agenter_core::SessionId,
        agenter_core::SlashCommandRequest,
        agenter_core::SlashCommandDefinition,
    )>,
) -> Response {
    let Some((runner_id, workspace)) = state
        .resolve_runner_workspace(workspace_id, &provider_id)
        .await
    else {
        return StatusCode::NOT_FOUND.into_response();
    };
    let request_id = agenter_protocol::RequestId::from(uuid::Uuid::new_v4().to_string());
    let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::RefreshSessions(
                agenter_protocol::runner::RefreshSessionsCommand {
                    workspace,
                    provider_id: provider_id.clone(),
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
            let result = agenter_core::SlashCommandResult {
                accepted: true,
                message: format!(
                    "Refresh complete: {} discovered, {} cache refreshed, {} skipped.",
                    summary.discovered_count,
                    summary.refreshed_cache_count,
                    summary.skipped_failed_count
                ),
                session: None,
                provider_payload: Some(serde_json::to_value(summary).unwrap_or_default()),
            };
            if let Some((session_id, request, definition)) = slash_context {
                slash_result_response(
                    &state,
                    session_id,
                    &request,
                    &definition,
                    StatusCode::OK,
                    result,
                )
                .await
            } else {
                Json(result).into_response()
            }
        }
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            tracing::warn!(%workspace_id, code = %error.code, message = %error.message, "slash refresh failed in runner");
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: error.message.clone(),
                session: None,
                provider_payload: Some(serde_json::json!({ "code": error.code })),
            };
            if let Some((session_id, request, definition)) = slash_context {
                slash_result_response(
                    &state,
                    session_id,
                    &request,
                    &definition,
                    StatusCode::BAD_GATEWAY,
                    result,
                )
                .await
            } else {
                (StatusCode::BAD_GATEWAY, Json(error)).into_response()
            }
        }
        Err(error) => {
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: super::runner_wait_error_message(error).to_owned(),
                session: None,
                provider_payload: Some(serde_json::json!({
                    "code": super::runner_wait_error_code(error),
                })),
            };
            if let Some((session_id, request, definition)) = slash_context {
                slash_result_response(
                    &state,
                    session_id,
                    &request,
                    &definition,
                    super::runner_wait_error_status(error),
                    result,
                )
                .await
            } else {
                super::runner_wait_error_status(error).into_response()
            }
        }
    }
}

pub(super) fn local_slash_commands(
    provider_id: &agenter_core::AgentProviderId,
) -> Vec<agenter_core::SlashCommandDefinition> {
    let mut commands: Vec<agenter_core::SlashCommandDefinition> = vec![
        slash_command(
            "local.help",
            "help",
            "Show available slash commands.",
            "local",
            agenter_core::SlashCommandTarget::Local,
        ),
        slash_command(
            "local.model",
            "model",
            "Set the session model.",
            "settings",
            agenter_core::SlashCommandTarget::Local,
        )
        .with_argument(
            "model",
            agenter_core::SlashCommandArgumentKind::String,
            true,
            "Model id",
        ),
        slash_command(
            "local.mode",
            "mode",
            "Set collaboration mode.",
            "settings",
            agenter_core::SlashCommandTarget::Local,
        )
        .with_argument(
            "mode",
            agenter_core::SlashCommandArgumentKind::String,
            true,
            "Mode id",
        ),
        slash_command(
            "local.reasoning",
            "reasoning",
            "Set reasoning effort.",
            "settings",
            agenter_core::SlashCommandTarget::Local,
        )
        .with_argument(
            "effort",
            agenter_core::SlashCommandArgumentKind::Enum,
            true,
            "Reasoning effort",
        )
        .with_choices(["none", "minimal", "low", "medium", "high", "xhigh"]),
        slash_command(
            "local.title",
            "title",
            "Rename this session.",
            "session",
            agenter_core::SlashCommandTarget::Local,
        )
        .with_argument(
            "title",
            agenter_core::SlashCommandArgumentKind::Rest,
            true,
            "New title",
        ),
        slash_command(
            "local.refresh",
            "refresh",
            "Refresh provider sessions from persistence.",
            "session",
            agenter_core::SlashCommandTarget::Local,
        ),
        slash_command(
            "local.new",
            "new",
            "Create a new session in this workspace.",
            "session",
            agenter_core::SlashCommandTarget::Local,
        )
        .with_argument(
            "title",
            agenter_core::SlashCommandArgumentKind::Rest,
            false,
            "Optional title",
        ),
        slash_command(
            "runner.interrupt",
            "interrupt",
            "Interrupt the current provider turn.",
            "runner",
            agenter_core::SlashCommandTarget::Runner,
        ),
    ]
    .into_iter()
    .map(Into::into)
    .collect();
    commands.extend(provider_slash_commands(provider_id));
    commands
}

fn provider_slash_commands(
    provider_id: &agenter_core::AgentProviderId,
) -> Vec<agenter_core::SlashCommandDefinition> {
    if provider_id.as_str() != agenter_core::AgentProviderId::CODEX {
        return Vec::new();
    }
    vec![
        slash_command(
            "codex.compact",
            "compact",
            "Start native Codex context compaction.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .into(),
        slash_command(
            "codex.review",
            "review",
            "Start a native Codex code review.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .with_argument(
            "target",
            agenter_core::SlashCommandArgumentKind::Rest,
            false,
            "Review target flags",
        )
        .into(),
        slash_command(
            "codex.steer",
            "steer",
            "Steer the active Codex turn.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .with_argument(
            "input",
            agenter_core::SlashCommandArgumentKind::Rest,
            true,
            "Text to steer with",
        )
        .into(),
        slash_command(
            "codex.fork",
            "fork",
            "Fork the current Codex thread.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .into(),
        slash_command(
            "codex.archive",
            "archive",
            "Archive the current Codex thread.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .into(),
        slash_command(
            "codex.unarchive",
            "unarchive",
            "Unarchive the current Codex thread.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .into(),
        slash_command(
            "codex.rollback",
            "rollback",
            "Drop recent turns from Codex history. Does not revert files.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .danger(agenter_core::SlashCommandDangerLevel::Dangerous)
        .with_argument(
            "numTurns",
            agenter_core::SlashCommandArgumentKind::Number,
            true,
            "Number of turns",
        )
        .into(),
        slash_command(
            "codex.shell",
            "shell",
            "Run an unsandboxed provider-native shell command.",
            "provider",
            agenter_core::SlashCommandTarget::Provider,
        )
        .danger(agenter_core::SlashCommandDangerLevel::Dangerous)
        .with_alias("sh")
        .with_argument(
            "command",
            agenter_core::SlashCommandArgumentKind::Rest,
            true,
            "Shell command",
        )
        .into(),
    ]
}

struct SlashCommandBuilder(agenter_core::SlashCommandDefinition);

impl SlashCommandBuilder {
    fn with_argument(
        mut self,
        name: &str,
        kind: agenter_core::SlashCommandArgumentKind,
        required: bool,
        description: &str,
    ) -> Self {
        self.0.arguments.push(agenter_core::SlashCommandArgument {
            name: name.to_owned(),
            kind,
            required,
            description: Some(description.to_owned()),
            choices: Vec::new(),
        });
        self
    }

    fn with_choices<const N: usize>(mut self, choices: [&str; N]) -> Self {
        if let Some(argument) = self.0.arguments.last_mut() {
            argument.choices = choices.into_iter().map(str::to_owned).collect();
        }
        self
    }

    fn with_alias(mut self, alias: &str) -> Self {
        self.0.aliases.push(alias.to_owned());
        self
    }

    fn danger(mut self, danger_level: agenter_core::SlashCommandDangerLevel) -> Self {
        self.0.danger_level = danger_level;
        self
    }
}

impl From<SlashCommandBuilder> for agenter_core::SlashCommandDefinition {
    fn from(builder: SlashCommandBuilder) -> Self {
        builder.0
    }
}

fn slash_command(
    id: &str,
    name: &str,
    description: &str,
    category: &str,
    target: agenter_core::SlashCommandTarget,
) -> SlashCommandBuilder {
    SlashCommandBuilder(agenter_core::SlashCommandDefinition {
        id: id.to_owned(),
        name: name.to_owned(),
        aliases: Vec::new(),
        description: description.to_owned(),
        category: category.to_owned(),
        provider_id: id.split_once('.').and_then(|(prefix, _)| {
            (!matches!(prefix, "local" | "runner"))
                .then(|| agenter_core::AgentProviderId::from(prefix))
        }),
        target,
        danger_level: agenter_core::SlashCommandDangerLevel::Safe,
        arguments: Vec::new(),
        examples: Vec::new(),
    })
}

fn agent_reasoning_effort_from_str(value: &str) -> Option<agenter_core::AgentReasoningEffort> {
    match value {
        "none" => Some(agenter_core::AgentReasoningEffort::None),
        "minimal" => Some(agenter_core::AgentReasoningEffort::Minimal),
        "low" => Some(agenter_core::AgentReasoningEffort::Low),
        "medium" => Some(agenter_core::AgentReasoningEffort::Medium),
        "high" => Some(agenter_core::AgentReasoningEffort::High),
        "xhigh" => Some(agenter_core::AgentReasoningEffort::Xhigh),
        _ => None,
    }
}

fn string_pointer<'a>(value: &'a serde_json::Value, pointers: &[&str]) -> Option<&'a str> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(serde_json::Value::as_str))
}
