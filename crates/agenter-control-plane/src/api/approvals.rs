use crate::state::{ApprovalResolutionStart, RunnerCommandWaitError};
use agenter_core::SessionId;
use axum::{
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use serde::Deserialize;
use uuid::Uuid;

#[derive(Debug, Deserialize)]
pub(super) struct ListApprovalsQuery {
    pub session_id: SessionId,
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

    let envelopes = state
        .pending_approval_request_envelopes(query.session_id)
        .await;
    Json(envelopes).into_response()
}

pub(super) async fn decide_approval(
    state: State<crate::state::AppState>,
    headers: HeaderMap,
    Path(approval_id): Path<agenter_core::ApprovalId>,
    decision: Json<agenter_core::ApprovalDecision>,
) -> Response {
    let state = state.0;
    let decision = decision.0;
    let Some(user) = super::authenticated_user_from_headers(&state, &headers).await else {
        tracing::debug!(%approval_id, "approval decision rejected missing or invalid session");
        return StatusCode::UNAUTHORIZED.into_response();
    };
    let session_id = match state
        .begin_approval_resolution(approval_id, decision.clone())
        .await
    {
        ApprovalResolutionStart::Started { session_id } => session_id,
        ApprovalResolutionStart::AlreadyResolved {
            session_id,
            envelope,
        } => {
            if state.session(user.user_id, session_id).await.is_none() {
                return StatusCode::NOT_FOUND.into_response();
            }
            return Json(*envelope).into_response();
        }
        ApprovalResolutionStart::InProgress {
            session_id,
            envelope,
        } => {
            if state.session(user.user_id, session_id).await.is_none() {
                return StatusCode::NOT_FOUND.into_response();
            }
            tracing::debug!(%approval_id, "approval decision already resolving");
            return Json(*envelope).into_response();
        }
        ApprovalResolutionStart::Missing => {
            tracing::warn!(%approval_id, "approval decision rejected missing approval");
            return StatusCode::NOT_FOUND.into_response();
        }
    };
    let Some(session) = state.session(user.user_id, session_id).await else {
        state.cancel_approval_resolution(approval_id).await;
        tracing::warn!(user_id = %user.user_id, %approval_id, %session_id, "approval decision rejected session not found");
        return StatusCode::NOT_FOUND.into_response();
    };

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

async fn finish_approval_operation(
    state: crate::state::AppState,
    approval_id: agenter_core::ApprovalId,
    session_id: SessionId,
    decision: agenter_core::ApprovalDecision,
    resolved_by_user_id: Option<agenter_core::UserId>,
    outcome: Result<
        agenter_protocol::runner::RunnerResponseOutcome,
        crate::state::RunnerCommandWaitError,
    >,
) -> Result<agenter_protocol::browser::BrowserEventEnvelope, StatusCode> {
    match outcome {
        Ok(agenter_protocol::runner::RunnerResponseOutcome::Ok {
            result: agenter_protocol::runner::RunnerCommandResult::Accepted,
        }) => {
            let resolved =
                agenter_core::AppEvent::ApprovalResolved(agenter_core::ApprovalResolvedEvent {
                    session_id,
                    approval_id,
                    decision,
                    resolved_by_user_id,
                    resolved_at: Utc::now(),
                    provider_payload: None,
                });
            let Some(envelope) = state
                .finish_approval_resolution(approval_id, session_id, resolved)
                .await
            else {
                tracing::warn!(%approval_id, %session_id, "approval decision could not finish resolution");
                return Err(StatusCode::CONFLICT);
            };
            tracing::info!(%approval_id, %session_id, "approval decision resolved after runner acknowledgement");
            Ok(envelope)
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
            Err(super::runner_wait_error_status(error))
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
        .publish_event(
            session_id,
            agenter_core::AppEvent::SessionStatusChanged(agenter_core::SessionStatusChangedEvent {
                session_id,
                status: agenter_core::SessionStatus::WaitingForApproval,
                reason: Some("Approval is still waiting.".to_owned()),
            }),
        )
        .await;
    state
        .publish_event(
            session_id,
            agenter_core::AppEvent::Error(agenter_core::AgentErrorEvent {
                session_id: Some(session_id),
                code: Some(code.to_owned()),
                message,
                provider_payload: None,
            }),
        )
        .await;
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
    let Some(session_id) = state.question_session(question_id).await else {
        tracing::warn!(%question_id, "question answer rejected missing question");
        return StatusCode::NOT_FOUND.into_response();
    };
    let Some(session) = state.session(user.user_id, session_id).await else {
        tracing::warn!(user_id = %user.user_id, %question_id, %session_id, "question answer rejected session not found");
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
            let envelope = state.finish_question_answer(session_id, answer).await;
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
            StatusCode::BAD_GATEWAY.into_response()
        }
        Ok(other) => {
            tracing::warn!(user_id = %user.user_id, %question_id, ?other, "question answer received unexpected runner response");
            StatusCode::BAD_GATEWAY.into_response()
        }
        Err(error) => {
            tracing::warn!(user_id = %user.user_id, %question_id, ?error, "question answer failed because runner is unavailable");
            super::runner_wait_error_status(error).into_response()
        }
    }
}
