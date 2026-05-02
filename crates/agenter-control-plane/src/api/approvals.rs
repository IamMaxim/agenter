use crate::state::{ApprovalResolutionStart, RunnerSendError};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
    routing::{get, post},
    Json, Router,
};
use chrono::Utc;
use uuid::Uuid;

pub(super) fn router() -> Router<crate::state::AppState> {
    Router::new()
        .route("/", get(list_approvals))
        .route("/{approval_id}/decision", post(decide_approval))
}

pub(super) async fn list_approvals(
    state: State<crate::state::AppState>,
    headers: HeaderMap,
) -> Response {
    let state = state.0;
    if super::authenticated_user_from_headers(&state, &headers)
        .await
        .is_none()
    {
        return StatusCode::UNAUTHORIZED.into_response();
    }

    Json(Vec::<agenter_protocol::browser::BrowserEventEnvelope>::new()).into_response()
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
    let session_id = match state.begin_approval_resolution(approval_id).await {
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
        ApprovalResolutionStart::InProgress => {
            tracing::warn!(%approval_id, "approval decision rejected because resolution is already in progress");
            return StatusCode::CONFLICT.into_response();
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

    let command = agenter_protocol::runner::RunnerServerMessage::Command(Box::new(
        agenter_protocol::runner::RunnerCommandEnvelope {
            request_id: agenter_protocol::RequestId::from(Uuid::new_v4().to_string()),
            command: agenter_protocol::runner::RunnerCommand::AnswerApproval(
                agenter_protocol::runner::ApprovalAnswerCommand {
                    session_id,
                    approval_id,
                    decision: decision.clone(),
                },
            ),
        },
    ));
    match state.send_runner_message(session.runner_id, command).await {
        Ok(()) => {}
        Err(RunnerSendError::NotConnected | RunnerSendError::Closed) => {
            state.cancel_approval_resolution(approval_id).await;
            tracing::warn!(
                user_id = %user.user_id,
                %approval_id,
                %session_id,
                runner_id = %session.runner_id,
                "approval decision failed because runner is unavailable"
            );
            return StatusCode::SERVICE_UNAVAILABLE.into_response();
        }
        Err(RunnerSendError::StaleApproval) => {
            tracing::warn!(%approval_id, %session_id, "approval decision raced with runner-side resolution");
            return match state.begin_approval_resolution(approval_id).await {
                ApprovalResolutionStart::AlreadyResolved {
                    session_id,
                    envelope,
                } => {
                    if state.session(user.user_id, session_id).await.is_none() {
                        StatusCode::NOT_FOUND.into_response()
                    } else {
                        Json(*envelope).into_response()
                    }
                }
                _ => StatusCode::CONFLICT.into_response(),
            };
        }
    }

    let resolved = agenter_core::AppEvent::ApprovalResolved(agenter_core::ApprovalResolvedEvent {
        session_id,
        approval_id,
        decision: decision.clone(),
        resolved_by_user_id: Some(user.user_id),
        resolved_at: Utc::now(),
        provider_payload: None,
    });
    let Some(envelope) = state
        .finish_approval_resolution(approval_id, session_id, resolved)
        .await
    else {
        tracing::warn!(%approval_id, %session_id, "approval decision could not finish resolution");
        return StatusCode::CONFLICT.into_response();
    };

    tracing::info!(user_id = %user.user_id, %approval_id, %session_id, "approval decision resolved");
    Json(envelope).into_response()
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
