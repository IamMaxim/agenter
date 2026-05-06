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
    let definition = local_slash_commands(&session.provider_id)
        .into_iter()
        .find(|command| command.id == request.command_id);
    let definition = if let Some(definition) = definition {
        definition
    } else if request.command_id == "runner.interrupt" {
        local_slash_commands(&session.provider_id)
            .into_iter()
            .find(|command| command.id == request.command_id)
            .expect("runner interrupt is local")
    } else {
        let Some(definition) =
            provider_slash_command_definition(&state, &session, &request.command_id).await
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
        definition
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
    if request.command_id.starts_with("local.") {
        return execute_local_slash_command(state, user.user_id, session, request, definition)
            .await;
    }
    if request.command_id == "runner.interrupt" {
        return execute_runner_interrupt_slash_command(state, session, request, definition).await;
    }
    execute_provider_slash_command(state, user.user_id, session, request, definition).await
}

async fn provider_slash_command_definition(
    state: &crate::state::AppState,
    session: &crate::state::RegisteredSession,
    command_id: &str,
) -> Option<agenter_core::SlashCommandDefinition> {
    let request_id = agenter_protocol::RequestId::from(uuid::Uuid::new_v4().to_string());
    let message = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::ListProviderCommands(
                agenter_protocol::runner::ListProviderCommandsCommand {
                    session_id: session.session_id,
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
            result: agenter_protocol::runner::RunnerCommandResult::ProviderCommands { commands },
        }) => commands
            .into_iter()
            .find(|command| command.id == command_id),
        other => {
            tracing::debug!(session_id = %session.session_id, ?other, "provider slash command definition unavailable");
            None
        }
    }
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
            let universal_command = slash_command_envelope(
                user_id,
                session.session_id,
                &request,
                agenter_core::UniversalCommand::ExecuteProviderCommand {
                    command: semantic_slash_request(&request),
                },
            );
            if let Some(response) = super::command_replay_response(
                state.begin_universal_command(&universal_command).await,
            ) {
                return response;
            }
            publish_slash_user_echo(&state, user_id, session.session_id, &request).await;
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
                if let Some(response) = super::finish_command_or_error_response(
                    &state,
                    &universal_command,
                    crate::state::UniversalCommandIdempotencyStatus::Failed,
                    super::command_response(StatusCode::NOT_FOUND, None),
                )
                .await
                {
                    return response;
                }
                return StatusCode::NOT_FOUND.into_response();
            };
            let result = agenter_core::SlashCommandResult {
                accepted: true,
                message: "Session title updated.".to_owned(),
                session: Some(session),
                provider_payload: None,
            };
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Succeeded,
                super::command_response(StatusCode::OK, serde_json::to_value(&result).ok()),
            )
            .await
            {
                return response;
            }
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
            let mut settings_patch = agenter_core::AgentTurnSettings::default();
            match request.command_id.as_str() {
                "local.model" => {
                    settings_patch.model = request
                        .arguments
                        .get("model")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned);
                }
                "local.mode" => {
                    settings_patch.collaboration_mode = request
                        .arguments
                        .get("mode")
                        .and_then(Value::as_str)
                        .filter(|value| !value.is_empty())
                        .map(str::to_owned);
                }
                "local.reasoning" => {
                    settings_patch.reasoning_effort = request
                        .arguments
                        .get("effort")
                        .and_then(Value::as_str)
                        .and_then(agent_reasoning_effort_from_str);
                }
                _ => {}
            }
            let settings_command_payload = set_turn_settings_command_payload(
                state
                    .session_turn_settings(user_id, session.session_id)
                    .await,
                settings_patch.clone(),
            );
            let universal_command_kind = match request.command_id.as_str() {
                "local.model" => request
                    .arguments
                    .get("model")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(|model| agenter_core::UniversalCommand::SetModel {
                        model: model.to_owned(),
                    }),
                "local.mode" => request
                    .arguments
                    .get("mode")
                    .and_then(Value::as_str)
                    .filter(|value| !value.is_empty())
                    .map(|mode| agenter_core::UniversalCommand::SetMode {
                        mode: mode.to_owned(),
                    }),
                _ => None,
            }
            .unwrap_or(agenter_core::UniversalCommand::SetTurnSettings {
                settings: settings_command_payload,
            });
            let universal_command = slash_command_envelope(
                user_id,
                session.session_id,
                &request,
                universal_command_kind,
            );
            if let Some(response) = super::command_replay_response(
                state.begin_universal_command(&universal_command).await,
            ) {
                return response;
            }
            publish_slash_user_echo(&state, user_id, session.session_id, &request).await;
            let settings = match state
                .update_session_turn_settings(user_id, session.session_id, settings_patch)
                .await
            {
                Ok(Some(settings)) => settings,
                Ok(None) => {
                    if let Some(response) = super::finish_command_or_error_response(
                        &state,
                        &universal_command,
                        crate::state::UniversalCommandIdempotencyStatus::Failed,
                        super::command_response(StatusCode::NOT_FOUND, None),
                    )
                    .await
                    {
                        return response;
                    }
                    return StatusCode::NOT_FOUND.into_response();
                }
                Err(error) => {
                    tracing::warn!(%user_id, session_id = %session.session_id, ?error, "slash settings update failed");
                    if let Some(response) = super::finish_command_or_error_response(
                        &state,
                        &universal_command,
                        crate::state::UniversalCommandIdempotencyStatus::Failed,
                        super::command_response(StatusCode::INTERNAL_SERVER_ERROR, None),
                    )
                    .await
                    {
                        return response;
                    }
                    return StatusCode::INTERNAL_SERVER_ERROR.into_response();
                }
            };
            let result = agenter_core::SlashCommandResult {
                accepted: true,
                message: "Session settings updated.".to_owned(),
                session: None,
                provider_payload: Some(serde_json::to_value(settings).unwrap_or_default()),
            };
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Succeeded,
                super::command_response(StatusCode::OK, serde_json::to_value(&result).ok()),
            )
            .await
            {
                return response;
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
        "local.help" => {
            publish_slash_user_echo(&state, user_id, session.session_id, &request).await;
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
            let universal_command = slash_command_envelope(
                user_id,
                session.session_id,
                &request,
                agenter_core::UniversalCommand::ExecuteProviderCommand {
                    command: semantic_slash_request(&request),
                },
            );
            if let Some(response) = super::command_replay_response(
                state.begin_universal_command(&universal_command).await,
            ) {
                return response;
            }
            publish_slash_user_echo(&state, user_id, session.session_id, &request).await;
            let title = request
                .arguments
                .get("title")
                .and_then(Value::as_str)
                .map(str::trim)
                .filter(|title| !title.is_empty())
                .map(str::to_owned);
            create_session_from_slash(
                state,
                user_id,
                session,
                request,
                definition,
                title,
                universal_command,
            )
            .await
        }
        "local.refresh" => {
            let universal_command = slash_command_envelope(
                user_id,
                session.session_id,
                &request,
                agenter_core::UniversalCommand::ExecuteProviderCommand {
                    command: semantic_slash_request(&request),
                },
            );
            if let Some(response) = super::command_replay_response(
                state.begin_universal_command(&universal_command).await,
            ) {
                return response;
            }
            publish_slash_user_echo(&state, user_id, session.session_id, &request).await;
            refresh_workspace_provider_sessions_for_user(
                state,
                user_id,
                session.workspace.workspace_id,
                session.provider_id,
                Some((session.session_id, request, definition, universal_command)),
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
    universal_command: agenter_core::UniversalCommandEnvelope,
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
                if let Some(response) = super::finish_command_or_error_response(
                    &state,
                    &universal_command,
                    crate::state::UniversalCommandIdempotencyStatus::Failed,
                    super::command_response(
                        StatusCode::BAD_GATEWAY,
                        serde_json::to_value(&result).ok(),
                    ),
                )
                .await
                {
                    return response;
                }
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
                if let Some(response) = super::finish_command_or_error_response(
                    &state,
                    &universal_command,
                    crate::state::UniversalCommandIdempotencyStatus::Failed,
                    super::command_response(
                        StatusCode::BAD_GATEWAY,
                        serde_json::to_value(&result).ok(),
                    ),
                )
                .await
                {
                    return response;
                }
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
                let status = super::runner_wait_error_status(error);
                if let Some(response) = super::finish_command_or_error_response(
                    &state,
                    &universal_command,
                    crate::state::UniversalCommandIdempotencyStatus::Failed,
                    super::command_response(status, serde_json::to_value(&result).ok()),
                )
                .await
                {
                    return response;
                }
                return slash_result_response(
                    &state,
                    source_session_id,
                    &request,
                    &definition,
                    status,
                    result,
                )
                .await;
            }
        }
    };

    if let Err(error) = state
        .ensure_approval_mode_rules_for_new_session(
            user_id,
            source.workspace.workspace_id,
            &source.provider_id,
            source.turn_settings.as_ref(),
        )
        .await
    {
        tracing::warn!(%user_id, workspace_id = %source.workspace.workspace_id, provider_id = %source.provider_id, %error, "slash session creation rejected because approval-mode rule setup failed");
        let result = agenter_core::SlashCommandResult {
            accepted: false,
            message: "Could not prepare approval policy for the new session.".to_owned(),
            session: None,
            provider_payload: None,
        };
        if let Some(response) = super::finish_command_or_error_response(
            &state,
            &universal_command,
            crate::state::UniversalCommandIdempotencyStatus::Failed,
            super::command_response(
                StatusCode::INTERNAL_SERVER_ERROR,
                serde_json::to_value(&result).ok(),
            ),
        )
        .await
        {
            return response;
        }
        return slash_result_response(
            &state,
            source_session_id,
            &request,
            &definition,
            StatusCode::INTERNAL_SERVER_ERROR,
            result,
        )
        .await;
    }

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
        .publish_universal_event(
            info.session_id,
            None,
            None,
            agenter_core::UniversalEventKind::SessionCreated {
                session: Box::new(info.clone()),
            },
        )
        .await
        .ok();
    let result = agenter_core::SlashCommandResult {
        accepted: true,
        message: "New session created.".to_owned(),
        session: Some(info),
        provider_payload: None,
    };
    if let Some(response) = super::finish_command_or_error_response(
        &state,
        &universal_command,
        crate::state::UniversalCommandIdempotencyStatus::Succeeded,
        super::command_response(StatusCode::OK, serde_json::to_value(&result).ok()),
    )
    .await
    {
        return response;
    }
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

fn slash_command_envelope(
    user_id: agenter_core::UserId,
    session_id: agenter_core::SessionId,
    request: &agenter_core::SlashCommandRequest,
    command: agenter_core::UniversalCommand,
) -> agenter_core::UniversalCommandEnvelope {
    agenter_core::UniversalCommandEnvelope {
        command_id: request
            .universal_command_id
            .unwrap_or_else(agenter_core::CommandId::new),
        idempotency_key: request.idempotency_key.clone().unwrap_or_else(|| {
            format!("uap:slash:{user_id}:{session_id}:{}", uuid::Uuid::new_v4())
        }),
        session_id: Some(session_id),
        turn_id: None,
        command,
    }
}

fn semantic_slash_request(
    request: &agenter_core::SlashCommandRequest,
) -> agenter_core::SlashCommandRequest {
    let mut request = request.clone();
    request.universal_command_id = None;
    request.idempotency_key = None;
    request
}

async fn execute_runner_interrupt_slash_command(
    state: crate::state::AppState,
    session: crate::state::RegisteredSession,
    request: agenter_core::SlashCommandRequest,
    definition: agenter_core::SlashCommandDefinition,
) -> Response {
    let universal_command = slash_command_envelope(
        session.owner_user_id,
        session.session_id,
        &request,
        agenter_core::UniversalCommand::CancelTurn {
            request: Some(semantic_slash_request(&request)),
        },
    );
    if let Some(response) =
        super::command_replay_response(state.begin_universal_command(&universal_command).await)
    {
        return response;
    }
    publish_slash_user_echo(&state, session.owner_user_id, session.session_id, &request).await;
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
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Succeeded,
                super::command_response(StatusCode::OK, serde_json::to_value(&result).ok()),
            )
            .await
            {
                return response;
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
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Failed,
                super::command_response(
                    StatusCode::BAD_GATEWAY,
                    serde_json::to_value(&result).ok(),
                ),
            )
            .await
            {
                return response;
            }
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
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Failed,
                super::command_response(
                    StatusCode::BAD_GATEWAY,
                    serde_json::to_value(&result).ok(),
                ),
            )
            .await
            {
                return response;
            }
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
            let status = super::runner_wait_error_status(error);
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Failed,
                super::command_response(status, serde_json::to_value(&result).ok()),
            )
            .await
            {
                return response;
            }
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                status,
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
    let request_id = agenter_protocol::RequestId::from(uuid::Uuid::new_v4().to_string());
    let universal_command = slash_command_envelope(
        user_id,
        session.session_id,
        &request,
        agenter_core::UniversalCommand::ExecuteProviderCommand {
            command: semantic_slash_request(&request),
        },
    );
    if let Some(response) =
        super::command_replay_response(state.begin_universal_command(&universal_command).await)
    {
        return response;
    }
    publish_slash_user_echo(&state, user_id, session.session_id, &request).await;
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
                agenter_protocol::runner::RunnerCommandResult::ProviderCommandExecuted { result },
        }) => {
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Succeeded,
                super::command_response(StatusCode::OK, serde_json::to_value(&result).ok()),
            )
            .await
            {
                return response;
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
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Failed,
                super::command_response(
                    StatusCode::BAD_GATEWAY,
                    serde_json::to_value(&result).ok(),
                ),
            )
            .await
            {
                return response;
            }
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
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Failed,
                super::command_response(
                    StatusCode::BAD_GATEWAY,
                    serde_json::to_value(&result).ok(),
                ),
            )
            .await
            {
                return response;
            }
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
            let status = super::runner_wait_error_status(error);
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Failed,
                super::command_response(status, serde_json::to_value(&result).ok()),
            )
            .await
            {
                return response;
            }
            slash_result_response(
                &state,
                session.session_id,
                &request,
                &definition,
                status,
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
    let item_id = agenter_core::ItemId::new();
    state
        .publish_universal_event(
            session_id,
            None,
            Some(item_id),
            agenter_core::UniversalEventKind::ItemCreated {
                item: Box::new(agenter_core::ItemState {
                    item_id,
                    session_id,
                    turn_id: None,
                    role: agenter_core::ItemRole::User,
                    status: agenter_core::ItemStatus::Completed,
                    content: vec![agenter_core::ContentBlock {
                        block_id: format!("slash-{}", uuid::Uuid::new_v4()),
                        kind: agenter_core::ContentBlockKind::Text,
                        text: Some(request.raw_input.clone()),
                        mime_type: None,
                        artifact_id: None,
                    }],
                    tool: None,
                    native: None,
                }),
            },
        )
        .await
        .ok();
    let _ = user_id;
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
        .publish_universal_event(
            session_id,
            None,
            None,
            agenter_core::UniversalEventKind::ProviderNotification {
                notification: agenter_core::ProviderNotification {
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
                    severity: Some(if result.accepted {
                        agenter_core::ProviderNotificationSeverity::Info
                    } else {
                        agenter_core::ProviderNotificationSeverity::Error
                    }),
                    subject: Some(request.command_id.clone()),
                },
            },
        )
        .await
        .ok();
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
        agenter_core::UniversalCommandEnvelope,
    )>,
) -> Response {
    let Some((runner_id, workspace)) = state
        .resolve_runner_workspace(workspace_id, &provider_id)
        .await
    else {
        if let Some((session_id, request, definition, universal_command)) = slash_context {
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: "Workspace/provider is not available for refresh.".to_owned(),
                session: None,
                provider_payload: None,
            };
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Failed,
                super::command_response(StatusCode::NOT_FOUND, serde_json::to_value(&result).ok()),
            )
            .await
            {
                return response;
            }
            return slash_result_response(
                &state,
                session_id,
                &request,
                &definition,
                StatusCode::NOT_FOUND,
                result,
            )
            .await;
        }
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
        .start_workspace_session_refresh(
            runner_id,
            request_id.clone(),
            command,
            false,
            super::RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await
    {
        Ok(()) => {
            let result = agenter_core::SlashCommandResult {
                accepted: true,
                message: format!("Refresh queued: {request_id}."),
                session: None,
                provider_payload: Some(serde_json::json!({
                    "refresh_id": request_id.to_string(),
                    "status": "queued",
                })),
            };
            if let Some((session_id, request, definition, universal_command)) = slash_context {
                if let Some(response) = super::finish_command_or_error_response(
                    &state,
                    &universal_command,
                    crate::state::UniversalCommandIdempotencyStatus::Succeeded,
                    super::command_response(StatusCode::OK, serde_json::to_value(&result).ok()),
                )
                .await
                {
                    return response;
                }
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
        Err(error) => {
            let result = agenter_core::SlashCommandResult {
                accepted: false,
                message: super::runner_wait_error_message(error).to_owned(),
                session: None,
                provider_payload: Some(serde_json::json!({
                    "code": super::runner_wait_error_code(error),
                })),
            };
            if let Some((session_id, request, definition, universal_command)) = slash_context {
                let status = super::runner_wait_error_status(error);
                if let Some(response) = super::finish_command_or_error_response(
                    &state,
                    &universal_command,
                    crate::state::UniversalCommandIdempotencyStatus::Failed,
                    super::command_response(status, serde_json::to_value(&result).ok()),
                )
                .await
                {
                    return response;
                }
                slash_result_response(&state, session_id, &request, &definition, status, result)
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
    let commands: Vec<agenter_core::SlashCommandDefinition> = vec![
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
    let _ = provider_id;
    commands
}

fn set_turn_settings_command_payload(
    current: Option<agenter_core::AgentTurnSettings>,
    patch: agenter_core::AgentTurnSettings,
) -> agenter_core::AgentTurnSettings {
    let mut merged = current.unwrap_or_default();
    if patch.model.is_some() {
        merged.model = patch.model;
    }
    if patch.reasoning_effort.is_some() {
        merged.reasoning_effort = patch.reasoning_effort;
    }
    if patch.collaboration_mode.is_some() {
        merged.collaboration_mode = patch.collaboration_mode;
    }
    if patch.approval_mode.is_some() {
        merged.approval_mode = patch.approval_mode;
    }
    merged
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_turn_settings_command_payload_keeps_existing_sparse_fields() {
        let current = agenter_core::AgentTurnSettings {
            model: Some("gpt-5.2".to_owned()),
            reasoning_effort: Some(agenter_core::AgentReasoningEffort::Medium),
            collaboration_mode: Some("plan".to_owned()),
            approval_mode: Some(agenter_core::ApprovalMode::AllowAllWorkspace),
        };
        let patch = agenter_core::AgentTurnSettings {
            model: None,
            reasoning_effort: Some(agenter_core::AgentReasoningEffort::High),
            collaboration_mode: None,
            approval_mode: None,
        };

        let merged = set_turn_settings_command_payload(Some(current), patch);

        assert_eq!(merged.model.as_deref(), Some("gpt-5.2"));
        assert_eq!(
            merged.reasoning_effort,
            Some(agenter_core::AgentReasoningEffort::High)
        );
        assert_eq!(merged.collaboration_mode.as_deref(), Some("plan"));
        assert_eq!(
            merged.approval_mode,
            Some(agenter_core::ApprovalMode::AllowAllWorkspace)
        );
    }
}
