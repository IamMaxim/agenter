mod agents;
mod modes;
mod runner_host;
mod wal;

use std::{
    collections::HashMap,
    env,
    path::{Path, PathBuf},
    sync::Arc,
};

use agenter_core::{
    AgentCapabilities, AgentProviderId, ApprovalDecision, ApprovalId, RunnerId, SessionId,
    WorkspaceId, WorkspaceRef,
};
use agenter_protocol::runner::{
    AgentInput, AgentProviderAdvertisement, RunnerCapabilities, RunnerCommandResult, RunnerError,
    RunnerHello, RunnerResponseOutcome, PROTOCOL_VERSION,
};
use agents::acp::AcpProviderProfile;
use agents::approval_state::{PendingApprovalSubmitError, PendingProviderApproval};
use agents::codex::runtime::CodexRunnerRuntime;
use tokio::sync::Mutex;

const DEFAULT_CONTROL_PLANE_WS: &str = "ws://127.0.0.1:7777/api/runner/ws";
const DEFAULT_DEV_RUNNER_TOKEN: &str = "dev-runner-token";

async fn answer_pending_provider_approval(
    approval_id: ApprovalId,
    decision: ApprovalDecision,
    pending: PendingProviderApproval,
    provider_label: &'static str,
) -> RunnerResponseOutcome {
    match pending.submit(decision).await {
        Ok(()) => {
            tracing::info!(
                %approval_id,
                provider = provider_label,
                "provider approval decision acknowledged"
            );
            RunnerResponseOutcome::Ok {
                result: RunnerCommandResult::Accepted,
            }
        }
        Err(PendingApprovalSubmitError::ConflictingDecision) => RunnerResponseOutcome::Error {
            error: RunnerError {
                code: "approval_conflicting_decision".to_owned(),
                message: format!(
                    "{provider_label} approval {approval_id} is already resolving with a different decision"
                ),
            },
        },
        Err(PendingApprovalSubmitError::ProviderWaiterDropped) => RunnerResponseOutcome::Error {
            error: RunnerError {
                code: format!("{}_approval_response_failed", provider_label.to_lowercase()),
                message: format!(
                    "{provider_label} approval waiter was dropped before the decision could be delivered"
                ),
            },
        },
        Err(PendingApprovalSubmitError::ProviderRejected(message)) => RunnerResponseOutcome::Error {
            error: RunnerError {
                code: format!("{}_approval_response_failed", provider_label.to_lowercase()),
                message,
            },
        },
        Err(PendingApprovalSubmitError::AcknowledgementDropped) => RunnerResponseOutcome::Error {
            error: RunnerError {
                code: format!("{}_approval_response_failed", provider_label.to_lowercase()),
                message: format!(
                    "{provider_label} approval response acknowledgement was dropped"
                ),
            },
        },
    }
}

async fn cancel_pending_provider_approvals_for_session(
    session_id: SessionId,
    approvals: Arc<Mutex<HashMap<ApprovalId, PendingProviderApproval>>>,
    provider_label: &'static str,
) -> usize {
    let candidates = {
        let approvals = approvals.lock().await;
        approvals
            .iter()
            .map(|(&approval_id, approval)| (approval_id, approval.clone()))
            .collect::<Vec<_>>()
    };
    let mut pending = Vec::new();
    for (approval_id, approval) in candidates {
        if approval.session_id().await == session_id && approval.is_live().await {
            pending.push((approval_id, approval));
        }
    }
    let mut cancelled = 0;
    for (approval_id, approval) in pending {
        match answer_pending_provider_approval(
            approval_id,
            ApprovalDecision::Cancel,
            approval,
            provider_label,
        )
        .await
        {
            RunnerResponseOutcome::Ok { .. } => cancelled += 1,
            RunnerResponseOutcome::Error { error } => {
                tracing::warn!(
                    %session_id,
                    %approval_id,
                    code = %error.code,
                    message = %error.message,
                    "failed to cancel blocked provider approval"
                );
            }
        }
    }
    cancelled
}

fn provider_cancel_unsupported(provider_label: &'static str) -> RunnerResponseOutcome {
    RunnerResponseOutcome::Error {
        error: RunnerError {
            code: "provider_cancel_not_supported".to_owned(),
            message: format!(
                "{provider_label} cannot interrupt the current turn in this runner path."
            ),
        },
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    agenter_core::logging::init_tracing("agenter-runner");

    if fake_mode_requested() {
        tracing::info!("starting fake runner mode");
        modes::fake::run().await?;
    } else if codex_mode_requested() {
        tracing::info!("starting codex app-server runner mode");
        modes::codex::run().await?;
    } else if acp_mode_requested() {
        tracing::info!("starting multi-provider ACP runner mode");
        modes::acp::run(AcpProviderProfile::available_all()).await?;
    } else if qwen_mode_requested() {
        tracing::info!("starting qwen ACP runner mode");
        modes::acp::run(vec![AcpProviderProfile::qwen()]).await?;
    } else if gemini_mode_requested() {
        tracing::info!("starting gemini ACP runner mode");
        modes::acp::run(vec![AcpProviderProfile::gemini()]).await?;
    } else if opencode_mode_requested() {
        tracing::info!("starting opencode ACP runner mode");
        modes::acp::run(vec![AcpProviderProfile::opencode()]).await?;
    } else {
        tracing::info!("starting unified runner mode");
        modes::acp::run(AcpProviderProfile::available_all()).await?
    }

    Ok(())
}

fn fake_mode_requested() -> bool {
    env::args().any(|arg| arg == "fake" || arg == "--fake")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "fake")
}

fn qwen_mode_requested() -> bool {
    env::args().any(|arg| arg == "qwen" || arg == "--qwen")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "qwen")
}

fn acp_mode_requested() -> bool {
    env::args().any(|arg| arg == "acp" || arg == "--acp")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "acp")
}

fn codex_mode_requested() -> bool {
    env::args().any(|arg| arg == "codex" || arg == "--codex")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "codex")
}

fn gemini_mode_requested() -> bool {
    env::args().any(|arg| arg == "gemini" || arg == "--gemini")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "gemini")
}

fn opencode_mode_requested() -> bool {
    env::args().any(|arg| arg == "opencode" || arg == "--opencode")
        || env::var("AGENTER_RUNNER_MODE").is_ok_and(|mode| mode == "opencode")
}

fn runner_wal_path(runner_id: RunnerId, workspace_path: &Path) -> PathBuf {
    env::var("AGENTER_RUNNER_WAL")
        .map(PathBuf::from)
        .unwrap_or_else(|_| {
            workspace_path
                .join(".agenter")
                .join(format!("runner-{runner_id}-events.jsonl"))
        })
}

fn runner_error(code: &str, error: anyhow::Error) -> RunnerError {
    RunnerError {
        code: code.to_owned(),
        message: error.to_string(),
    }
}

fn default_agent_capabilities(session_resume: bool, interrupt: bool) -> AgentCapabilities {
    AgentCapabilities {
        session_resume,
        interrupt,
        ..AgentCapabilities::default()
    }
}

fn acp_hello(
    token: String,
    workspace_path: PathBuf,
    profiles: &[AcpProviderProfile],
) -> RunnerHello {
    let provider_id = AgentProviderId::from("acp");
    let runner_id = configured_runner_id(&provider_id, &workspace_path);
    let workspace_id = configured_workspace_id(&provider_id, &workspace_path);
    let agent_providers = profiles
        .iter()
        .map(|profile| AgentProviderAdvertisement {
            provider_id: profile.provider_id.clone(),
            capabilities: profile.advertised_capabilities(),
        })
        .collect();
    RunnerHello {
        runner_id,
        protocol_version: PROTOCOL_VERSION.to_owned(),
        token,
        capabilities: RunnerCapabilities {
            agent_providers,
            transports: vec!["acp-stdio".to_owned()],
            workspace_discovery: false,
        },
        acked_runner_event_seq: None,
        replay_from_runner_event_seq: None,
        workspaces: vec![WorkspaceRef {
            workspace_id,
            runner_id,
            path: workspace_path.display().to_string(),
            display_name: workspace_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
                .or_else(|| Some("acp workspace".to_owned())),
        }],
    }
}

fn codex_hello(token: String, workspace_path: PathBuf) -> RunnerHello {
    let registration = CodexRunnerRuntime::registration();
    let runner_id = configured_runner_id(&registration.provider_id, &workspace_path);
    let workspace_id = configured_workspace_id(&registration.provider_id, &workspace_path);
    RunnerHello {
        runner_id,
        protocol_version: PROTOCOL_VERSION.to_owned(),
        token,
        capabilities: RunnerCapabilities {
            agent_providers: vec![AgentProviderAdvertisement {
                provider_id: registration.provider_id,
                capabilities: registration.capabilities,
            }],
            transports: vec!["codex-app-server".to_owned()],
            workspace_discovery: false,
        },
        acked_runner_event_seq: None,
        replay_from_runner_event_seq: None,
        workspaces: vec![WorkspaceRef {
            workspace_id,
            runner_id,
            path: workspace_path.display().to_string(),
            display_name: workspace_path
                .file_name()
                .and_then(|name| name.to_str())
                .map(str::to_owned)
                .or_else(|| Some("codex workspace".to_owned())),
        }],
    }
}

fn configured_runner_id(provider_id: &AgentProviderId, workspace_path: &Path) -> RunnerId {
    env::var("AGENTER_RUNNER_ID")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| {
            RunnerId::from_uuid(uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_URL,
                format!("agenter:runner:{provider_id}:{}", workspace_path.display()).as_bytes(),
            ))
        })
}

fn configured_workspace_id(provider_id: &AgentProviderId, workspace_path: &Path) -> WorkspaceId {
    env::var("AGENTER_WORKSPACE_ID")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or_else(|| {
            WorkspaceId::from_uuid(uuid::Uuid::new_v5(
                &uuid::Uuid::NAMESPACE_URL,
                format!(
                    "agenter:workspace:{provider_id}:{}",
                    workspace_path.display()
                )
                .as_bytes(),
            ))
        })
}

fn agent_input_text(input: &AgentInput) -> String {
    match input {
        AgentInput::Text { text } => text.clone(),
        AgentInput::UserMessage { payload } => payload.content.clone(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn interrupt_cancels_blocked_approval_for_same_session() {
        let session_id = SessionId::new();
        let other_session_id = SessionId::new();
        let approval_id = ApprovalId::new();
        let other_approval_id = ApprovalId::new();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let (other_sender, _other_receiver) = tokio::sync::oneshot::channel();
        let approvals = Arc::new(Mutex::new(HashMap::from([
            (
                approval_id,
                PendingProviderApproval::new(session_id, sender),
            ),
            (
                other_approval_id,
                PendingProviderApproval::new(other_session_id, other_sender),
            ),
        ])));

        let cancel = tokio::spawn(cancel_pending_provider_approvals_for_session(
            session_id,
            approvals.clone(),
            "test",
        ));
        let provider_decision = receiver.await.expect("provider decision");
        assert_eq!(provider_decision.decision, ApprovalDecision::Cancel);
        provider_decision
            .acknowledged
            .send(Ok(()))
            .expect("ack cancel");

        assert_eq!(cancel.await.expect("cancel task"), 1);
    }

    #[tokio::test]
    async fn interrupt_does_not_count_completed_approval_cancel_replay_as_new_cancel() {
        let session_id = SessionId::new();
        let approval_id = ApprovalId::new();
        let (sender, receiver) = tokio::sync::oneshot::channel();
        let pending = PendingProviderApproval::new(session_id, sender);
        let first_cancel = tokio::spawn({
            let pending = pending.clone();
            async move { pending.submit(ApprovalDecision::Cancel).await }
        });
        let provider_decision = receiver.await.expect("provider decision");
        assert_eq!(provider_decision.decision, ApprovalDecision::Cancel);
        provider_decision
            .acknowledged
            .send(Ok(()))
            .expect("ack cancel");
        assert_eq!(first_cancel.await.expect("first cancel"), Ok(()));

        let approvals = Arc::new(Mutex::new(HashMap::from([(approval_id, pending)])));
        let cancelled =
            cancel_pending_provider_approvals_for_session(session_id, approvals, "test").await;
        assert_eq!(cancelled, 0);
    }
}
