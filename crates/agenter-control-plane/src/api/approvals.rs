use crate::state::{ApprovalResolutionLookup, ApprovalResolutionStart, RunnerCommandWaitError};
use agenter_core::SessionId;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub(super) struct ListApprovalsQuery {
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Deserialize, Serialize)]
pub(super) struct ApprovalDecisionRequest {
    #[serde(flatten)]
    pub decision: agenter_core::ApprovalDecision,
    #[serde(default)]
    pub option_id: Option<String>,
    #[serde(default)]
    pub feedback: Option<String>,
}

pub(super) fn router() -> Router<crate::state::AppState> {
    Router::new()
        .route("/", get(list_approvals))
        .route("/{approval_id}/decision", post(decide_approval))
}

pub(super) async fn list_approvals(
    state: State<crate::state::AppState>,
    headers: HeaderMap,
    Query(query): Query<ListApprovalsQuery>,
) -> Response {
    let state = state.0;
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        return StatusCode::UNAUTHORIZED.into_response();
    };
    if !state
        .can_access_session(user.user_id, query.session_id)
        .await
    {
        tracing::debug!(
            user_id = %user.user_id,
            session_id = %query.session_id,
            "list approvals rejected session access"
        );
        return StatusCode::FORBIDDEN.into_response();
    }

    Json(state.pending_approval_requests(query.session_id).await).into_response()
}

pub(super) async fn decide_approval(
    state: State<crate::state::AppState>,
    headers: HeaderMap,
    Path(approval_id): Path<agenter_core::ApprovalId>,
    decision: Json<ApprovalDecisionRequest>,
) -> Response {
    let state = state.0;
    let decision_request = decision.0;
    let decision = decision_request.decision.clone();
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%approval_id, "approval decision rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let lookup = state.lookup_approval_resolution(approval_id).await;
    let session_id = match &lookup {
        ApprovalResolutionLookup::Missing => {
            tracing::warn!(%approval_id, "approval decision rejected missing approval");
            return StatusCode::NOT_FOUND.into_response();
        }
        ApprovalResolutionLookup::Pending { session_id }
        | ApprovalResolutionLookup::InProgress { session_id, .. }
        | ApprovalResolutionLookup::AlreadyResolved { session_id, .. } => *session_id,
    };
    let Some(session) = state.session(user.user_id, session_id).await else {
        tracing::warn!(user_id = %user.user_id, %approval_id, %session_id, "approval decision rejected session not found");
        return StatusCode::NOT_FOUND.into_response();
    };

    let universal_command = approval_command_envelope(
        user.user_id,
        session_id,
        approval_id,
        &decision,
        decision_request.option_id.as_deref(),
        decision_request.feedback.clone(),
    );

    match lookup {
        ApprovalResolutionLookup::AlreadyResolved { request, .. } => {
            if !approval_request_matches_decision(&request, &decision) {
                return super::command_conflict_response(approval_decision_conflict())
                    .into_response();
            }
            return Json(*request).into_response();
        }
        ApprovalResolutionLookup::InProgress { request, .. } => {
            if !approval_request_matches_decision(&request, &decision) {
                return super::command_conflict_response(approval_decision_conflict())
                    .into_response();
            }
            tracing::debug!(%approval_id, "approval decision already resolving");
            return Json(*request).into_response();
        }
        ApprovalResolutionLookup::Pending { .. } => {
            let command_start = state.begin_universal_command(&universal_command).await;
            if let Some(response) = super::command_replay_response(command_start) {
                return response;
            }
        }
        ApprovalResolutionLookup::Missing => unreachable!("handled before session lookup"),
    }

    match state
        .begin_approval_resolution(approval_id, decision.clone())
        .await
    {
        ApprovalResolutionStart::Started => {}
        ApprovalResolutionStart::AlreadyResolved { request, .. } => {
            let body = serde_json::to_value(&*request).ok();
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Succeeded,
                super::command_response(StatusCode::OK, body),
            )
            .await
            {
                return response;
            }
            return Json(*request).into_response();
        }
        ApprovalResolutionStart::InProgress { request, .. } => {
            return Json(*request).into_response();
        }
        ApprovalResolutionStart::Missing => {
            let response = super::command_response(StatusCode::NOT_FOUND, None);
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Failed,
                response,
            )
            .await
            {
                return response;
            }
            return StatusCode::NOT_FOUND.into_response();
        }
    }

    if let Some(option_id) = decision_request.option_id.as_deref() {
        if option_id.starts_with("persist_rule:") {
            persist_approval_rule_option(&state, user.user_id, session_id, approval_id, option_id)
                .await;
        }
    }

    let request_id = agenter_protocol::RequestId::from(Uuid::new_v4().to_string());
    let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::AnswerApproval(
                agenter_protocol::runner::ApprovalAnswerCommand {
                    session_id,
                    approval_id,
                    decision: decision.clone(),
                },
            ),
        },
    ));
    let operation = state
        .start_runner_command_operation(
            session.runner_id,
            request_id,
            command,
            super::RUNNER_COMMAND_RESPONSE_TIMEOUT,
        )
        .await;
    let (result_sender, result_receiver) = tokio::sync::oneshot::channel();
    let state_for_resolution = state.clone();
    let decision_for_resolution = decision.clone();
    let command_for_resolution = universal_command.clone();
    let user_id = user.user_id;
    tokio::spawn(async move {
        let outcome = operation
            .await
            .unwrap_or(Err(RunnerCommandWaitError::Closed));
        let result = finish_approval_operation(
            state_for_resolution,
            approval_id,
            session_id,
            decision_for_resolution,
            Some(user_id),
            command_for_resolution,
            outcome,
        )
        .await;
        result_sender.send(result).ok();
    });

    match result_receiver
        .await
        .unwrap_or(Err(StatusCode::BAD_GATEWAY))
    {
        Ok(envelope) => Json(envelope).into_response(),
        Err(status) => status.into_response(),
    }
}

async fn persist_approval_rule_option(
    state: &crate::state::AppState,
    user_id: agenter_core::UserId,
    session_id: SessionId,
    approval_id: agenter_core::ApprovalId,
    option_id: &str,
) {
    let Some(pool) = state.db_pool() else {
        tracing::warn!(%approval_id, "persistent approval rule ignored without database");
        return;
    };
    let Some(session) = state.session(user_id, session_id).await else {
        tracing::warn!(%approval_id, %session_id, "persistent approval rule ignored for missing session");
        return;
    };
    let Some(request) = state.approval_request(approval_id).await else {
        tracing::warn!(%approval_id, "persistent approval rule ignored for missing pending request");
        return;
    };
    let Some(preview) = request
        .options
        .iter()
        .find(|option| option.option_id == option_id)
        .and_then(|option| option.policy_rule.clone())
    else {
        tracing::warn!(%approval_id, option_id, "persistent approval rule option missing preview");
        return;
    };

    if let Err(error) = agenter_db::create_approval_policy_rule(
        pool,
        agenter_db::NewApprovalPolicyRule {
            owner_user_id: user_id,
            workspace_id: session.workspace.workspace_id,
            provider_id: &session.provider_id,
            kind: preview.kind,
            label: &preview.label,
            matcher: &preview.matcher,
            decision: &preview.decision,
            source_approval_id: Some(approval_id),
            created_by_user_id: Some(user_id),
        },
    )
    .await
    {
        tracing::warn!(%approval_id, %error, "failed to persist approval policy rule");
    }
}

fn approval_request_matches_decision(
    request: &agenter_core::ApprovalRequest,
    incoming: &agenter_core::ApprovalDecision,
) -> bool {
    request.resolving_decision.as_ref() == Some(incoming)
        || (request.status.is_terminal() && request.resolving_decision.is_none())
}

fn approval_decision_conflict() -> crate::state::UniversalCommandConflict {
    crate::state::UniversalCommandConflict {
        code: "approval_decision_conflict".to_owned(),
        message: "Approval already has a different decision in progress or resolved.".to_owned(),
    }
}

fn approval_command_envelope(
    user_id: agenter_core::UserId,
    session_id: SessionId,
    approval_id: agenter_core::ApprovalId,
    decision: &agenter_core::ApprovalDecision,
    option_id: Option<&str>,
    feedback: Option<String>,
) -> agenter_core::UniversalCommandEnvelope {
    let effective_option_id = option_id
        .filter(|option_id| !option_id.is_empty())
        .unwrap_or_else(|| approval_option_id(decision))
        .to_owned();
    let effective_feedback = feedback.or_else(|| approval_feedback(decision));
    agenter_core::UniversalCommandEnvelope {
        command_id: agenter_core::CommandId::new(),
        idempotency_key: format!("uap:approval:{user_id}:{approval_id}:{effective_option_id}"),
        session_id: Some(session_id),
        turn_id: None,
        command: agenter_core::UniversalCommand::ResolveApproval {
            approval_id,
            option_id: effective_option_id,
            feedback: effective_feedback,
        },
    }
}

fn approval_option_id(decision: &agenter_core::ApprovalDecision) -> &'static str {
    decision.canonical_option_id()
}

fn approval_feedback(decision: &agenter_core::ApprovalDecision) -> Option<String> {
    match decision {
        agenter_core::ApprovalDecision::ProviderSpecific { payload } => {
            let payload_bytes = serde_json::to_vec(payload).unwrap_or_default();
            let hash = Sha256::digest(&payload_bytes);
            let fingerprint = format!("provider_specific_sha256:{hash:x}");
            payload
                .get("feedback")
                .and_then(serde_json::Value::as_str)
                .filter(|feedback| !feedback.is_empty())
                .map(|feedback| format!("{feedback}\n{fingerprint}"))
                .or(Some(fingerprint))
        }
        _ => None,
    }
}

async fn finish_approval_operation(
    state: crate::state::AppState,
    approval_id: agenter_core::ApprovalId,
    session_id: SessionId,
    decision: agenter_core::ApprovalDecision,
    resolved_by_user_id: Option<agenter_core::UserId>,
    command: agenter_core::UniversalCommandEnvelope,
    outcome: Result<
        agenter_protocol::runner::RunnerResponseOutcome,
        crate::state::RunnerCommandWaitError,
    >,
) -> Result<agenter_core::ApprovalRequest, StatusCode> {
    match outcome {
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok {
            result: agenter_protocol::runner::RunnerCommandResult::Accepted,
        }) => {
            let Some(request) = state
                .finish_approval_resolution(approval_id, session_id, decision, resolved_by_user_id)
                .await
            else {
                tracing::warn!(%approval_id, %session_id, "approval decision could not finish resolution");
                return Err(StatusCode::CONFLICT);
            };
            if state
                .finish_universal_command(
                    &command,
                    crate::state::UniversalCommandIdempotencyStatus::Succeeded,
                    super::command_response(StatusCode::OK, serde_json::to_value(&request).ok()),
                )
                .await
                .is_err()
            {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
            tracing::info!(%approval_id, %session_id, "approval decision resolved after runner acknowledgement");
            Ok(request)
        }
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            cancel_failed_approval_resolution(
                &state,
                approval_id,
                session_id,
                "approval_rejected_by_runner",
                format!(
                    "Approval decision was rejected by the runner: {}",
                    error.message
                ),
            )
            .await;
            if state
                .finish_universal_command(
                    &command,
                    crate::state::UniversalCommandIdempotencyStatus::Failed,
                    super::command_response(
                        StatusCode::BAD_GATEWAY,
                        serde_json::to_value(&error).ok(),
                    ),
                )
                .await
                .is_err()
            {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
            Err(StatusCode::BAD_GATEWAY)
        }
        Ok(other) => {
            cancel_failed_approval_resolution(
                &state,
                approval_id,
                session_id,
                "approval_unexpected_runner_response",
                format!("Approval decision received an unexpected runner response: {other:?}"),
            )
            .await;
            if state
                .finish_universal_command(
                    &command,
                    crate::state::UniversalCommandIdempotencyStatus::Failed,
                    super::command_response(StatusCode::BAD_GATEWAY, None),
                )
                .await
                .is_err()
            {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
            Err(StatusCode::BAD_GATEWAY)
        }
        Err(error) => {
            cancel_failed_approval_resolution(
                &state,
                approval_id,
                session_id,
                "approval_runner_unavailable",
                format!("Approval decision could not reach the runner: {error:?}"),
            )
            .await;
            let status = super::runner_wait_error_status(error);
            if state.clear_universal_command(&command).await.is_err() {
                return Err(StatusCode::SERVICE_UNAVAILABLE);
            }
            Err(status)
        }
    }
}

async fn cancel_failed_approval_resolution(
    state: &crate::state::AppState,
    approval_id: agenter_core::ApprovalId,
    session_id: SessionId,
    code: &str,
    message: String,
) {
    state.cancel_approval_resolution(approval_id).await;
    state
        .publish_universal_event(
            session_id,
            None,
            None,
            agenter_core::UniversalEventKind::SessionStatusChanged {
                status: agenter_core::SessionStatus::WaitingForApproval,
                reason: Some("Approval is still waiting.".to_owned()),
            },
        )
        .await
        .ok();
    state
        .publish_universal_event(
            session_id,
            None,
            None,
            agenter_core::UniversalEventKind::ErrorReported {
                code: Some(code.to_owned()),
                message,
            },
        )
        .await
        .ok();
}

pub(super) async fn answer_question(
    state: State<crate::state::AppState>,
    headers: HeaderMap,
    Path(question_id): Path<agenter_core::QuestionId>,
    answer: Json<agenter_core::AgentQuestionAnswer>,
) -> Response {
    let state = state.0;
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%question_id, "question answer rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let mut answer = answer.0;
    answer.question_id = question_id;
    let universal_command = agenter_core::UniversalCommandEnvelope {
        command_id: agenter_core::CommandId::new(),
        idempotency_key: format!("uap:question:{}:{question_id}", user.user_id),
        session_id: None,
        turn_id: None,
        command: agenter_core::UniversalCommand::AnswerQuestion {
            question_id,
            answer: answer.clone(),
        },
    };
    if let Some(response) =
        super::command_replay_response(state.begin_universal_command(&universal_command).await)
    {
        return response;
    }
    let Some(session_id) = state.question_session(question_id).await else {
        tracing::warn!(%question_id, "question answer rejected missing question");
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
    let Some(session) = state.session(user.user_id, session_id).await else {
        tracing::warn!(user_id = %user.user_id, %question_id, %session_id, "question answer rejected session not found");
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
    let request_id = agenter_protocol::RequestId::from(Uuid::new_v4().to_string());
    let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: request_id.clone(),
            command: agenter_protocol::runner::RunnerCommand::AnswerQuestion(
                agenter_protocol::runner::QuestionAnswerCommand {
                    session_id,
                    answer: answer.clone(),
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
            result: agenter_protocol::runner::RunnerCommandResult::Accepted,
        }) => {
            let Ok(envelope) = state.finish_question_answer(session_id, answer).await else {
                return StatusCode::SERVICE_UNAVAILABLE.into_response();
            };
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Succeeded,
                super::command_response(StatusCode::OK, serde_json::to_value(&envelope).ok()),
            )
            .await
            {
                return response;
            }
            Json(envelope).into_response()
        }
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Error { error }) => {
            tracing::warn!(
                user_id = %user.user_id,
                %question_id,
                code = %error.code,
                message = %error.message,
                "question answer rejected by runner"
            );
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Failed,
                super::command_response(StatusCode::BAD_GATEWAY, serde_json::to_value(&error).ok()),
            )
            .await
            {
                return response;
            }
            StatusCode::BAD_GATEWAY.into_response()
        }
        Ok(other) => {
            tracing::warn!(user_id = %user.user_id, %question_id, ?other, "question answer received unexpected runner response");
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Failed,
                super::command_response(StatusCode::BAD_GATEWAY, None),
            )
            .await
            {
                return response;
            }
            StatusCode::BAD_GATEWAY.into_response()
        }
        Err(error) => {
            tracing::warn!(user_id = %user.user_id, %question_id, ?error, "question answer failed because runner is unavailable");
            let status = super::runner_wait_error_status(error);
            if let Some(response) = super::finish_command_or_error_response(
                &state,
                &universal_command,
                crate::state::UniversalCommandIdempotencyStatus::Failed,
                super::command_response(status, None),
            )
            .await
            {
                return response;
            }
            status.into_response()
        }
    }
}
