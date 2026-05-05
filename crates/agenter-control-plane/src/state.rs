use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use agenter_core::{
    AgentProviderId, AgentQuestionAnswer, AgentTurnSettings, ApprovalDecision, ApprovalId,
    ApprovalOption, ApprovalRequest, ApprovalResolutionState,
    ApprovalStatus as UniversalApprovalStatus, CapabilitySet, CommandAction, CommandOutputStream,
    ContentBlock, ContentBlockKind, ItemRole, ItemState, ItemStatus, NativeNotification, NativeRef,
    NormalizedEvent, ProviderNotification, ProviderNotificationSeverity, QuestionId, QuestionState,
    QuestionStatus, RunnerId, SessionId, SessionInfo, SessionSnapshot, SessionStatus,
    SessionUsageContext, SessionUsageSnapshot, SessionUsageWindow, ToolActionProjection,
    ToolCommandProjection, ToolEvent, ToolMcpProjection, ToolProjection, ToolProjectionKind,
    ToolSubagentOperation, ToolSubagentProjection, ToolSubagentStateProjection, TurnStatus,
    UniversalCommandEnvelope, UniversalEventEnvelope, UniversalEventKind, UniversalEventSource,
    UniversalSeq, UserId, WorkspaceId, WorkspaceRef, UNIVERSAL_PROTOCOL_VERSION,
};
use agenter_protocol::{
    browser::BrowserSessionSnapshot,
    runner::{
        AgentUniversalEvent, DiscoveredFileChangeStatus, DiscoveredSessionHistoryItem,
        DiscoveredSessionHistoryStatus, DiscoveredSessions, DiscoveredToolStatus,
        RunnerCapabilities, RunnerOperationKind, RunnerOperationLogLevel, RunnerOperationProgress,
        RunnerOperationStatus, RunnerOperationUpdate, RunnerResponseOutcome, RunnerServerMessage,
    },
    RequestId,
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use tokio::{
    sync::{broadcast, mpsc, oneshot, Mutex},
    time::timeout,
};
use uuid::Uuid;

use crate::auth::CookieSecurity;
use crate::auth::{self, AuthenticatedUser, BootstrapAdmin};
use crate::policy::PolicyEngine;

const SESSION_EVENT_CACHE_LIMIT: usize = 128;
const UNIVERSAL_EVENT_REPLAY_LIMIT: usize = 1024;
const REFRESH_JOB_LOG_LIMIT: usize = 200;
pub const BROWSER_AUTH_SESSION_TTL: ChronoDuration = ChronoDuration::days(30);

#[derive(Clone, Debug)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

#[derive(Debug)]
struct AppStateInner {
    runner_token: String,
    cookie_security: CookieSecurity,
    bootstrap_admin: Option<BootstrapAdmin>,
    db_pool: Option<sqlx::PgPool>,
    auth_sessions: Mutex<HashMap<String, AuthenticatedUser>>,
    registry: Mutex<Registry>,
    sessions: Mutex<HashMap<SessionId, SessionEvents>>,
    runner_connections: Mutex<HashMap<RunnerId, RunnerConnection>>,
    pending_runner_responses:
        Mutex<HashMap<(RunnerId, RequestId), oneshot::Sender<RunnerResponseOutcome>>>,
    runner_command_operations: Mutex<HashMap<RequestId, RunnerCommandOperation>>,
    refresh_summaries: Mutex<HashMap<RequestId, WorkspaceSessionRefreshSummary>>,
    refresh_jobs: Mutex<HashMap<RequestId, WorkspaceSessionRefreshJob>>,
    refresh_force_reload_requests: Mutex<HashSet<RequestId>>,
    universal_command_idempotency: Mutex<HashMap<String, UniversalCommandIdempotencyEntry>>,
    runner_event_acks: Mutex<HashMap<RunnerId, u64>>,
    seen_runner_events: Mutex<HashSet<(RunnerId, u64)>>,
}

#[derive(Debug, Default)]
struct Registry {
    runners: HashMap<RunnerId, RegisteredRunner>,
    sessions: HashMap<SessionId, RegisteredSession>,
    approvals: HashMap<ApprovalId, RegisteredApproval>,
    questions: HashMap<QuestionId, RegisteredQuestion>,
}

#[derive(Clone, Debug)]
pub struct RegisteredApproval {
    pub session_id: SessionId,
    status: ApprovalStatus,
}

#[derive(Clone, Debug)]
pub struct RegisteredQuestion {
    pub session_id: SessionId,
    pub status: QuestionStatus,
    pub request: Option<Box<CachedEventEnvelope>>,
    pub state: Option<Box<QuestionState>>,
}

/// Tracks client-visible approval lifecycle. Pending/Resolving carry the original
/// `approval_requested` envelope so reconnects still render cards after cache eviction.
#[derive(Clone, Debug)]
enum ApprovalStatus {
    Pending(Box<CachedEventEnvelope>),
    Presented(Box<CachedEventEnvelope>),
    Resolving {
        request: Box<CachedEventEnvelope>,
        decision: ApprovalDecision,
    },
    Resolved(Box<CachedEventEnvelope>),
    Orphaned(Box<CachedEventEnvelope>),
}

#[derive(Debug)]
struct RunnerCommandOperation {
    runner_id: RunnerId,
    request_id: RequestId,
    status: RunnerCommandOperationStatus,
    waiters: Vec<oneshot::Sender<Result<RunnerResponseOutcome, RunnerCommandWaitError>>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum RunnerCommandOperationStatus {
    Queued,
    Delivered,
    Waiting,
    Succeeded,
    Failed,
    TimedOut,
}

fn envelope_references_approval_id(
    envelope: &CachedEventEnvelope,
    approval_id: ApprovalId,
) -> bool {
    match &envelope.event {
        NormalizedEvent::ApprovalRequested(req) => req.approval_id == approval_id,
        NormalizedEvent::ApprovalResolved(res) => res.approval_id == approval_id,
        _ => false,
    }
}

fn approval_request_envelope_with_state(
    envelope: &CachedEventEnvelope,
    state: ApprovalResolutionState,
    decision: Option<ApprovalDecision>,
) -> CachedEventEnvelope {
    let mut envelope = envelope.clone();
    if let NormalizedEvent::ApprovalRequested(request) = &mut envelope.event {
        request.resolution_state = Some(state);
        request.resolving_decision = decision;
    }
    envelope
}

fn session_status_orphans_approvals(status: &SessionStatus) -> bool {
    matches!(
        status,
        SessionStatus::Stopped | SessionStatus::Failed | SessionStatus::Archived
    )
}

fn enrich_approval_event(event: &mut NormalizedEvent) {
    let NormalizedEvent::ApprovalRequested(request) = event else {
        return;
    };
    if request.status.is_none() {
        request.status = Some(UniversalApprovalStatus::Pending);
    }
    if request.options.is_empty() {
        request.options = ApprovalOption::canonical_defaults();
    }
    let existing_policy_options = request
        .options
        .iter()
        .any(|option| option.policy_rule.is_some());
    if !existing_policy_options {
        request
            .options
            .extend(PolicyEngine.persistent_rule_options(request));
    }
    if request.subject.is_none() {
        request.subject = request
            .details
            .clone()
            .or_else(|| Some(request.title.clone()));
    }
    if request.native_request_id.is_none() {
        request.native_request_id = safe_native_request_id(request.provider_payload.as_ref());
    }
    request.native_blocking = true;
    if request.policy.is_none() || request.risk.is_none() {
        let decision = PolicyEngine.evaluate_approval_request(request);
        if request.risk.is_none() {
            request.risk = Some(decision.risk.as_str().to_owned());
        }
        if request.policy.is_none() {
            request.policy = Some(decision.into());
        }
    }
}

fn merge_pending_approval_envelopes_for_session(
    session_id: SessionId,
    mut envelopes: Vec<CachedEventEnvelope>,
    registry: &Registry,
) -> Vec<CachedEventEnvelope> {
    for (&approval_id, approval) in &registry.approvals {
        if approval.session_id != session_id {
            continue;
        }
        let replacement = match &approval.status {
            ApprovalStatus::Pending(request) => approval_request_envelope_with_state(
                request,
                ApprovalResolutionState::Pending,
                None,
            ),
            ApprovalStatus::Presented(request) => request.as_ref().clone(),
            ApprovalStatus::Resolving { request, decision } => {
                approval_request_envelope_with_state(
                    request,
                    ApprovalResolutionState::Resolving,
                    Some(decision.clone()),
                )
            }
            ApprovalStatus::Resolved(_) | ApprovalStatus::Orphaned(_) => continue,
        };
        match envelopes
            .iter()
            .position(|e| envelope_references_approval_id(e, approval_id))
        {
            Some(index) => {
                if matches!(
                    envelopes[index].event,
                    NormalizedEvent::ApprovalRequested(_)
                ) {
                    envelopes[index] = replacement;
                }
            }
            None => envelopes.push(replacement),
        }
    }
    envelopes
}

#[derive(Clone, Debug)]
pub enum ApprovalResolutionStart {
    Missing,
    InProgress { envelope: Box<CachedEventEnvelope> },
    AlreadyResolved { envelope: Box<CachedEventEnvelope> },
    Started,
}

#[derive(Clone, Debug)]
pub enum ApprovalResolutionLookup {
    Missing,
    Pending {
        session_id: SessionId,
    },
    InProgress {
        session_id: SessionId,
        envelope: Box<CachedEventEnvelope>,
    },
    AlreadyResolved {
        session_id: SessionId,
        envelope: Box<CachedEventEnvelope>,
    },
}

#[derive(Clone, Debug)]
pub struct RegisteredRunner {
    pub runner_id: RunnerId,
    pub capabilities: RunnerCapabilities,
    pub workspaces: Vec<WorkspaceRef>,
}

#[derive(Clone, Debug)]
pub struct RunnerListEntry {
    pub runner: RegisteredRunner,
    pub connected: bool,
}

#[derive(Clone, Debug)]
pub struct RegisteredSession {
    pub session_id: SessionId,
    pub owner_user_id: UserId,
    pub runner_id: RunnerId,
    pub workspace: WorkspaceRef,
    pub provider_id: AgentProviderId,
    pub status: SessionStatus,
    pub title: Option<String>,
    pub external_session_id: Option<String>,
    pub turn_settings: Option<AgentTurnSettings>,
    pub usage: Option<SessionUsageSnapshot>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Clone, Debug)]
pub struct SessionRegistration {
    pub session_id: SessionId,
    pub owner_user_id: UserId,
    pub runner_id: RunnerId,
    pub workspace: WorkspaceRef,
    pub provider_id: AgentProviderId,
    pub title: Option<String>,
    pub external_session_id: Option<String>,
    pub turn_settings: Option<AgentTurnSettings>,
    pub usage: Option<SessionUsageSnapshot>,
}

impl RegisteredSession {
    #[must_use]
    pub fn info(&self) -> SessionInfo {
        session_info(self)
    }
}

#[derive(Debug)]
struct SessionEvents {
    sender: broadcast::Sender<SessionBroadcastEvent>,
    cache: Vec<CachedEventEnvelope>,
    universal_cache: Vec<UniversalEventEnvelope>,
    snapshot: SessionSnapshot,
    next_seq: i64,
}

#[derive(Clone, Debug, serde::Deserialize, serde::Serialize)]
pub struct CachedEventEnvelope {
    pub event_id: Option<agenter_protocol::EventId>,
    pub event: NormalizedEvent,
}

#[derive(Clone, Debug)]
pub struct SessionBroadcastEvent {
    #[cfg(test)]
    pub normalized_event: CachedEventEnvelope,
    pub universal_event: Option<UniversalEventEnvelope>,
}

#[derive(Debug)]
struct UniversalCommandIdempotencyEntry {
    command_json: Value,
    status: UniversalCommandIdempotencyStatus,
    response: Option<UniversalCommandResponse>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum UniversalCommandIdempotencyStatus {
    Pending,
    Succeeded,
    Failed,
}

#[derive(Clone, Debug, serde::Deserialize, PartialEq, serde::Serialize)]
pub struct UniversalCommandResponse {
    pub status: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub body: Option<Value>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UniversalCommandConflict {
    pub code: String,
    pub message: String,
}

#[derive(Clone, Debug, PartialEq)]
pub enum UniversalCommandStart {
    Started,
    Duplicate {
        status: UniversalCommandIdempotencyStatus,
        response: Option<UniversalCommandResponse>,
    },
    Conflict(UniversalCommandConflict),
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct UniversalCommandPersistenceError {
    pub code: String,
    pub message: String,
}

#[derive(Debug)]
pub struct SessionSubscription {
    #[cfg(test)]
    pub cached_events: Vec<CachedEventEnvelope>,
    pub snapshot: Option<BrowserSessionSnapshot>,
    pub receiver: broadcast::Receiver<SessionBroadcastEvent>,
}

#[derive(Clone, Debug)]
struct RunnerConnection {
    connection_id: Uuid,
    sender: mpsc::UnboundedSender<OutboundRunnerMessage>,
}

#[derive(Debug)]
pub struct OutboundRunnerMessage {
    pub message: RunnerServerMessage,
    pub(crate) delivered: oneshot::Sender<Result<(), RunnerSendError>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunnerSendError {
    NotConnected,
    Closed,
    StaleApproval,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunnerCommandWaitError {
    NotConnected,
    Closed,
    StaleApproval,
    TimedOut,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SessionImportMode {
    Automatic,
    Forced,
    ForceReload,
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct WorkspaceSessionRefreshSummary {
    pub discovered_count: usize,
    pub refreshed_cache_count: usize,
    pub skipped_failed_count: usize,
}

#[derive(Clone, Debug, Deserialize, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSessionRefreshStatus {
    Queued,
    Sent,
    Accepted,
    Discovering,
    ReadingHistory,
    SendingResults,
    Importing,
    Succeeded,
    Failed,
    Cancelled,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkspaceSessionRefreshLogEntry {
    pub ts: DateTime<Utc>,
    pub level: WorkspaceSessionRefreshLogLevel,
    pub status: WorkspaceSessionRefreshStatus,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<RunnerOperationProgress>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum WorkspaceSessionRefreshLogLevel {
    Debug,
    Info,
    Warning,
    Error,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize)]
pub struct WorkspaceSessionRefreshJob {
    pub refresh_id: RequestId,
    pub status: WorkspaceSessionRefreshStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub progress: Option<RunnerOperationProgress>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub log: Vec<WorkspaceSessionRefreshLogEntry>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<WorkspaceSessionRefreshSummary>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub updated_at: DateTime<Utc>,
}

impl AppState {
    #[must_use]
    pub fn new(runner_token: String, cookie_security: CookieSecurity) -> Self {
        Self {
            inner: Arc::new(AppStateInner {
                runner_token,
                cookie_security,
                bootstrap_admin: None,
                db_pool: None,
                auth_sessions: Mutex::new(HashMap::new()),
                registry: Mutex::new(Registry::default()),
                sessions: Mutex::new(HashMap::new()),
                runner_connections: Mutex::new(HashMap::new()),
                pending_runner_responses: Mutex::new(HashMap::new()),
                runner_command_operations: Mutex::new(HashMap::new()),
                refresh_summaries: Mutex::new(HashMap::new()),
                refresh_jobs: Mutex::new(HashMap::new()),
                refresh_force_reload_requests: Mutex::new(HashSet::new()),
                universal_command_idempotency: Mutex::new(HashMap::new()),
                runner_event_acks: Mutex::new(HashMap::new()),
                seen_runner_events: Mutex::new(HashSet::new()),
            }),
        }
    }

    pub fn new_with_bootstrap_admin(
        runner_token: String,
        email: String,
        password: String,
        cookie_security: CookieSecurity,
    ) -> anyhow::Result<Self> {
        let password_hash = auth::hash_password(&password)?;
        let user = AuthenticatedUser {
            user_id: agenter_core::UserId::new(),
            email,
            display_name: Some("Local Admin".to_owned()),
        };
        Ok(Self {
            inner: Arc::new(AppStateInner {
                runner_token,
                cookie_security,
                bootstrap_admin: Some(BootstrapAdmin {
                    user,
                    password_hash,
                }),
                db_pool: None,
                auth_sessions: Mutex::new(HashMap::new()),
                registry: Mutex::new(Registry::default()),
                sessions: Mutex::new(HashMap::new()),
                runner_connections: Mutex::new(HashMap::new()),
                pending_runner_responses: Mutex::new(HashMap::new()),
                runner_command_operations: Mutex::new(HashMap::new()),
                refresh_summaries: Mutex::new(HashMap::new()),
                refresh_jobs: Mutex::new(HashMap::new()),
                refresh_force_reload_requests: Mutex::new(HashSet::new()),
                universal_command_idempotency: Mutex::new(HashMap::new()),
                runner_event_acks: Mutex::new(HashMap::new()),
                seen_runner_events: Mutex::new(HashSet::new()),
            }),
        })
    }

    pub async fn new_with_database(
        runner_token: String,
        cookie_security: CookieSecurity,
        pool: sqlx::PgPool,
        bootstrap_admin: Option<(String, String)>,
    ) -> anyhow::Result<Self> {
        let bootstrap_admin = if let Some((email, password)) = bootstrap_admin {
            let password_hash = auth::hash_password(&password)?;
            let user = agenter_db::bootstrap_password_admin(
                &pool,
                &email,
                Some("Local Admin"),
                &password_hash,
            )
            .await?;
            Some(BootstrapAdmin {
                user: AuthenticatedUser {
                    user_id: user.user_id,
                    email: user.email,
                    display_name: user.display_name,
                },
                password_hash,
            })
        } else {
            None
        };

        Ok(Self {
            inner: Arc::new(AppStateInner {
                runner_token,
                cookie_security,
                bootstrap_admin,
                db_pool: Some(pool),
                auth_sessions: Mutex::new(HashMap::new()),
                registry: Mutex::new(Registry::default()),
                sessions: Mutex::new(HashMap::new()),
                runner_connections: Mutex::new(HashMap::new()),
                pending_runner_responses: Mutex::new(HashMap::new()),
                runner_command_operations: Mutex::new(HashMap::new()),
                refresh_summaries: Mutex::new(HashMap::new()),
                refresh_jobs: Mutex::new(HashMap::new()),
                refresh_force_reload_requests: Mutex::new(HashSet::new()),
                universal_command_idempotency: Mutex::new(HashMap::new()),
                runner_event_acks: Mutex::new(HashMap::new()),
                seen_runner_events: Mutex::new(HashSet::new()),
            }),
        })
    }

    #[cfg(test)]
    fn new_with_test_db_pool(pool: sqlx::PgPool) -> Self {
        Self {
            inner: Arc::new(AppStateInner {
                runner_token: "dev-token".to_owned(),
                cookie_security: CookieSecurity::DevelopmentInsecure,
                bootstrap_admin: None,
                db_pool: Some(pool),
                auth_sessions: Mutex::new(HashMap::new()),
                registry: Mutex::new(Registry::default()),
                sessions: Mutex::new(HashMap::new()),
                runner_connections: Mutex::new(HashMap::new()),
                pending_runner_responses: Mutex::new(HashMap::new()),
                runner_command_operations: Mutex::new(HashMap::new()),
                refresh_summaries: Mutex::new(HashMap::new()),
                refresh_jobs: Mutex::new(HashMap::new()),
                refresh_force_reload_requests: Mutex::new(HashSet::new()),
                universal_command_idempotency: Mutex::new(HashMap::new()),
                runner_event_acks: Mutex::new(HashMap::new()),
                seen_runner_events: Mutex::new(HashSet::new()),
            }),
        }
    }

    #[must_use]
    pub fn is_runner_token_valid(&self, token: &str) -> bool {
        self.inner.runner_token == token
    }

    #[must_use]
    pub fn cookie_security(&self) -> CookieSecurity {
        self.inner.cookie_security
    }

    pub fn db_pool(&self) -> Option<&sqlx::PgPool> {
        self.inner.db_pool.as_ref()
    }

    pub async fn login_password(&self, email: &str, password: &str) -> Option<String> {
        if let Some(pool) = &self.inner.db_pool {
            let (user, password_hash) = agenter_db::find_password_credential_by_email(pool, email)
                .await
                .ok()??;
            if !auth::verify_password(password, &password_hash) {
                return None;
            }
            let user = AuthenticatedUser {
                user_id: user.user_id,
                email: user.email,
                display_name: user.display_name,
            };
            let token = self.create_authenticated_session(user).await;
            tracing::debug!(email, "created database-backed authenticated session");
            return Some(token);
        }

        let admin = self.inner.bootstrap_admin.as_ref()?;
        if admin.user.email != email || !auth::verify_password(password, &admin.password_hash) {
            return None;
        }

        let token = Uuid::new_v4().to_string();
        self.inner
            .auth_sessions
            .lock()
            .await
            .insert(token.clone(), admin.user.clone());
        tracing::debug!(email, "created bootstrap authenticated session");
        Some(token)
    }

    pub async fn authenticated_user(&self, token: &str) -> Option<AuthenticatedUser> {
        if let Some(user) = self.inner.auth_sessions.lock().await.get(token).cloned() {
            return Some(user);
        }

        let pool = self.inner.db_pool.as_ref()?;
        let token_hash = auth::session_token_hash(token);
        let user = agenter_db::find_browser_auth_session_user(pool, &token_hash, Utc::now())
            .await
            .ok()??;
        Some(AuthenticatedUser {
            user_id: user.user_id,
            email: user.email,
            display_name: user.display_name,
        })
    }

    pub async fn create_authenticated_session(&self, user: AuthenticatedUser) -> String {
        let token = Uuid::new_v4().to_string();
        if let Some(pool) = &self.inner.db_pool {
            let token_hash = auth::session_token_hash(&token);
            if let Err(error) = agenter_db::create_browser_auth_session(
                pool,
                &token_hash,
                user.user_id,
                Utc::now() + BROWSER_AUTH_SESSION_TTL,
            )
            .await
            {
                tracing::error!(
                    user_id = %user.user_id,
                    %error,
                    "failed to persist browser authenticated session"
                );
            }
        }
        self.inner
            .auth_sessions
            .lock()
            .await
            .insert(token.clone(), user);
        tracing::debug!("created authenticated session");
        token
    }

    #[cfg(test)]
    pub fn bootstrap_user_id(&self) -> Option<UserId> {
        self.inner
            .bootstrap_admin
            .as_ref()
            .map(|admin| admin.user.user_id)
    }

    pub async fn logout(&self, token: &str) {
        let removed = self
            .inner
            .auth_sessions
            .lock()
            .await
            .remove(token)
            .is_some();
        if let Some(pool) = &self.inner.db_pool {
            let token_hash = auth::session_token_hash(token);
            if let Err(error) = agenter_db::revoke_browser_auth_session(pool, &token_hash).await {
                tracing::warn!(%error, "failed to revoke persisted browser auth session");
            }
        }
        tracing::debug!(removed, "removed authenticated session");
    }

    pub async fn register_runner(
        &self,
        runner_id: RunnerId,
        capabilities: RunnerCapabilities,
        workspaces: Vec<WorkspaceRef>,
    ) -> RegisteredRunner {
        if let Some(pool) = &self.inner.db_pool {
            if let Err(error) = agenter_db::upsert_runner_with_id(
                pool,
                runner_id,
                &format!("runner-{runner_id}"),
                None,
            )
            .await
            {
                tracing::warn!(%runner_id, %error, "failed to persist runner registry row");
            }
            for workspace in &workspaces {
                if let Err(error) = agenter_db::upsert_workspace_with_id(
                    pool,
                    workspace.workspace_id,
                    runner_id,
                    &workspace.path,
                    workspace.display_name.as_deref(),
                )
                .await
                {
                    tracing::warn!(
                        %runner_id,
                        workspace_id = %workspace.workspace_id,
                        %error,
                        "failed to persist runner workspace row"
                    );
                }
            }
        }
        let runner = RegisteredRunner {
            runner_id,
            capabilities,
            workspaces,
        };
        self.inner
            .registry
            .lock()
            .await
            .runners
            .insert(runner_id, runner.clone());
        tracing::info!(
            %runner_id,
            workspace_count = runner.workspaces.len(),
            provider_count = runner.capabilities.agent_providers.len(),
            "runner registered"
        );
        runner
    }

    pub async fn connect_runner(
        &self,
        runner_id: RunnerId,
        sender: mpsc::UnboundedSender<OutboundRunnerMessage>,
    ) -> Uuid {
        let connection_id = Uuid::new_v4();
        self.inner.runner_connections.lock().await.insert(
            runner_id,
            RunnerConnection {
                connection_id,
                sender,
            },
        );
        tracing::info!(%runner_id, %connection_id, "runner connected");
        connection_id
    }

    #[cfg(test)]
    pub async fn seed_runner_event_ack(&self, runner_id: RunnerId, acked_seq: Option<u64>) {
        let Some(acked_seq) = acked_seq else {
            return;
        };
        self.seed_process_runner_event_ack(runner_id, acked_seq)
            .await;
    }

    async fn seed_process_runner_event_ack(&self, runner_id: RunnerId, acked_seq: u64) {
        self.inner
            .runner_event_acks
            .lock()
            .await
            .entry(runner_id)
            .and_modify(|existing| *existing = (*existing).max(acked_seq))
            .or_insert(acked_seq);
        let mut seen = self.inner.seen_runner_events.lock().await;
        for seq in 1..=acked_seq {
            seen.insert((runner_id, seq));
        }
    }

    pub async fn seed_runner_event_ack_from_hello(
        &self,
        runner_id: RunnerId,
        runner_supplied_acked_seq: Option<u64>,
    ) -> Option<u64> {
        let acked_seq = if let Some(pool) = &self.inner.db_pool {
            match agenter_db::durable_runner_event_ack_cursor(pool, runner_id).await {
                Ok(acked_seq) => acked_seq,
                Err(error) => {
                    tracing::warn!(
                        %runner_id,
                        %error,
                        "failed to derive runner event ack cursor from durable receipts"
                    );
                    None
                }
            }
        } else {
            runner_supplied_acked_seq
        };
        if let Some(acked_seq) = acked_seq {
            self.seed_process_runner_event_ack(runner_id, acked_seq)
                .await;
        }
        acked_seq
    }

    pub async fn runner_event_already_accepted(
        &self,
        runner_id: RunnerId,
        runner_event_seq: Option<u64>,
    ) -> bool {
        let Some(seq) = runner_event_seq else {
            return false;
        };
        if self
            .inner
            .runner_event_acks
            .lock()
            .await
            .get(&runner_id)
            .is_some_and(|acked| seq <= *acked)
        {
            return true;
        }
        if self
            .inner
            .seen_runner_events
            .lock()
            .await
            .contains(&(runner_id, seq))
        {
            return true;
        }
        if let Some(pool) = &self.inner.db_pool {
            match agenter_db::runner_event_receipt_exists(pool, runner_id, seq).await {
                Ok(true) => {
                    self.inner
                        .seen_runner_events
                        .lock()
                        .await
                        .insert((runner_id, seq));
                    return true;
                }
                Ok(false) => {}
                Err(error) => {
                    tracing::warn!(
                        %runner_id,
                        runner_event_seq = seq,
                        %error,
                        "failed to check durable runner event receipt"
                    );
                }
            }
        }
        false
    }

    pub async fn mark_runner_event_accepted(&self, runner_id: RunnerId, runner_event_seq: u64) {
        self.inner
            .seen_runner_events
            .lock()
            .await
            .insert((runner_id, runner_event_seq));
        self.ack_runner_event(runner_id, runner_event_seq).await;
    }

    pub async fn ack_runner_event(&self, runner_id: RunnerId, runner_event_seq: u64) {
        self.inner
            .runner_event_acks
            .lock()
            .await
            .entry(runner_id)
            .and_modify(|existing| *existing = (*existing).max(runner_event_seq))
            .or_insert(runner_event_seq);
    }

    pub async fn disconnect_runner(&self, runner_id: RunnerId, connection_id: Uuid) {
        let disconnected = {
            let mut connections = self.inner.runner_connections.lock().await;
            if connections
                .get(&runner_id)
                .is_some_and(|connection| connection.connection_id == connection_id)
            {
                connections.remove(&runner_id);
                true
            } else {
                false
            }
        };
        if disconnected {
            tracing::info!(%runner_id, %connection_id, "runner disconnected");
            tracing::debug!(
                %runner_id,
                "leaving runner-owned sessions unchanged after transient websocket disconnect"
            );
        } else {
            tracing::debug!(%runner_id, %connection_id, "ignored stale runner disconnect");
        }
    }

    pub async fn send_runner_message(
        &self,
        runner_id: RunnerId,
        message: RunnerServerMessage,
    ) -> Result<(), RunnerSendError> {
        let sender = {
            let Some(connection) = self
                .inner
                .runner_connections
                .lock()
                .await
                .get(&runner_id)
                .cloned()
            else {
                tracing::warn!(%runner_id, "runner send failed: not connected");
                return Err(RunnerSendError::NotConnected);
            };
            connection.sender
        };
        let (delivered, delivered_receiver) = oneshot::channel();
        sender
            .send(OutboundRunnerMessage { message, delivered })
            .map_err(|_| {
                tracing::warn!(%runner_id, "runner send failed: outbound queue closed");
                RunnerSendError::Closed
            })?;
        let result = delivered_receiver
            .await
            .unwrap_or(Err(RunnerSendError::Closed));
        if let Err(error) = result {
            tracing::warn!(%runner_id, ?error, "runner send delivery failed");
        }
        result
    }

    pub async fn send_runner_command_and_wait(
        &self,
        runner_id: RunnerId,
        request_id: RequestId,
        message: RunnerServerMessage,
        wait_for: Duration,
    ) -> Result<RunnerResponseOutcome, RunnerCommandWaitError> {
        let receiver = self
            .start_runner_command_operation(runner_id, request_id, message, wait_for)
            .await;
        receiver
            .await
            .unwrap_or(Err(RunnerCommandWaitError::Closed))
    }

    pub async fn start_runner_command_operation(
        &self,
        runner_id: RunnerId,
        request_id: RequestId,
        message: RunnerServerMessage,
        wait_for: Duration,
    ) -> oneshot::Receiver<Result<RunnerResponseOutcome, RunnerCommandWaitError>> {
        let (waiter, receiver) = oneshot::channel();
        self.inner.runner_command_operations.lock().await.insert(
            request_id.clone(),
            RunnerCommandOperation {
                runner_id,
                request_id: request_id.clone(),
                status: RunnerCommandOperationStatus::Queued,
                waiters: vec![waiter],
            },
        );

        let state = self.clone();
        tokio::spawn(async move {
            let result = state
                .run_runner_command_operation(runner_id, request_id.clone(), message, wait_for)
                .await;
            state
                .complete_runner_command_operation(request_id, result)
                .await;
        });

        receiver
    }

    pub async fn start_workspace_session_refresh(
        &self,
        runner_id: RunnerId,
        request_id: RequestId,
        message: RunnerServerMessage,
        force_reload: bool,
        wait_for: Duration,
    ) -> Result<(), RunnerCommandWaitError> {
        if !self
            .inner
            .runner_connections
            .lock()
            .await
            .contains_key(&runner_id)
        {
            return Err(RunnerCommandWaitError::NotConnected);
        }
        if force_reload {
            self.inner
                .refresh_force_reload_requests
                .lock()
                .await
                .insert(request_id.clone());
        }
        self.upsert_refresh_job(
            request_id.clone(),
            WorkspaceSessionRefreshStatus::Queued,
            None,
            None,
        )
        .await;
        let receiver = self
            .start_runner_command_operation(runner_id, request_id.clone(), message, wait_for)
            .await;
        let state = self.clone();
        tokio::spawn(async move {
            match receiver.await {
                Ok(Ok(RunnerResponseOutcome::Ok { .. })) => {
                    state
                        .upsert_refresh_job(
                            request_id,
                            WorkspaceSessionRefreshStatus::Discovering,
                            None,
                            None,
                        )
                        .await;
                }
                Ok(Ok(RunnerResponseOutcome::Error { error })) => {
                    state
                        .upsert_refresh_job(
                            request_id,
                            WorkspaceSessionRefreshStatus::Failed,
                            None,
                            Some(format!("{}: {}", error.code, error.message)),
                        )
                        .await;
                }
                Ok(Err(error)) => {
                    state
                        .upsert_refresh_job(
                            request_id,
                            WorkspaceSessionRefreshStatus::Failed,
                            None,
                            Some(format!("{error:?}")),
                        )
                        .await;
                }
                Err(_) => {
                    state
                        .upsert_refresh_job(
                            request_id,
                            WorkspaceSessionRefreshStatus::Failed,
                            None,
                            Some("runner command waiter closed".to_owned()),
                        )
                        .await;
                }
            }
        });
        Ok(())
    }

    async fn run_runner_command_operation(
        &self,
        runner_id: RunnerId,
        request_id: RequestId,
        message: RunnerServerMessage,
        wait_for: Duration,
    ) -> Result<RunnerResponseOutcome, RunnerCommandWaitError> {
        let (response_sender, response_receiver) = oneshot::channel();
        self.inner
            .pending_runner_responses
            .lock()
            .await
            .insert((runner_id, request_id.clone()), response_sender);

        self.update_runner_command_operation_status(
            &request_id,
            RunnerCommandOperationStatus::Delivered,
        )
        .await;
        if let Err(error) = self.send_runner_message(runner_id, message).await {
            self.inner
                .pending_runner_responses
                .lock()
                .await
                .remove(&(runner_id, request_id));
            return Err(match error {
                RunnerSendError::NotConnected => RunnerCommandWaitError::NotConnected,
                RunnerSendError::Closed => RunnerCommandWaitError::Closed,
                RunnerSendError::StaleApproval => RunnerCommandWaitError::StaleApproval,
            });
        }

        self.update_runner_command_operation_status(
            &request_id,
            RunnerCommandOperationStatus::Waiting,
        )
        .await;
        match timeout(wait_for, response_receiver).await {
            Ok(Ok(outcome)) => Ok(outcome),
            Ok(Err(_)) => Err(RunnerCommandWaitError::Closed),
            Err(_) => {
                self.inner
                    .pending_runner_responses
                    .lock()
                    .await
                    .remove(&(runner_id, request_id));
                Err(RunnerCommandWaitError::TimedOut)
            }
        }
    }

    async fn update_runner_command_operation_status(
        &self,
        request_id: &RequestId,
        status: RunnerCommandOperationStatus,
    ) {
        if let Some(operation) = self
            .inner
            .runner_command_operations
            .lock()
            .await
            .get_mut(request_id)
        {
            operation.status = status;
            tracing::debug!(
                runner_id = %operation.runner_id,
                request_id = %operation.request_id,
                ?status,
                "runner command operation status changed"
            );
        }
        if matches!(status, RunnerCommandOperationStatus::Delivered) {
            self.update_refresh_job_status(request_id, WorkspaceSessionRefreshStatus::Sent)
                .await;
        }
    }

    async fn complete_runner_command_operation(
        &self,
        request_id: RequestId,
        result: Result<RunnerResponseOutcome, RunnerCommandWaitError>,
    ) {
        let mut operation = self
            .inner
            .runner_command_operations
            .lock()
            .await
            .remove(&request_id);
        if let Some(operation) = &mut operation {
            operation.status = match &result {
                Ok(_) => RunnerCommandOperationStatus::Succeeded,
                Err(RunnerCommandWaitError::TimedOut) => RunnerCommandOperationStatus::TimedOut,
                Err(_) => RunnerCommandOperationStatus::Failed,
            };
            tracing::debug!(
                runner_id = %operation.runner_id,
                request_id = %operation.request_id,
                status = ?operation.status,
                "runner command operation completed"
            );
            for waiter in operation.waiters.drain(..) {
                waiter.send(result.clone()).ok();
            }
        }
    }

    pub async fn finish_runner_response(
        &self,
        runner_id: RunnerId,
        request_id: RequestId,
        outcome: RunnerResponseOutcome,
    ) {
        let sender = self
            .inner
            .pending_runner_responses
            .lock()
            .await
            .remove(&(runner_id, request_id));
        if let Some(sender) = sender {
            sender.send(outcome).ok();
        }
    }

    pub async fn list_runners_with_connection_status(&self) -> Vec<RunnerListEntry> {
        let runners: Vec<_> = self
            .inner
            .registry
            .lock()
            .await
            .runners
            .values()
            .cloned()
            .collect();
        let connections = self.inner.runner_connections.lock().await;
        runners
            .into_iter()
            .map(|runner| RunnerListEntry {
                connected: connections.contains_key(&runner.runner_id),
                runner,
            })
            .collect()
    }

    pub async fn list_runner_workspaces(&self, runner_id: RunnerId) -> Option<Vec<WorkspaceRef>> {
        self.inner
            .registry
            .lock()
            .await
            .runners
            .get(&runner_id)
            .map(|runner| runner.workspaces.clone())
    }

    #[cfg(test)]
    pub async fn create_session(
        &self,
        session_id: SessionId,
        owner_user_id: UserId,
        runner_id: RunnerId,
        workspace: WorkspaceRef,
        provider_id: AgentProviderId,
    ) -> RegisteredSession {
        self.create_session_with_title(
            session_id,
            owner_user_id,
            runner_id,
            workspace,
            provider_id,
            None,
        )
        .await
    }

    #[cfg(test)]
    pub async fn create_session_with_title(
        &self,
        session_id: SessionId,
        owner_user_id: UserId,
        runner_id: RunnerId,
        workspace: WorkspaceRef,
        provider_id: AgentProviderId,
        title: Option<String>,
    ) -> RegisteredSession {
        self.register_session(SessionRegistration {
            session_id,
            owner_user_id,
            runner_id,
            workspace,
            provider_id,
            title,
            external_session_id: None,
            turn_settings: None,
            usage: None,
        })
        .await
    }

    pub async fn register_session(&self, registration: SessionRegistration) -> RegisteredSession {
        let now = Utc::now();
        let mut session = RegisteredSession {
            session_id: registration.session_id,
            owner_user_id: registration.owner_user_id,
            runner_id: registration.runner_id,
            workspace: registration.workspace,
            provider_id: registration.provider_id,
            status: SessionStatus::Idle,
            title: registration.title,
            external_session_id: registration.external_session_id,
            turn_settings: registration.turn_settings,
            usage: registration.usage,
            created_at: now,
            updated_at: now,
        };
        if let Some(pool) = &self.inner.db_pool {
            match agenter_db::create_session_with_id(
                pool,
                agenter_db::CreateSessionRecord {
                    session_id: session.session_id,
                    owner_user_id: session.owner_user_id,
                    runner_id: session.runner_id,
                    workspace_id: session.workspace.workspace_id,
                    provider_id: session.provider_id.clone(),
                    external_session_id: session.external_session_id.clone(),
                    title: session.title.clone(),
                    status: session.status.clone(),
                    usage_snapshot: session.usage.clone(),
                    turn_settings: session.turn_settings.clone(),
                },
            )
            .await
            {
                Ok(persisted) => {
                    session.status = persisted.status;
                    session.title = persisted.title;
                    session.external_session_id = persisted.external_session_id;
                    session.turn_settings = persisted.turn_settings;
                    session.usage = persisted.usage_snapshot;
                    session.created_at = persisted.created_at;
                    session.updated_at = persisted.updated_at;
                }
                Err(error) => {
                    tracing::warn!(
                        session_id = %session.session_id,
                        %error,
                        "failed to persist session registry row"
                    );
                }
            }
        }
        self.inner
            .registry
            .lock()
            .await
            .sessions
            .insert(session.session_id, session.clone());
        tracing::info!(
            session_id = %session.session_id,
            owner_user_id = %session.owner_user_id,
            runner_id = %session.runner_id,
            workspace_id = %session.workspace.workspace_id,
            provider_id = %session.provider_id,
            "session registered"
        );
        session
    }

    pub async fn resolve_runner_workspace(
        &self,
        workspace_id: WorkspaceId,
        provider_id: &AgentProviderId,
    ) -> Option<(RunnerId, WorkspaceRef)> {
        let registry = self.inner.registry.lock().await;
        registry.runners.values().find_map(|runner| {
            let supports_provider = runner
                .capabilities
                .agent_providers
                .iter()
                .any(|provider| &provider.provider_id == provider_id);
            if !supports_provider {
                return None;
            }
            runner
                .workspaces
                .iter()
                .find(|workspace| workspace.workspace_id == workspace_id)
                .cloned()
                .map(|workspace| (runner.runner_id, workspace))
        })
    }

    pub async fn can_access_session(&self, user_id: UserId, session_id: SessionId) -> bool {
        if let Some(pool) = &self.inner.db_pool {
            return agenter_db::find_session_for_user(pool, user_id, session_id)
                .await
                .ok()
                .flatten()
                .is_some();
        }
        self.inner
            .registry
            .lock()
            .await
            .sessions
            .get(&session_id)
            .is_some_and(|session| session.owner_user_id == user_id)
    }

    pub async fn list_sessions(&self, user_id: UserId) -> Vec<SessionInfo> {
        if let Some(pool) = &self.inner.db_pool {
            return agenter_db::list_sessions_for_user(pool, user_id)
                .await
                .map(|sessions| {
                    sessions
                        .iter()
                        .map(|session| db_session_info(&session.session))
                        .collect()
                })
                .unwrap_or_else(|error| {
                    tracing::warn!(%user_id, %error, "failed to list persisted sessions");
                    Vec::new()
                });
        }
        let mut sessions: Vec<_> = self
            .inner
            .registry
            .lock()
            .await
            .sessions
            .values()
            .filter(|session| session.owner_user_id == user_id)
            .map(session_info)
            .collect();
        sort_session_infos(&mut sessions);
        sessions
    }

    pub async fn update_session_title(
        &self,
        user_id: UserId,
        session_id: SessionId,
        title: Option<String>,
    ) -> Option<SessionInfo> {
        if let Some(pool) = &self.inner.db_pool {
            return agenter_db::update_session_title(pool, user_id, session_id, title.as_deref())
                .await
                .ok()
                .flatten()
                .map(|session| db_session_info(&session));
        }

        let mut registry = self.inner.registry.lock().await;
        let session = registry.sessions.get_mut(&session_id)?;
        if session.owner_user_id != user_id {
            return None;
        }
        session.title = title;
        session.updated_at = Utc::now();
        Some(session_info(session))
    }

    pub async fn update_session_status(
        &self,
        user_id: UserId,
        session_id: SessionId,
        status: SessionStatus,
    ) -> Option<SessionInfo> {
        if let Some(pool) = &self.inner.db_pool {
            return agenter_db::update_session_status(pool, user_id, session_id, status)
                .await
                .ok()
                .flatten()
                .map(|session| db_session_info(&session));
        }

        let mut registry = self.inner.registry.lock().await;
        let session = registry.sessions.get_mut(&session_id)?;
        if session.owner_user_id != user_id {
            return None;
        }
        session.status = status;
        session.updated_at = Utc::now();
        Some(session_info(session))
    }

    pub async fn session(
        &self,
        user_id: UserId,
        session_id: SessionId,
    ) -> Option<RegisteredSession> {
        if let Some(pool) = &self.inner.db_pool {
            return agenter_db::find_session_for_user(pool, user_id, session_id)
                .await
                .ok()
                .flatten()
                .map(|session| RegisteredSession {
                    session_id: session.session.session_id,
                    owner_user_id: session.session.owner_user_id,
                    runner_id: session.session.runner_id,
                    workspace: WorkspaceRef {
                        workspace_id: session.workspace.workspace_id,
                        runner_id: session.workspace.runner_id,
                        path: session.workspace.path,
                        display_name: session.workspace.display_name,
                    },
                    provider_id: session.session.provider_id,
                    status: session.session.status,
                    title: session.session.title,
                    external_session_id: session.session.external_session_id,
                    turn_settings: session.session.turn_settings,
                    usage: session.session.usage_snapshot,
                    created_at: session.session.created_at,
                    updated_at: session.session.updated_at,
                });
        }
        self.inner
            .registry
            .lock()
            .await
            .sessions
            .get(&session_id)
            .filter(|session| session.owner_user_id == user_id)
            .cloned()
    }

    async fn session_by_id(&self, session_id: SessionId) -> Option<RegisteredSession> {
        if let Some(pool) = &self.inner.db_pool {
            let session = agenter_db::find_session_by_id(pool, session_id)
                .await
                .ok()??;
            return Some(RegisteredSession {
                session_id: session.session.session_id,
                owner_user_id: session.session.owner_user_id,
                runner_id: session.session.runner_id,
                workspace: WorkspaceRef {
                    workspace_id: session.workspace.workspace_id,
                    runner_id: session.workspace.runner_id,
                    path: session.workspace.path,
                    display_name: session.workspace.display_name,
                },
                provider_id: session.session.provider_id,
                status: session.session.status,
                title: session.session.title,
                external_session_id: session.session.external_session_id,
                turn_settings: session.session.turn_settings,
                usage: session.session.usage_snapshot,
                created_at: session.session.created_at,
                updated_at: session.session.updated_at,
            });
        }
        self.inner
            .registry
            .lock()
            .await
            .sessions
            .get(&session_id)
            .cloned()
    }

    pub async fn subscribe_session(
        &self,
        session_id: SessionId,
        after_seq: Option<UniversalSeq>,
        include_snapshot: bool,
    ) -> SessionSubscription {
        let (cached, receiver) = {
            let mut sessions = self.inner.sessions.lock().await;
            let events = sessions
                .entry(session_id)
                .or_insert_with(SessionEvents::new);
            (events.cache.clone(), events.sender.subscribe())
        };

        let snapshot = self
            .session_snapshot_replay(session_id, after_seq, include_snapshot)
            .await;
        #[cfg(not(test))]
        let _ = cached;
        SessionSubscription {
            #[cfg(test)]
            cached_events: cached,
            snapshot,
            receiver,
        }
    }

    async fn session_snapshot_replay(
        &self,
        session_id: SessionId,
        after_seq: Option<UniversalSeq>,
        include_snapshot: bool,
    ) -> Option<BrowserSessionSnapshot> {
        if !include_snapshot && after_seq.is_none() {
            return None;
        }

        let Some(pool) = &self.inner.db_pool else {
            let sessions = self.inner.sessions.lock().await;
            let (mut snapshot, events, latest_seq, has_more) =
                if let Some(events) = sessions.get(&session_id) {
                    let replay = events
                        .universal_cache
                        .iter()
                        .filter(|event| {
                            after_seq
                                .map(|after_seq| event.seq > after_seq)
                                .unwrap_or(true)
                        })
                        .cloned()
                        .collect::<Vec<_>>();
                    let starts_after_requested_cursor =
                        replay.first().is_some_and(|first| match after_seq {
                            Some(after_seq) => first.seq.as_i64() > after_seq.as_i64() + 1,
                            None => first.seq.as_i64() > 1,
                        });
                    let has_more = starts_after_requested_cursor
                        || replay.len() > UNIVERSAL_EVENT_REPLAY_LIMIT;
                    let mut replay = replay;
                    if has_more {
                        replay.truncate(UNIVERSAL_EVENT_REPLAY_LIMIT);
                    }
                    let latest_seq = if has_more {
                        replay.last().map(|event| event.seq)
                    } else {
                        events
                            .snapshot
                            .latest_seq
                            .or_else(|| events.universal_cache.last().map(|event| event.seq))
                    };
                    let mut snapshot = events.snapshot.clone();
                    snapshot.session_id = session_id;
                    (snapshot, replay, latest_seq, has_more)
                } else {
                    (
                        SessionSnapshot {
                            session_id,
                            ..SessionSnapshot::default()
                        },
                        Vec::new(),
                        None,
                        false,
                    )
                };
            self.hydrate_snapshot_capabilities(&mut snapshot).await;
            return Some(BrowserSessionSnapshot {
                protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
                request_id: None,
                snapshot,
                events,
                latest_seq,
                has_more,
            });
        };

        let mut snapshot = agenter_db::load_session_snapshot(pool, session_id)
            .await
            .map_err(|error| {
                tracing::warn!(%session_id, %error, "failed to load universal session snapshot");
            })
            .ok()
            .flatten()
            .unwrap_or_else(|| SessionSnapshot {
                session_id,
                ..SessionSnapshot::default()
            });
        self.hydrate_snapshot_capabilities(&mut snapshot).await;
        let replay = agenter_db::list_universal_events_after(
            pool,
            session_id,
            after_seq,
            UNIVERSAL_EVENT_REPLAY_LIMIT + 1,
        )
        .await
        .map(|events| {
            events
                .into_iter()
                .map(|event| event.envelope())
                .collect::<Vec<_>>()
        });
        let replay_failed = replay.is_err();
        let mut events = replay.unwrap_or_else(|error| {
            tracing::warn!(%session_id, %error, "failed to replay universal session events");
            Vec::new()
        });
        let has_more = replay_failed || events.len() > UNIVERSAL_EVENT_REPLAY_LIMIT;
        if has_more {
            events.truncate(UNIVERSAL_EVENT_REPLAY_LIMIT);
        }
        let latest_seq = if replay_failed {
            after_seq
        } else if has_more {
            events.last().map(|event| event.seq)
        } else {
            snapshot
                .latest_seq
                .or_else(|| events.last().map(|event| event.seq))
        };
        Some(BrowserSessionSnapshot {
            protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
            request_id: None,
            snapshot,
            events,
            latest_seq,
            has_more,
        })
    }

    async fn hydrate_snapshot_capabilities(&self, snapshot: &mut SessionSnapshot) {
        if let Some(provider_capabilities) = self
            .provider_capabilities_for_session(snapshot.session_id, snapshot.info.as_ref())
            .await
        {
            merge_provider_capabilities(&mut snapshot.capabilities, provider_capabilities);
        }
    }

    async fn provider_capabilities_for_session(
        &self,
        session_id: SessionId,
        info: Option<&SessionInfo>,
    ) -> Option<CapabilitySet> {
        let registry = self.inner.registry.lock().await;
        let runner_and_provider = registry
            .sessions
            .get(&session_id)
            .map(|session| (session.runner_id, session.provider_id.clone()))
            .or_else(|| info.map(|info| (info.runner_id, info.provider_id.clone())))?;
        let runner = registry.runners.get(&runner_and_provider.0)?;
        runner
            .capabilities
            .agent_providers
            .iter()
            .find(|provider| provider.provider_id == runner_and_provider.1)
            .map(|provider| CapabilitySet::from(provider.capabilities.clone()))
    }

    pub async fn begin_universal_command(
        &self,
        envelope: &UniversalCommandEnvelope,
    ) -> Result<UniversalCommandStart, UniversalCommandPersistenceError> {
        let command_json = universal_command_fingerprint(envelope);
        if let Some(pool) = &self.inner.db_pool {
            match agenter_db::begin_command_idempotency(
                pool,
                &envelope.idempotency_key,
                envelope.command_id,
                envelope.session_id,
                command_json.clone(),
            )
            .await
            {
                Ok((record, inserted)) => {
                    return Ok(command_start_from_record(&record, inserted, &command_json));
                }
                Err(error) => {
                    tracing::warn!(
                        command_id = %envelope.command_id,
                        idempotency_key = %envelope.idempotency_key,
                        %error,
                        "failed to persist universal command idempotency"
                    );
                    return Err(UniversalCommandPersistenceError {
                        code: "idempotency_begin_failed".to_owned(),
                        message: "Could not durably record command idempotency before dispatch."
                            .to_owned(),
                    });
                }
            }
        }

        let mut idempotency = self.inner.universal_command_idempotency.lock().await;
        match idempotency.get(&envelope.idempotency_key) {
            Some(existing) if existing.command_json != command_json => {
                Ok(UniversalCommandStart::Conflict(UniversalCommandConflict {
                    code: "idempotency_conflict".to_owned(),
                    message: "idempotency key was already used for a different command".to_owned(),
                }))
            }
            Some(existing) => Ok(UniversalCommandStart::Duplicate {
                status: existing.status,
                response: existing.response.clone(),
            }),
            None => {
                idempotency.insert(
                    envelope.idempotency_key.clone(),
                    UniversalCommandIdempotencyEntry {
                        command_json,
                        status: UniversalCommandIdempotencyStatus::Pending,
                        response: None,
                    },
                );
                Ok(UniversalCommandStart::Started)
            }
        }
    }

    pub async fn finish_universal_command(
        &self,
        envelope: &UniversalCommandEnvelope,
        status: UniversalCommandIdempotencyStatus,
        response: UniversalCommandResponse,
    ) -> Result<(), UniversalCommandPersistenceError> {
        let command_json = universal_command_fingerprint(envelope);
        if let Some(pool) = &self.inner.db_pool {
            let db_status = match status {
                UniversalCommandIdempotencyStatus::Pending => {
                    agenter_db::models::CommandIdempotencyStatus::Pending
                }
                UniversalCommandIdempotencyStatus::Succeeded => {
                    agenter_db::models::CommandIdempotencyStatus::Succeeded
                }
                UniversalCommandIdempotencyStatus::Failed => {
                    agenter_db::models::CommandIdempotencyStatus::Failed
                }
            };
            if let Err(error) = agenter_db::finish_command_idempotency(
                pool,
                &envelope.idempotency_key,
                db_status,
                command_json.clone(),
                serde_json::to_value(&response).unwrap_or_default(),
            )
            .await
            {
                tracing::warn!(
                    command_id = %envelope.command_id,
                    idempotency_key = %envelope.idempotency_key,
                    %error,
                    "failed to update universal command idempotency"
                );
                return Err(UniversalCommandPersistenceError {
                    code: "idempotency_finish_failed".to_owned(),
                    message: "Command completed but its idempotency response could not be durably recorded."
                        .to_owned(),
                });
            }
        }

        let mut idempotency = self.inner.universal_command_idempotency.lock().await;
        let entry = idempotency
            .entry(envelope.idempotency_key.clone())
            .or_insert_with(|| UniversalCommandIdempotencyEntry {
                command_json: command_json.clone(),
                status,
                response: None,
            });
        if entry.command_json == command_json {
            entry.status = status;
            entry.response = Some(response);
        }
        Ok(())
    }

    pub async fn clear_universal_command(
        &self,
        envelope: &UniversalCommandEnvelope,
    ) -> Result<(), UniversalCommandPersistenceError> {
        if let Some(pool) = &self.inner.db_pool {
            if let Err(error) =
                agenter_db::delete_command_idempotency(pool, &envelope.idempotency_key).await
            {
                tracing::warn!(
                    command_id = %envelope.command_id,
                    idempotency_key = %envelope.idempotency_key,
                    %error,
                    "failed to clear universal command idempotency"
                );
                return Err(UniversalCommandPersistenceError {
                    code: "idempotency_clear_failed".to_owned(),
                    message:
                        "Command failed transiently but its idempotency key could not be cleared."
                            .to_owned(),
                });
            }
        }

        let mut idempotency = self.inner.universal_command_idempotency.lock().await;
        idempotency.remove(&envelope.idempotency_key);
        Ok(())
    }

    pub async fn session_history(
        &self,
        user_id: UserId,
        session_id: SessionId,
    ) -> Option<Vec<CachedEventEnvelope>> {
        if !self.can_access_session(user_id, session_id).await {
            return None;
        }

        let mut history = if self.inner.db_pool.is_some() {
            Vec::new()
        } else {
            self.inner
                .sessions
                .lock()
                .await
                .get(&session_id)
                .map(|events| events.cache.clone())
                .unwrap_or_default()
        };

        let registry = self.inner.registry.lock().await;
        history = merge_pending_approval_envelopes_for_session(session_id, history, &registry);
        Some(history)
    }

    /// Pending or in-flight resolving approvals for a session (for API listing / tools).
    pub async fn pending_approval_request_envelopes(
        &self,
        session_id: SessionId,
    ) -> Vec<CachedEventEnvelope> {
        let mut to_persist = Vec::new();
        let mut registry = self.inner.registry.lock().await;
        let mut out = Vec::new();
        for approval in registry.approvals.values_mut() {
            if approval.session_id != session_id {
                continue;
            }
            match &approval.status {
                ApprovalStatus::Pending(env) => {
                    let mut presented = approval_request_envelope_with_state(
                        env,
                        ApprovalResolutionState::Pending,
                        None,
                    );
                    if let NormalizedEvent::ApprovalRequested(request) = &mut presented.event {
                        request.status = Some(UniversalApprovalStatus::Presented);
                    }
                    approval.status = ApprovalStatus::Presented(Box::new(presented.clone()));
                    to_persist.push(presented.clone());
                    out.push(presented);
                }
                ApprovalStatus::Presented(env) => {
                    out.push(*env.clone());
                }
                ApprovalStatus::Resolving { request, decision } => {
                    out.push(approval_request_envelope_with_state(
                        request,
                        ApprovalResolutionState::Resolving,
                        Some(decision.clone()),
                    ));
                }
                ApprovalStatus::Resolved(_) | ApprovalStatus::Orphaned(_) => {}
            }
        }
        drop(registry);
        for envelope in to_persist {
            self.store_event(session_id, envelope).await;
        }
        out
    }

    pub async fn session_turn_settings(
        &self,
        user_id: UserId,
        session_id: SessionId,
    ) -> Option<AgentTurnSettings> {
        if let Some(pool) = &self.inner.db_pool {
            return agenter_db::session_turn_settings(pool, user_id, session_id)
                .await
                .ok()
                .flatten();
        }
        self.inner
            .registry
            .lock()
            .await
            .sessions
            .get(&session_id)
            .filter(|session| session.owner_user_id == user_id)
            .and_then(|session| session.turn_settings.clone())
    }

    pub async fn update_session_turn_settings(
        &self,
        user_id: UserId,
        session_id: SessionId,
        settings: AgentTurnSettings,
    ) -> Option<AgentTurnSettings> {
        if let Some(pool) = &self.inner.db_pool {
            let updated = agenter_db::update_session_turn_settings(
                pool,
                user_id,
                session_id,
                Some(&settings),
            )
            .await
            .ok()
            .flatten()?;
            return updated.turn_settings;
        }
        let mut registry = self.inner.registry.lock().await;
        let session = registry
            .sessions
            .get_mut(&session_id)
            .filter(|session| session.owner_user_id == user_id)?;
        session.turn_settings = Some(settings.clone());
        Some(settings)
    }

    pub async fn publish_event(
        &self,
        session_id: SessionId,
        mut event: NormalizedEvent,
    ) -> CachedEventEnvelope {
        enrich_approval_event(&mut event);
        let envelope = CachedEventEnvelope {
            event_id: Some(Uuid::new_v4().to_string().into()),
            event,
        };
        let mut orphan_session = None;
        let mut auto_approval_request = None;
        match &envelope.event {
            NormalizedEvent::ApprovalRequested(request) => {
                tracing::info!(
                    approval_id = %request.approval_id,
                    %session_id,
                    kind = ?request.kind,
                    "approval requested"
                );
                self.inner.registry.lock().await.approvals.insert(
                    request.approval_id,
                    RegisteredApproval {
                        session_id,
                        status: ApprovalStatus::Pending(Box::new(envelope.clone())),
                    },
                );
                auto_approval_request = Some(request.clone());
            }
            NormalizedEvent::ApprovalResolved(resolved) => {
                tracing::info!(
                    approval_id = %resolved.approval_id,
                    %session_id,
                    "approval resolved event received"
                );
                let mut registry = self.inner.registry.lock().await;
                match registry.approvals.get_mut(&resolved.approval_id) {
                    Some(approval) => match &approval.status {
                        ApprovalStatus::Resolved(existing) => return *existing.clone(),
                        ApprovalStatus::Pending(_)
                        | ApprovalStatus::Presented(_)
                        | ApprovalStatus::Resolving { .. } => {
                            approval.status = ApprovalStatus::Resolved(Box::new(envelope.clone()));
                        }
                        ApprovalStatus::Orphaned(existing) => return *existing.clone(),
                    },
                    None => {
                        registry.approvals.insert(
                            resolved.approval_id,
                            RegisteredApproval {
                                session_id,
                                status: ApprovalStatus::Resolved(Box::new(envelope.clone())),
                            },
                        );
                    }
                }
            }
            NormalizedEvent::QuestionRequested(request) => {
                tracing::info!(
                    question_id = %request.question_id,
                    %session_id,
                    "question requested"
                );
                self.inner.registry.lock().await.questions.insert(
                    request.question_id,
                    RegisteredQuestion {
                        session_id,
                        status: QuestionStatus::Pending,
                        request: Some(Box::new(envelope.clone())),
                        state: None,
                    },
                );
            }
            NormalizedEvent::QuestionAnswered(answered) => {
                let mut registry = self.inner.registry.lock().await;
                if let Some(question) = registry.questions.get_mut(&answered.question_id) {
                    question.status = QuestionStatus::Answered;
                }
            }
            NormalizedEvent::SessionStatusChanged(status) => {
                self.apply_session_status(status.session_id, status.status.clone())
                    .await;
                if session_status_orphans_approvals(&status.status) {
                    orphan_session = Some(status.session_id);
                }
            }
            NormalizedEvent::NativeNotification(native_notification)
            | NormalizedEvent::TurnDiffUpdated(native_notification)
            | NormalizedEvent::ItemReasoning(native_notification)
            | NormalizedEvent::ServerRequestResolved(native_notification)
            | NormalizedEvent::McpToolCallProgress(native_notification)
            | NormalizedEvent::ThreadRealtimeEvent(native_notification) => {
                if let Some(usage) = self
                    .apply_provider_usage_event(session_id, native_notification)
                    .await
                {
                    if let Some(pool) = &self.inner.db_pool {
                        if let Err(error) =
                            agenter_db::update_session_usage_snapshot(pool, session_id, &usage)
                                .await
                        {
                            tracing::warn!(
                                %session_id,
                                %error,
                                "failed to persist session usage snapshot"
                            );
                        }
                    }
                }
            }
            _ => {}
        }

        let stored = self.store_event(session_id, envelope).await;
        if let Some(request) = auto_approval_request {
            self.maybe_auto_resolve_approval(session_id, request).await;
        }
        if let Some(session_id) = orphan_session {
            self.orphan_pending_approvals_for_session(
                session_id,
                "runner reported native session ownership ended",
            )
            .await;
            self.orphan_pending_questions_for_session(
                session_id,
                "runner reported native session ownership ended",
            )
            .await;
        }
        stored
    }

    pub async fn accept_runner_agent_event(
        &self,
        universal_event: AgentUniversalEvent,
    ) -> anyhow::Result<UniversalEventEnvelope> {
        let session_id = universal_event.session_id;
        let stored = self
            .store_universal_event_with_acceptance(session_id, universal_event, true)
            .await?;
        self.apply_accepted_universal_event(&stored).await;
        Ok(stored)
    }

    pub async fn accept_runner_agent_event_with_receipt(
        &self,
        runner_id: RunnerId,
        runner_event_seq: Option<u64>,
        universal_event: AgentUniversalEvent,
    ) -> anyhow::Result<Option<UniversalEventEnvelope>> {
        let Some(runner_event_seq) = runner_event_seq else {
            return self
                .accept_runner_agent_event(universal_event)
                .await
                .map(Some);
        };
        let session_id = universal_event.session_id;
        let stored = self
            .store_runner_universal_event_with_receipt(
                runner_id,
                runner_event_seq,
                session_id,
                universal_event,
                true,
            )
            .await?;
        if let Some(stored) = &stored {
            self.apply_accepted_universal_event(stored).await;
        }
        Ok(stored)
    }

    pub async fn publish_universal_event(
        &self,
        session_id: SessionId,
        turn_id: Option<agenter_core::TurnId>,
        item_id: Option<agenter_core::ItemId>,
        event: UniversalEventKind,
    ) -> anyhow::Result<UniversalEventEnvelope> {
        let stored = self
            .store_universal_event_with_acceptance(
                session_id,
                AgentUniversalEvent {
                    protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
                    session_id,
                    event_id: None,
                    turn_id,
                    item_id,
                    ts: None,
                    source: UniversalEventSource::ControlPlane,
                    native: None,
                    event,
                },
                false,
            )
            .await?;
        self.apply_accepted_universal_event(&stored).await;
        Ok(stored)
    }

    async fn apply_accepted_universal_event(&self, envelope: &UniversalEventEnvelope) {
        match &envelope.event {
            UniversalEventKind::ApprovalRequested { approval } => {
                self.inner.registry.lock().await.approvals.insert(
                    approval.approval_id,
                    RegisteredApproval {
                        session_id: envelope.session_id,
                        status: ApprovalStatus::Pending(Box::new(CachedEventEnvelope {
                            event_id: Some(envelope.event_id.clone().into()),
                            event: NormalizedEvent::ApprovalRequested(
                                source_approval_request_event_from_universal(approval),
                            ),
                        })),
                    },
                );
            }
            UniversalEventKind::QuestionRequested { question } => {
                self.inner.registry.lock().await.questions.insert(
                    question.question_id,
                    RegisteredQuestion {
                        session_id: envelope.session_id,
                        status: question.status.clone(),
                        request: None,
                        state: Some(question.clone()),
                    },
                );
            }
            UniversalEventKind::QuestionAnswered { question } => {
                let mut registry = self.inner.registry.lock().await;
                if let Some(registered) = registry.questions.get_mut(&question.question_id) {
                    registered.status = question.status.clone();
                    registered.state = Some(question.clone());
                }
            }
            UniversalEventKind::SessionCreated { session } => {
                self.apply_session_status(session.session_id, session.status.clone())
                    .await;
            }
            UniversalEventKind::SessionStatusChanged { status, .. } => {
                self.apply_session_status(envelope.session_id, status.clone())
                    .await;
                if session_status_orphans_approvals(status) {
                    self.orphan_pending_approvals_for_session(
                        envelope.session_id,
                        "runner reported native session ownership ended",
                    )
                    .await;
                    self.orphan_pending_questions_for_session(
                        envelope.session_id,
                        "runner reported native session ownership ended",
                    )
                    .await;
                }
            }
            UniversalEventKind::SessionMetadataChanged { title } => {
                self.apply_session_title(envelope.session_id, title.clone())
                    .await;
            }
            UniversalEventKind::UsageUpdated { usage } => {
                if let Some(pool) = &self.inner.db_pool {
                    if let Err(error) =
                        agenter_db::update_session_usage_snapshot(pool, envelope.session_id, usage)
                            .await
                    {
                        tracing::warn!(
                            session_id = %envelope.session_id,
                            %error,
                            "failed to persist session usage snapshot"
                        );
                    }
                }
            }
            UniversalEventKind::ProviderNotification { .. } => {}
            _ => {}
        }
    }

    async fn apply_session_status(&self, session_id: SessionId, status: SessionStatus) {
        {
            let mut registry = self.inner.registry.lock().await;
            if let Some(session) = registry.sessions.get_mut(&session_id) {
                session.status = status.clone();
                session.updated_at = Utc::now();
            }
        }
        if let Some(pool) = &self.inner.db_pool {
            if let Err(error) =
                agenter_db::update_session_status_by_id(pool, session_id, status).await
            {
                tracing::warn!(
                    %session_id,
                    %error,
                    "failed to persist session status update"
                );
            }
        }
    }

    async fn apply_session_title(&self, session_id: SessionId, title: Option<String>) {
        {
            let mut registry = self.inner.registry.lock().await;
            if let Some(session) = registry.sessions.get_mut(&session_id) {
                session.title = title.clone();
                session.updated_at = Utc::now();
            }
        }
        if let Some(pool) = &self.inner.db_pool {
            if let Err(error) =
                agenter_db::update_session_title_by_id(pool, session_id, title.as_deref()).await
            {
                tracing::warn!(
                    %session_id,
                    %error,
                    "failed to persist session title update"
                );
            }
        }
    }

    async fn apply_provider_usage_event(
        &self,
        session_id: SessionId,
        event: &NativeNotification,
    ) -> Option<SessionUsageSnapshot> {
        let usage_update = usage_snapshot_from_native_notification(event)?;
        let mut registry = self.inner.registry.lock().await;
        let session = registry.sessions.get_mut(&session_id)?;
        let mut usage = session.usage.clone().unwrap_or_default();
        merge_usage_snapshot(&mut usage, usage_update);
        apply_turn_settings_to_usage(&mut usage, session.turn_settings.as_ref());
        session.usage = Some(usage.clone());
        session.updated_at = Utc::now();
        Some(usage)
    }

    pub async fn begin_approval_resolution(
        &self,
        approval_id: ApprovalId,
        decision: ApprovalDecision,
    ) -> ApprovalResolutionStart {
        let mut transition = None;
        let result = {
            let mut registry = self.inner.registry.lock().await;
            let Some(approval) = registry.approvals.get_mut(&approval_id) else {
                return ApprovalResolutionStart::Missing;
            };
            match &approval.status {
                ApprovalStatus::Pending(request_env) | ApprovalStatus::Presented(request_env) => {
                    let mut resolving = approval_request_envelope_with_state(
                        request_env,
                        ApprovalResolutionState::Resolving,
                        Some(decision.clone()),
                    );
                    if let NormalizedEvent::ApprovalRequested(request) = &mut resolving.event {
                        request.status = Some(UniversalApprovalStatus::Resolving);
                    }
                    transition = Some((approval.session_id, resolving));
                    approval.status = ApprovalStatus::Resolving {
                        request: request_env.clone(),
                        decision,
                    };
                    tracing::debug!(%approval_id, session_id = %approval.session_id, "approval resolution started");
                    ApprovalResolutionStart::Started
                }
                ApprovalStatus::Resolving { request, decision } => {
                    ApprovalResolutionStart::InProgress {
                        envelope: Box::new(approval_request_envelope_with_state(
                            request,
                            ApprovalResolutionState::Resolving,
                            Some(decision.clone()),
                        )),
                    }
                }
                ApprovalStatus::Resolved(envelope) | ApprovalStatus::Orphaned(envelope) => {
                    ApprovalResolutionStart::AlreadyResolved {
                        envelope: envelope.clone(),
                    }
                }
            }
        };
        if let Some((session_id, envelope)) = transition {
            self.store_event(session_id, envelope).await;
        }
        result
    }

    pub async fn lookup_approval_resolution(
        &self,
        approval_id: ApprovalId,
    ) -> ApprovalResolutionLookup {
        let registry = self.inner.registry.lock().await;
        let Some(approval) = registry.approvals.get(&approval_id) else {
            return ApprovalResolutionLookup::Missing;
        };
        match &approval.status {
            ApprovalStatus::Pending(_) | ApprovalStatus::Presented(_) => {
                ApprovalResolutionLookup::Pending {
                    session_id: approval.session_id,
                }
            }
            ApprovalStatus::Resolving { request, decision } => {
                ApprovalResolutionLookup::InProgress {
                    session_id: approval.session_id,
                    envelope: Box::new(approval_request_envelope_with_state(
                        request,
                        ApprovalResolutionState::Resolving,
                        Some(decision.clone()),
                    )),
                }
            }
            ApprovalStatus::Resolved(envelope) | ApprovalStatus::Orphaned(envelope) => {
                ApprovalResolutionLookup::AlreadyResolved {
                    session_id: approval.session_id,
                    envelope: envelope.clone(),
                }
            }
        }
    }

    pub async fn approval_request_event(
        &self,
        approval_id: ApprovalId,
    ) -> Option<agenter_core::ApprovalRequestEvent> {
        let registry = self.inner.registry.lock().await;
        let approval = registry.approvals.get(&approval_id)?;
        let envelope = match &approval.status {
            ApprovalStatus::Pending(envelope) | ApprovalStatus::Presented(envelope) => envelope,
            ApprovalStatus::Resolving { request, .. } => request,
            ApprovalStatus::Resolved(_) | ApprovalStatus::Orphaned(_) => return None,
        };
        match &envelope.event {
            NormalizedEvent::ApprovalRequested(request) => Some(request.clone()),
            _ => None,
        }
    }

    pub async fn cancel_approval_resolution(&self, approval_id: ApprovalId) {
        let mut registry = self.inner.registry.lock().await;
        let Some(approval) = registry.approvals.get_mut(&approval_id) else {
            return;
        };
        if let ApprovalStatus::Resolving { request, .. } = &approval.status {
            approval.status = ApprovalStatus::Pending(request.clone());
        }
    }

    pub async fn approval_is_resolving(&self, approval_id: ApprovalId) -> bool {
        self.inner
            .registry
            .lock()
            .await
            .approvals
            .get(&approval_id)
            .is_some_and(|approval| matches!(approval.status, ApprovalStatus::Resolving { .. }))
    }

    pub async fn finish_approval_resolution(
        &self,
        approval_id: ApprovalId,
        session_id: SessionId,
        event: NormalizedEvent,
    ) -> Option<CachedEventEnvelope> {
        let envelope = CachedEventEnvelope {
            event_id: Some(Uuid::new_v4().to_string().into()),
            event,
        };
        {
            let mut registry = self.inner.registry.lock().await;
            let approval = registry.approvals.get_mut(&approval_id)?;
            if approval.session_id != session_id {
                return None;
            }
            match &approval.status {
                ApprovalStatus::Resolved(existing) | ApprovalStatus::Orphaned(existing) => {
                    return Some(*existing.clone());
                }
                ApprovalStatus::Pending(_) | ApprovalStatus::Presented(_) => return None,
                ApprovalStatus::Resolving { .. } => {
                    approval.status = ApprovalStatus::Resolved(Box::new(envelope.clone()));
                }
            }
        }

        self.store_event(session_id, envelope.clone()).await;
        Some(envelope)
    }

    async fn maybe_auto_resolve_approval(
        &self,
        session_id: SessionId,
        request: agenter_core::ApprovalRequestEvent,
    ) {
        let Some(pool) = &self.inner.db_pool else {
            return;
        };
        let Some(session) = self.session_by_id(session_id).await else {
            return;
        };
        let Ok(rules) = agenter_db::list_active_approval_policy_rules(
            pool,
            session.owner_user_id,
            session.workspace.workspace_id,
            &session.provider_id,
        )
        .await
        else {
            tracing::warn!(%session_id, approval_id = %request.approval_id, "failed to load approval policy rules");
            return;
        };
        let Some(rule) = PolicyEngine.matching_rule(&request, &rules).cloned() else {
            return;
        };
        tracing::info!(
            %session_id,
            approval_id = %request.approval_id,
            rule_id = %rule.rule_id,
            "auto-resolving approval from policy rule"
        );
        let state = self.clone();
        tokio::spawn(async move {
            state
                .resolve_approval_from_policy(session, request.approval_id, rule.decision)
                .await;
        });
    }

    async fn resolve_approval_from_policy(
        &self,
        session: RegisteredSession,
        approval_id: ApprovalId,
        decision: ApprovalDecision,
    ) {
        if !matches!(
            self.begin_approval_resolution(approval_id, decision.clone())
                .await,
            ApprovalResolutionStart::Started
        ) {
            return;
        }
        let request_id = RequestId::from(Uuid::new_v4().to_string());
        let command = RunnerServerMessage::Command(Box::new(
            agenter_protocol::runner::RunnerCommandEnvelope {
                request_id: request_id.clone(),
                command: agenter_protocol::runner::RunnerCommand::AnswerApproval(
                    agenter_protocol::runner::ApprovalAnswerCommand {
                        session_id: session.session_id,
                        approval_id,
                        decision: decision.clone(),
                    },
                ),
            },
        ));
        let operation = self
            .start_runner_command_operation(
                session.runner_id,
                request_id,
                command,
                Duration::from_secs(30),
            )
            .await;
        match operation
            .await
            .unwrap_or(Err(RunnerCommandWaitError::Closed))
        {
            Ok(RunnerResponseOutcome::Ok {
                result: agenter_protocol::runner::RunnerCommandResult::Accepted,
            }) => {
                let resolved =
                    NormalizedEvent::ApprovalResolved(agenter_core::ApprovalResolvedEvent {
                        session_id: session.session_id,
                        approval_id,
                        decision,
                        resolved_by_user_id: None,
                        resolved_at: Utc::now(),
                        provider_payload: None,
                    });
                self.finish_approval_resolution(approval_id, session.session_id, resolved)
                    .await;
            }
            other => {
                tracing::warn!(
                    %approval_id,
                    session_id = %session.session_id,
                    outcome = ?other,
                    "policy approval auto-resolution failed"
                );
                self.cancel_approval_resolution(approval_id).await;
            }
        }
    }

    pub async fn question_session(&self, question_id: QuestionId) -> Option<SessionId> {
        self.inner
            .registry
            .lock()
            .await
            .questions
            .get(&question_id)
            .filter(|question| question.status == QuestionStatus::Pending)
            .map(|question| question.session_id)
    }

    pub async fn finish_question_answer(
        &self,
        session_id: SessionId,
        answer: AgentQuestionAnswer,
    ) -> CachedEventEnvelope {
        let event = NormalizedEvent::QuestionAnswered(agenter_core::QuestionAnsweredEvent {
            session_id,
            question_id: answer.question_id,
            answer,
            provider_payload: None,
        });
        self.publish_event(session_id, event).await
    }

    async fn orphan_pending_approvals_for_session(&self, session_id: SessionId, reason: &str) {
        let orphaned = {
            let mut registry = self.inner.registry.lock().await;
            let mut orphaned = Vec::new();
            for (&approval_id, approval) in &mut registry.approvals {
                if approval.session_id != session_id {
                    continue;
                }
                let request = match &approval.status {
                    ApprovalStatus::Pending(request) | ApprovalStatus::Presented(request) => {
                        request.clone()
                    }
                    ApprovalStatus::Resolving { request, .. } => request.clone(),
                    ApprovalStatus::Resolved(_) | ApprovalStatus::Orphaned(_) => continue,
                };
                let mut envelope = request.as_ref().clone();
                if let NormalizedEvent::ApprovalRequested(request) = &mut envelope.event {
                    request.status = Some(UniversalApprovalStatus::Orphaned);
                    request.resolution_state = None;
                    request.resolving_decision = None;
                    request.details = request.details.clone().or_else(|| Some(reason.to_owned()));
                }
                approval.status = ApprovalStatus::Orphaned(Box::new(envelope.clone()));
                tracing::warn!(%session_id, %approval_id, "approval marked orphaned");
                orphaned.push(envelope);
            }
            orphaned
        };

        for envelope in orphaned {
            self.store_event(session_id, envelope).await;
        }
    }

    async fn orphan_pending_questions_for_session(&self, session_id: SessionId, reason: &str) {
        let orphaned = {
            let mut registry = self.inner.registry.lock().await;
            let mut orphaned = Vec::new();
            for (&question_id, question) in &mut registry.questions {
                if question.session_id != session_id || question.status.is_terminal() {
                    continue;
                }
                let mut state = question
                    .state
                    .as_deref()
                    .cloned()
                    .or_else(|| {
                        let request = question.request.as_deref()?;
                        let NormalizedEvent::QuestionRequested(request) = &request.event else {
                            return None;
                        };
                        Some(source_question_request(request, Utc::now()))
                    })
                    .unwrap_or_else(|| QuestionState {
                        question_id,
                        session_id,
                        turn_id: None,
                        title: "Input requested".to_owned(),
                        description: None,
                        fields: Vec::new(),
                        status: QuestionStatus::Pending,
                        answer: None,
                        native: None,
                        requested_at: None,
                        answered_at: None,
                    });
                state.status = QuestionStatus::Orphaned;
                state.answer = None;
                if state.description.is_none() {
                    state.description = Some(reason.to_owned());
                }
                question.status = QuestionStatus::Orphaned;
                question.state = Some(Box::new(state.clone()));
                tracing::warn!(%session_id, %question_id, "question marked orphaned");
                orphaned.push(state);
            }
            orphaned
        };

        for question in orphaned {
            self.store_universal_event_with_acceptance(
                session_id,
                AgentUniversalEvent {
                    protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
                    session_id,
                    event_id: None,
                    turn_id: question.turn_id,
                    item_id: None,
                    ts: None,
                    source: UniversalEventSource::ControlPlane,
                    native: None,
                    event: UniversalEventKind::QuestionRequested {
                        question: Box::new(question),
                    },
                },
                false,
            )
            .await
            .ok();
        }
    }

    async fn store_event(
        &self,
        session_id: SessionId,
        envelope: CachedEventEnvelope,
    ) -> CachedEventEnvelope {
        self.store_event_with_acceptance(session_id, envelope, false)
            .await
            .unwrap_or_else(|error| {
                tracing::warn!(%session_id, %error, "event storage failed");
                CachedEventEnvelope {
                    event_id: Some(Uuid::new_v4().to_string().into()),
                    event: NormalizedEvent::Error(agenter_core::AgentErrorEvent {
                        session_id: Some(session_id),
                        code: Some("event_storage_failed".to_owned()),
                        message: error.to_string(),
                        provider_payload: None,
                    }),
                }
            })
    }

    async fn store_event_with_acceptance(
        &self,
        session_id: SessionId,
        envelope: CachedEventEnvelope,
        strict_db: bool,
    ) -> anyhow::Result<CachedEventEnvelope> {
        self.store_event_with_universal_acceptance(session_id, envelope, None, strict_db)
            .await
    }

    async fn store_universal_event_with_acceptance(
        &self,
        session_id: SessionId,
        universal_event: AgentUniversalEvent,
        strict_db: bool,
    ) -> anyhow::Result<UniversalEventEnvelope> {
        let fallback_event_id = universal_event
            .event_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let mut envelope =
            runner_universal_event_envelope(session_id, &fallback_event_id, &universal_event);
        if let Some(pool) = &self.inner.db_pool {
            if let Some(workspace_id) = self.workspace_id_for_session(session_id).await {
                match agenter_db::append_universal_event_reducing_snapshot(
                    pool,
                    workspace_id,
                    envelope.clone(),
                    None,
                    apply_universal_event_to_snapshot,
                )
                .await
                {
                    Ok(outcome) => {
                        envelope = outcome.event.envelope();
                    }
                    Err(error) => {
                        if strict_db {
                            return Err(anyhow::anyhow!(
                                "failed to durably append universal runner event: {error}"
                            ));
                        }
                        tracing::warn!(%session_id, %error, "failed to persist universal runner event");
                    }
                }
            } else if strict_db {
                return Err(anyhow::anyhow!(
                    "failed to durably append universal runner event without a registered workspace"
                ));
            }
        }

        let sender = {
            let mut sessions = self.inner.sessions.lock().await;
            let events = sessions
                .entry(session_id)
                .or_insert_with(SessionEvents::new);
            if envelope.seq == UniversalSeq::zero() {
                events.next_seq += 1;
                envelope.seq = UniversalSeq::new(events.next_seq);
            }
            apply_universal_event_to_snapshot(&mut events.snapshot, &envelope);
            events.universal_cache.push(envelope.clone());
            if events.universal_cache.len() > UNIVERSAL_EVENT_REPLAY_LIMIT {
                let overflow = events.universal_cache.len() - UNIVERSAL_EVENT_REPLAY_LIMIT;
                events.universal_cache.drain(..overflow);
            }
            events.sender.clone()
        };

        let _ = sender.send(SessionBroadcastEvent {
            #[cfg(test)]
            normalized_event: CachedEventEnvelope {
                event_id: Some(envelope.event_id.clone().into()),
                event: NormalizedEvent::NativeNotification(NativeNotification {
                    session_id,
                    provider_id: AgentProviderId::from("universal"),
                    event_id: Some(envelope.event_id.clone()),
                    category: "universal".to_owned(),
                    method: "universal_event".to_owned(),
                    title: "Universal event".to_owned(),
                    detail: None,
                    status: None,
                    provider_payload: None,
                }),
            },
            universal_event: Some(envelope.clone()),
        });
        Ok(envelope)
    }

    async fn store_runner_universal_event_with_receipt(
        &self,
        runner_id: RunnerId,
        runner_event_seq: u64,
        session_id: SessionId,
        universal_event: AgentUniversalEvent,
        strict_db: bool,
    ) -> anyhow::Result<Option<UniversalEventEnvelope>> {
        let fallback_event_id = universal_event
            .event_id
            .clone()
            .unwrap_or_else(|| Uuid::new_v4().to_string());
        let mut envelope =
            runner_universal_event_envelope(session_id, &fallback_event_id, &universal_event);
        let Some(pool) = &self.inner.db_pool else {
            let stored = self
                .store_universal_event_with_acceptance(session_id, universal_event, strict_db)
                .await?;
            return Ok(Some(stored));
        };
        let Some(workspace_id) = self.workspace_id_for_session(session_id).await else {
            if strict_db {
                return Err(anyhow::anyhow!(
                    "failed to durably append universal runner event without a registered workspace"
                ));
            }
            tracing::warn!(
                %session_id,
                "failed to persist universal runner event without a registered workspace; continuing with in-memory universal broadcast"
            );
            let stored = self
                .store_universal_event_with_acceptance(session_id, universal_event, false)
                .await?;
            return Ok(Some(stored));
        };

        match agenter_db::append_runner_universal_event_reducing_snapshot(
            pool,
            workspace_id,
            runner_id,
            runner_event_seq,
            envelope.clone(),
            None,
            apply_universal_event_to_snapshot,
        )
        .await
        {
            Ok(agenter_db::models::RunnerUniversalAppendOutcome::Accepted(outcome)) => {
                envelope = outcome.event.envelope();
            }
            Ok(agenter_db::models::RunnerUniversalAppendOutcome::Duplicate(receipt)) => {
                tracing::debug!(
                    %runner_id,
                    runner_event_seq,
                    event_id = %receipt.event_id,
                    "deduped runner event from durable receipt"
                );
                self.inner
                    .seen_runner_events
                    .lock()
                    .await
                    .insert((runner_id, runner_event_seq));
                return Ok(None);
            }
            Err(error) => {
                if strict_db {
                    return Err(anyhow::anyhow!(
                        "failed to durably append universal runner event with receipt: {error}"
                    ));
                }
                tracing::warn!(%session_id, %error, "failed to persist universal runner event with receipt");
            }
        }

        let sender = {
            let mut sessions = self.inner.sessions.lock().await;
            let events = sessions
                .entry(session_id)
                .or_insert_with(SessionEvents::new);
            apply_universal_event_to_snapshot(&mut events.snapshot, &envelope);
            events.universal_cache.push(envelope.clone());
            if events.universal_cache.len() > UNIVERSAL_EVENT_REPLAY_LIMIT {
                let overflow = events.universal_cache.len() - UNIVERSAL_EVENT_REPLAY_LIMIT;
                events.universal_cache.drain(..overflow);
            }
            events.sender.clone()
        };

        let _ = sender.send(SessionBroadcastEvent {
            #[cfg(test)]
            normalized_event: CachedEventEnvelope {
                event_id: Some(envelope.event_id.clone().into()),
                event: NormalizedEvent::NativeNotification(NativeNotification {
                    session_id,
                    provider_id: AgentProviderId::from("universal"),
                    event_id: Some(envelope.event_id.clone()),
                    category: "universal".to_owned(),
                    method: "universal_event".to_owned(),
                    title: "Universal event".to_owned(),
                    detail: None,
                    status: None,
                    provider_payload: None,
                }),
            },
            universal_event: Some(envelope.clone()),
        });
        Ok(Some(envelope))
    }

    async fn store_event_with_universal_acceptance(
        &self,
        session_id: SessionId,
        mut envelope: CachedEventEnvelope,
        universal_event: Option<AgentUniversalEvent>,
        strict_db: bool,
    ) -> anyhow::Result<CachedEventEnvelope> {
        let mut broadcast_universal_event = None;
        if let Some(pool) = &self.inner.db_pool {
            if let Some(workspace_id) = self.workspace_id_for_session(session_id).await {
                let event_id = envelope
                    .event_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                let universal_event = universal_event
                    .as_ref()
                    .map(|universal| {
                        runner_universal_event_envelope(session_id, &event_id, universal)
                    })
                    .unwrap_or_else(|| {
                        universal_projection_universal_event(session_id, event_id, &envelope.event)
                    });
                match agenter_db::append_universal_event_reducing_snapshot(
                    pool,
                    workspace_id,
                    universal_event,
                    None,
                    apply_universal_event_to_snapshot,
                )
                .await
                {
                    Ok(outcome) => {
                        envelope.event_id = Some(outcome.event.event_id.to_string().into());
                        broadcast_universal_event = Some(outcome.event.envelope());
                    }
                    Err(error) => {
                        if strict_db {
                            return Err(anyhow::anyhow!(
                                "failed to durably append universal runner event: {error}"
                            ));
                        }
                        tracing::warn!(
                            %session_id,
                            %error,
                            "failed to persist universal normalized event; continuing with in-memory universal broadcast"
                        );
                    }
                }
            } else {
                if strict_db {
                    return Err(anyhow::anyhow!(
                        "failed to durably append universal runner event without a registered workspace"
                    ));
                }
                tracing::warn!(
                    %session_id,
                    "failed to persist universal normalized event without a registered workspace; continuing with in-memory universal broadcast"
                );
            }
        }
        let sender = {
            let mut sessions = self.inner.sessions.lock().await;
            let events = sessions
                .entry(session_id)
                .or_insert_with(SessionEvents::new);
            if broadcast_universal_event.is_none() {
                let event_id = envelope
                    .event_id
                    .as_ref()
                    .map(ToString::to_string)
                    .unwrap_or_else(|| Uuid::new_v4().to_string());
                events.next_seq += 1;
                let mut universal = universal_event
                    .as_ref()
                    .map(|universal| {
                        runner_universal_event_envelope(session_id, &event_id, universal)
                    })
                    .unwrap_or_else(|| {
                        universal_projection_universal_event(session_id, event_id, &envelope.event)
                    });
                universal.seq = UniversalSeq::new(events.next_seq);
                apply_universal_event_to_snapshot(&mut events.snapshot, &universal);
                broadcast_universal_event = Some(universal.clone());
                events.universal_cache.push(universal);
                if events.universal_cache.len() > UNIVERSAL_EVENT_REPLAY_LIMIT {
                    let overflow = events.universal_cache.len() - UNIVERSAL_EVENT_REPLAY_LIMIT;
                    events.universal_cache.drain(..overflow);
                }
            }
            events.cache.push(envelope.clone());
            if events.cache.len() > SESSION_EVENT_CACHE_LIMIT {
                let overflow = events.cache.len() - SESSION_EVENT_CACHE_LIMIT;
                events.cache.drain(..overflow);
            }
            events.sender.clone()
        };

        let _ = sender.send(SessionBroadcastEvent {
            #[cfg(test)]
            normalized_event: envelope.clone(),
            universal_event: broadcast_universal_event,
        });
        tracing::debug!(
            %session_id,
            event_id = envelope.event_id.as_ref().map(ToString::to_string).as_deref(),
            event_type = normalized_event_name(&envelope.event),
            "stored and broadcast normalized event"
        );
        Ok(envelope)
    }

    async fn workspace_id_for_session(&self, session_id: SessionId) -> Option<WorkspaceId> {
        self.inner
            .registry
            .lock()
            .await
            .sessions
            .get(&session_id)
            .map(|session| session.workspace.workspace_id)
    }

    pub async fn import_discovered_sessions(
        &self,
        runner_id: RunnerId,
        discovered: DiscoveredSessions,
        mode: SessionImportMode,
    ) -> WorkspaceSessionRefreshSummary {
        let mut summary = WorkspaceSessionRefreshSummary {
            discovered_count: discovered.sessions.len(),
            ..WorkspaceSessionRefreshSummary::default()
        };
        let Some(owner_user_id) = self
            .inner
            .bootstrap_admin
            .as_ref()
            .map(|admin| admin.user.user_id)
        else {
            tracing::warn!(%runner_id, "cannot import discovered sessions without bootstrap admin user");
            summary.skipped_failed_count = summary.discovered_count;
            return summary;
        };

        if let Some(pool) = &self.inner.db_pool {
            if let Err(error) = agenter_db::upsert_workspace_with_id(
                pool,
                discovered.workspace.workspace_id,
                runner_id,
                &discovered.workspace.path,
                discovered.workspace.display_name.as_deref(),
            )
            .await
            {
                tracing::warn!(
                    %runner_id,
                    workspace_id = %discovered.workspace.workspace_id,
                    %error,
                    "failed to persist discovered session workspace"
                );
                summary.skipped_failed_count = summary.discovered_count;
                return summary;
            }
        }

        for discovered_session in discovered.sessions {
            let discovered_session_updated_at =
                discovered_session_timestamp(discovered_session.updated_at.as_deref());
            let existing_import = if let Some(pool) = &self.inner.db_pool {
                match agenter_db::find_imported_session_by_external_id(
                    pool,
                    runner_id,
                    discovered.workspace.workspace_id,
                    &discovered.provider_id,
                    &discovered_session.external_session_id,
                )
                .await
                {
                    Ok(existing) => existing,
                    Err(error) => {
                        tracing::warn!(
                            %runner_id,
                            external_session_id = %discovered_session.external_session_id,
                            %error,
                            "failed to load existing discovered session"
                        );
                        None
                    }
                }
            } else {
                None
            };
            let session = if let Some(pool) = &self.inner.db_pool {
                let can_reuse_existing = existing_import.as_ref().is_some_and(|existing| {
                    discovered_session.title.as_deref() == existing.session.title.as_deref()
                        && discovered_session_updated_at
                            .map(|updated_at| updated_at == existing.session.updated_at)
                            .unwrap_or(true)
                });
                if can_reuse_existing {
                    let existing = existing_import.as_ref().expect("checked existing session");
                    registered_session_from_db(&existing.session, &discovered.workspace)
                } else {
                    match agenter_db::upsert_session_by_external_id(
                        pool,
                        agenter_db::UpsertSessionByExternalId {
                            owner_user_id,
                            runner_id,
                            workspace_id: discovered.workspace.workspace_id,
                            provider_id: discovered.provider_id.clone(),
                            external_session_id: &discovered_session.external_session_id,
                            title: discovered_session.title.as_deref(),
                            updated_at: discovered_session_updated_at,
                        },
                    )
                    .await
                    {
                        Ok(session) => registered_session_from_db(&session, &discovered.workspace),
                        Err(error) => {
                            tracing::warn!(
                                %runner_id,
                                external_session_id = %discovered_session.external_session_id,
                                %error,
                                "failed to persist discovered session"
                            );
                            continue;
                        }
                    }
                }
            } else {
                let mut registry = self.inner.registry.lock().await;
                if let Some(existing) = registry
                    .sessions
                    .values_mut()
                    .find(|session| {
                        session.runner_id == runner_id
                            && session.workspace.workspace_id == discovered.workspace.workspace_id
                            && session.provider_id == discovered.provider_id
                            && session.external_session_id.as_deref()
                                == Some(discovered_session.external_session_id.as_str())
                    })
                    .cloned()
                {
                    existing
                } else {
                    let now = Utc::now();
                    let updated_at = discovered_session_updated_at.unwrap_or(now);
                    RegisteredSession {
                        session_id: SessionId::new(),
                        owner_user_id,
                        runner_id,
                        workspace: discovered.workspace.clone(),
                        provider_id: discovered.provider_id.clone(),
                        status: SessionStatus::Idle,
                        title: discovered_session.title.clone(),
                        external_session_id: Some(discovered_session.external_session_id.clone()),
                        turn_settings: None,
                        usage: None,
                        created_at: now,
                        updated_at,
                    }
                }
            };

            self.inner
                .registry
                .lock()
                .await
                .sessions
                .insert(session.session_id, session.clone());

            let discovered_events = discovered_history_events(
                session.session_id,
                owner_user_id,
                &discovered_session.history,
            );
            let history_loaded = matches!(
                discovered_session.history_status,
                DiscoveredSessionHistoryStatus::Loaded
            );
            if !history_loaded {
                if matches!(
                    discovered_session.history_status,
                    DiscoveredSessionHistoryStatus::Failed { .. }
                ) {
                    summary.skipped_failed_count += 1;
                }
                continue;
            }
            if !discovered_events.is_empty()
                || matches!(
                    mode,
                    SessionImportMode::Forced | SessionImportMode::ForceReload
                )
            {
                let history_fingerprint = discovered_history_fingerprint(
                    &discovered_session.history_status,
                    &discovered_session.history,
                );
                if !matches!(mode, SessionImportMode::ForceReload)
                    && existing_import.as_ref().is_some_and(|existing| {
                        existing.imported_history_fingerprint == history_fingerprint
                    })
                {
                    continue;
                }
                if matches!(
                    mode,
                    SessionImportMode::Forced | SessionImportMode::ForceReload
                ) {
                    if let Some(pool) = &self.inner.db_pool {
                        let envelopes = discovered_events
                            .iter()
                            .map(|event| {
                                let event_id = Uuid::new_v4().to_string();
                                universal_projection_universal_event(
                                    session.session_id,
                                    event_id,
                                    event,
                                )
                            })
                            .collect::<Vec<_>>();
                        match agenter_db::replace_session_event_projection(
                            pool,
                            discovered.workspace.workspace_id,
                            session.session_id,
                            &envelopes,
                        )
                        .await
                        {
                            Ok(inserted) => {
                                let mut snapshot = SessionSnapshot {
                                    session_id: session.session_id,
                                    ..SessionSnapshot::default()
                                };
                                for event in &inserted {
                                    let envelope = event.envelope();
                                    apply_universal_event_to_snapshot(&mut snapshot, &envelope);
                                    snapshot.latest_seq = Some(envelope.seq);
                                }
                                if let Err(error) =
                                    agenter_db::store_session_snapshot(pool, &snapshot).await
                                {
                                    tracing::warn!(%session.session_id, %error, "failed to store imported session snapshot");
                                }
                                if let Err(error) =
                                    agenter_db::update_session_imported_history_fingerprint(
                                        pool,
                                        session.session_id,
                                        history_fingerprint.as_deref(),
                                    )
                                    .await
                                {
                                    tracing::warn!(%session.session_id, %error, "failed to store imported history fingerprint");
                                }
                                summary.refreshed_cache_count += 1;
                                continue;
                            }
                            Err(error) => {
                                tracing::warn!(%session.session_id, %error, "failed to replace discovered session event projection");
                                summary.skipped_failed_count += 1;
                                continue;
                            }
                        }
                    } else {
                        let mut sessions = self.inner.sessions.lock().await;
                        sessions
                            .entry(session.session_id)
                            .or_insert_with(SessionEvents::new)
                            .cache
                            .clear();
                        if let Some(events) = sessions.get_mut(&session.session_id) {
                            events.universal_cache.clear();
                            events.snapshot = SessionSnapshot {
                                session_id: session.session_id,
                                ..SessionSnapshot::default()
                            };
                            events.next_seq = 0;
                        }
                    }
                }
                summary.refreshed_cache_count += 1;
                for event in discovered_events {
                    self.store_event(
                        session.session_id,
                        CachedEventEnvelope {
                            event_id: Some(Uuid::new_v4().to_string().into()),
                            event,
                        },
                    )
                    .await;
                }
            }
        }
        summary
    }

    pub async fn process_runner_discovered_sessions(
        &self,
        runner_id: RunnerId,
        request_id: Option<RequestId>,
        discovered: DiscoveredSessions,
    ) -> bool {
        let db_backed = self.inner.db_pool.is_some();
        let mode = if let Some(request_id) = &request_id {
            if self
                .inner
                .refresh_force_reload_requests
                .lock()
                .await
                .remove(request_id)
            {
                SessionImportMode::ForceReload
            } else {
                SessionImportMode::Forced
            }
        } else {
            SessionImportMode::Automatic
        };
        if let Some(request_id) = &request_id {
            self.update_refresh_job_status(request_id, WorkspaceSessionRefreshStatus::Importing)
                .await;
        }
        let summary = self
            .import_discovered_sessions(runner_id, discovered, mode)
            .await;
        if let Some(request_id) = request_id {
            self.record_refresh_summary(request_id.clone(), summary.clone())
                .await;
            let accepted = !db_backed || summary.skipped_failed_count == 0;
            self.upsert_refresh_job(
                request_id,
                if accepted {
                    WorkspaceSessionRefreshStatus::Succeeded
                } else {
                    WorkspaceSessionRefreshStatus::Failed
                },
                Some(summary.clone()),
                (!accepted)
                    .then(|| "discovered session import did not complete successfully".to_owned()),
            )
            .await;
        }
        !db_backed || summary.skipped_failed_count == 0
    }

    pub async fn record_refresh_summary(
        &self,
        request_id: RequestId,
        summary: WorkspaceSessionRefreshSummary,
    ) {
        self.inner
            .refresh_summaries
            .lock()
            .await
            .insert(request_id, summary);
    }

    async fn upsert_refresh_job(
        &self,
        request_id: RequestId,
        status: WorkspaceSessionRefreshStatus,
        summary: Option<WorkspaceSessionRefreshSummary>,
        error: Option<String>,
    ) {
        self.inner.refresh_jobs.lock().await.insert(
            request_id.clone(),
            WorkspaceSessionRefreshJob {
                refresh_id: request_id,
                status,
                progress: None,
                log: Vec::new(),
                summary,
                error,
                updated_at: Utc::now(),
            },
        );
    }

    async fn update_refresh_job_status(
        &self,
        request_id: &RequestId,
        status: WorkspaceSessionRefreshStatus,
    ) {
        if let Some(job) = self.inner.refresh_jobs.lock().await.get_mut(request_id) {
            if matches!(
                job.status,
                WorkspaceSessionRefreshStatus::Succeeded | WorkspaceSessionRefreshStatus::Failed
            ) {
                return;
            }
            job.status = status;
            job.updated_at = Utc::now();
        }
    }

    pub async fn record_refresh_operation_update(&self, update: RunnerOperationUpdate) {
        if update.kind != RunnerOperationKind::SessionRefresh {
            return;
        }
        let mut jobs = self.inner.refresh_jobs.lock().await;
        let Some(job) = jobs.get_mut(&update.operation_id) else {
            return;
        };
        if matches!(
            job.status,
            WorkspaceSessionRefreshStatus::Succeeded
                | WorkspaceSessionRefreshStatus::Failed
                | WorkspaceSessionRefreshStatus::Cancelled
        ) && !matches!(
            update.status,
            RunnerOperationStatus::Succeeded
                | RunnerOperationStatus::Failed
                | RunnerOperationStatus::Cancelled
        ) {
            return;
        }

        let status = workspace_refresh_status_from_runner(&update.status);
        job.status = status.clone();
        job.progress = update.progress.clone();
        if matches!(status, WorkspaceSessionRefreshStatus::Failed) {
            job.error = update.message.clone().or(Some(update.stage_label.clone()));
        }
        let message = update
            .message
            .clone()
            .unwrap_or_else(|| update.stage_label.clone());
        job.log.push(WorkspaceSessionRefreshLogEntry {
            ts: update.ts.unwrap_or_else(Utc::now),
            level: workspace_refresh_log_level_from_runner(&update.level),
            status,
            message,
            progress: update.progress,
        });
        if job.log.len() > REFRESH_JOB_LOG_LIMIT {
            let overflow = job.log.len() - REFRESH_JOB_LOG_LIMIT;
            job.log.drain(..overflow);
        }
        job.updated_at = Utc::now();
    }

    pub async fn workspace_session_refresh_status(
        &self,
        request_id: &RequestId,
    ) -> Option<WorkspaceSessionRefreshJob> {
        self.inner
            .refresh_jobs
            .lock()
            .await
            .get(request_id)
            .cloned()
    }

    pub async fn fail_workspace_session_refresh(&self, request_id: RequestId, error: String) {
        self.upsert_refresh_job(
            request_id,
            WorkspaceSessionRefreshStatus::Failed,
            None,
            Some(error),
        )
        .await;
    }

    #[cfg(test)]
    pub async fn take_refresh_summary(
        &self,
        request_id: &RequestId,
    ) -> Option<WorkspaceSessionRefreshSummary> {
        self.inner.refresh_summaries.lock().await.remove(request_id)
    }
}

pub fn apply_universal_event_to_snapshot(
    snapshot: &mut SessionSnapshot,
    envelope: &UniversalEventEnvelope,
) {
    snapshot.session_id = envelope.session_id;
    snapshot.latest_seq = Some(envelope.seq);
    match &envelope.event {
        UniversalEventKind::SessionCreated { session } => {
            snapshot.info = Some((**session).clone());
        }
        UniversalEventKind::SessionStatusChanged { status, .. } => {
            if let Some(info) = &mut snapshot.info {
                info.status = status.clone();
            }
        }
        UniversalEventKind::SessionMetadataChanged { title } => {
            if let Some(info) = &mut snapshot.info {
                info.title = title.clone();
            }
        }
        UniversalEventKind::TurnStarted { turn }
        | UniversalEventKind::TurnStatusChanged { turn }
        | UniversalEventKind::TurnCompleted { turn }
        | UniversalEventKind::TurnFailed { turn }
        | UniversalEventKind::TurnCancelled { turn }
        | UniversalEventKind::TurnInterrupted { turn }
        | UniversalEventKind::TurnDetached { turn } => {
            snapshot.turns.insert(turn.turn_id, turn.clone());
            set_active_turn(snapshot, turn.turn_id, is_active_turn_status(&turn.status));
        }
        UniversalEventKind::ItemCreated { item } => {
            snapshot.items.insert(item.item_id, item.as_ref().clone());
        }
        UniversalEventKind::ContentDelta {
            block_id,
            kind,
            delta,
        } => {
            let Some(item_id) = envelope.item_id else {
                return;
            };
            let item = snapshot.items.entry(item_id).or_insert_with(|| ItemState {
                item_id,
                session_id: envelope.session_id,
                turn_id: envelope.turn_id,
                role: ItemRole::Assistant,
                status: ItemStatus::Streaming,
                content: Vec::new(),
                tool: None,
                native: envelope.native.clone(),
            });
            if let Some(block) = item
                .content
                .iter_mut()
                .find(|block| block.block_id == *block_id)
            {
                match &mut block.text {
                    Some(text) => text.push_str(delta),
                    None => block.text = Some(delta.clone()),
                }
            } else {
                item.content.push(ContentBlock {
                    block_id: block_id.clone(),
                    kind: kind.clone().unwrap_or(ContentBlockKind::Text),
                    text: Some(delta.clone()),
                    mime_type: None,
                    artifact_id: None,
                });
            }
            item.status = ItemStatus::Streaming;
            if let Some(tool) = &mut item.tool {
                tool.status = ItemStatus::Streaming;
            }
        }
        UniversalEventKind::ContentCompleted {
            block_id,
            kind,
            text,
        } => {
            let Some(item_id) = envelope.item_id else {
                return;
            };
            let item = snapshot.items.entry(item_id).or_insert_with(|| ItemState {
                item_id,
                session_id: envelope.session_id,
                turn_id: envelope.turn_id,
                role: ItemRole::Assistant,
                status: ItemStatus::Completed,
                content: Vec::new(),
                tool: None,
                native: envelope.native.clone(),
            });
            if let Some(block) = item
                .content
                .iter_mut()
                .find(|block| block.block_id == *block_id)
            {
                if let Some(text) = text {
                    block.text = Some(text.clone());
                }
                if let Some(kind) = kind {
                    block.kind = kind.clone();
                }
            } else if text.is_some() || kind.is_some() {
                item.content.push(ContentBlock {
                    block_id: block_id.clone(),
                    kind: kind.clone().unwrap_or(ContentBlockKind::Text),
                    text: text.clone(),
                    mime_type: None,
                    artifact_id: None,
                });
            }
            item.status = ItemStatus::Completed;
            if let Some(tool) = &mut item.tool {
                tool.status = ItemStatus::Completed;
            }
        }
        UniversalEventKind::ApprovalRequested { approval } => {
            merge_approval_into_snapshot(snapshot, approval);
            if let Some(turn_id) = approval.turn_id {
                if let Some(turn) = snapshot.turns.get_mut(&turn_id) {
                    turn.status = TurnStatus::WaitingForApproval;
                }
                set_active_turn(snapshot, turn_id, true);
            }
        }
        UniversalEventKind::ApprovalResolved {
            approval_id,
            status,
            resolved_at,
            native,
            ..
        } => {
            if let Some(existing) = snapshot.approvals.get_mut(approval_id) {
                existing.status = status.clone();
                existing.resolved_at = Some(*resolved_at);
                if native.is_some() {
                    existing.native = native.clone();
                }
            }
        }
        UniversalEventKind::QuestionRequested { question }
        | UniversalEventKind::QuestionAnswered { question } => {
            merge_question_into_snapshot(snapshot, question);
            if let Some(turn_id) = question.turn_id {
                if let Some(turn) = snapshot.turns.get_mut(&turn_id) {
                    turn.status = match &question.status {
                        QuestionStatus::Pending => TurnStatus::WaitingForInput,
                        QuestionStatus::Answered
                        | QuestionStatus::Cancelled
                        | QuestionStatus::Expired
                        | QuestionStatus::Orphaned => turn.status.clone(),
                        QuestionStatus::Detached => TurnStatus::Detached,
                    };
                }
                set_active_turn(
                    snapshot,
                    turn_id,
                    question.status == QuestionStatus::Pending,
                );
            }
        }
        UniversalEventKind::PlanUpdated { plan } => {
            merge_plan_into_snapshot(snapshot, envelope, plan);
        }
        UniversalEventKind::DiffUpdated { diff } => {
            snapshot.diffs.insert(diff.diff_id, diff.clone());
        }
        UniversalEventKind::ArtifactCreated { artifact } => {
            snapshot
                .artifacts
                .insert(artifact.artifact_id, artifact.clone());
        }
        UniversalEventKind::UsageUpdated { usage } => {
            if let Some(info) = &mut snapshot.info {
                info.usage = Some(usage.clone());
            }
        }
        UniversalEventKind::ErrorReported { .. } => {}
        UniversalEventKind::ProviderNotification { .. } => {}
        UniversalEventKind::NativeUnknown { .. } => {}
    }
}

fn merge_approval_into_snapshot(snapshot: &mut SessionSnapshot, approval: &ApprovalRequest) {
    if let Some(existing) = snapshot.approvals.get_mut(&approval.approval_id) {
        if !is_final_universal_approval_status(&existing.status)
            || is_final_universal_approval_status(&approval.status)
        {
            existing.status = approval.status.clone();
        }
        existing.resolved_at = approval.resolved_at.or(existing.resolved_at);
        if approval.requested_at.is_some() {
            existing.requested_at = approval.requested_at;
        }
        if approval.native.is_some() {
            existing.native = approval.native.clone();
        }
        if approval.risk.is_some() {
            existing.risk = approval.risk.clone();
        }
        if approval.subject.is_some() {
            existing.subject = approval.subject.clone();
        }
        if !approval.options.is_empty() {
            existing.options = approval.options.clone();
        }
        if approval.details.is_some() {
            existing.details = approval.details.clone();
        }
        if approval.title != "Approval resolved" {
            existing.title = approval.title.clone();
            existing.kind = approval.kind.clone();
        }
    } else {
        snapshot
            .approvals
            .insert(approval.approval_id, approval.clone());
    }
}

fn merge_question_into_snapshot(snapshot: &mut SessionSnapshot, question: &QuestionState) {
    if let Some(existing) = snapshot.questions.get_mut(&question.question_id) {
        existing.status = question.status.clone();
        existing.answered_at = question.answered_at.or(existing.answered_at);
        if question.answer.is_some() {
            existing.answer = question.answer.clone();
        }
        if !question.fields.is_empty() {
            existing.fields = question.fields.clone();
        }
        if question.description.is_some() {
            existing.description = question.description.clone();
        }
        if question.native.is_some() {
            existing.native = question.native.clone();
        }
        if question.title != "Input requested" {
            existing.title = question.title.clone();
        }
    } else {
        snapshot
            .questions
            .insert(question.question_id, question.clone());
    }
}

fn is_final_universal_approval_status(status: &UniversalApprovalStatus) -> bool {
    matches!(
        status,
        UniversalApprovalStatus::Approved
            | UniversalApprovalStatus::Denied
            | UniversalApprovalStatus::Cancelled
            | UniversalApprovalStatus::Expired
            | UniversalApprovalStatus::Orphaned
    )
}

fn set_active_turn(snapshot: &mut SessionSnapshot, turn_id: agenter_core::TurnId, active: bool) {
    if active {
        if !snapshot.active_turns.contains(&turn_id) {
            snapshot.active_turns.push(turn_id);
        }
    } else {
        snapshot.active_turns.retain(|id| *id != turn_id);
    }
}

fn is_active_turn_status(status: &TurnStatus) -> bool {
    matches!(
        status,
        TurnStatus::Starting
            | TurnStatus::Running
            | TurnStatus::WaitingForInput
            | TurnStatus::WaitingForApproval
            | TurnStatus::Interrupting
    )
}

fn merge_plan_into_snapshot(
    snapshot: &mut SessionSnapshot,
    envelope: &UniversalEventEnvelope,
    plan: &agenter_core::PlanState,
) {
    let mut next = if plan.partial {
        snapshot
            .plans
            .get(&plan.plan_id)
            .cloned()
            .unwrap_or_else(|| plan.clone())
    } else {
        plan.clone()
    };

    if plan.partial {
        next.status = plan.status.clone();
        if let Some(title) = &plan.title {
            next.title = Some(title.clone());
        }
        if let Some(content) = &plan.content {
            match &mut next.content {
                Some(existing) => existing.push_str(content),
                None => next.content = Some(content.clone()),
            }
        }
        if !plan.entries.is_empty() {
            for incoming in &plan.entries {
                if let Some(existing) = next
                    .entries
                    .iter_mut()
                    .find(|entry| entry.entry_id == incoming.entry_id)
                {
                    existing.label = incoming.label.clone();
                    existing.status = incoming.status.clone();
                } else {
                    next.entries.push(incoming.clone());
                }
            }
        }
        if !plan.artifact_refs.is_empty() {
            next.artifact_refs.extend(plan.artifact_refs.clone());
            next.artifact_refs.sort();
            next.artifact_refs.dedup();
        }
        next.updated_at = plan.updated_at;
    }

    materialize_plan_item(snapshot, envelope, &next);
    snapshot.plans.insert(next.plan_id, next);
}

fn materialize_plan_item(
    snapshot: &mut SessionSnapshot,
    envelope: &UniversalEventEnvelope,
    plan: &agenter_core::PlanState,
) {
    if plan.content.is_none() && plan.entries.is_empty() {
        if let Some(item_id) = envelope.item_id {
            snapshot.items.remove(&item_id);
        }
        snapshot
            .items
            .remove(&universal_projection_item_id(&format!(
                "plan:item:{}",
                plan.plan_id
            )));
        return;
    }
    let item_id = envelope
        .item_id
        .unwrap_or_else(|| universal_projection_item_id(&format!("plan:item:{}", plan.plan_id)));
    let mut text = plan.content.clone().unwrap_or_default();
    if !plan.entries.is_empty() {
        if !text.is_empty() {
            text.push_str("\n\n");
        }
        for entry in &plan.entries {
            text.push_str("- ");
            text.push_str(&entry.label);
            text.push('\n');
        }
    }
    snapshot.items.insert(
        item_id,
        ItemState {
            item_id,
            session_id: envelope.session_id,
            turn_id: plan.turn_id.or(envelope.turn_id),
            role: ItemRole::Assistant,
            status: match &plan.status {
                agenter_core::PlanStatus::Completed | agenter_core::PlanStatus::Approved => {
                    ItemStatus::Completed
                }
                agenter_core::PlanStatus::Failed => ItemStatus::Failed,
                agenter_core::PlanStatus::Cancelled => ItemStatus::Cancelled,
                _ => ItemStatus::Streaming,
            },
            content: vec![ContentBlock {
                block_id: format!("plan-{}", plan.plan_id),
                kind: ContentBlockKind::Text,
                text: Some(text),
                mime_type: Some("text/markdown".to_owned()),
                artifact_id: None,
            }],
            tool: None,
            native: envelope.native.clone(),
        },
    );
}

fn universal_projection_universal_event(
    session_id: SessionId,
    event_id: String,
    event: &NormalizedEvent,
) -> UniversalEventEnvelope {
    let ts = Utc::now();
    let mut turn_id = None;
    let mut item_id = None;
    let universal_event = match event {
        NormalizedEvent::SessionStarted(info) => UniversalEventKind::SessionCreated {
            session: Box::new(info.clone()),
        },
        NormalizedEvent::SessionStatusChanged(status) => UniversalEventKind::SessionStatusChanged {
            status: status.status.clone(),
            reason: status.reason.clone(),
        },
        NormalizedEvent::UserMessage(message) => {
            let message_key = message.message_id.as_deref().unwrap_or(event_id.as_str());
            let user_item_id =
                universal_projection_item_id(&format!("user:{}:{message_key}", message.session_id));
            item_id = Some(user_item_id);
            UniversalEventKind::ItemCreated {
                item: Box::new(ItemState {
                    item_id: user_item_id,
                    session_id: message.session_id,
                    turn_id: None,
                    role: ItemRole::User,
                    status: ItemStatus::Completed,
                    content: vec![ContentBlock {
                        block_id: format!("text-{message_key}"),
                        kind: ContentBlockKind::Text,
                        text: Some(message.content.clone()),
                        mime_type: None,
                        artifact_id: None,
                    }],
                    tool: None,
                    native: None,
                }),
            }
        }
        NormalizedEvent::AgentMessageDelta(message) => {
            turn_id = universal_projection_turn_id_from_payload(message.provider_payload.as_ref())
                .or_else(|| Some(universal_projection_turn_id(&message.message_id)));
            item_id = Some(universal_projection_item_id(&format!(
                "assistant:{}:{}",
                message.session_id, message.message_id
            )));
            UniversalEventKind::ContentDelta {
                block_id: format!("text-{}", message.message_id),
                kind: Some(ContentBlockKind::Text),
                delta: message.delta.clone(),
            }
        }
        NormalizedEvent::AgentMessageCompleted(message) => {
            turn_id = universal_projection_turn_id_from_payload(message.provider_payload.as_ref())
                .or_else(|| Some(universal_projection_turn_id(&message.message_id)));
            if universal_projection_payload_method(message.provider_payload.as_ref())
                == Some("turn/completed")
            {
                let completed_turn_id =
                    turn_id.unwrap_or_else(|| universal_projection_turn_id(&message.message_id));
                turn_id = Some(completed_turn_id);
                UniversalEventKind::TurnCompleted {
                    turn: universal_projection_turn_state(
                        message.session_id,
                        completed_turn_id,
                        TurnStatus::Completed,
                    ),
                }
            } else {
                item_id = Some(universal_projection_item_id(&format!(
                    "assistant:{}:{}",
                    message.session_id, message.message_id
                )));
                UniversalEventKind::ContentCompleted {
                    block_id: format!("text-{}", message.message_id),
                    kind: Some(ContentBlockKind::Text),
                    text: message.content.clone(),
                }
            }
        }
        NormalizedEvent::PlanUpdated(plan) => {
            turn_id = plan
                .plan_id
                .as_deref()
                .map(universal_projection_turn_id)
                .or_else(|| {
                    universal_projection_turn_id_from_payload(plan.provider_payload.as_ref())
                });
            UniversalEventKind::PlanUpdated {
                plan: agenter_core::PlanState {
                    plan_id: agenter_core::PlanId::from_uuid(universal_projection_uuid(&format!(
                        "plan:{}:{}",
                        plan.session_id,
                        plan.plan_id.as_deref().unwrap_or("default")
                    ))),
                    session_id: plan.session_id,
                    turn_id,
                    status: universal_projection_plan_status(plan),
                    title: plan.title.clone(),
                    content: plan.content.clone(),
                    entries: plan
                        .entries
                        .iter()
                        .enumerate()
                        .map(|(index, entry)| agenter_core::UniversalPlanEntry {
                            entry_id: format!("entry-{index}"),
                            label: entry.label.clone(),
                            status: entry.status.clone(),
                        })
                        .collect(),
                    artifact_refs: Vec::new(),
                    source: agenter_core::PlanSource::NativeStructured,
                    partial: plan.append,
                    updated_at: None,
                },
            }
        }
        NormalizedEvent::CommandStarted(command) => {
            turn_id = universal_projection_turn_id_from_payload(command.provider_payload.as_ref());
            let command_item_id = universal_projection_item_id(&format!(
                "command:{}:{}",
                command.session_id, command.command_id
            ));
            item_id = Some(command_item_id);
            UniversalEventKind::ItemCreated {
                item: Box::new(ItemState {
                    item_id: command_item_id,
                    session_id: command.session_id,
                    turn_id,
                    role: ItemRole::Tool,
                    status: ItemStatus::Streaming,
                    content: vec![ContentBlock {
                        block_id: format!("command-{}", command.command_id),
                        kind: ContentBlockKind::ToolCall,
                        text: Some(command.command.clone()),
                        mime_type: None,
                        artifact_id: None,
                    }],
                    tool: Some(universal_projection_command_tool_projection(command)),
                    native: None,
                }),
            }
        }
        NormalizedEvent::CommandOutputDelta(output) => {
            turn_id = universal_projection_turn_id_from_payload(output.provider_payload.as_ref());
            item_id = Some(universal_projection_item_id(&format!(
                "command:{}:{}",
                output.session_id, output.command_id
            )));
            let stream = match output.stream {
                CommandOutputStream::Stdout => "stdout",
                CommandOutputStream::Stderr => "stderr",
            };
            UniversalEventKind::ContentDelta {
                block_id: format!("command-{}-{stream}", output.command_id),
                kind: Some(ContentBlockKind::CommandOutput),
                delta: output.delta.clone(),
            }
        }
        NormalizedEvent::CommandCompleted(command) => {
            turn_id = universal_projection_turn_id_from_payload(command.provider_payload.as_ref());
            item_id = Some(universal_projection_item_id(&format!(
                "command:{}:{}",
                command.session_id, command.command_id
            )));
            UniversalEventKind::ContentCompleted {
                block_id: format!("command-{}-status", command.command_id),
                kind: Some(ContentBlockKind::CommandOutput),
                text: Some(if command.success {
                    "command completed".to_owned()
                } else {
                    "command failed".to_owned()
                }),
            }
        }
        NormalizedEvent::ToolStarted(tool)
        | NormalizedEvent::ToolUpdated(tool)
        | NormalizedEvent::ToolCompleted(tool) => {
            turn_id = universal_projection_turn_id_from_payload(tool.provider_payload.as_ref());
            let tool_item_id = universal_projection_item_id(&format!(
                "tool:{}:{}",
                tool.session_id, tool.tool_call_id
            ));
            item_id = Some(tool_item_id);
            let status = match event {
                NormalizedEvent::ToolCompleted(_) => {
                    universal_projection_tool_completed_status(tool)
                }
                _ => ItemStatus::Streaming,
            };
            UniversalEventKind::ItemCreated {
                item: Box::new(universal_projection_tool_item(
                    tool,
                    tool_item_id,
                    turn_id,
                    status,
                )),
            }
        }
        NormalizedEvent::FileChangeProposed(change)
        | NormalizedEvent::FileChangeApplied(change)
        | NormalizedEvent::FileChangeRejected(change) => {
            turn_id = universal_projection_turn_id_from_payload(change.provider_payload.as_ref());
            UniversalEventKind::DiffUpdated {
                diff: agenter_core::DiffState {
                    diff_id: agenter_core::DiffId::from_uuid(universal_projection_uuid(&format!(
                        "file:{}:{}",
                        change.session_id, change.path
                    ))),
                    session_id: change.session_id,
                    turn_id,
                    title: Some(change.path.clone()),
                    files: vec![agenter_core::DiffFile {
                        path: change.path.clone(),
                        status: change.change_kind.clone(),
                        diff: change.diff.clone(),
                    }],
                    updated_at: None,
                },
            }
        }
        NormalizedEvent::ApprovalRequested(request) => UniversalEventKind::ApprovalRequested {
            approval: Box::new(source_approval_request(request, ts)),
        },
        NormalizedEvent::ApprovalResolved(resolved) => UniversalEventKind::ApprovalResolved {
            approval_id: resolved.approval_id,
            status: approval_decision_universal_status(&resolved.decision),
            resolved_at: resolved.resolved_at,
            resolved_by_user_id: resolved.resolved_by_user_id,
            native: Some(NativeRef {
                protocol: "uap.universal_projection".to_owned(),
                method: Some("approval_resolved".to_owned()),
                kind: Some("universal_projection".to_owned()),
                native_id: safe_native_request_id(resolved.provider_payload.as_ref()),
                summary: Some("Approval resolved".to_owned()),
                hash: None,
                pointer: None,
            }),
        },
        NormalizedEvent::QuestionRequested(request) => UniversalEventKind::QuestionRequested {
            question: Box::new(source_question_request(request, ts)),
        },
        NormalizedEvent::QuestionAnswered(answered) => UniversalEventKind::QuestionAnswered {
            question: Box::new(source_answered_question(answered, ts)),
        },
        NormalizedEvent::TurnStatusChanged(turn) => {
            turn_id = Some(turn.turn_id);
            UniversalEventKind::TurnStatusChanged { turn: turn.clone() }
        }
        NormalizedEvent::TurnFailed(turn) => {
            turn_id = Some(turn.turn_id);
            UniversalEventKind::TurnFailed { turn: turn.clone() }
        }
        NormalizedEvent::TurnCancelled(turn) => {
            turn_id = Some(turn.turn_id);
            UniversalEventKind::TurnCancelled { turn: turn.clone() }
        }
        NormalizedEvent::TurnInterrupted(turn) => {
            turn_id = Some(turn.turn_id);
            UniversalEventKind::TurnInterrupted { turn: turn.clone() }
        }
        NormalizedEvent::NativeNotification(native_notification)
        | NormalizedEvent::TurnDiffUpdated(native_notification)
        | NormalizedEvent::ItemReasoning(native_notification)
        | NormalizedEvent::ServerRequestResolved(native_notification)
        | NormalizedEvent::McpToolCallProgress(native_notification)
        | NormalizedEvent::ThreadRealtimeEvent(native_notification) => {
            if let Some(usage) = usage_snapshot_from_native_notification(native_notification) {
                UniversalEventKind::UsageUpdated {
                    usage: Box::new(usage),
                }
            } else if native_notification.method == "turn/started" {
                let started_turn_id = native_notification
                    .event_id
                    .as_deref()
                    .map(universal_projection_turn_id)
                    .unwrap_or_else(|| {
                        universal_projection_turn_id(&format!(
                            "{}:turn",
                            native_notification.session_id
                        ))
                    });
                turn_id = Some(started_turn_id);
                UniversalEventKind::TurnStarted {
                    turn: universal_projection_turn_state(
                        native_notification.session_id,
                        started_turn_id,
                        TurnStatus::Running,
                    ),
                }
            } else if native_notification.method == "thread/name/updated" {
                UniversalEventKind::SessionMetadataChanged {
                    title: universal_projection_thread_title(
                        native_notification.provider_payload.as_ref(),
                    ),
                }
            } else if matches!(event, NormalizedEvent::TurnDiffUpdated(_)) {
                turn_id = native_notification
                    .event_id
                    .as_deref()
                    .map(universal_projection_turn_id)
                    .or_else(|| {
                        universal_projection_turn_id_from_payload(
                            native_notification.provider_payload.as_ref(),
                        )
                    });
                UniversalEventKind::DiffUpdated {
                    diff: agenter_core::DiffState {
                        diff_id: agenter_core::DiffId::from_uuid(universal_projection_uuid(
                            &format!(
                                "provider:{}:{}",
                                native_notification.session_id,
                                native_notification
                                    .event_id
                                    .as_deref()
                                    .unwrap_or(&native_notification.method)
                            ),
                        )),
                        session_id: native_notification.session_id,
                        turn_id,
                        title: Some(native_notification.title.clone()),
                        files: Vec::new(),
                        updated_at: None,
                    },
                }
            } else {
                UniversalEventKind::ProviderNotification {
                    notification: provider_notification_from_native(native_notification),
                }
            }
        }
        _ => UniversalEventKind::NativeUnknown {
            summary: Some(normalized_event_name(event).to_owned()),
        },
    };

    UniversalEventEnvelope {
        event_id,
        seq: UniversalSeq::zero(),
        session_id,
        turn_id,
        item_id,
        ts,
        source: UniversalEventSource::ControlPlane,
        native: Some(NativeRef {
            protocol: "uap.universal_projection".to_owned(),
            method: Some(normalized_event_name(event).to_owned()),
            kind: Some("universal_projection".to_owned()),
            native_id: None,
            summary: Some("universal_projection from source NormalizedEvent".to_owned()),
            hash: None,
            pointer: None,
        }),
        event: universal_event,
    }
}

fn runner_universal_event_envelope(
    session_id: SessionId,
    fallback_event_id: &str,
    event: &AgentUniversalEvent,
) -> UniversalEventEnvelope {
    UniversalEventEnvelope {
        event_id: event
            .event_id
            .clone()
            .unwrap_or_else(|| fallback_event_id.to_owned()),
        seq: UniversalSeq::zero(),
        session_id,
        turn_id: event.turn_id,
        item_id: event.item_id,
        ts: event.ts.unwrap_or_else(Utc::now),
        source: event.source.clone(),
        native: event.native.clone(),
        event: event.event.clone(),
    }
}

fn universal_projection_command_tool_projection(
    event: &agenter_core::CommandEvent,
) -> ToolProjection {
    ToolProjection {
        kind: ToolProjectionKind::Command,
        name: "command".to_owned(),
        title: event.command.clone(),
        status: ItemStatus::Streaming,
        detail: command_projection_detail(event),
        input_summary: None,
        output_summary: None,
        command: Some(ToolCommandProjection {
            command: event.command.clone(),
            cwd: event.cwd.clone(),
            source: event.source.clone(),
            process_id: event.process_id.clone(),
            actions: event
                .actions
                .iter()
                .map(universal_projection_command_action_projection)
                .collect(),
            exit_code: None,
            duration_ms: None,
            success: None,
        }),
        subagent: None,
        mcp: None,
    }
}

fn command_projection_detail(event: &agenter_core::CommandEvent) -> Option<String> {
    let parts = [
        event.cwd.clone(),
        event.source.clone(),
        event.process_id.clone().map(|pid| format!("pid {pid}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    (!parts.is_empty()).then_some(parts.join(" · "))
}

fn universal_projection_command_action_projection(action: &CommandAction) -> ToolActionProjection {
    ToolActionProjection {
        kind: action.kind.clone(),
        label: action
            .name
            .clone()
            .or_else(|| action.command.clone())
            .or_else(|| action.path.clone())
            .or_else(|| action.query.clone())
            .unwrap_or_else(|| action.kind.clone()),
        detail: action
            .path
            .clone()
            .or_else(|| action.query.clone())
            .or_else(|| action.command.clone()),
        path: action.path.clone(),
    }
}

fn universal_projection_tool_item(
    event: &ToolEvent,
    item_id: agenter_core::ItemId,
    turn_id: Option<agenter_core::TurnId>,
    status: ItemStatus,
) -> ItemState {
    let projection = universal_projection_tool_projection(event, status.clone());
    let title = projection.title.clone();
    ItemState {
        item_id,
        session_id: event.session_id,
        turn_id,
        role: ItemRole::Tool,
        status,
        content: vec![ContentBlock {
            block_id: format!("tool-{}", event.tool_call_id),
            kind: ContentBlockKind::ToolCall,
            text: Some(title),
            mime_type: None,
            artifact_id: None,
        }],
        tool: Some(projection),
        native: universal_projection_tool_native_ref(event),
    }
}

fn universal_projection_tool_projection(event: &ToolEvent, status: ItemStatus) -> ToolProjection {
    if let Some(subagent) = universal_projection_subagent_tool(event) {
        let title = universal_projection_subagent_title(&subagent);
        return ToolProjection {
            kind: ToolProjectionKind::Subagent,
            name: event.name.clone(),
            title,
            status,
            detail: None,
            input_summary: event.input.as_ref().map(compact_json_summary),
            output_summary: event.output.as_ref().map(compact_json_summary),
            command: None,
            subagent: Some(subagent),
            mcp: None,
        };
    }

    let mcp = universal_projection_mcp_tool(event);
    ToolProjection {
        kind: if mcp.is_some() {
            ToolProjectionKind::Mcp
        } else {
            ToolProjectionKind::Tool
        },
        name: event.name.clone(),
        title: event
            .title
            .clone()
            .unwrap_or_else(|| event.name.replace('_', " ")),
        status,
        detail: None,
        input_summary: event.input.as_ref().map(compact_json_summary),
        output_summary: event.output.as_ref().map(compact_json_summary),
        command: None,
        subagent: None,
        mcp,
    }
}

fn universal_projection_subagent_tool(event: &ToolEvent) -> Option<ToolSubagentProjection> {
    let item = event.provider_payload.as_ref()?.pointer("/params/item")?;
    if item.get("type")?.as_str()? != "collabAgentToolCall" {
        return None;
    }

    Some(ToolSubagentProjection {
        operation: match item.get("tool").and_then(Value::as_str) {
            Some("wait") => ToolSubagentOperation::Wait,
            Some("closeAgent") => ToolSubagentOperation::Close,
            _ => ToolSubagentOperation::Spawn,
        },
        agent_ids: string_array_at(item, "/receiverThreadIds"),
        model: item
            .get("model")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        reasoning_effort: item
            .get("reasoningEffort")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        prompt: item
            .get("prompt")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        states: subagent_states_from_item(item),
    })
}

fn universal_projection_subagent_title(subagent: &ToolSubagentProjection) -> String {
    match subagent.operation {
        ToolSubagentOperation::Wait => "Wait for subagent".to_owned(),
        ToolSubagentOperation::Close => "Close subagent".to_owned(),
        ToolSubagentOperation::Spawn => {
            if subagent.agent_ids.is_empty() {
                "Spawn subagent".to_owned()
            } else {
                format!("Spawned {} subagent(s)", subagent.agent_ids.len())
            }
        }
    }
}

fn subagent_states_from_item(item: &Value) -> Vec<ToolSubagentStateProjection> {
    let Some(states) = item.get("agentsStates").and_then(Value::as_object) else {
        return Vec::new();
    };

    states
        .iter()
        .map(|(agent_id, state)| ToolSubagentStateProjection {
            agent_id: agent_id.clone(),
            status: state
                .get("status")
                .and_then(Value::as_str)
                .unwrap_or("unknown")
                .to_owned(),
            message: state
                .get("message")
                .and_then(Value::as_str)
                .map(ToOwned::to_owned),
        })
        .collect()
}

fn universal_projection_mcp_tool(event: &ToolEvent) -> Option<ToolMcpProjection> {
    let item = event.provider_payload.as_ref()?.pointer("/params/item")?;
    if item.get("type").and_then(Value::as_str) != Some("mcpToolCall") {
        return None;
    }
    Some(ToolMcpProjection {
        server: item
            .get("server")
            .or_else(|| item.get("serverName"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned),
        tool: item
            .get("tool")
            .or_else(|| item.get("toolName"))
            .and_then(Value::as_str)
            .unwrap_or(&event.name)
            .to_owned(),
        arguments_summary: item.get("arguments").map(compact_json_summary),
        result_summary: item
            .get("result")
            .or_else(|| item.get("output"))
            .map(compact_json_summary),
    })
}

fn universal_projection_tool_native_ref(event: &ToolEvent) -> Option<NativeRef> {
    let payload = event.provider_payload.as_ref()?;
    Some(NativeRef {
        protocol: "codex.app_server".to_owned(),
        method: universal_projection_payload_method(Some(payload)).map(ToOwned::to_owned),
        kind: payload
            .pointer("/params/item/type")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| Some("tool".to_owned())),
        native_id: payload
            .pointer("/params/item/id")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .or_else(|| Some(event.tool_call_id.clone())),
        summary: event
            .title
            .clone()
            .or_else(|| Some(event.name.replace('_', " "))),
        hash: None,
        pointer: None,
    })
}

fn universal_projection_tool_completed_status(event: &ToolEvent) -> ItemStatus {
    match event
        .provider_payload
        .as_ref()
        .and_then(|payload| payload.pointer("/params/item/status"))
        .and_then(Value::as_str)
    {
        Some("failed") => ItemStatus::Failed,
        Some("inProgress") => ItemStatus::Streaming,
        _ => ItemStatus::Completed,
    }
}

fn string_array_at(value: &Value, pointer: &str) -> Vec<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .map(|values| {
            values
                .iter()
                .filter_map(Value::as_str)
                .map(ToOwned::to_owned)
                .collect()
        })
        .unwrap_or_default()
}

fn compact_json_summary(value: &Value) -> String {
    const MAX_SUMMARY_LEN: usize = 240;
    let mut summary = match value {
        Value::String(value) => value.clone(),
        other => other.to_string(),
    };
    if summary.len() > MAX_SUMMARY_LEN {
        summary.truncate(MAX_SUMMARY_LEN);
        summary.push_str("...");
    }
    summary
}

fn universal_projection_turn_state(
    session_id: SessionId,
    turn_id: agenter_core::TurnId,
    status: TurnStatus,
) -> agenter_core::TurnState {
    agenter_core::TurnState {
        turn_id,
        session_id,
        status,
        started_at: None,
        completed_at: None,
        model: None,
        mode: None,
    }
}

fn universal_projection_plan_status(plan: &agenter_core::PlanEvent) -> agenter_core::PlanStatus {
    plan.provider_payload
        .as_ref()
        .and_then(|payload| {
            string_at_value(
                payload,
                &[
                    "/params/status",
                    "/params/planStatus",
                    "/params/phase",
                    "/params/update/status",
                    "/params/update/planStatus",
                    "/params/update/phase",
                    "/params/update/state",
                ],
            )
        })
        .and_then(|status| match status {
            "none" => Some(agenter_core::PlanStatus::None),
            "discovering" | "planning" | "started" | "starting" => {
                Some(agenter_core::PlanStatus::Discovering)
            }
            "draft" | "updated" | "ready" => Some(agenter_core::PlanStatus::Draft),
            "awaiting_approval" | "awaitingApproval" | "approval_requested"
            | "approvalRequested" | "needs_approval" | "needsApproval" => {
                Some(agenter_core::PlanStatus::AwaitingApproval)
            }
            "revision_requested" | "revisionRequested" | "needs_revision" | "needsRevision" => {
                Some(agenter_core::PlanStatus::RevisionRequested)
            }
            "approved" | "accepted" => Some(agenter_core::PlanStatus::Approved),
            "implementing" | "implementation_started" | "implementationStarted" => {
                Some(agenter_core::PlanStatus::Implementing)
            }
            "completed" | "complete" | "done" => Some(agenter_core::PlanStatus::Completed),
            "cancelled" | "canceled" => Some(agenter_core::PlanStatus::Cancelled),
            "failed" | "error" => Some(agenter_core::PlanStatus::Failed),
            _ => None,
        })
        .unwrap_or(agenter_core::PlanStatus::Draft)
}

fn universal_projection_turn_id(value: &str) -> agenter_core::TurnId {
    agenter_core::TurnId::from_uuid(universal_projection_uuid(&format!("turn:{value}")))
}

fn universal_projection_item_id(value: &str) -> agenter_core::ItemId {
    agenter_core::ItemId::from_uuid(universal_projection_uuid(&format!("item:{value}")))
}

fn universal_projection_uuid(value: &str) -> Uuid {
    if let Ok(uuid) = Uuid::parse_str(value) {
        return uuid;
    }
    let digest = Sha256::digest(value.as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    Uuid::from_bytes(bytes)
}

fn universal_projection_turn_id_from_payload(
    payload: Option<&Value>,
) -> Option<agenter_core::TurnId> {
    let payload = payload?;
    string_at_value(
        payload,
        &[
            "/params/turn/id",
            "/params/turnId",
            "/params/item/turnId",
            "/params/item/turn/id",
            "/result/turn/id",
            "/result/turnId",
            "/turnId",
        ],
    )
    .map(universal_projection_turn_id)
}

fn universal_projection_payload_method(payload: Option<&Value>) -> Option<&str> {
    payload.and_then(|payload| payload.get("method"))?.as_str()
}

fn universal_projection_thread_title(payload: Option<&Value>) -> Option<String> {
    let payload = payload?;
    string_at_value(
        payload,
        &[
            "/params/threadName",
            "/params/thread_name",
            "/params/name",
            "/params/title",
        ],
    )
    .map(str::to_owned)
}

fn string_at_value<'a>(value: &'a Value, pointers: &[&str]) -> Option<&'a str> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
}

fn source_approval_request_event_from_universal(
    approval: &ApprovalRequest,
) -> agenter_core::ApprovalRequestEvent {
    agenter_core::ApprovalRequestEvent {
        session_id: approval.session_id,
        approval_id: approval.approval_id,
        kind: approval.kind.clone(),
        title: approval.title.clone(),
        details: approval.details.clone(),
        expires_at: None,
        presentation: None,
        resolution_state: Some(match approval.status {
            UniversalApprovalStatus::Resolving => ApprovalResolutionState::Resolving,
            _ => ApprovalResolutionState::Pending,
        }),
        resolving_decision: None,
        status: Some(approval.status.clone()),
        turn_id: approval.turn_id,
        item_id: approval.item_id,
        options: approval.options.clone(),
        risk: approval.risk.clone(),
        subject: approval.subject.clone(),
        native_request_id: approval.native_request_id.clone(),
        native_blocking: approval.native_blocking,
        policy: approval.policy.clone(),
        provider_payload: None,
    }
}

fn source_approval_request(
    request: &agenter_core::ApprovalRequestEvent,
    requested_at: DateTime<Utc>,
) -> ApprovalRequest {
    let native_id = safe_native_request_id(request.provider_payload.as_ref());
    ApprovalRequest {
        approval_id: request.approval_id,
        session_id: request.session_id,
        turn_id: None,
        item_id: None,
        kind: request.kind.clone(),
        title: request.title.clone(),
        details: request.details.clone(),
        options: vec![
            ApprovalOption::approve_once(),
            ApprovalOption::approve_always(),
            ApprovalOption::deny(),
            ApprovalOption::deny_with_feedback(),
            ApprovalOption::cancel_turn(),
        ],
        status: request
            .status
            .clone()
            .unwrap_or(match request.resolution_state {
                Some(ApprovalResolutionState::Resolving) => UniversalApprovalStatus::Resolving,
                Some(ApprovalResolutionState::Pending) | None => UniversalApprovalStatus::Pending,
            }),
        risk: request.risk.clone(),
        subject: request.subject.clone().or_else(|| request.details.clone()),
        native_request_id: request.native_request_id.clone().or(native_id.clone()),
        native_blocking: request.native_blocking,
        policy: request.policy.clone(),
        native: Some(NativeRef {
            protocol: "uap.universal_projection".to_owned(),
            method: Some("approval_requested".to_owned()),
            kind: Some(format!("{:?}", request.kind)),
            native_id,
            summary: Some(request.title.clone()),
            hash: None,
            pointer: None,
        }),
        requested_at: Some(requested_at),
        resolved_at: None,
    }
}

fn source_question_request(
    request: &agenter_core::QuestionRequestedEvent,
    requested_at: DateTime<Utc>,
) -> QuestionState {
    QuestionState {
        question_id: request.question_id,
        session_id: request.session_id,
        turn_id: None,
        title: request.title.clone(),
        description: request.description.clone(),
        fields: request.fields.clone(),
        status: QuestionStatus::Pending,
        answer: None,
        native: Some(NativeRef {
            protocol: "uap.universal_projection".to_owned(),
            method: Some("question_requested".to_owned()),
            kind: Some("universal_projection".to_owned()),
            native_id: safe_native_request_id(request.provider_payload.as_ref()),
            summary: Some(request.title.clone()),
            hash: None,
            pointer: None,
        }),
        requested_at: Some(requested_at),
        answered_at: None,
    }
}

fn source_answered_question(
    answered: &agenter_core::QuestionAnsweredEvent,
    answered_at: DateTime<Utc>,
) -> QuestionState {
    QuestionState {
        question_id: answered.question_id,
        session_id: answered.session_id,
        turn_id: None,
        title: "Input requested".to_owned(),
        description: None,
        fields: Vec::new(),
        status: QuestionStatus::Answered,
        answer: Some(answered.answer.clone()),
        native: Some(NativeRef {
            protocol: "uap.universal_projection".to_owned(),
            method: Some("question_answered".to_owned()),
            kind: Some("universal_projection".to_owned()),
            native_id: safe_native_request_id(answered.provider_payload.as_ref()),
            summary: Some("Question answered".to_owned()),
            hash: None,
            pointer: None,
        }),
        requested_at: None,
        answered_at: Some(answered_at),
    }
}

fn approval_decision_universal_status(decision: &ApprovalDecision) -> UniversalApprovalStatus {
    match decision {
        ApprovalDecision::Accept | ApprovalDecision::AcceptForSession => {
            UniversalApprovalStatus::Approved
        }
        ApprovalDecision::Cancel => UniversalApprovalStatus::Cancelled,
        ApprovalDecision::Decline => UniversalApprovalStatus::Denied,
        ApprovalDecision::ProviderSpecific { payload } => {
            let status = payload
                .pointer("/decision")
                .or_else(|| payload.pointer("/status"))
                .or_else(|| payload.pointer("/kind"))
                .and_then(Value::as_str);
            match status {
                Some("accept" | "approve" | "approved" | "allow" | "allowed") => {
                    UniversalApprovalStatus::Approved
                }
                Some("cancel" | "cancelled" | "canceled") => UniversalApprovalStatus::Cancelled,
                _ => UniversalApprovalStatus::Denied,
            }
        }
    }
}

fn safe_native_request_id(payload: Option<&Value>) -> Option<String> {
    let payload = payload?;
    [
        "/native_id",
        "/request_id",
        "/approval_id",
        "/params/id",
        "/params/requestId",
    ]
    .iter()
    .find_map(|pointer| payload.pointer(pointer)?.as_str().map(str::to_owned))
}

fn discovered_history_events(
    session_id: SessionId,
    owner_user_id: UserId,
    history: &[DiscoveredSessionHistoryItem],
) -> Vec<NormalizedEvent> {
    history
        .iter()
        .flat_map(|item| match item {
            DiscoveredSessionHistoryItem::UserMessage {
                message_id,
                content,
            } => vec![NormalizedEvent::UserMessage(
                agenter_core::UserMessageEvent {
                    session_id,
                    message_id: message_id.clone(),
                    author_user_id: Some(owner_user_id),
                    content: content.clone(),
                },
            )],
            DiscoveredSessionHistoryItem::AgentMessage {
                message_id,
                content,
            } => vec![NormalizedEvent::AgentMessageCompleted(
                agenter_core::MessageCompletedEvent {
                    session_id,
                    message_id: message_id.clone(),
                    content: Some(content.clone()),
                    provider_payload: None,
                },
            )],
            DiscoveredSessionHistoryItem::Plan {
                plan_id,
                title,
                content,
                provider_payload,
            } => vec![NormalizedEvent::PlanUpdated(agenter_core::PlanEvent {
                session_id,
                plan_id: Some(plan_id.clone()),
                title: title.clone(),
                content: Some(content.clone()),
                entries: Vec::new(),
                append: false,
                provider_payload: provider_payload.clone(),
            })],
            DiscoveredSessionHistoryItem::Tool {
                tool_call_id,
                name,
                title,
                status,
                input,
                output,
                provider_payload,
            } => {
                let event = agenter_core::ToolEvent {
                    session_id,
                    tool_call_id: tool_call_id.clone(),
                    name: name.clone(),
                    title: title.clone(),
                    input: input.clone(),
                    output: output.clone(),
                    provider_payload: provider_payload.clone(),
                };
                match status {
                    DiscoveredToolStatus::Completed | DiscoveredToolStatus::Failed => {
                        vec![NormalizedEvent::ToolCompleted(event)]
                    }
                    DiscoveredToolStatus::Running => vec![NormalizedEvent::ToolStarted(event)],
                }
            }
            DiscoveredSessionHistoryItem::Command {
                command_id,
                command,
                cwd,
                source,
                process_id,
                duration_ms,
                actions,
                output,
                exit_code,
                success,
                provider_payload,
            } => {
                let mut events = vec![NormalizedEvent::CommandStarted(
                    agenter_core::CommandEvent {
                        session_id,
                        command_id: command_id.clone(),
                        command: command.clone(),
                        cwd: cwd.clone(),
                        source: source.clone(),
                        process_id: process_id.clone(),
                        actions: actions
                            .iter()
                            .map(|action| CommandAction {
                                kind: action.kind.clone(),
                                command: action.command.clone(),
                                path: action.path.clone(),
                                name: action.name.clone(),
                                query: action.query.clone(),
                                provider_payload: action.provider_payload.clone(),
                            })
                            .collect(),
                        provider_payload: provider_payload.clone(),
                    },
                )];
                if let Some(output) = output {
                    if !output.is_empty() {
                        events.push(NormalizedEvent::CommandOutputDelta(
                            agenter_core::CommandOutputEvent {
                                session_id,
                                command_id: command_id.clone(),
                                stream: CommandOutputStream::Stdout,
                                delta: output.clone(),
                                provider_payload: provider_payload.clone(),
                            },
                        ));
                    }
                }
                events.push(NormalizedEvent::CommandCompleted(
                    agenter_core::CommandCompletedEvent {
                        session_id,
                        command_id: command_id.clone(),
                        exit_code: *exit_code,
                        duration_ms: *duration_ms,
                        success: *success,
                        provider_payload: provider_payload.clone(),
                    },
                ));
                events
            }
            DiscoveredSessionHistoryItem::FileChange {
                path,
                change_kind,
                status,
                diff,
                provider_payload,
                ..
            } => {
                let event = agenter_core::FileChangeEvent {
                    session_id,
                    path: path.clone(),
                    change_kind: change_kind.clone(),
                    diff: diff.clone(),
                    provider_payload: provider_payload.clone(),
                };
                match status {
                    DiscoveredFileChangeStatus::Applied => {
                        vec![NormalizedEvent::FileChangeApplied(event)]
                    }
                    DiscoveredFileChangeStatus::Rejected => {
                        vec![NormalizedEvent::FileChangeRejected(event)]
                    }
                    DiscoveredFileChangeStatus::Proposed => {
                        vec![NormalizedEvent::FileChangeProposed(event)]
                    }
                }
            }
            DiscoveredSessionHistoryItem::NativeNotification {
                event_id,
                category,
                title,
                detail,
                status,
                provider_payload,
            } => vec![NormalizedEvent::NativeNotification(
                agenter_core::NativeNotification {
                    session_id,
                    provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                    method: category.clone(),
                    event_id: event_id.clone(),
                    category: category.clone(),
                    title: title.clone(),
                    detail: detail.clone(),
                    status: status.clone(),
                    provider_payload: provider_payload.clone(),
                },
            )],
        })
        .collect()
}

fn normalized_event_name(event: &NormalizedEvent) -> &'static str {
    match event {
        NormalizedEvent::SessionStarted(_) => "session_started",
        NormalizedEvent::SessionStatusChanged(_) => "session_status_changed",
        NormalizedEvent::UserMessage(_) => "user_message",
        NormalizedEvent::AgentMessageDelta(_) => "agent_message_delta",
        NormalizedEvent::AgentMessageCompleted(_) => "agent_message_completed",
        NormalizedEvent::PlanUpdated(_) => "plan_updated",
        NormalizedEvent::ToolStarted(_) => "tool_started",
        NormalizedEvent::ToolUpdated(_) => "tool_updated",
        NormalizedEvent::ToolCompleted(_) => "tool_completed",
        NormalizedEvent::CommandStarted(_) => "command_started",
        NormalizedEvent::CommandOutputDelta(_) => "command_output_delta",
        NormalizedEvent::CommandCompleted(_) => "command_completed",
        NormalizedEvent::FileChangeProposed(_) => "file_change_proposed",
        NormalizedEvent::FileChangeApplied(_) => "file_change_applied",
        NormalizedEvent::FileChangeRejected(_) => "file_change_rejected",
        NormalizedEvent::ApprovalRequested(_) => "approval_requested",
        NormalizedEvent::ApprovalResolved(_) => "approval_resolved",
        NormalizedEvent::QuestionRequested(_) => "question_requested",
        NormalizedEvent::QuestionAnswered(_) => "question_answered",
        NormalizedEvent::TurnStatusChanged(_) => "turn_status_changed",
        NormalizedEvent::TurnFailed(_) => "turn_failed",
        NormalizedEvent::TurnCancelled(_) => "turn_cancelled",
        NormalizedEvent::TurnInterrupted(_) => "turn_interrupted",
        NormalizedEvent::TurnDiffUpdated(_) => "turn_diff_updated",
        NormalizedEvent::ItemReasoning(_) => "item_reasoning",
        NormalizedEvent::ServerRequestResolved(_) => "server_request_resolved",
        NormalizedEvent::McpToolCallProgress(_) => "mcp_tool_call_progress",
        NormalizedEvent::ThreadRealtimeEvent(_) => "thread_realtime_event",
        NormalizedEvent::NativeNotification(_) => "native_notification",
        NormalizedEvent::Error(_) => "error",
    }
}

fn universal_command_fingerprint(envelope: &UniversalCommandEnvelope) -> Value {
    serde_json::json!({
        "session_id": envelope.session_id,
        "turn_id": envelope.turn_id,
        "command": envelope.command,
    })
}

fn command_start_from_record(
    record: &agenter_db::models::CommandIdempotencyRecord,
    inserted: bool,
    command_json: &Value,
) -> UniversalCommandStart {
    let existing_command = record
        .response_json
        .as_ref()
        .and_then(|value| value.get("command"))
        .cloned()
        .unwrap_or(Value::Null);
    if existing_command != *command_json {
        return UniversalCommandStart::Conflict(UniversalCommandConflict {
            code: "idempotency_conflict".to_owned(),
            message: "idempotency key was already used for a different command".to_owned(),
        });
    }
    if inserted {
        return UniversalCommandStart::Started;
    }
    UniversalCommandStart::Duplicate {
        status: match record.status {
            agenter_db::models::CommandIdempotencyStatus::Pending => {
                UniversalCommandIdempotencyStatus::Pending
            }
            agenter_db::models::CommandIdempotencyStatus::Succeeded => {
                UniversalCommandIdempotencyStatus::Succeeded
            }
            agenter_db::models::CommandIdempotencyStatus::Failed
            | agenter_db::models::CommandIdempotencyStatus::Conflict => {
                UniversalCommandIdempotencyStatus::Failed
            }
        },
        response: record
            .response_json
            .as_ref()
            .and_then(|value| value.get("response"))
            .cloned()
            .filter(|value| !value.is_null())
            .and_then(|value| serde_json::from_value(value).ok()),
    }
}

impl SessionEvents {
    fn new() -> Self {
        let (sender, _) = broadcast::channel(SESSION_EVENT_CACHE_LIMIT);
        Self {
            sender,
            cache: Vec::new(),
            universal_cache: Vec::new(),
            snapshot: SessionSnapshot::default(),
            next_seq: 0,
        }
    }
}

fn session_info(session: &RegisteredSession) -> SessionInfo {
    let mut usage = session.usage.clone();
    if usage.is_some() || session.turn_settings.is_some() {
        let usage_ref = usage.get_or_insert_with(SessionUsageSnapshot::default);
        apply_turn_settings_to_usage(usage_ref, session.turn_settings.as_ref());
    }
    SessionInfo {
        session_id: session.session_id,
        owner_user_id: session.owner_user_id,
        runner_id: session.runner_id,
        workspace_id: session.workspace.workspace_id,
        provider_id: session.provider_id.clone(),
        status: session.status.clone(),
        external_session_id: session.external_session_id.clone(),
        title: session.title.clone(),
        created_at: Some(session.created_at),
        updated_at: Some(session.updated_at),
        usage: usage.map(Box::new),
    }
}

fn db_session_info(session: &agenter_db::models::AgentSession) -> SessionInfo {
    let mut usage = session.usage_snapshot.clone();
    if usage.is_some() || session.turn_settings.is_some() {
        let usage_ref = usage.get_or_insert_with(SessionUsageSnapshot::default);
        apply_turn_settings_to_usage(usage_ref, session.turn_settings.as_ref());
    }
    SessionInfo {
        session_id: session.session_id,
        owner_user_id: session.owner_user_id,
        runner_id: session.runner_id,
        workspace_id: session.workspace_id,
        provider_id: session.provider_id.clone(),
        status: session.status.clone(),
        external_session_id: session.external_session_id.clone(),
        title: session.title.clone(),
        created_at: Some(session.created_at),
        updated_at: Some(session.updated_at),
        usage: usage.map(Box::new),
    }
}

fn merge_provider_capabilities(target: &mut CapabilitySet, provider: CapabilitySet) {
    for detail in provider.provider_details {
        if !target
            .provider_details
            .iter()
            .any(|existing| existing.key == detail.key)
        {
            target.provider_details.push(detail);
        }
    }
}

fn apply_turn_settings_to_usage(
    usage: &mut SessionUsageSnapshot,
    settings: Option<&AgentTurnSettings>,
) {
    let Some(settings) = settings else {
        return;
    };
    if let Some(mode) = &settings.collaboration_mode {
        usage.mode_label = Some(mode.clone());
    }
    if let Some(model) = &settings.model {
        usage.model = Some(model.clone());
    }
    if let Some(reasoning_effort) = &settings.reasoning_effort {
        usage.reasoning_effort = Some(reasoning_effort.clone());
    }
}

fn merge_usage_snapshot(target: &mut SessionUsageSnapshot, update: SessionUsageSnapshot) {
    if update.mode_label.is_some() {
        target.mode_label = update.mode_label;
    }
    if update.model.is_some() {
        target.model = update.model;
    }
    if update.reasoning_effort.is_some() {
        target.reasoning_effort = update.reasoning_effort;
    }
    if update.context.is_some() {
        target.context = update.context;
    }
    if update.window_5h.is_some() {
        target.window_5h = update.window_5h;
    }
    if update.week.is_some() {
        target.week = update.week;
    }
}

fn usage_snapshot_from_native_notification(
    event: &NativeNotification,
) -> Option<SessionUsageSnapshot> {
    match event.category.as_str() {
        "token_usage" => usage_snapshot_from_token_usage(event.provider_payload.as_ref()),
        "rate_limits" => usage_snapshot_from_rate_limits(event.provider_payload.as_ref()),
        _ => None,
    }
}

fn provider_notification_from_native(event: &NativeNotification) -> ProviderNotification {
    ProviderNotification {
        category: event.category.clone(),
        title: event.title.clone(),
        detail: event.detail.clone(),
        status: event.status.clone(),
        severity: provider_notification_severity(event),
        subject: event.event_id.clone(),
    }
}

fn provider_notification_severity(
    event: &NativeNotification,
) -> Option<ProviderNotificationSeverity> {
    match event.status.as_deref().or(Some(event.category.as_str())) {
        Some("failed" | "error") => Some(ProviderNotificationSeverity::Error),
        Some("warning" | "guardian" | "config_warning") => {
            Some(ProviderNotificationSeverity::Warning)
        }
        Some("debug") => Some(ProviderNotificationSeverity::Debug),
        _ => Some(ProviderNotificationSeverity::Info),
    }
}

fn usage_snapshot_from_token_usage(payload: Option<&Value>) -> Option<SessionUsageSnapshot> {
    let payload = payload?;
    let used_tokens = integer_at(
        payload,
        &[
            "/params/tokenUsage/last/totalTokens",
            "/params/tokenUsage/current/totalTokens",
            "/params/tokenUsage/total/totalTokens",
            "/params/summary/tokenUsage/total/totalTokens",
            "/params/summary/estimatedTotalTokens",
        ],
    );
    let total_tokens = integer_at(
        payload,
        &[
            "/params/tokenUsage/modelContextWindow",
            "/params/modelContextWindow",
            "/params/summary/tokenUsage/modelContextWindow",
        ],
    );
    let used_percent = match (used_tokens, total_tokens) {
        (Some(used), Some(total)) if total > 0 => Some(((used * 100) / total).min(100)),
        _ => None,
    };
    (used_tokens.is_some() || total_tokens.is_some() || used_percent.is_some()).then(|| {
        SessionUsageSnapshot {
            context: Some(SessionUsageContext {
                used_percent,
                used_tokens,
                total_tokens,
            }),
            ..SessionUsageSnapshot::default()
        }
    })
}

fn usage_snapshot_from_rate_limits(payload: Option<&Value>) -> Option<SessionUsageSnapshot> {
    let payload = payload?;
    let window_5h = usage_window_at(payload, "/params/rateLimits/primary", Some("5h".to_owned()));
    let week = usage_window_at(
        payload,
        "/params/rateLimits/secondary",
        Some("weekly".to_owned()),
    );
    (window_5h.is_some() || week.is_some()).then(|| SessionUsageSnapshot {
        window_5h,
        week,
        ..SessionUsageSnapshot::default()
    })
}

fn usage_window_at(
    payload: &Value,
    pointer: &str,
    window_label: Option<String>,
) -> Option<SessionUsageWindow> {
    let value = payload.pointer(pointer)?;
    let used_percent =
        integer_at(value, &["/usedPercent", "/used_percent"]).map(|value| value.min(100));
    let remaining_percent = used_percent
        .map(|value| 100_u64.saturating_sub(value))
        .or_else(|| {
            integer_at(value, &["/remainingPercent", "/remaining_percent"])
                .map(|value| value.min(100))
        });
    let resets_at = integer_at(value, &["/resetsAt", "/resets_at"])
        .and_then(|timestamp| DateTime::from_timestamp(timestamp as i64, 0));
    (used_percent.is_some() || remaining_percent.is_some() || resets_at.is_some()).then_some({
        SessionUsageWindow {
            used_percent,
            remaining_percent,
            resets_at,
            window_label,
            remaining_text_hint: None,
        }
    })
}

fn discovered_session_timestamp(value: Option<&str>) -> Option<DateTime<Utc>> {
    let value = value?;

    DateTime::parse_from_rfc3339(value)
        .map(|value| value.with_timezone(&Utc))
        .ok()
        .or_else(|| {
            value.trim().parse::<i64>().ok().and_then(|value| {
                if value.abs() > 10_000_000_000_000 {
                    let secs = value / 1000;
                    let nanos = ((value % 1000).unsigned_abs() as u32) * 1_000_000;
                    DateTime::from_timestamp(secs, nanos)
                } else {
                    DateTime::from_timestamp(value, 0)
                }
            })
        })
        .or_else(|| {
            value
                .trim()
                .parse::<f64>()
                .ok()
                .map(|seconds| seconds.round() as i64)
                .and_then(|seconds| DateTime::from_timestamp(seconds, 0))
        })
}

fn registered_session_from_db(
    session: &agenter_db::models::AgentSession,
    workspace: &WorkspaceRef,
) -> RegisteredSession {
    RegisteredSession {
        session_id: session.session_id,
        owner_user_id: session.owner_user_id,
        runner_id: session.runner_id,
        workspace: workspace.clone(),
        provider_id: session.provider_id.clone(),
        status: session.status.clone(),
        title: session.title.clone(),
        external_session_id: session.external_session_id.clone(),
        turn_settings: session.turn_settings.clone(),
        usage: session.usage_snapshot.clone(),
        created_at: session.created_at,
        updated_at: session.updated_at,
    }
}

fn discovered_history_fingerprint(
    status: &DiscoveredSessionHistoryStatus,
    history: &[DiscoveredSessionHistoryItem],
) -> Option<String> {
    if !matches!(status, DiscoveredSessionHistoryStatus::Loaded) {
        return None;
    }
    let value = serde_json::to_vec(history).ok()?;
    Some(format!("{:x}", Sha256::digest(value)))
}

fn workspace_refresh_status_from_runner(
    status: &RunnerOperationStatus,
) -> WorkspaceSessionRefreshStatus {
    match status {
        RunnerOperationStatus::Queued => WorkspaceSessionRefreshStatus::Queued,
        RunnerOperationStatus::Accepted => WorkspaceSessionRefreshStatus::Accepted,
        RunnerOperationStatus::Discovering => WorkspaceSessionRefreshStatus::Discovering,
        RunnerOperationStatus::ReadingHistory => WorkspaceSessionRefreshStatus::ReadingHistory,
        RunnerOperationStatus::SendingResults => WorkspaceSessionRefreshStatus::SendingResults,
        RunnerOperationStatus::Importing => WorkspaceSessionRefreshStatus::Importing,
        RunnerOperationStatus::Succeeded => WorkspaceSessionRefreshStatus::Succeeded,
        RunnerOperationStatus::Failed => WorkspaceSessionRefreshStatus::Failed,
        RunnerOperationStatus::Cancelled => WorkspaceSessionRefreshStatus::Cancelled,
    }
}

fn workspace_refresh_log_level_from_runner(
    level: &RunnerOperationLogLevel,
) -> WorkspaceSessionRefreshLogLevel {
    match level {
        RunnerOperationLogLevel::Debug => WorkspaceSessionRefreshLogLevel::Debug,
        RunnerOperationLogLevel::Info => WorkspaceSessionRefreshLogLevel::Info,
        RunnerOperationLogLevel::Warning => WorkspaceSessionRefreshLogLevel::Warning,
        RunnerOperationLogLevel::Error => WorkspaceSessionRefreshLogLevel::Error,
    }
}

fn integer_at(value: &Value, pointers: &[&str]) -> Option<u64> {
    pointers.iter().find_map(|pointer| {
        let value = value.pointer(pointer)?;
        value
            .as_u64()
            .or_else(|| value.as_i64().and_then(|value| u64::try_from(value).ok()))
            .or_else(|| value.as_f64().map(|value| value.round() as u64))
    })
}

fn sort_session_infos(sessions: &mut [SessionInfo]) {
    sessions.sort_by(|left, right| {
        right
            .updated_at
            .or(right.created_at)
            .cmp(&left.updated_at.or(left.created_at))
            .then_with(|| {
                left.session_id
                    .to_string()
                    .cmp(&right.session_id.to_string())
            })
    });
}

#[cfg(test)]
mod tests {
    use agenter_core::{
        AgentCapabilities, AgentMessageDeltaEvent, AgentProviderId, ApprovalDecision, ApprovalId,
        ApprovalKind, ApprovalRequestEvent, ApprovalStatus as UniversalApprovalStatus,
        CommandCompletedEvent, CommandEvent, CommandOutputEvent, ContentBlockKind, ItemId,
        ItemRole, ItemState, ItemStatus, MessageCompletedEvent, NormalizedEvent,
        ProviderCapabilityDetail, ProviderCapabilityStatus, RunnerId, SessionId, SessionInfo,
        TurnId, TurnState, TurnStatus, UniversalEventEnvelope, UniversalEventKind,
        UniversalEventSource, UniversalSeq, UserId, UserMessageEvent, WorkspaceId, WorkspaceRef,
    };
    use agenter_protocol::runner::{
        AgentProviderAdvertisement, RunnerCapabilities, RunnerHeartbeatAck, RunnerServerMessage,
    };

    use super::*;

    #[test]
    fn universal_reducer_reconstructs_snapshot_from_ordered_events() {
        let session_id = SessionId::new();
        let runner_id = RunnerId::new();
        let workspace_id = WorkspaceId::new();
        let owner_user_id = UserId::new();
        let turn_id = TurnId::new();
        let item_id = ItemId::new();
        let ts = Utc::now();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };

        let events = [
            UniversalEventEnvelope {
                event_id: Uuid::new_v4().to_string(),
                seq: UniversalSeq::new(1),
                session_id,
                turn_id: None,
                item_id: None,
                ts,
                source: UniversalEventSource::ControlPlane,
                native: None,
                event: UniversalEventKind::SessionCreated {
                    session: Box::new(SessionInfo {
                        session_id,
                        owner_user_id,
                        runner_id,
                        workspace_id,
                        provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                        status: SessionStatus::Running,
                        external_session_id: Some("thread-1".to_owned()),
                        title: Some("Universal session".to_owned()),
                        created_at: Some(ts),
                        updated_at: Some(ts),
                        usage: None,
                    }),
                },
            },
            UniversalEventEnvelope {
                event_id: Uuid::new_v4().to_string(),
                seq: UniversalSeq::new(2),
                session_id,
                turn_id: Some(turn_id),
                item_id: None,
                ts,
                source: UniversalEventSource::Runner,
                native: None,
                event: UniversalEventKind::TurnStarted {
                    turn: TurnState {
                        turn_id,
                        session_id,
                        status: TurnStatus::Running,
                        started_at: Some(ts),
                        completed_at: None,
                        model: Some("gpt-5".to_owned()),
                        mode: None,
                    },
                },
            },
            UniversalEventEnvelope {
                event_id: Uuid::new_v4().to_string(),
                seq: UniversalSeq::new(3),
                session_id,
                turn_id: Some(turn_id),
                item_id: Some(item_id),
                ts,
                source: UniversalEventSource::Runner,
                native: None,
                event: UniversalEventKind::ItemCreated {
                    item: Box::new(ItemState {
                        item_id,
                        session_id,
                        turn_id: Some(turn_id),
                        role: ItemRole::Assistant,
                        status: ItemStatus::Created,
                        content: Vec::new(),
                        tool: None,
                        native: None,
                    }),
                },
            },
            UniversalEventEnvelope {
                event_id: Uuid::new_v4().to_string(),
                seq: UniversalSeq::new(4),
                session_id,
                turn_id: Some(turn_id),
                item_id: Some(item_id),
                ts,
                source: UniversalEventSource::Runner,
                native: None,
                event: UniversalEventKind::ContentDelta {
                    block_id: "text-1".to_owned(),
                    kind: None,
                    delta: "hello".to_owned(),
                },
            },
            UniversalEventEnvelope {
                event_id: Uuid::new_v4().to_string(),
                seq: UniversalSeq::new(5),
                session_id,
                turn_id: Some(turn_id),
                item_id: Some(item_id),
                ts,
                source: UniversalEventSource::Runner,
                native: None,
                event: UniversalEventKind::ContentDelta {
                    block_id: "text-1".to_owned(),
                    kind: None,
                    delta: " world".to_owned(),
                },
            },
        ];

        for event in &events {
            apply_universal_event_to_snapshot(&mut snapshot, event);
        }

        assert_eq!(snapshot.latest_seq, Some(UniversalSeq::new(5)));
        assert_eq!(
            snapshot
                .info
                .as_ref()
                .and_then(|info| info.title.as_deref()),
            Some("Universal session")
        );
        assert_eq!(snapshot.active_turns, vec![turn_id]);
        let item = snapshot.items.get(&item_id).expect("item");
        assert_eq!(item.status, ItemStatus::Streaming);
        assert_eq!(item.content[0].kind, ContentBlockKind::Text);
        assert_eq!(item.content[0].text.as_deref(), Some("hello world"));
    }

    #[test]
    fn universal_reducer_materializes_universal_projection_message_content_in_snapshot() {
        let session_id = SessionId::new();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };
        let delta = universal_projection_universal_event(
            session_id,
            "event-1".to_owned(),
            &NormalizedEvent::AgentMessageDelta(AgentMessageDeltaEvent {
                session_id,
                message_id: "msg-1".to_owned(),
                delta: "hello ".to_owned(),
                provider_payload: Some(serde_json::json!({
                    "method": "agentMessage/delta",
                    "params": {"turnId": "turn-1"}
                })),
            }),
        );
        let completed = universal_projection_universal_event(
            session_id,
            "event-2".to_owned(),
            &NormalizedEvent::AgentMessageCompleted(MessageCompletedEvent {
                session_id,
                message_id: "msg-1".to_owned(),
                content: Some("hello world".to_owned()),
                provider_payload: Some(serde_json::json!({
                    "method": "agentMessage/completed",
                    "params": {"turnId": "turn-1"}
                })),
            }),
        );

        apply_universal_event_to_snapshot(&mut snapshot, &delta);
        apply_universal_event_to_snapshot(&mut snapshot, &completed);

        assert_eq!(snapshot.items.len(), 1);
        let item = snapshot.items.values().next().expect("item");
        assert_eq!(item.status, ItemStatus::Completed);
        assert_eq!(item.content[0].kind, ContentBlockKind::Text);
        assert_eq!(item.content[0].text.as_deref(), Some("hello world"));
    }

    #[test]
    fn universal_reducer_preserves_command_invocation_when_status_completes() {
        let session_id = SessionId::new();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };
        let events = [
            universal_projection_universal_event(
                session_id,
                "event-1".to_owned(),
                &NormalizedEvent::CommandStarted(CommandEvent {
                    session_id,
                    command_id: "cmd-1".to_owned(),
                    command: "cargo test".to_owned(),
                    cwd: None,
                    source: None,
                    process_id: None,
                    actions: Vec::new(),
                    provider_payload: Some(serde_json::json!({
                        "method": "item/started",
                        "params": {"turnId": "turn-1", "itemId": "cmd-1"}
                    })),
                }),
            ),
            universal_projection_universal_event(
                session_id,
                "event-2".to_owned(),
                &NormalizedEvent::CommandOutputDelta(CommandOutputEvent {
                    session_id,
                    command_id: "cmd-1".to_owned(),
                    stream: CommandOutputStream::Stdout,
                    delta: "running tests\n".to_owned(),
                    provider_payload: Some(serde_json::json!({
                        "method": "item/commandExecution/outputDelta",
                        "params": {"turnId": "turn-1", "itemId": "cmd-1"}
                    })),
                }),
            ),
            universal_projection_universal_event(
                session_id,
                "event-3".to_owned(),
                &NormalizedEvent::CommandCompleted(CommandCompletedEvent {
                    session_id,
                    command_id: "cmd-1".to_owned(),
                    exit_code: Some(0),
                    duration_ms: None,
                    success: true,
                    provider_payload: Some(serde_json::json!({
                        "method": "item/completed",
                        "params": {"turnId": "turn-1", "item": {"id": "cmd-1"}}
                    })),
                }),
            ),
        ];

        for event in events {
            apply_universal_event_to_snapshot(&mut snapshot, &event);
        }

        let item = snapshot.items.values().next().expect("command item");
        assert_eq!(item.status, ItemStatus::Completed);
        let tool = item
            .tool
            .as_ref()
            .expect("universal_projection command projection");
        assert_eq!(tool.kind, ToolProjectionKind::Command);
        assert_eq!(tool.name, "command");
        assert_eq!(tool.title, "cargo test");
        let invocation = item
            .content
            .iter()
            .find(|block| block.block_id == "command-cmd-1")
            .expect("invocation block");
        assert_eq!(invocation.kind, ContentBlockKind::ToolCall);
        assert_eq!(invocation.text.as_deref(), Some("cargo test"));
        let stdout = item
            .content
            .iter()
            .find(|block| block.block_id == "command-cmd-1-stdout")
            .expect("stdout block");
        assert_eq!(stdout.kind, ContentBlockKind::CommandOutput);
        assert_eq!(stdout.text.as_deref(), Some("running tests\n"));
        let status = item
            .content
            .iter()
            .find(|block| block.block_id == "command-cmd-1-status")
            .expect("status block");
        assert_eq!(status.kind, ContentBlockKind::CommandOutput);
        assert_eq!(status.text.as_deref(), Some("command completed"));
    }

    #[test]
    fn universal_projection_materializes_codex_collab_agent_tool_calls() {
        let session_id = SessionId::new();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };
        let events = [
            universal_projection_universal_event(
                session_id,
                "event-1".to_owned(),
                &NormalizedEvent::ToolStarted(ToolEvent {
                    session_id,
                    tool_call_id: "spawn-call-1".to_owned(),
                    name: "collabAgentToolCall".to_owned(),
                    title: None,
                    input: None,
                    output: None,
                    provider_payload: Some(serde_json::json!({
                        "method": "item/started",
                        "params": {
                            "turnId": "turn-1",
                            "item": {
                                "type": "collabAgentToolCall",
                                "id": "spawn-call-1",
                                "tool": "spawnAgent",
                                "status": "inProgress",
                                "senderThreadId": "parent-thread",
                                "receiverThreadIds": [],
                                "prompt": "Inspect auth flow",
                                "model": "gpt-5.3-codex",
                                "reasoningEffort": "high",
                                "agentsStates": {}
                            }
                        }
                    })),
                }),
            ),
            universal_projection_universal_event(
                session_id,
                "event-2".to_owned(),
                &NormalizedEvent::ToolCompleted(ToolEvent {
                    session_id,
                    tool_call_id: "spawn-call-1".to_owned(),
                    name: "collabAgentToolCall".to_owned(),
                    title: None,
                    input: None,
                    output: None,
                    provider_payload: Some(serde_json::json!({
                        "method": "item/completed",
                        "params": {
                            "turnId": "turn-1",
                            "item": {
                                "type": "collabAgentToolCall",
                                "id": "spawn-call-1",
                                "tool": "spawnAgent",
                                "status": "completed",
                                "senderThreadId": "parent-thread",
                                "receiverThreadIds": ["child-thread"],
                                "prompt": "Inspect auth flow",
                                "model": "gpt-5.3-codex",
                                "reasoningEffort": "high",
                                "agentsStates": {
                                    "child-thread": {
                                        "status": "running",
                                        "message": null
                                    }
                                }
                            }
                        }
                    })),
                }),
            ),
        ];

        for event in events {
            apply_universal_event_to_snapshot(&mut snapshot, &event);
        }

        let item = snapshot.items.values().next().expect("subagent item");
        assert_eq!(item.status, ItemStatus::Completed);
        assert_eq!(
            item.native
                .as_ref()
                .and_then(|native| native.kind.as_deref()),
            Some("collabAgentToolCall")
        );
        let tool = item.tool.as_ref().expect("subagent tool projection");
        assert_eq!(tool.kind, ToolProjectionKind::Subagent);
        assert_eq!(tool.status, ItemStatus::Completed);
        assert_eq!(tool.title, "Spawned 1 subagent(s)");
        let subagent = tool.subagent.as_ref().expect("subagent details");
        assert_eq!(subagent.operation, ToolSubagentOperation::Spawn);
        assert_eq!(subagent.agent_ids, vec!["child-thread"]);
        assert_eq!(subagent.model.as_deref(), Some("gpt-5.3-codex"));
        assert_eq!(subagent.reasoning_effort.as_deref(), Some("high"));
        assert_eq!(subagent.prompt.as_deref(), Some("Inspect auth flow"));
        assert_eq!(subagent.states[0].agent_id, "child-thread");
        assert_eq!(subagent.states[0].status, "running");
    }

    #[test]
    fn universal_plan_reducer_appends_partial_deltas_and_materializes_plan_item() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let plan_id = agenter_core::PlanId::new();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };
        let full = UniversalEventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            seq: UniversalSeq::new(1),
            session_id,
            turn_id: Some(turn_id),
            item_id: None,
            ts: Utc::now(),
            source: UniversalEventSource::Runner,
            native: None,
            event: UniversalEventKind::PlanUpdated {
                plan: agenter_core::PlanState {
                    plan_id,
                    session_id,
                    turn_id: Some(turn_id),
                    status: agenter_core::PlanStatus::Draft,
                    title: Some("Implementation plan".to_owned()),
                    content: Some("Base plan".to_owned()),
                    entries: vec![agenter_core::UniversalPlanEntry {
                        entry_id: "entry-0".to_owned(),
                        label: "Inspect".to_owned(),
                        status: agenter_core::PlanEntryStatus::Pending,
                    }],
                    artifact_refs: Vec::new(),
                    source: agenter_core::PlanSource::NativeStructured,
                    partial: false,
                    updated_at: None,
                },
            },
        };
        let partial = UniversalEventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            seq: UniversalSeq::new(2),
            session_id,
            turn_id: Some(turn_id),
            item_id: None,
            ts: Utc::now(),
            source: UniversalEventSource::Runner,
            native: None,
            event: UniversalEventKind::PlanUpdated {
                plan: agenter_core::PlanState {
                    plan_id,
                    session_id,
                    turn_id: Some(turn_id),
                    status: agenter_core::PlanStatus::AwaitingApproval,
                    title: None,
                    content: Some("\nAwait approval".to_owned()),
                    entries: vec![agenter_core::UniversalPlanEntry {
                        entry_id: "entry-1".to_owned(),
                        label: "Implement".to_owned(),
                        status: agenter_core::PlanEntryStatus::Pending,
                    }],
                    artifact_refs: Vec::new(),
                    source: agenter_core::PlanSource::NativeStructured,
                    partial: true,
                    updated_at: None,
                },
            },
        };

        apply_universal_event_to_snapshot(&mut snapshot, &full);
        apply_universal_event_to_snapshot(&mut snapshot, &partial);

        let plan = snapshot.plans.get(&plan_id).expect("plan");
        assert_eq!(plan.status, agenter_core::PlanStatus::AwaitingApproval);
        assert_eq!(plan.content.as_deref(), Some("Base plan\nAwait approval"));
        assert_eq!(plan.entries.len(), 2);
        assert!(snapshot.items.values().any(|item| {
            item.content.iter().any(|block| {
                block
                    .text
                    .as_deref()
                    .is_some_and(|text| text.contains("Base plan") && text.contains("Implement"))
            })
        }));
    }

    #[test]
    fn universal_plan_reducer_merges_partial_entries_by_id() {
        let session_id = SessionId::new();
        let plan_id = agenter_core::PlanId::new();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };
        for (seq, status, partial) in [
            (1, agenter_core::PlanEntryStatus::Pending, false),
            (2, agenter_core::PlanEntryStatus::Completed, true),
        ] {
            apply_universal_event_to_snapshot(
                &mut snapshot,
                &UniversalEventEnvelope {
                    event_id: Uuid::new_v4().to_string(),
                    seq: UniversalSeq::new(seq),
                    session_id,
                    turn_id: None,
                    item_id: None,
                    ts: Utc::now(),
                    source: UniversalEventSource::Runner,
                    native: None,
                    event: UniversalEventKind::PlanUpdated {
                        plan: agenter_core::PlanState {
                            plan_id,
                            session_id,
                            turn_id: None,
                            status: agenter_core::PlanStatus::Draft,
                            title: None,
                            content: None,
                            entries: vec![agenter_core::UniversalPlanEntry {
                                entry_id: "entry-0".to_owned(),
                                label: "Inspect".to_owned(),
                                status,
                            }],
                            artifact_refs: Vec::new(),
                            source: agenter_core::PlanSource::NativeStructured,
                            partial,
                            updated_at: None,
                        },
                    },
                },
            );
        }

        let plan = snapshot.plans.get(&plan_id).expect("plan");
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].entry_id, "entry-0");
        assert_eq!(
            plan.entries[0].status,
            agenter_core::PlanEntryStatus::Completed
        );
    }

    #[test]
    fn universal_plan_reducer_clears_materialized_item_for_empty_full_replace() {
        let session_id = SessionId::new();
        let plan_id = agenter_core::PlanId::new();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };
        apply_universal_event_to_snapshot(
            &mut snapshot,
            &UniversalEventEnvelope {
                event_id: Uuid::new_v4().to_string(),
                seq: UniversalSeq::new(1),
                session_id,
                turn_id: None,
                item_id: None,
                ts: Utc::now(),
                source: UniversalEventSource::Runner,
                native: None,
                event: UniversalEventKind::PlanUpdated {
                    plan: agenter_core::PlanState {
                        plan_id,
                        session_id,
                        turn_id: None,
                        status: agenter_core::PlanStatus::Draft,
                        title: Some("Plan".to_owned()),
                        content: Some("Visible stale content".to_owned()),
                        entries: Vec::new(),
                        artifact_refs: Vec::new(),
                        source: agenter_core::PlanSource::NativeStructured,
                        partial: false,
                        updated_at: None,
                    },
                },
            },
        );
        assert_eq!(snapshot.items.len(), 1);

        apply_universal_event_to_snapshot(
            &mut snapshot,
            &UniversalEventEnvelope {
                event_id: Uuid::new_v4().to_string(),
                seq: UniversalSeq::new(2),
                session_id,
                turn_id: None,
                item_id: None,
                ts: Utc::now(),
                source: UniversalEventSource::Runner,
                native: None,
                event: UniversalEventKind::PlanUpdated {
                    plan: agenter_core::PlanState {
                        plan_id,
                        session_id,
                        turn_id: None,
                        status: agenter_core::PlanStatus::Completed,
                        title: None,
                        content: None,
                        entries: Vec::new(),
                        artifact_refs: Vec::new(),
                        source: agenter_core::PlanSource::NativeStructured,
                        partial: false,
                        updated_at: None,
                    },
                },
            },
        );

        assert!(snapshot.items.is_empty());
        assert_eq!(
            snapshot.plans.get(&plan_id).map(|plan| &plan.status),
            Some(&agenter_core::PlanStatus::Completed)
        );
    }

    #[test]
    fn universal_plan_reducer_keeps_implementing_item_streaming() {
        let session_id = SessionId::new();
        let plan_id = agenter_core::PlanId::new();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };
        apply_universal_event_to_snapshot(
            &mut snapshot,
            &UniversalEventEnvelope {
                event_id: Uuid::new_v4().to_string(),
                seq: UniversalSeq::new(1),
                session_id,
                turn_id: None,
                item_id: None,
                ts: Utc::now(),
                source: UniversalEventSource::Runner,
                native: None,
                event: UniversalEventKind::PlanUpdated {
                    plan: agenter_core::PlanState {
                        plan_id,
                        session_id,
                        turn_id: None,
                        status: agenter_core::PlanStatus::Implementing,
                        title: None,
                        content: Some("Implementing".to_owned()),
                        entries: Vec::new(),
                        artifact_refs: Vec::new(),
                        source: agenter_core::PlanSource::NativeStructured,
                        partial: false,
                        updated_at: None,
                    },
                },
            },
        );

        assert_eq!(
            snapshot.items.values().next().map(|item| &item.status),
            Some(&ItemStatus::Streaming)
        );
    }

    #[test]
    fn universal_plan_reducer_replaces_complete_acp_updates() {
        let session_id = SessionId::new();
        let plan_id = agenter_core::PlanId::new();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };
        for (seq, label) in [(1, "Old"), (2, "New")] {
            apply_universal_event_to_snapshot(
                &mut snapshot,
                &UniversalEventEnvelope {
                    event_id: Uuid::new_v4().to_string(),
                    seq: UniversalSeq::new(seq),
                    session_id,
                    turn_id: None,
                    item_id: None,
                    ts: Utc::now(),
                    source: UniversalEventSource::Runner,
                    native: None,
                    event: UniversalEventKind::PlanUpdated {
                        plan: agenter_core::PlanState {
                            plan_id,
                            session_id,
                            turn_id: None,
                            status: agenter_core::PlanStatus::Draft,
                            title: None,
                            content: None,
                            entries: vec![agenter_core::UniversalPlanEntry {
                                entry_id: format!("entry-{seq}"),
                                label: label.to_owned(),
                                status: agenter_core::PlanEntryStatus::Pending,
                            }],
                            artifact_refs: Vec::new(),
                            source: agenter_core::PlanSource::NativeStructured,
                            partial: false,
                            updated_at: None,
                        },
                    },
                },
            );
        }

        let plan = snapshot.plans.get(&plan_id).expect("plan");
        assert_eq!(plan.entries.len(), 1);
        assert_eq!(plan.entries[0].label, "New");
    }

    #[test]
    fn universal_plan_reducer_tracks_lifecycle_statuses() {
        let session_id = SessionId::new();
        let plan_id = agenter_core::PlanId::new();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };
        for (index, status) in [
            agenter_core::PlanStatus::AwaitingApproval,
            agenter_core::PlanStatus::RevisionRequested,
            agenter_core::PlanStatus::Approved,
            agenter_core::PlanStatus::Implementing,
            agenter_core::PlanStatus::Completed,
            agenter_core::PlanStatus::Cancelled,
            agenter_core::PlanStatus::Failed,
        ]
        .into_iter()
        .enumerate()
        {
            apply_universal_event_to_snapshot(
                &mut snapshot,
                &UniversalEventEnvelope {
                    event_id: Uuid::new_v4().to_string(),
                    seq: UniversalSeq::new((index + 1) as i64),
                    session_id,
                    turn_id: None,
                    item_id: None,
                    ts: Utc::now(),
                    source: UniversalEventSource::Runner,
                    native: None,
                    event: UniversalEventKind::PlanUpdated {
                        plan: agenter_core::PlanState {
                            plan_id,
                            session_id,
                            turn_id: None,
                            status: status.clone(),
                            title: None,
                            content: None,
                            entries: Vec::new(),
                            artifact_refs: Vec::new(),
                            source: agenter_core::PlanSource::NativeStructured,
                            partial: false,
                            updated_at: None,
                        },
                    },
                },
            );
            assert_eq!(
                snapshot.plans.get(&plan_id).map(|plan| &plan.status),
                Some(&status)
            );
        }
    }

    #[test]
    fn universal_reducer_materializes_pending_approval_in_snapshot() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let approval_id = ApprovalId::new();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };
        let turn = UniversalEventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            seq: UniversalSeq::new(1),
            session_id,
            turn_id: Some(turn_id),
            item_id: None,
            ts: Utc::now(),
            source: UniversalEventSource::Runner,
            native: None,
            event: UniversalEventKind::TurnStarted {
                turn: TurnState {
                    turn_id,
                    session_id,
                    status: TurnStatus::Running,
                    started_at: Some(Utc::now()),
                    completed_at: None,
                    model: None,
                    mode: None,
                },
            },
        };
        let approval = UniversalEventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            seq: UniversalSeq::new(2),
            session_id,
            turn_id: Some(turn_id),
            item_id: None,
            ts: Utc::now(),
            source: UniversalEventSource::Runner,
            native: None,
            event: UniversalEventKind::ApprovalRequested {
                approval: Box::new(ApprovalRequest {
                    approval_id,
                    session_id,
                    turn_id: Some(turn_id),
                    item_id: None,
                    kind: ApprovalKind::Command,
                    title: "Run command".to_owned(),
                    details: Some("cargo test".to_owned()),
                    options: Vec::new(),
                    status: UniversalApprovalStatus::Pending,
                    risk: Some("writes".to_owned()),
                    subject: Some("cargo test".to_owned()),
                    native_request_id: Some("native-approval-1".to_owned()),
                    native_blocking: true,
                    policy: None,
                    native: None,
                    requested_at: Some(Utc::now()),
                    resolved_at: None,
                }),
            },
        };

        apply_universal_event_to_snapshot(&mut snapshot, &turn);
        apply_universal_event_to_snapshot(&mut snapshot, &approval);

        assert_eq!(snapshot.latest_seq, Some(UniversalSeq::new(2)));
        assert_eq!(
            snapshot.turns.get(&turn_id).map(|turn| &turn.status),
            Some(&TurnStatus::WaitingForApproval)
        );
        assert_eq!(
            snapshot
                .approvals
                .get(&approval_id)
                .map(|approval| approval.title.as_str()),
            Some("Run command")
        );
    }

    #[test]
    fn universal_snapshot_reducer_resolves_approval_event_without_losing_request_details() {
        let session_id = SessionId::new();
        let approval_id = ApprovalId::new();
        let resolved_at = Utc::now();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };
        let requested = universal_projection_universal_event(
            session_id,
            Uuid::new_v4().to_string(),
            &NormalizedEvent::ApprovalRequested(ApprovalRequestEvent {
                session_id,
                approval_id,
                kind: ApprovalKind::Command,
                title: "Run command".to_owned(),
                details: Some("cargo test".to_owned()),
                expires_at: None,
                presentation: None,
                resolution_state: None,
                resolving_decision: None,
                status: None,
                turn_id: None,
                item_id: None,
                options: Vec::new(),
                risk: None,
                subject: None,
                native_request_id: None,
                native_blocking: true,
                policy: None,
                provider_payload: Some(serde_json::json!({ "request_id": "native-1" })),
            }),
        );
        let resolved = universal_projection_universal_event(
            session_id,
            Uuid::new_v4().to_string(),
            &NormalizedEvent::ApprovalResolved(agenter_core::ApprovalResolvedEvent {
                session_id,
                approval_id,
                decision: ApprovalDecision::Accept,
                resolved_by_user_id: Some(UserId::new()),
                resolved_at,
                provider_payload: Some(serde_json::json!({ "request_id": "native-1" })),
            }),
        );

        apply_universal_event_to_snapshot(&mut snapshot, &requested);
        apply_universal_event_to_snapshot(&mut snapshot, &resolved);

        let approval = snapshot.approvals.get(&approval_id).expect("approval");
        assert_eq!(approval.title, "Run command");
        assert_eq!(approval.kind, ApprovalKind::Command);
        assert_eq!(approval.status, UniversalApprovalStatus::Approved);
        assert_eq!(approval.resolved_at, Some(resolved_at));
        assert_eq!(
            approval
                .native
                .as_ref()
                .and_then(|native| native.native_id.as_deref()),
            Some("native-1")
        );
    }

    #[test]
    fn universal_projection_projects_approval_resolved_as_lifecycle_update() {
        let session_id = SessionId::new();
        let approval_id = ApprovalId::new();
        let resolved_at = Utc::now();

        let universal = universal_projection_universal_event(
            session_id,
            Uuid::new_v4().to_string(),
            &NormalizedEvent::ApprovalResolved(agenter_core::ApprovalResolvedEvent {
                session_id,
                approval_id,
                decision: ApprovalDecision::Accept,
                resolved_by_user_id: Some(UserId::new()),
                resolved_at,
                provider_payload: Some(serde_json::json!({ "request_id": "native-1" })),
            }),
        );

        let UniversalEventKind::ApprovalResolved {
            approval_id: projected_id,
            status,
            resolved_at: projected_resolved_at,
            ..
        } = universal.event
        else {
            panic!("approval resolution should be a lifecycle update");
        };

        assert_eq!(projected_id, approval_id);
        assert_eq!(status, UniversalApprovalStatus::Approved);
        assert_eq!(projected_resolved_at, resolved_at);
    }

    #[test]
    fn universal_snapshot_reducer_ignores_orphan_approval_resolution_state() {
        let session_id = SessionId::new();
        let approval_id = ApprovalId::new();
        let resolved_at = Utc::now();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };
        let resolved = universal_projection_universal_event(
            session_id,
            Uuid::new_v4().to_string(),
            &NormalizedEvent::ApprovalResolved(agenter_core::ApprovalResolvedEvent {
                session_id,
                approval_id,
                decision: ApprovalDecision::Accept,
                resolved_by_user_id: Some(UserId::new()),
                resolved_at,
                provider_payload: Some(serde_json::json!({ "request_id": "native-1" })),
            }),
        );

        apply_universal_event_to_snapshot(&mut snapshot, &resolved);

        assert!(
            !snapshot.approvals.contains_key(&approval_id),
            "orphan approval resolution events should remain events, not transcript approval rows"
        );
    }

    #[test]
    fn universal_projection_projection_does_not_persist_raw_approval_payload() {
        let session_id = SessionId::new();
        let approval_id = ApprovalId::new();
        let event = NormalizedEvent::ApprovalRequested(ApprovalRequestEvent {
            session_id,
            approval_id,
            kind: ApprovalKind::Command,
            title: "Run shell".to_owned(),
            details: Some("echo ok".to_owned()),
            expires_at: None,
            presentation: None,
            resolution_state: None,
            resolving_decision: None,
            status: None,
            turn_id: None,
            item_id: None,
            options: Vec::new(),
            risk: None,
            subject: None,
            native_request_id: None,
            native_blocking: true,
            policy: None,
            provider_payload: Some(serde_json::json!({
                "request_id": "native-approval-1",
                "raw_secret": "must-not-copy"
            })),
        });

        let universal =
            universal_projection_universal_event(session_id, Uuid::new_v4().to_string(), &event);
        let UniversalEventKind::ApprovalRequested { approval } = universal.event else {
            panic!("approval event should materialize");
        };

        assert_eq!(
            approval
                .native
                .as_ref()
                .and_then(|native| native.native_id.as_deref()),
            Some("native-approval-1")
        );
        let approval_json = serde_json::to_value(&approval).expect("approval json");
        assert!(
            !approval_json.to_string().contains("must-not-copy"),
            "universal_projection universal approval must keep only safe native references"
        );
    }

    #[tokio::test]
    async fn subscribers_receive_published_session_events() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let session_id = SessionId::new();
        let mut subscription = state.subscribe_session(session_id, None, false).await;
        assert!(subscription.cached_events.is_empty());

        state
            .publish_event(
                session_id,
                NormalizedEvent::UserMessage(UserMessageEvent {
                    session_id,
                    message_id: Some("user-1".to_owned()),
                    author_user_id: None,
                    content: "hello".to_owned(),
                }),
            )
            .await;

        let received = subscription
            .receiver
            .recv()
            .await
            .expect("event is broadcast");
        let received = received.normalized_event;
        assert!(matches!(received.event, NormalizedEvent::UserMessage(_)));
        assert!(received.event_id.is_some());
    }

    #[tokio::test]
    async fn session_snapshot_hydrates_provider_capabilities_from_runner_hello() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let runner_id = RunnerId::new();
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: "/tmp/agenter-workspace".to_owned(),
            display_name: Some("agenter-workspace".to_owned()),
        };
        let provider_detail = ProviderCapabilityDetail {
            key: "dynamic_tools".to_owned(),
            status: ProviderCapabilityStatus::Degraded,
            methods: vec!["item/tool/call".to_owned()],
            reason: Some("Visible but not executed remotely.".to_owned()),
        };
        state
            .register_runner(
                runner_id,
                RunnerCapabilities {
                    agent_providers: vec![AgentProviderAdvertisement {
                        provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                        capabilities: AgentCapabilities {
                            approvals: true,
                            provider_details: vec![provider_detail.clone()],
                            ..AgentCapabilities::default()
                        },
                    }],
                    transports: vec!["codex-app-server".to_owned()],
                    workspace_discovery: false,
                },
                vec![workspace.clone()],
            )
            .await;
        let session_id = SessionId::new();
        state
            .create_session(
                session_id,
                UserId::new(),
                runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        {
            let mut sessions = state.inner.sessions.lock().await;
            let events = sessions
                .entry(session_id)
                .or_insert_with(SessionEvents::new);
            events.snapshot.session_id = session_id;
            events.snapshot.capabilities.protocol.snapshots = true;
            events.snapshot.capabilities.protocol.after_seq_replay = true;
        }

        let subscription = state.subscribe_session(session_id, None, true).await;
        let snapshot = subscription.snapshot.expect("session snapshot").snapshot;

        assert!(snapshot.capabilities.protocol.snapshots);
        assert!(snapshot.capabilities.protocol.after_seq_replay);
        assert!(!snapshot.capabilities.approvals.enabled);
        assert_eq!(
            snapshot.capabilities.provider_details,
            vec![provider_detail]
        );
    }

    #[tokio::test]
    async fn provider_usage_events_update_session_snapshot() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let owner_user_id = state
            .inner
            .bootstrap_admin
            .as_ref()
            .expect("bootstrap admin")
            .user
            .user_id;
        let runner_id = RunnerId::new();
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: "/work/agenter".to_owned(),
            display_name: Some("agenter".to_owned()),
        };
        let session = state
            .register_session(SessionRegistration {
                session_id: SessionId::new(),
                owner_user_id,
                runner_id,
                workspace,
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                title: None,
                external_session_id: None,
                turn_settings: Some(AgentTurnSettings {
                    model: Some("gpt-5.4".to_owned()),
                    reasoning_effort: Some(agenter_core::AgentReasoningEffort::High),
                    collaboration_mode: Some("plan".to_owned()),
                }),
                usage: None,
            })
            .await;

        state
            .publish_event(
                session.session_id,
                NormalizedEvent::NativeNotification(agenter_core::NativeNotification {
                    session_id: session.session_id,
                    provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                    event_id: None,
                    method: "thread/tokenUsage/updated".to_owned(),
                    category: "token_usage".to_owned(),
                    title: "Token usage updated".to_owned(),
                    detail: None,
                    status: Some("updated".to_owned()),
                    provider_payload: Some(serde_json::json!({
                        "params": {
                            "tokenUsage": {
                                "last": { "totalTokens": 42000 },
                                "modelContextWindow": 258000
                            }
                        }
                    })),
                }),
            )
            .await;
        state
            .publish_event(
                session.session_id,
                NormalizedEvent::NativeNotification(agenter_core::NativeNotification {
                    session_id: session.session_id,
                    provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                    event_id: None,
                    method: "account/rateLimits/updated".to_owned(),
                    category: "rate_limits".to_owned(),
                    title: "Rate limits updated".to_owned(),
                    detail: None,
                    status: Some("updated".to_owned()),
                    provider_payload: Some(serde_json::json!({
                        "params": {
                            "rateLimits": {
                                "primary": { "usedPercent": 58, "resetsAt": 1777640533 },
                                "secondary": { "usedPercent": 20, "resetsAt": 1777968663 }
                            }
                        }
                    })),
                }),
            )
            .await;

        let info = state
            .session(owner_user_id, session.session_id)
            .await
            .expect("session")
            .info();
        let usage = info.usage.expect("usage snapshot");
        assert_eq!(usage.mode_label.as_deref(), Some("plan"));
        assert_eq!(usage.model.as_deref(), Some("gpt-5.4"));
        assert_eq!(
            usage.context.and_then(|context| context.used_percent),
            Some(16)
        );
        assert_eq!(
            usage.window_5h.and_then(|window| window.remaining_percent),
            Some(42)
        );
        assert_eq!(
            usage.week.and_then(|window| window.remaining_percent),
            Some(80)
        );
    }

    #[tokio::test]
    async fn session_status_event_updates_registered_session_status() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let owner_user_id = state
            .inner
            .bootstrap_admin
            .as_ref()
            .expect("bootstrap admin")
            .user
            .user_id;
        let runner_id = RunnerId::new();
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: "/work/agenter".to_owned(),
            display_name: Some("agenter".to_owned()),
        };
        let session = state
            .create_session(
                SessionId::new(),
                owner_user_id,
                runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;

        state
            .publish_event(
                session.session_id,
                NormalizedEvent::SessionStatusChanged(agenter_core::SessionStatusChangedEvent {
                    session_id: session.session_id,
                    status: SessionStatus::WaitingForApproval,
                    reason: Some("waiting".to_owned()),
                }),
            )
            .await;

        let info = state
            .session(owner_user_id, session.session_id)
            .await
            .expect("session")
            .info();
        assert_eq!(info.status, SessionStatus::WaitingForApproval);
    }

    #[tokio::test]
    async fn stopped_session_orphans_pending_approval() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let owner_user_id = state
            .inner
            .bootstrap_admin
            .as_ref()
            .expect("bootstrap admin")
            .user
            .user_id;
        let runner_id = RunnerId::new();
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: "/work/agenter".to_owned(),
            display_name: Some("agenter".to_owned()),
        };
        let session = state
            .create_session(
                SessionId::new(),
                owner_user_id,
                runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let approval_id = ApprovalId::new();
        state
            .publish_event(
                session.session_id,
                NormalizedEvent::ApprovalRequested(ApprovalRequestEvent {
                    session_id: session.session_id,
                    approval_id,
                    kind: ApprovalKind::Command,
                    title: "Run command".to_owned(),
                    details: Some("cargo test".to_owned()),
                    expires_at: None,
                    presentation: None,
                    resolution_state: None,
                    resolving_decision: None,
                    status: None,
                    turn_id: None,
                    item_id: None,
                    options: Vec::new(),
                    risk: None,
                    subject: None,
                    native_request_id: None,
                    native_blocking: true,
                    policy: None,
                    provider_payload: None,
                }),
            )
            .await;

        state
            .publish_event(
                session.session_id,
                NormalizedEvent::SessionStatusChanged(agenter_core::SessionStatusChangedEvent {
                    session_id: session.session_id,
                    status: SessionStatus::Stopped,
                    reason: Some("provider exited".to_owned()),
                }),
            )
            .await;

        let history = state
            .session_history(owner_user_id, session.session_id)
            .await
            .expect("history");
        let orphaned = history
            .iter()
            .filter_map(|envelope| match &envelope.event {
                NormalizedEvent::ApprovalRequested(request)
                    if request.approval_id == approval_id =>
                {
                    request.status.clone()
                }
                _ => None,
            })
            .any(|status| status == UniversalApprovalStatus::Orphaned);
        assert!(orphaned, "stopped provider should orphan pending approvals");
    }

    #[tokio::test]
    async fn stopped_session_orphans_pending_question_in_universal_snapshot() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let owner_user_id = state
            .inner
            .bootstrap_admin
            .as_ref()
            .expect("bootstrap admin")
            .user
            .user_id;
        let runner_id = RunnerId::new();
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: "/work/agenter".to_owned(),
            display_name: Some("agenter".to_owned()),
        };
        let session = state
            .create_session(
                SessionId::new(),
                owner_user_id,
                runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let question_id = QuestionId::new();
        state
            .publish_event(
                session.session_id,
                NormalizedEvent::QuestionRequested(agenter_core::QuestionRequestedEvent {
                    session_id: session.session_id,
                    question_id,
                    title: "Pick target".to_owned(),
                    description: Some("Need a deployment target".to_owned()),
                    fields: Vec::new(),
                    provider_payload: None,
                }),
            )
            .await;

        state
            .publish_event(
                session.session_id,
                NormalizedEvent::SessionStatusChanged(agenter_core::SessionStatusChangedEvent {
                    session_id: session.session_id,
                    status: SessionStatus::Stopped,
                    reason: Some("provider exited".to_owned()),
                }),
            )
            .await;

        let snapshot = state
            .inner
            .sessions
            .lock()
            .await
            .get(&session.session_id)
            .expect("session events")
            .snapshot
            .clone();
        assert_eq!(
            snapshot
                .questions
                .get(&question_id)
                .map(|question| &question.status),
            Some(&QuestionStatus::Orphaned),
            "stopped provider should orphan pending questions for reload snapshots"
        );
        assert_eq!(state.question_session(question_id).await, None);
    }

    #[tokio::test]
    async fn runner_disconnect_leaves_sessions_live_until_runner_evidence() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let owner_user_id = state
            .inner
            .bootstrap_admin
            .as_ref()
            .expect("bootstrap admin")
            .user
            .user_id;
        let runner_id = RunnerId::new();
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: "/work/agenter".to_owned(),
            display_name: Some("agenter".to_owned()),
        };
        let active = state
            .create_session(
                SessionId::new(),
                owner_user_id,
                runner_id,
                workspace.clone(),
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let failed = state
            .create_session(
                SessionId::new(),
                owner_user_id,
                runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        state
            .publish_event(
                active.session_id,
                NormalizedEvent::SessionStatusChanged(agenter_core::SessionStatusChangedEvent {
                    session_id: active.session_id,
                    status: SessionStatus::Running,
                    reason: None,
                }),
            )
            .await;
        state
            .publish_event(
                failed.session_id,
                NormalizedEvent::SessionStatusChanged(agenter_core::SessionStatusChangedEvent {
                    session_id: failed.session_id,
                    status: SessionStatus::Failed,
                    reason: None,
                }),
            )
            .await;
        let (sender, _receiver) = tokio::sync::mpsc::unbounded_channel();
        let connection_id = state.connect_runner(runner_id, sender).await;

        state.disconnect_runner(runner_id, connection_id).await;

        let active_status = state
            .session(owner_user_id, active.session_id)
            .await
            .expect("active session")
            .status;
        let failed_status = state
            .session(owner_user_id, failed.session_id)
            .await
            .expect("failed session")
            .status;
        assert_eq!(active_status, SessionStatus::Running);
        assert_eq!(failed_status, SessionStatus::Failed);
    }

    #[tokio::test]
    async fn session_history_reinjects_evicted_pending_approval() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("bootstrap");
        let owner = state.inner.bootstrap_admin.as_ref().unwrap().user.user_id;
        let runner_id = RunnerId::new();
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: "/tmp/agenter-test".to_owned(),
            display_name: None,
        };
        let session = state
            .create_session(
                SessionId::new(),
                owner,
                runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let sid = session.session_id;
        let approval_id = ApprovalId::new();
        state
            .publish_event(
                sid,
                NormalizedEvent::ApprovalRequested(ApprovalRequestEvent {
                    session_id: sid,
                    approval_id,
                    kind: ApprovalKind::FileChange,
                    title: "Approve change".to_owned(),
                    details: Some("files".to_owned()),
                    expires_at: None,
                    presentation: None,
                    resolution_state: None,
                    resolving_decision: None,
                    status: None,
                    turn_id: None,
                    item_id: None,
                    options: Vec::new(),
                    risk: None,
                    subject: None,
                    native_request_id: None,
                    native_blocking: true,
                    policy: None,
                    provider_payload: None,
                }),
            )
            .await;

        for i in 0..super::SESSION_EVENT_CACHE_LIMIT {
            state
                .publish_event(
                    sid,
                    NormalizedEvent::UserMessage(UserMessageEvent {
                        session_id: sid,
                        message_id: Some(format!("filler-{i}")),
                        author_user_id: None,
                        content: "x".to_owned(),
                    }),
                )
                .await;
        }

        let cache_len = state
            .inner
            .sessions
            .lock()
            .await
            .get(&sid)
            .map(|e| e.cache.len())
            .unwrap_or(0);
        assert_eq!(cache_len, super::SESSION_EVENT_CACHE_LIMIT);
        assert!(
            !state.inner.sessions.lock().await[&sid]
                .cache
                .iter()
                .any(|e| matches!(e.event, NormalizedEvent::ApprovalRequested(_))),
            "oldest approval_requested should be evicted from ring buffer"
        );

        let history = state.session_history(owner, sid).await.expect("history");
        let found = history.iter().find_map(|env| match &env.event {
            NormalizedEvent::ApprovalRequested(req) if req.approval_id == approval_id => Some(()),
            _ => None,
        });
        assert!(
            found.is_some(),
            "history should merge pending approval from registry"
        );

        let pending = state.pending_approval_request_envelopes(sid).await;
        assert_eq!(pending.len(), 1);
        assert!(matches!(
            pending[0].event,
            NormalizedEvent::ApprovalRequested(_)
        ));
    }

    #[tokio::test]
    async fn resolving_approval_replay_carries_in_flight_decision() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("bootstrap");
        let owner = state.inner.bootstrap_admin.as_ref().unwrap().user.user_id;
        let runner_id = RunnerId::new();
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: "/tmp/agenter-test".to_owned(),
            display_name: None,
        };
        let session = state
            .create_session(
                SessionId::new(),
                owner,
                runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let sid = session.session_id;
        let approval_id = ApprovalId::new();
        state
            .publish_event(
                sid,
                NormalizedEvent::ApprovalRequested(ApprovalRequestEvent {
                    session_id: sid,
                    approval_id,
                    kind: ApprovalKind::FileChange,
                    title: "Approve change".to_owned(),
                    details: Some("files".to_owned()),
                    expires_at: None,
                    presentation: None,
                    resolution_state: None,
                    resolving_decision: None,
                    status: None,
                    turn_id: None,
                    item_id: None,
                    options: Vec::new(),
                    risk: None,
                    subject: None,
                    native_request_id: None,
                    native_blocking: true,
                    policy: None,
                    provider_payload: None,
                }),
            )
            .await;

        let started = state
            .begin_approval_resolution(approval_id, ApprovalDecision::Accept)
            .await;
        assert!(matches!(started, ApprovalResolutionStart::Started));

        let history = state.session_history(owner, sid).await.expect("history");
        let approval = history
            .iter()
            .find_map(|env| match &env.event {
                NormalizedEvent::ApprovalRequested(req) if req.approval_id == approval_id => {
                    Some(req)
                }
                _ => None,
            })
            .expect("replayed approval");
        let approval_json = serde_json::to_value(approval).expect("approval json");
        assert_eq!(approval_json["resolution_state"], "resolving");
        assert_eq!(approval_json["resolving_decision"]["decision"], "accept");

        let pending = state.pending_approval_request_envelopes(sid).await;
        let pending_json = serde_json::to_value(&pending[0].event).expect("pending json");
        assert_eq!(pending_json["payload"]["resolution_state"], "resolving");
    }

    #[tokio::test]
    async fn subscribe_snapshots_cached_events_and_live_receiver_atomically() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let session_id = SessionId::new();
        let event = NormalizedEvent::UserMessage(UserMessageEvent {
            session_id,
            message_id: Some("cached".to_owned()),
            author_user_id: None,
            content: "cached".to_owned(),
        });
        state.publish_event(session_id, event).await;

        let mut subscription = state.subscribe_session(session_id, None, false).await;
        assert_eq!(subscription.cached_events.len(), 1);
        assert!(matches!(
            subscription.cached_events[0].event,
            NormalizedEvent::UserMessage(_)
        ));

        state
            .publish_event(
                session_id,
                NormalizedEvent::UserMessage(UserMessageEvent {
                    session_id,
                    message_id: Some("live".to_owned()),
                    author_user_id: None,
                    content: "live".to_owned(),
                }),
            )
            .await;
        assert!(matches!(
            subscription
                .receiver
                .recv()
                .await
                .expect("live event")
                .normalized_event
                .event,
            NormalizedEvent::UserMessage(_)
        ));
    }

    #[tokio::test]
    async fn subscribe_snapshot_replays_universal_events_after_source_cache_miss() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let session_id = SessionId::new();
        state
            .publish_event(
                session_id,
                NormalizedEvent::UserMessage(UserMessageEvent {
                    session_id,
                    message_id: Some("first".to_owned()),
                    author_user_id: None,
                    content: "first".to_owned(),
                }),
            )
            .await;
        for i in 0..SESSION_EVENT_CACHE_LIMIT {
            state
                .publish_event(
                    session_id,
                    NormalizedEvent::UserMessage(UserMessageEvent {
                        session_id,
                        message_id: Some(format!("filler-{i}")),
                        author_user_id: None,
                        content: "x".to_owned(),
                    }),
                )
                .await;
        }

        let subscription = state
            .subscribe_session(session_id, Some(UniversalSeq::zero()), true)
            .await;
        assert_eq!(subscription.cached_events.len(), SESSION_EVENT_CACHE_LIMIT);
        assert!(!subscription.cached_events.iter().any(|event| {
            matches!(
                &event.event,
                NormalizedEvent::UserMessage(message) if message.message_id.as_deref() == Some("first")
            )
        }));
        let snapshot = subscription.snapshot.expect("snapshot replay");
        assert_eq!(
            snapshot.latest_seq,
            Some(UniversalSeq::new((SESSION_EVENT_CACHE_LIMIT + 1) as i64))
        );
        assert_eq!(snapshot.events.len(), SESSION_EVENT_CACHE_LIMIT + 1);
        assert_eq!(snapshot.events[0].seq, UniversalSeq::new(1));
    }

    #[tokio::test]
    async fn subscribe_snapshot_replays_after_seq_in_strict_order() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let session_id = SessionId::new();
        for message_id in ["first", "second", "third"] {
            state
                .publish_event(
                    session_id,
                    NormalizedEvent::UserMessage(UserMessageEvent {
                        session_id,
                        message_id: Some(message_id.to_owned()),
                        author_user_id: None,
                        content: message_id.to_owned(),
                    }),
                )
                .await;
        }

        let subscription = state
            .subscribe_session(session_id, Some(UniversalSeq::new(1)), true)
            .await;
        let snapshot = subscription.snapshot.expect("snapshot replay");

        assert_eq!(snapshot.latest_seq, Some(UniversalSeq::new(3)));
        assert_eq!(
            snapshot
                .events
                .iter()
                .map(|event| event.seq)
                .collect::<Vec<_>>(),
            vec![UniversalSeq::new(2), UniversalSeq::new(3)]
        );
        assert_eq!(snapshot.snapshot.latest_seq, Some(UniversalSeq::new(3)));
    }

    #[tokio::test]
    async fn subscribe_snapshot_materializes_user_messages_from_universal_projection() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let session_id = SessionId::new();

        state
            .publish_event(
                session_id,
                NormalizedEvent::UserMessage(UserMessageEvent {
                    session_id,
                    message_id: Some("user-1".to_owned()),
                    author_user_id: None,
                    content: "original prompt".to_owned(),
                }),
            )
            .await;

        let subscription = state.subscribe_session(session_id, None, true).await;
        let snapshot = subscription.snapshot.expect("snapshot replay").snapshot;

        let user_items = snapshot
            .items
            .values()
            .filter(|item| item.role == ItemRole::User)
            .collect::<Vec<_>>();
        assert_eq!(user_items.len(), 1);
        assert_eq!(user_items[0].status, ItemStatus::Completed);
        assert_eq!(
            user_items[0].content[0].text.as_deref(),
            Some("original prompt")
        );
    }

    #[tokio::test]
    async fn subscribe_snapshot_marks_universal_replay_has_more_when_bounded() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let session_id = SessionId::new();
        for i in 0..(UNIVERSAL_EVENT_REPLAY_LIMIT + 2) {
            state
                .publish_event(
                    session_id,
                    NormalizedEvent::UserMessage(UserMessageEvent {
                        session_id,
                        message_id: Some(format!("event-{i}")),
                        author_user_id: None,
                        content: "x".to_owned(),
                    }),
                )
                .await;
        }

        let subscription = state
            .subscribe_session(session_id, Some(UniversalSeq::zero()), true)
            .await;
        let snapshot = subscription.snapshot.expect("snapshot replay");

        assert!(snapshot.has_more);
        assert_eq!(snapshot.events.len(), UNIVERSAL_EVENT_REPLAY_LIMIT);
        assert!(snapshot.events[0].seq > UniversalSeq::new(1));
    }

    #[tokio::test]
    async fn forced_import_rewrites_loaded_history_cache() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let owner_user_id = state
            .inner
            .bootstrap_admin
            .as_ref()
            .expect("bootstrap admin")
            .user
            .user_id;
        let runner_id = RunnerId::new();
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: "/work/agenter".to_owned(),
            display_name: Some("agenter".to_owned()),
        };
        let session = state
            .register_session(SessionRegistration {
                session_id: SessionId::new(),
                owner_user_id,
                runner_id,
                workspace: workspace.clone(),
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                title: Some("Old".to_owned()),
                external_session_id: Some("codex-thread-1".to_owned()),
                turn_settings: None,
                usage: None,
            })
            .await;
        state
            .publish_event(
                session.session_id,
                NormalizedEvent::UserMessage(UserMessageEvent {
                    session_id: session.session_id,
                    message_id: Some("old".to_owned()),
                    author_user_id: Some(owner_user_id),
                    content: "old cache".to_owned(),
                }),
            )
            .await;

        let summary = state
            .import_discovered_sessions(
                runner_id,
                DiscoveredSessions {
                    workspace,
                    provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                    sessions: vec![agenter_protocol::DiscoveredSession {
                        external_session_id: "codex-thread-1".to_owned(),
                        title: Some("New".to_owned()),
                        updated_at: None,
                        history_status: DiscoveredSessionHistoryStatus::Loaded,
                        history: vec![
                            DiscoveredSessionHistoryItem::UserMessage {
                                message_id: Some("user-new".to_owned()),
                                content: "new user prompt".to_owned(),
                            },
                            DiscoveredSessionHistoryItem::AgentMessage {
                                message_id: "agent-new".to_owned(),
                                content: "new cache".to_owned(),
                            },
                        ],
                    }],
                },
                SessionImportMode::Forced,
            )
            .await;

        let history = state
            .session_history(owner_user_id, session.session_id)
            .await
            .expect("history");
        assert_eq!(
            summary,
            WorkspaceSessionRefreshSummary {
                discovered_count: 1,
                refreshed_cache_count: 1,
                skipped_failed_count: 0,
            }
        );
        assert_eq!(history.len(), 2);
        assert!(matches!(history[0].event, NormalizedEvent::UserMessage(_)));
        assert!(matches!(
            history[1].event,
            NormalizedEvent::AgentMessageCompleted(_)
        ));

        let subscription = state
            .subscribe_session(session.session_id, None, true)
            .await;
        let snapshot = subscription.snapshot.expect("snapshot replay").snapshot;
        let user_items = snapshot
            .items
            .values()
            .filter(|item| item.role == ItemRole::User)
            .collect::<Vec<_>>();
        assert_eq!(user_items.len(), 1);
        assert_eq!(
            user_items[0].content[0].text.as_deref(),
            Some("new user prompt")
        );
    }

    #[tokio::test]
    async fn force_reload_import_rebuilds_cache_even_when_history_fingerprint_matches() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let owner_user_id = state
            .inner
            .bootstrap_admin
            .as_ref()
            .expect("bootstrap admin")
            .user
            .user_id;
        let runner_id = RunnerId::new();
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: "/work/agenter".to_owned(),
            display_name: Some("agenter".to_owned()),
        };
        let discovered = DiscoveredSessions {
            workspace: workspace.clone(),
            provider_id: AgentProviderId::from(AgentProviderId::CODEX),
            sessions: vec![agenter_protocol::DiscoveredSession {
                external_session_id: "codex-thread-1".to_owned(),
                title: Some("Imported".to_owned()),
                updated_at: None,
                history_status: DiscoveredSessionHistoryStatus::Loaded,
                history: vec![DiscoveredSessionHistoryItem::AgentMessage {
                    message_id: "agent-new".to_owned(),
                    content: "rebuilt cache".to_owned(),
                }],
            }],
        };

        state
            .import_discovered_sessions(runner_id, discovered.clone(), SessionImportMode::Forced)
            .await;
        let sessions = state.list_sessions(owner_user_id).await;
        let session = sessions
            .iter()
            .find(|session| session.external_session_id.as_deref() == Some("codex-thread-1"))
            .expect("imported session")
            .clone();
        state
            .publish_event(
                session.session_id,
                NormalizedEvent::UserMessage(UserMessageEvent {
                    session_id: session.session_id,
                    message_id: Some("bad-local-cache".to_owned()),
                    author_user_id: Some(owner_user_id),
                    content: "bad local cache".to_owned(),
                }),
            )
            .await;

        let summary = state
            .import_discovered_sessions(runner_id, discovered, SessionImportMode::ForceReload)
            .await;
        let history = state
            .session_history(owner_user_id, session.session_id)
            .await
            .expect("history");

        assert_eq!(
            summary,
            WorkspaceSessionRefreshSummary {
                discovered_count: 1,
                refreshed_cache_count: 1,
                skipped_failed_count: 0,
            }
        );
        assert_eq!(history.len(), 1);
        assert!(matches!(
            history[0].event,
            NormalizedEvent::AgentMessageCompleted(_)
        ));
    }

    #[tokio::test]
    async fn automatic_import_preserves_existing_loaded_history_cache() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let owner_user_id = state
            .inner
            .bootstrap_admin
            .as_ref()
            .expect("bootstrap admin")
            .user
            .user_id;
        let runner_id = RunnerId::new();
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: "/work/agenter".to_owned(),
            display_name: Some("agenter".to_owned()),
        };
        let session = state
            .register_session(SessionRegistration {
                session_id: SessionId::new(),
                owner_user_id,
                runner_id,
                workspace: workspace.clone(),
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                title: Some("Old".to_owned()),
                external_session_id: Some("codex-thread-1".to_owned()),
                turn_settings: None,
                usage: None,
            })
            .await;
        state
            .publish_event(
                session.session_id,
                NormalizedEvent::UserMessage(UserMessageEvent {
                    session_id: session.session_id,
                    message_id: Some("old".to_owned()),
                    author_user_id: Some(owner_user_id),
                    content: "old cache".to_owned(),
                }),
            )
            .await;

        let summary = state
            .import_discovered_sessions(
                runner_id,
                DiscoveredSessions {
                    workspace,
                    provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                    sessions: vec![agenter_protocol::DiscoveredSession {
                        external_session_id: "codex-thread-1".to_owned(),
                        title: Some("New".to_owned()),
                        updated_at: None,
                        history_status: DiscoveredSessionHistoryStatus::Loaded,
                        history: vec![DiscoveredSessionHistoryItem::AgentMessage {
                            message_id: "agent-new".to_owned(),
                            content: "new cache".to_owned(),
                        }],
                    }],
                },
                SessionImportMode::Automatic,
            )
            .await;

        let history = state
            .session_history(owner_user_id, session.session_id)
            .await
            .expect("history");
        assert_eq!(
            summary,
            WorkspaceSessionRefreshSummary {
                discovered_count: 1,
                refreshed_cache_count: 1,
                skipped_failed_count: 0,
            }
        );
        assert_eq!(history.len(), 2);
        assert!(matches!(history[0].event, NormalizedEvent::UserMessage(_)));
        assert!(matches!(
            history[1].event,
            NormalizedEvent::AgentMessageCompleted(_)
        ));
    }

    #[tokio::test]
    async fn forced_import_keeps_cache_when_history_read_failed() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let owner_user_id = state
            .inner
            .bootstrap_admin
            .as_ref()
            .expect("bootstrap admin")
            .user
            .user_id;
        let runner_id = RunnerId::new();
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: "/work/agenter".to_owned(),
            display_name: Some("agenter".to_owned()),
        };
        let session = state
            .register_session(SessionRegistration {
                session_id: SessionId::new(),
                owner_user_id,
                runner_id,
                workspace: workspace.clone(),
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                title: Some("Old".to_owned()),
                external_session_id: Some("codex-thread-1".to_owned()),
                turn_settings: None,
                usage: None,
            })
            .await;
        state
            .publish_event(
                session.session_id,
                NormalizedEvent::UserMessage(UserMessageEvent {
                    session_id: session.session_id,
                    message_id: Some("old".to_owned()),
                    author_user_id: Some(owner_user_id),
                    content: "old cache".to_owned(),
                }),
            )
            .await;

        let summary = state
            .import_discovered_sessions(
                runner_id,
                DiscoveredSessions {
                    workspace,
                    provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                    sessions: vec![agenter_protocol::DiscoveredSession {
                        external_session_id: "codex-thread-1".to_owned(),
                        title: Some("New".to_owned()),
                        updated_at: None,
                        history_status: DiscoveredSessionHistoryStatus::Failed {
                            message: "thread/read failed".to_owned(),
                        },
                        history: Vec::new(),
                    }],
                },
                SessionImportMode::Forced,
            )
            .await;

        let history = state
            .session_history(owner_user_id, session.session_id)
            .await
            .expect("history");
        assert_eq!(
            summary,
            WorkspaceSessionRefreshSummary {
                discovered_count: 1,
                refreshed_cache_count: 0,
                skipped_failed_count: 1,
            }
        );
        assert_eq!(history.len(), 1);
        assert!(matches!(history[0].event, NormalizedEvent::UserMessage(_)));
    }

    #[tokio::test]
    async fn discovered_session_timestamp_is_preserved_when_provided_by_runner() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let owner_user_id = state
            .inner
            .bootstrap_admin
            .as_ref()
            .expect("bootstrap admin")
            .user
            .user_id;
        let runner_id = RunnerId::new();
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: "/work/agenter".to_owned(),
            display_name: Some("agenter".to_owned()),
        };

        let source_updated_at = DateTime::parse_from_rfc3339("2026-01-01T00:00:00Z")
            .expect("parse fixed timestamp")
            .with_timezone(&Utc);

        state
            .import_discovered_sessions(
                runner_id,
                DiscoveredSessions {
                    workspace,
                    provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                    sessions: vec![agenter_protocol::DiscoveredSession {
                        external_session_id: "codex-thread-1".to_owned(),
                        title: Some("New".to_owned()),
                        updated_at: Some(source_updated_at.to_rfc3339()),
                        history_status: DiscoveredSessionHistoryStatus::Loaded,
                        history: vec![DiscoveredSessionHistoryItem::AgentMessage {
                            message_id: "agent-new".to_owned(),
                            content: "new cache".to_owned(),
                        }],
                    }],
                },
                SessionImportMode::Forced,
            )
            .await;

        let sessions = state.list_sessions(owner_user_id).await;
        let session = sessions
            .iter()
            .find(|session| session.external_session_id.as_deref() == Some("codex-thread-1"))
            .expect("imported session");

        assert_eq!(session.updated_at, Some(source_updated_at));
    }

    #[test]
    fn discovered_history_items_are_rewritten_to_app_session_events() {
        let session_id = SessionId::nil();
        let owner_user_id = UserId::nil();
        let events = discovered_history_events(
            session_id,
            owner_user_id,
            &[
                DiscoveredSessionHistoryItem::UserMessage {
                    message_id: Some("user-1".to_owned()),
                    content: "hello".to_owned(),
                },
                DiscoveredSessionHistoryItem::AgentMessage {
                    message_id: "agent-1".to_owned(),
                    content: "hi".to_owned(),
                },
                DiscoveredSessionHistoryItem::Command {
                    command_id: "cmd-1".to_owned(),
                    command: "cargo test".to_owned(),
                    cwd: Some("/work/agenter".to_owned()),
                    source: Some("unifiedExecStartup".to_owned()),
                    process_id: Some("123".to_owned()),
                    duration_ms: Some(17),
                    actions: vec![agenter_protocol::DiscoveredCommandAction {
                        kind: "read".to_owned(),
                        command: Some("sed -n '1,20p' SKILL.md".to_owned()),
                        path: Some("/tmp/skills/demo/SKILL.md".to_owned()),
                        name: Some("SKILL.md".to_owned()),
                        query: None,
                        provider_payload: None,
                    }],
                    output: Some("ok".to_owned()),
                    exit_code: Some(0),
                    success: true,
                    provider_payload: None,
                },
                DiscoveredSessionHistoryItem::Tool {
                    tool_call_id: "tool-1".to_owned(),
                    name: "spawnAgent".to_owned(),
                    title: Some("spawnAgent".to_owned()),
                    status: DiscoveredToolStatus::Completed,
                    input: None,
                    output: None,
                    provider_payload: None,
                },
                DiscoveredSessionHistoryItem::FileChange {
                    change_id: "file-1".to_owned(),
                    path: "README.md".to_owned(),
                    change_kind: agenter_core::FileChangeKind::Modify,
                    status: DiscoveredFileChangeStatus::Applied,
                    diff: Some("+hello".to_owned()),
                    provider_payload: None,
                },
                DiscoveredSessionHistoryItem::Plan {
                    plan_id: "plan-1".to_owned(),
                    title: Some("Implementation plan".to_owned()),
                    content: "1. Test".to_owned(),
                    provider_payload: None,
                },
                DiscoveredSessionHistoryItem::NativeNotification {
                    event_id: Some("compact-1".to_owned()),
                    category: "compaction".to_owned(),
                    title: "Context compacted".to_owned(),
                    detail: None,
                    status: Some("completed".to_owned()),
                    provider_payload: None,
                },
            ],
        );

        assert!(matches!(events[0], NormalizedEvent::UserMessage(_)));
        assert!(matches!(
            events[1],
            NormalizedEvent::AgentMessageCompleted(_)
        ));
        assert!(matches!(events[2], NormalizedEvent::CommandStarted(_)));
        assert!(matches!(events[3], NormalizedEvent::CommandOutputDelta(_)));
        assert!(matches!(events[4], NormalizedEvent::CommandCompleted(_)));
        assert!(matches!(events[5], NormalizedEvent::ToolCompleted(_)));
        assert!(matches!(events[6], NormalizedEvent::FileChangeApplied(_)));
        assert!(matches!(events[7], NormalizedEvent::PlanUpdated(_)));
        assert!(matches!(events[8], NormalizedEvent::NativeNotification(_)));
    }

    #[test]
    fn discovered_history_codex_thread_item_maps_to_provider_normalized_event() {
        let session_id = SessionId::nil();
        let owner_user_id = UserId::nil();
        let events = discovered_history_events(
            session_id,
            owner_user_id,
            &[DiscoveredSessionHistoryItem::NativeNotification {
                event_id: Some("og-1".to_owned()),
                category: "codex_thread_item".to_owned(),
                title: "orphanGadget".to_owned(),
                detail: Some("experimental row".to_owned()),
                status: Some("done".to_owned()),
                provider_payload: None,
            }],
        );

        assert_eq!(events.len(), 1);
        let NormalizedEvent::NativeNotification(pe) = &events[0] else {
            panic!("expected NativeNotification normalized event");
        };
        assert_eq!(pe.session_id, session_id);
        assert_eq!(pe.method.as_str(), "codex_thread_item");
        assert_eq!(pe.category.as_str(), "codex_thread_item");
        assert_eq!(pe.title.as_str(), "orphanGadget");
        assert_eq!(pe.detail.as_deref(), Some("experimental row"));
    }

    #[test]
    fn runner_token_uses_dev_secret() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);

        assert!(state.is_runner_token_valid("dev-token"));
        assert!(!state.is_runner_token_valid("wrong-token"));
    }

    #[tokio::test]
    async fn stale_runner_disconnect_does_not_remove_new_connection() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let runner_id = RunnerId::new();
        let (old_sender, mut old_receiver) = mpsc::unbounded_channel();
        let old_connection_id = state.connect_runner(runner_id, old_sender).await;
        let (new_sender, mut new_receiver) = mpsc::unbounded_channel();
        let new_connection_id = state.connect_runner(runner_id, new_sender).await;

        state.disconnect_runner(runner_id, old_connection_id).await;

        let send_state = state.clone();
        let send_task = tokio::spawn(async move {
            send_state
                .send_runner_message(
                    runner_id,
                    RunnerServerMessage::HeartbeatAck(RunnerHeartbeatAck { sequence: 1 }),
                )
                .await
        });
        let outbound = new_receiver
            .recv()
            .await
            .expect("new connection receives message");
        assert!(old_receiver.try_recv().is_err());
        assert!(matches!(
            outbound.message,
            RunnerServerMessage::HeartbeatAck(RunnerHeartbeatAck { sequence: 1 })
        ));
        outbound.delivered.send(Ok(())).expect("ack delivery");
        assert!(send_task.await.expect("send task joins").is_ok());

        state.disconnect_runner(runner_id, new_connection_id).await;
        assert!(matches!(
            state
                .send_runner_message(
                    runner_id,
                    RunnerServerMessage::HeartbeatAck(RunnerHeartbeatAck { sequence: 2 }),
                )
                .await,
            Err(RunnerSendError::NotConnected)
        ));
    }

    #[tokio::test]
    async fn runner_event_ack_state_dedupes_replayed_sequences() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let runner_id = RunnerId::new();

        assert!(
            !state
                .runner_event_already_accepted(runner_id, Some(1))
                .await
        );
        assert!(
            !state
                .runner_event_already_accepted(runner_id, Some(1))
                .await
        );

        state.mark_runner_event_accepted(runner_id, 1).await;
        assert!(
            state
                .runner_event_already_accepted(runner_id, Some(1))
                .await
        );
        assert!(
            !state
                .runner_event_already_accepted(runner_id, Some(2))
                .await
        );
    }

    #[tokio::test]
    async fn runner_event_duplicate_check_does_not_poison_unaccepted_sequence() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let runner_id = RunnerId::new();

        assert!(
            !state
                .runner_event_already_accepted(runner_id, Some(7))
                .await
        );
        assert!(
            !state
                .runner_event_already_accepted(runner_id, Some(7))
                .await
        );

        state.mark_runner_event_accepted(runner_id, 7).await;
        assert!(
            state
                .runner_event_already_accepted(runner_id, Some(7))
                .await
        );
    }

    #[tokio::test]
    async fn strict_runner_event_acceptance_fails_without_durable_workspace() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://agenter:agenter@127.0.0.1:1/agenter")
            .expect("lazy pool");
        let state = AppState::new_with_test_db_pool(pool);
        let session_id = SessionId::new();

        let result = state
            .accept_runner_agent_event(AgentUniversalEvent {
                protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
                session_id,
                event_id: None,
                turn_id: None,
                item_id: None,
                ts: None,
                source: UniversalEventSource::Native,
                native: None,
                event: UniversalEventKind::NativeUnknown {
                    summary: Some("hello".to_owned()),
                },
            })
            .await;

        assert!(result.is_err());
    }

    #[tokio::test]
    async fn runner_agent_event_prefers_universal_projection_for_snapshot() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let session_id = SessionId::new();
        let native = NativeRef {
            protocol: "codex-app-server".to_owned(),
            method: Some("thread/item".to_owned()),
            kind: Some(AgentProviderId::CODEX.to_owned()),
            native_id: Some("native-event-1".to_owned()),
            summary: Some("codex native plan update".to_owned()),
            hash: None,
            pointer: None,
        };

        state
            .accept_runner_agent_event(AgentUniversalEvent {
                protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
                session_id,
                event_id: None,
                turn_id: None,
                item_id: None,
                ts: None,
                source: UniversalEventSource::Native,
                native: Some(native.clone()),
                event: UniversalEventKind::NativeUnknown {
                    summary: Some("codex native plan update".to_owned()),
                },
            })
            .await
            .expect("accept runner event");

        let sessions = state.inner.sessions.lock().await;
        let events = sessions.get(&session_id).expect("session events");
        let universal = events.universal_cache.last().expect("universal event");
        assert_eq!(universal.native, Some(native));
        assert!(matches!(
            &universal.event,
            UniversalEventKind::NativeUnknown { summary }
                if summary.as_deref() == Some("codex native plan update")
        ));
    }

    #[tokio::test]
    async fn runner_db_backed_discovered_sessions_rejects_ack_when_import_fails() {
        let pool = sqlx::postgres::PgPoolOptions::new()
            .connect_lazy("postgres://agenter:agenter@127.0.0.1:1/agenter")
            .expect("lazy pool");
        let state = AppState::new_with_test_db_pool(pool);
        let runner_id = RunnerId::new();
        let request_id = RequestId::from("refresh-1");

        let accepted = state
            .process_runner_discovered_sessions(
                runner_id,
                Some(request_id.clone()),
                DiscoveredSessions {
                    workspace: WorkspaceRef {
                        workspace_id: WorkspaceId::new(),
                        runner_id,
                        path: "/tmp/agenter".to_owned(),
                        display_name: Some("agenter".to_owned()),
                    },
                    provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                    sessions: vec![agenter_protocol::DiscoveredSession {
                        external_session_id: "thread-1".to_owned(),
                        title: Some("Thread".to_owned()),
                        updated_at: None,
                        history_status: DiscoveredSessionHistoryStatus::Loaded,
                        history: Vec::new(),
                    }],
                },
            )
            .await;

        assert!(!accepted);
        assert_eq!(
            state.take_refresh_summary(&request_id).await,
            Some(WorkspaceSessionRefreshSummary {
                discovered_count: 1,
                refreshed_cache_count: 0,
                skipped_failed_count: 1,
            })
        );
    }

    #[tokio::test]
    async fn runner_no_db_discovered_sessions_remain_ackable_after_import() {
        let state = AppState::new_with_bootstrap_admin(
            "dev-token".to_owned(),
            "admin@example.test".to_owned(),
            "correct horse battery staple".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        )
        .expect("create bootstrap admin");
        let runner_id = RunnerId::new();

        let accepted = state
            .process_runner_discovered_sessions(
                runner_id,
                None,
                DiscoveredSessions {
                    workspace: WorkspaceRef {
                        workspace_id: WorkspaceId::new(),
                        runner_id,
                        path: "/tmp/agenter".to_owned(),
                        display_name: Some("agenter".to_owned()),
                    },
                    provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                    sessions: vec![agenter_protocol::DiscoveredSession {
                        external_session_id: "thread-1".to_owned(),
                        title: Some("Thread".to_owned()),
                        updated_at: None,
                        history_status: DiscoveredSessionHistoryStatus::Loaded,
                        history: Vec::new(),
                    }],
                },
            )
            .await;

        assert!(accepted);
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at a disposable Postgres database"]
    async fn runner_event_receipts_survive_new_app_state_and_prevent_duplicate_append() {
        let pool = durable_test_pool().await;
        let suffix = Uuid::new_v4();
        let user = agenter_db::create_user(
            &pool,
            &format!("runner-receipt-state-{suffix}@example.test"),
            Some("Runner Receipt State"),
        )
        .await
        .expect("create user");
        let runner_id = RunnerId::new();
        let workspace = WorkspaceRef {
            workspace_id: WorkspaceId::new(),
            runner_id,
            path: format!("/tmp/agenter-runner-receipt-state-{suffix}"),
            display_name: Some("runner receipt state".to_owned()),
        };
        let session_id = SessionId::new();

        let state1 = AppState::new_with_test_db_pool(pool.clone());
        state1
            .register_runner(
                runner_id,
                runner_capabilities_for_test(),
                vec![workspace.clone()],
            )
            .await;
        state1
            .create_session(
                session_id,
                user.user_id,
                runner_id,
                workspace.clone(),
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;
        let stored = state1
            .accept_runner_agent_event_with_receipt(
                runner_id,
                Some(1),
                runner_native_unknown_for_test(session_id, "durable first event"),
            )
            .await
            .expect("accept first runner event")
            .expect("first event should append");
        assert!(stored.seq > UniversalSeq::zero());

        let state2 = AppState::new_with_test_db_pool(pool.clone());
        state2
            .register_runner(
                runner_id,
                runner_capabilities_for_test(),
                vec![workspace.clone()],
            )
            .await;
        state2
            .create_session(
                session_id,
                user.user_id,
                runner_id,
                workspace,
                AgentProviderId::from(AgentProviderId::CODEX),
            )
            .await;

        assert_eq!(
            state2
                .seed_runner_event_ack_from_hello(runner_id, Some(99))
                .await,
            Some(1),
            "DB-backed hello must use durable receipts, not the runner-supplied cursor"
        );
        assert!(
            state2
                .runner_event_already_accepted(runner_id, Some(1))
                .await
        );
        assert!(
            !state2
                .runner_event_already_accepted(runner_id, Some(2))
                .await
        );

        let duplicate = state2
            .accept_runner_agent_event_with_receipt(
                runner_id,
                Some(1),
                runner_native_unknown_for_test(session_id, "duplicate should not append"),
            )
            .await
            .expect("duplicate replay is ackable");
        assert!(duplicate.is_none());

        let replay = agenter_db::list_universal_events_after(&pool, session_id, None, 10)
            .await
            .expect("load durable events");
        assert_eq!(replay.len(), 1);
        assert_eq!(replay[0].event_id.to_string(), stored.event_id);
    }

    #[tokio::test]
    async fn refresh_operation_update_advances_job_and_records_log() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let request_id = RequestId::from("refresh-progress-1");
        state
            .upsert_refresh_job(
                request_id.clone(),
                WorkspaceSessionRefreshStatus::Queued,
                None,
                None,
            )
            .await;

        state
            .record_refresh_operation_update(agenter_protocol::runner::RunnerOperationUpdate {
                operation_id: request_id.clone(),
                kind: agenter_protocol::runner::RunnerOperationKind::SessionRefresh,
                status: agenter_protocol::runner::RunnerOperationStatus::ReadingHistory,
                stage_label: "Reading Codex history".to_owned(),
                progress: Some(agenter_protocol::runner::RunnerOperationProgress {
                    current: Some(2),
                    total: Some(4),
                    percent: Some(50),
                }),
                message: Some("Read 2 of 4 sessions".to_owned()),
                level: agenter_protocol::runner::RunnerOperationLogLevel::Info,
                ts: None,
            })
            .await;

        let job = state
            .workspace_session_refresh_status(&request_id)
            .await
            .expect("refresh job");
        assert_eq!(job.status, WorkspaceSessionRefreshStatus::ReadingHistory);
        assert_eq!(job.progress.expect("progress").percent, Some(50));
        assert_eq!(job.log.len(), 1);
        assert_eq!(job.log[0].message, "Read 2 of 4 sessions");
    }

    async fn durable_test_pool() -> sqlx::PgPool {
        let database_url = std::env::var("DATABASE_URL")
            .expect("DATABASE_URL must be set to run ignored SQLx integration tests");
        let pool = sqlx::PgPool::connect(&database_url)
            .await
            .expect("connect to DATABASE_URL");
        sqlx::migrate!("../../migrations")
            .run(&pool)
            .await
            .expect("run migrations");
        pool
    }

    fn runner_capabilities_for_test() -> RunnerCapabilities {
        RunnerCapabilities {
            agent_providers: vec![AgentProviderAdvertisement {
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                capabilities: AgentCapabilities::default(),
            }],
            transports: vec!["codex-app-server".to_owned()],
            workspace_discovery: false,
        }
    }

    fn runner_native_unknown_for_test(
        session_id: SessionId,
        summary: &str,
    ) -> agenter_protocol::runner::AgentUniversalEvent {
        agenter_protocol::runner::AgentUniversalEvent {
            protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
            session_id,
            event_id: Some(Uuid::new_v4().to_string()),
            turn_id: None,
            item_id: None,
            ts: None,
            source: UniversalEventSource::Runner,
            native: None,
            event: UniversalEventKind::NativeUnknown {
                summary: Some(summary.to_owned()),
            },
        }
    }

    #[tokio::test]
    async fn seeded_runner_ack_marks_old_replay_as_duplicate() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let runner_id = RunnerId::new();

        state.seed_runner_event_ack(runner_id, Some(3)).await;

        assert!(
            state
                .runner_event_already_accepted(runner_id, Some(1))
                .await
        );
        assert!(
            state
                .runner_event_already_accepted(runner_id, Some(3))
                .await
        );
        assert!(
            !state
                .runner_event_already_accepted(runner_id, Some(4))
                .await
        );
    }
}
