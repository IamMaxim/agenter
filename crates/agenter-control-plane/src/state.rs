use std::{
    collections::{HashMap, HashSet},
    sync::Arc,
    time::Duration,
};

use agenter_core::{
    AgentProviderId, AgentQuestionAnswer, AgentTurnSettings, ApprovalDecision, ApprovalId,
    ApprovalOption, ApprovalRequest, ApprovalStatus as UniversalApprovalStatus, CapabilitySet,
    ContentBlock, ContentBlockKind, DiffFile, ItemRole, ItemState, ItemStatus, NativeRef,
    ProviderNotification, ProviderNotificationSeverity, QuestionId, QuestionState, QuestionStatus,
    RunnerId, SessionId, SessionInfo, SessionSnapshot, SessionStatus, SessionUsageSnapshot,
    ToolActionProjection, ToolCommandProjection, ToolProjection, ToolProjectionKind, TurnStatus,
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

const SESSION_EVENT_BROADCAST_LIMIT: usize = 128;
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
    pub envelope: Option<Box<UniversalEventEnvelope>>,
    pub state: Option<Box<QuestionState>>,
}

/// Tracks client-visible approval lifecycle from universal approval state.
#[derive(Clone, Debug)]
enum ApprovalStatus {
    Pending {
        request: Box<ApprovalRequest>,
        envelope: Box<UniversalEventEnvelope>,
    },
    Presented {
        request: Box<ApprovalRequest>,
        envelope: Box<UniversalEventEnvelope>,
    },
    Resolving {
        request: Box<ApprovalRequest>,
        envelope: Box<UniversalEventEnvelope>,
        decision: ApprovalDecision,
    },
    Resolved {
        request: Box<ApprovalRequest>,
    },
    Orphaned {
        request: Box<ApprovalRequest>,
    },
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

fn session_status_orphans_approvals(status: &SessionStatus) -> bool {
    matches!(
        status,
        SessionStatus::Stopped | SessionStatus::Failed | SessionStatus::Archived
    )
}

fn enrich_approval_request(request: &mut ApprovalRequest) {
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
        request.native_request_id = request
            .native
            .as_ref()
            .and_then(|native| native.native_id.clone());
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

#[derive(Clone, Debug)]
pub enum ApprovalResolutionStart {
    Missing,
    InProgress { request: Box<ApprovalRequest> },
    AlreadyResolved { request: Box<ApprovalRequest> },
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
        request: Box<ApprovalRequest>,
    },
    AlreadyResolved {
        session_id: SessionId,
        request: Box<ApprovalRequest>,
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
    universal_cache: Vec<UniversalEventEnvelope>,
    snapshot: SessionSnapshot,
    next_seq: i64,
}

#[derive(Clone, Debug)]
pub struct SessionBroadcastEvent {
    pub universal_event: UniversalEventEnvelope,
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
    #[allow(dead_code)]
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
        let receiver = {
            let mut sessions = self.inner.sessions.lock().await;
            let events = sessions
                .entry(session_id)
                .or_insert_with(SessionEvents::new);
            events.sender.subscribe()
        };

        let snapshot = self
            .session_snapshot_replay(session_id, after_seq, include_snapshot)
            .await;
        SessionSubscription { snapshot, receiver }
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
            let (mut snapshot, events, replay_complete) =
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
                    let replay_incomplete = starts_after_requested_cursor
                        || (!include_snapshot && replay.len() > UNIVERSAL_EVENT_REPLAY_LIMIT);
                    let mut replay = replay;
                    if !include_snapshot && replay_incomplete {
                        replay.truncate(UNIVERSAL_EVENT_REPLAY_LIMIT);
                    }
                    let mut snapshot = events.snapshot.clone();
                    snapshot.session_id = session_id;
                    (snapshot, replay, !replay_incomplete)
                } else {
                    (
                        SessionSnapshot {
                            session_id,
                            ..SessionSnapshot::default()
                        },
                        Vec::new(),
                        true,
                    )
                };
            self.hydrate_snapshot_capabilities(&mut snapshot).await;
            let snapshot_seq = snapshot.latest_seq;
            let replay_from_seq = events.first().map(|event| event.seq);
            let replay_through_seq = events.last().map(|event| event.seq);
            return Some(BrowserSessionSnapshot {
                protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
                request_id: None,
                snapshot,
                events,
                snapshot_seq,
                replay_from_seq,
                replay_through_seq,
                replay_complete,
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
        let replay_limit = if include_snapshot {
            usize::MAX
        } else {
            UNIVERSAL_EVENT_REPLAY_LIMIT + 1
        };
        let replay =
            agenter_db::list_universal_events_after(pool, session_id, after_seq, replay_limit)
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
        let replay_incomplete =
            replay_failed || (!include_snapshot && events.len() > UNIVERSAL_EVENT_REPLAY_LIMIT);
        if !include_snapshot && replay_incomplete {
            events.truncate(UNIVERSAL_EVENT_REPLAY_LIMIT);
        }
        let snapshot_seq = snapshot.latest_seq;
        let replay_from_seq = events.first().map(|event| event.seq);
        let replay_through_seq = events.last().map(|event| event.seq).or(after_seq);
        Some(BrowserSessionSnapshot {
            protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
            request_id: None,
            snapshot,
            events,
            snapshot_seq,
            replay_from_seq,
            replay_through_seq,
            replay_complete: !replay_incomplete,
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
    ) -> Option<BrowserSessionSnapshot> {
        if !self.can_access_session(user_id, session_id).await {
            return None;
        }
        self.session_snapshot_replay(session_id, Some(UniversalSeq::zero()), true)
            .await
    }

    /// Pending or in-flight resolving approvals for a session (for API listing / tools).
    pub async fn pending_approval_requests(&self, session_id: SessionId) -> Vec<ApprovalRequest> {
        let mut to_persist = Vec::new();
        let mut registry = self.inner.registry.lock().await;
        let mut out = Vec::new();
        for approval in registry.approvals.values_mut() {
            if approval.session_id != session_id {
                continue;
            }
            match &approval.status {
                ApprovalStatus::Pending { request, envelope } => {
                    let mut presented = request.as_ref().clone();
                    presented.status = UniversalApprovalStatus::Presented;
                    presented.resolving_decision = None;
                    let presented_envelope = approval_request_envelope(envelope, presented.clone());
                    approval.status = ApprovalStatus::Presented {
                        request: Box::new(presented.clone()),
                        envelope: Box::new(presented_envelope.clone()),
                    };
                    to_persist.push(presented_envelope);
                    out.push(presented);
                }
                ApprovalStatus::Presented { request, .. } => {
                    out.push(*request.clone());
                }
                ApprovalStatus::Resolving {
                    request, decision, ..
                } => {
                    let mut resolving = request.as_ref().clone();
                    resolving.status = UniversalApprovalStatus::Resolving;
                    resolving.resolving_decision = Some(decision.clone());
                    out.push(resolving);
                }
                ApprovalStatus::Resolved { .. } | ApprovalStatus::Orphaned { .. } => {}
            }
        }
        drop(registry);
        for envelope in to_persist {
            self.store_control_plane_universal_envelope(session_id, envelope)
                .await
                .ok();
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
                let mut approval = approval.as_ref().clone();
                enrich_approval_request(&mut approval);
                let status = match &approval.status {
                    UniversalApprovalStatus::Presented => ApprovalStatus::Presented {
                        request: Box::new(approval.clone()),
                        envelope: Box::new(approval_request_envelope(envelope, approval.clone())),
                    },
                    UniversalApprovalStatus::Resolving => ApprovalStatus::Resolving {
                        request: Box::new(approval.clone()),
                        envelope: Box::new(approval_request_envelope(envelope, approval.clone())),
                        decision: approval
                            .resolving_decision
                            .clone()
                            .unwrap_or(ApprovalDecision::Decline),
                    },
                    UniversalApprovalStatus::Orphaned => ApprovalStatus::Orphaned {
                        request: Box::new(approval.clone()),
                    },
                    status if status.is_terminal() => ApprovalStatus::Resolved {
                        request: Box::new(approval.clone()),
                    },
                    _ => ApprovalStatus::Pending {
                        request: Box::new(approval.clone()),
                        envelope: Box::new(approval_request_envelope(envelope, approval.clone())),
                    },
                };
                self.inner.registry.lock().await.approvals.insert(
                    approval.approval_id,
                    RegisteredApproval {
                        session_id: envelope.session_id,
                        status,
                    },
                );
                if matches!(approval.status, UniversalApprovalStatus::Pending) {
                    self.maybe_auto_resolve_approval(envelope.session_id, approval)
                        .await;
                }
            }
            UniversalEventKind::QuestionRequested { question } => {
                self.inner.registry.lock().await.questions.insert(
                    question.question_id,
                    RegisteredQuestion {
                        session_id: envelope.session_id,
                        status: question.status.clone(),
                        envelope: Some(Box::new(envelope.clone())),
                        state: Some(question.clone()),
                    },
                );
            }
            UniversalEventKind::QuestionAnswered { question } => {
                let mut registry = self.inner.registry.lock().await;
                if let Some(registered) = registry.questions.get_mut(&question.question_id) {
                    registered.status = question.status.clone();
                    registered.envelope = Some(Box::new(envelope.clone()));
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
                ApprovalStatus::Pending { request, envelope }
                | ApprovalStatus::Presented { request, envelope } => {
                    let mut resolving = request.as_ref().clone();
                    resolving.status = UniversalApprovalStatus::Resolving;
                    resolving.resolving_decision = Some(decision.clone());
                    let resolving_envelope = approval_request_envelope(envelope, resolving.clone());
                    transition = Some((approval.session_id, resolving_envelope.clone()));
                    approval.status = ApprovalStatus::Resolving {
                        request: Box::new(resolving),
                        envelope: Box::new(resolving_envelope),
                        decision: decision.clone(),
                    };
                    tracing::debug!(%approval_id, session_id = %approval.session_id, "approval resolution started");
                    ApprovalResolutionStart::Started
                }
                ApprovalStatus::Resolving {
                    request, decision, ..
                } => {
                    let mut resolving = request.as_ref().clone();
                    resolving.status = UniversalApprovalStatus::Resolving;
                    resolving.resolving_decision = Some(decision.clone());
                    ApprovalResolutionStart::InProgress {
                        request: Box::new(resolving),
                    }
                }
                ApprovalStatus::Resolved { request, .. }
                | ApprovalStatus::Orphaned { request, .. } => {
                    ApprovalResolutionStart::AlreadyResolved {
                        request: request.clone(),
                    }
                }
            }
        };
        if let Some((session_id, envelope)) = transition {
            self.store_control_plane_universal_envelope(session_id, envelope)
                .await
                .ok();
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
            ApprovalStatus::Pending { .. } | ApprovalStatus::Presented { .. } => {
                ApprovalResolutionLookup::Pending {
                    session_id: approval.session_id,
                }
            }
            ApprovalStatus::Resolving {
                request, decision, ..
            } => {
                let mut resolving = request.as_ref().clone();
                resolving.status = UniversalApprovalStatus::Resolving;
                resolving.resolving_decision = Some(decision.clone());
                ApprovalResolutionLookup::InProgress {
                    session_id: approval.session_id,
                    request: Box::new(resolving),
                }
            }
            ApprovalStatus::Resolved { request, .. } | ApprovalStatus::Orphaned { request, .. } => {
                ApprovalResolutionLookup::AlreadyResolved {
                    session_id: approval.session_id,
                    request: request.clone(),
                }
            }
        }
    }

    pub async fn approval_request(&self, approval_id: ApprovalId) -> Option<ApprovalRequest> {
        let registry = self.inner.registry.lock().await;
        let approval = registry.approvals.get(&approval_id)?;
        let request = match &approval.status {
            ApprovalStatus::Pending { request, .. }
            | ApprovalStatus::Presented { request, .. }
            | ApprovalStatus::Resolving { request, .. } => request,
            ApprovalStatus::Resolved { .. } | ApprovalStatus::Orphaned { .. } => return None,
        };
        Some(*request.clone())
    }

    pub async fn cancel_approval_resolution(&self, approval_id: ApprovalId) {
        let mut registry = self.inner.registry.lock().await;
        let Some(approval) = registry.approvals.get_mut(&approval_id) else {
            return;
        };
        if let ApprovalStatus::Resolving {
            request, envelope, ..
        } = &approval.status
        {
            let mut pending = request.as_ref().clone();
            pending.status = UniversalApprovalStatus::Pending;
            pending.resolving_decision = None;
            approval.status = ApprovalStatus::Pending {
                request: Box::new(pending),
                envelope: envelope.clone(),
            };
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
        decision: ApprovalDecision,
        resolved_by_user_id: Option<UserId>,
    ) -> Option<ApprovalRequest> {
        let transition = {
            let mut registry = self.inner.registry.lock().await;
            let approval = registry.approvals.get_mut(&approval_id)?;
            if approval.session_id != session_id {
                return None;
            }
            match &approval.status {
                ApprovalStatus::Resolved { request, .. }
                | ApprovalStatus::Orphaned { request, .. } => {
                    return Some(*request.clone());
                }
                ApprovalStatus::Pending { .. } | ApprovalStatus::Presented { .. } => return None,
                ApprovalStatus::Resolving {
                    request, envelope, ..
                } => {
                    let mut resolved = request.as_ref().clone();
                    resolved.status = approval_decision_universal_status(&decision);
                    resolved.resolving_decision = None;
                    resolved.resolved_at = Some(Utc::now());
                    let resolved_envelope = approval_resolved_envelope(
                        envelope,
                        &resolved,
                        &decision,
                        resolved_by_user_id,
                    );
                    approval.status = ApprovalStatus::Resolved {
                        request: Box::new(resolved.clone()),
                    };
                    Some((resolved, resolved_envelope))
                }
            }
        };

        let (request, envelope) = transition?;
        self.store_control_plane_universal_envelope(session_id, envelope)
            .await
            .ok();
        Some(request)
    }

    async fn maybe_auto_resolve_approval(&self, session_id: SessionId, request: ApprovalRequest) {
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
                self.finish_approval_resolution(approval_id, session.session_id, decision, None)
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
    ) -> anyhow::Result<UniversalEventEnvelope> {
        let question = {
            let mut registry = self.inner.registry.lock().await;
            let registered = registry.questions.get_mut(&answer.question_id);
            let mut state = registered
                .as_ref()
                .and_then(|registered| registered.state.as_deref().cloned())
                .unwrap_or_else(|| QuestionState {
                    question_id: answer.question_id,
                    session_id,
                    turn_id: None,
                    title: "Input requested".to_owned(),
                    description: None,
                    fields: Vec::new(),
                    status: QuestionStatus::Pending,
                    answer: None,
                    native_request_id: None,
                    native_blocking: false,
                    native: None,
                    requested_at: None,
                    answered_at: None,
                });
            state.status = QuestionStatus::Answered;
            state.answer = Some(answer);
            state.answered_at = Some(Utc::now());
            if let Some(registered) = registered {
                registered.status = QuestionStatus::Answered;
                registered.state = Some(Box::new(state.clone()));
            }
            state
        };
        self.publish_universal_event(
            session_id,
            question.turn_id,
            None,
            UniversalEventKind::QuestionAnswered {
                question: Box::new(question),
            },
        )
        .await
    }

    async fn orphan_pending_approvals_for_session(&self, session_id: SessionId, reason: &str) {
        let orphaned = {
            let mut registry = self.inner.registry.lock().await;
            let mut orphaned = Vec::new();
            for (&approval_id, approval) in &mut registry.approvals {
                if approval.session_id != session_id {
                    continue;
                }
                let (request, envelope) = match &approval.status {
                    ApprovalStatus::Pending { request, envelope }
                    | ApprovalStatus::Presented { request, envelope }
                    | ApprovalStatus::Resolving {
                        request, envelope, ..
                    } => (request.clone(), envelope.clone()),
                    ApprovalStatus::Resolved { .. } | ApprovalStatus::Orphaned { .. } => continue,
                };
                let mut orphaned_request = request.as_ref().clone();
                orphaned_request.status = UniversalApprovalStatus::Orphaned;
                orphaned_request.resolving_decision = None;
                orphaned_request.details = orphaned_request
                    .details
                    .clone()
                    .or_else(|| Some(reason.to_owned()));
                let orphaned_envelope =
                    approval_request_envelope(&envelope, orphaned_request.clone());
                approval.status = ApprovalStatus::Orphaned {
                    request: Box::new(orphaned_request),
                };
                tracing::warn!(%session_id, %approval_id, "approval marked orphaned");
                orphaned.push(orphaned_envelope);
            }
            orphaned
        };

        for envelope in orphaned {
            self.store_control_plane_universal_envelope(session_id, envelope)
                .await
                .ok();
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
                let mut state =
                    question
                        .state
                        .as_deref()
                        .cloned()
                        .unwrap_or_else(|| QuestionState {
                            question_id,
                            session_id,
                            turn_id: None,
                            title: "Input requested".to_owned(),
                            description: None,
                            fields: Vec::new(),
                            status: QuestionStatus::Pending,
                            answer: None,
                            native_request_id: None,
                            native_blocking: false,
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
            universal_event: envelope.clone(),
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
            universal_event: envelope.clone(),
        });
        Ok(Some(envelope))
    }

    async fn store_control_plane_universal_envelope(
        &self,
        session_id: SessionId,
        envelope: UniversalEventEnvelope,
    ) -> anyhow::Result<UniversalEventEnvelope> {
        self.store_universal_event_with_acceptance(
            session_id,
            AgentUniversalEvent {
                protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
                session_id,
                event_id: Some(envelope.event_id),
                turn_id: envelope.turn_id,
                item_id: envelope.item_id,
                ts: Some(envelope.ts),
                source: envelope.source,
                native: envelope.native,
                event: envelope.event,
            },
            false,
        )
        .await
    }

    async fn workspace_id_for_session(&self, session_id: SessionId) -> Option<WorkspaceId> {
        if let Some(workspace_id) = self
            .inner
            .registry
            .lock()
            .await
            .sessions
            .get(&session_id)
            .map(|session| session.workspace.workspace_id)
        {
            return Some(workspace_id);
        }

        let pool = self.inner.db_pool.as_ref()?;
        let persisted = agenter_db::find_session_by_id(pool, session_id)
            .await
            .ok()??;
        let workspace = WorkspaceRef {
            workspace_id: persisted.workspace.workspace_id,
            runner_id: persisted.workspace.runner_id,
            path: persisted.workspace.path.clone(),
            display_name: persisted.workspace.display_name.clone(),
        };
        let session = registered_session_from_db(&persisted.session, &workspace);
        self.inner
            .registry
            .lock()
            .await
            .sessions
            .entry(session_id)
            .or_insert(session);
        Some(workspace.workspace_id)
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
                        match agenter_db::replace_session_event_projection(
                            pool,
                            discovered.workspace.workspace_id,
                            session.session_id,
                            &discovered_events,
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
                    self.store_control_plane_universal_envelope(session.session_id, event)
                        .await
                        .ok();
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
    #[allow(dead_code)]
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
            let mut approval = (**approval).clone();
            if approval.requested_at.is_none() {
                approval.requested_at = Some(envelope.ts);
            }
            merge_approval_into_snapshot(snapshot, &approval);
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
            let mut question = (**question).clone();
            if matches!(envelope.event, UniversalEventKind::QuestionRequested { .. })
                && question.requested_at.is_none()
            {
                question.requested_at = Some(envelope.ts);
            }
            merge_question_into_snapshot(snapshot, &question);
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
        existing.requested_at = question.requested_at.or(existing.requested_at);
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
    let incoming_updated_at = plan.updated_at.or(Some(envelope.ts));
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
        let existing_before = snapshot.plans.get(&plan.plan_id);
        let changed = existing_before
            .map(|existing| plan_changes_existing_plan(existing, plan))
            .unwrap_or(true);
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
        if changed || next.updated_at.is_none() {
            next.updated_at = incoming_updated_at;
        }
    } else if let Some(existing) = snapshot.plans.get(&plan.plan_id) {
        if plan_changes_existing_plan(existing, &next) {
            next.updated_at = incoming_updated_at;
        } else {
            next.updated_at = existing.updated_at.or(incoming_updated_at);
        }
    } else if next.updated_at.is_none() {
        next.updated_at = incoming_updated_at;
    }

    materialize_plan_item(snapshot, envelope, &next);
    snapshot.plans.insert(next.plan_id, next);
}

fn plan_changes_existing_plan(
    existing: &agenter_core::PlanState,
    incoming: &agenter_core::PlanState,
) -> bool {
    incoming.content.as_ref().is_some_and(|content| {
        existing
            .content
            .as_ref()
            .map(|existing| existing != content)
            .unwrap_or(true)
    }) || incoming
        .title
        .as_ref()
        .is_some_and(|title| existing.title.as_ref() != Some(title))
        || incoming.status != existing.status
        || !incoming.entries.is_empty()
        || !incoming.artifact_refs.is_empty()
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

fn approval_request_envelope(
    source: &UniversalEventEnvelope,
    approval: ApprovalRequest,
) -> UniversalEventEnvelope {
    UniversalEventEnvelope {
        event_id: Uuid::new_v4().to_string(),
        seq: UniversalSeq::zero(),
        session_id: source.session_id,
        turn_id: approval.turn_id.or(source.turn_id),
        item_id: approval.item_id.or(source.item_id),
        ts: Utc::now(),
        source: UniversalEventSource::ControlPlane,
        native: source.native.clone(),
        event: UniversalEventKind::ApprovalRequested {
            approval: Box::new(approval),
        },
    }
}

fn approval_resolved_envelope(
    source: &UniversalEventEnvelope,
    approval: &ApprovalRequest,
    decision: &ApprovalDecision,
    resolved_by_user_id: Option<UserId>,
) -> UniversalEventEnvelope {
    let resolved_at = approval.resolved_at.unwrap_or_else(Utc::now);
    UniversalEventEnvelope {
        event_id: Uuid::new_v4().to_string(),
        seq: UniversalSeq::zero(),
        session_id: source.session_id,
        turn_id: approval.turn_id.or(source.turn_id),
        item_id: approval.item_id.or(source.item_id),
        ts: resolved_at,
        source: UniversalEventSource::ControlPlane,
        native: source.native.clone(),
        event: UniversalEventKind::ApprovalResolved {
            approval_id: approval.approval_id,
            status: approval_decision_universal_status(decision),
            resolved_at,
            resolved_by_user_id,
            native: source.native.clone(),
        },
    }
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

fn discovered_history_events(
    session_id: SessionId,
    _owner_user_id: UserId,
    history: &[DiscoveredSessionHistoryItem],
) -> Vec<UniversalEventEnvelope> {
    history
        .iter()
        .enumerate()
        .map(|(index, item)| {
            let event_id = Uuid::new_v4().to_string();
            let ts = Utc::now();
            let (turn_id, item_id, native, event) =
                discovered_history_universal_event(session_id, index, item, &event_id, ts);
            UniversalEventEnvelope {
                event_id,
                seq: UniversalSeq::zero(),
                session_id,
                turn_id,
                item_id,
                ts,
                source: UniversalEventSource::Native,
                native,
                event,
            }
        })
        .collect()
}

fn discovered_history_universal_event(
    session_id: SessionId,
    index: usize,
    item: &DiscoveredSessionHistoryItem,
    event_id: &str,
    ts: DateTime<Utc>,
) -> (
    Option<agenter_core::TurnId>,
    Option<agenter_core::ItemId>,
    Option<NativeRef>,
    UniversalEventKind,
) {
    match item {
        DiscoveredSessionHistoryItem::UserMessage {
            message_id,
            content,
        } => {
            let item_id = universal_projection_item_id(
                message_id
                    .as_deref()
                    .unwrap_or(&format!("discovery-user-{index}")),
            );
            (
                None,
                Some(item_id),
                discovery_native_ref("user_message", message_id.as_deref(), Some(content), None),
                UniversalEventKind::ItemCreated {
                    item: Box::new(text_item(
                        session_id,
                        item_id,
                        ItemRole::User,
                        content.clone(),
                    )),
                },
            )
        }
        DiscoveredSessionHistoryItem::AgentMessage {
            message_id,
            content,
        } => {
            let item_id = universal_projection_item_id(message_id);
            (
                None,
                Some(item_id),
                discovery_native_ref("assistant_message", Some(message_id), Some(content), None),
                UniversalEventKind::ItemCreated {
                    item: Box::new(text_item(
                        session_id,
                        item_id,
                        ItemRole::Assistant,
                        content.clone(),
                    )),
                },
            )
        }
        DiscoveredSessionHistoryItem::Plan {
            plan_id,
            title,
            content,
            provider_payload,
        } => (
            None,
            None,
            discovery_native_ref(
                "plan",
                Some(plan_id),
                title.as_deref(),
                provider_payload.as_ref(),
            ),
            UniversalEventKind::PlanUpdated {
                plan: agenter_core::PlanState {
                    plan_id: agenter_core::PlanId::from_uuid(universal_projection_uuid(plan_id)),
                    session_id,
                    turn_id: None,
                    status: agenter_core::PlanStatus::Draft,
                    title: title.clone(),
                    content: Some(content.clone()),
                    entries: Vec::new(),
                    artifact_refs: Vec::new(),
                    source: agenter_core::PlanSource::NativeStructured,
                    partial: false,
                    updated_at: Some(ts),
                },
            },
        ),
        DiscoveredSessionHistoryItem::Tool {
            tool_call_id,
            name,
            title,
            status,
            input,
            output,
            provider_payload,
        } => {
            let item_id = universal_projection_item_id(tool_call_id);
            let status = match status {
                DiscoveredToolStatus::Running => ItemStatus::Streaming,
                DiscoveredToolStatus::Completed => ItemStatus::Completed,
                DiscoveredToolStatus::Failed => ItemStatus::Failed,
            };
            (
                None,
                Some(item_id),
                discovery_native_ref(
                    "tool",
                    Some(tool_call_id),
                    title.as_deref(),
                    provider_payload.as_ref(),
                ),
                UniversalEventKind::ItemCreated {
                    item: Box::new(ItemState {
                        item_id,
                        session_id,
                        turn_id: None,
                        role: ItemRole::Tool,
                        status: status.clone(),
                        content: vec![ContentBlock {
                            block_id: format!("tool-{tool_call_id}"),
                            kind: ContentBlockKind::ToolCall,
                            text: Some(title.clone().unwrap_or_else(|| name.clone())),
                            mime_type: None,
                            artifact_id: None,
                        }],
                        tool: Some(ToolProjection {
                            kind: ToolProjectionKind::Tool,
                            subkind: None,
                            name: name.clone(),
                            title: title.clone().unwrap_or_else(|| name.replace('_', " ")),
                            status,
                            detail: None,
                            input_summary: input.as_ref().map(compact_json_summary),
                            output_summary: output.as_ref().map(compact_json_summary),
                            command: None,
                            subagent: None,
                            mcp: None,
                        }),
                        native: discovery_native_ref(
                            "tool",
                            Some(tool_call_id),
                            title.as_deref(),
                            provider_payload.as_ref(),
                        ),
                    }),
                },
            )
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
            let item_id = universal_projection_item_id(command_id);
            let status = if !*success {
                ItemStatus::Failed
            } else {
                ItemStatus::Completed
            };
            (
                None,
                Some(item_id),
                discovery_native_ref(
                    "command",
                    Some(command_id),
                    Some(command),
                    provider_payload.as_ref(),
                ),
                UniversalEventKind::ItemCreated {
                    item: Box::new(ItemState {
                        item_id,
                        session_id,
                        turn_id: None,
                        role: ItemRole::Tool,
                        status: status.clone(),
                        content: vec![ContentBlock {
                            block_id: format!("command-{command_id}"),
                            kind: ContentBlockKind::CommandOutput,
                            text: output.clone().filter(|output| !output.is_empty()),
                            mime_type: None,
                            artifact_id: None,
                        }],
                        tool: Some(ToolProjection {
                            kind: ToolProjectionKind::Command,
                            subkind: Some("command".to_owned()),
                            name: "command".to_owned(),
                            title: command.clone(),
                            status,
                            detail: None,
                            input_summary: None,
                            output_summary: output.clone(),
                            command: Some(ToolCommandProjection {
                                command: command.clone(),
                                cwd: cwd.clone(),
                                source: source.clone(),
                                process_id: process_id.clone(),
                                actions: actions
                                    .iter()
                                    .map(|action| ToolActionProjection {
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
                                    })
                                    .collect(),
                                exit_code: *exit_code,
                                duration_ms: *duration_ms,
                                success: Some(*success),
                            }),
                            subagent: None,
                            mcp: None,
                        }),
                        native: discovery_native_ref(
                            "command",
                            Some(command_id),
                            Some(command),
                            provider_payload.as_ref(),
                        ),
                    }),
                },
            )
        }
        DiscoveredSessionHistoryItem::FileChange {
            change_id,
            path,
            change_kind,
            status,
            diff,
            provider_payload,
        } => {
            let diff_status = match status {
                DiscoveredFileChangeStatus::Applied
                | DiscoveredFileChangeStatus::Rejected
                | DiscoveredFileChangeStatus::Proposed => change_kind.clone(),
            };
            (
                None,
                None,
                discovery_native_ref(
                    "file_change",
                    Some(change_id),
                    Some(path),
                    provider_payload.as_ref(),
                ),
                UniversalEventKind::DiffUpdated {
                    diff: agenter_core::DiffState {
                        diff_id: agenter_core::DiffId::from_uuid(universal_projection_uuid(
                            change_id,
                        )),
                        session_id,
                        turn_id: None,
                        title: Some(path.clone()),
                        files: vec![DiffFile {
                            path: path.clone(),
                            status: diff_status,
                            diff: diff.clone(),
                        }],
                        updated_at: Some(ts),
                    },
                },
            )
        }
        DiscoveredSessionHistoryItem::NativeNotification {
            event_id: native_event_id,
            category,
            title,
            detail,
            status,
            provider_payload,
        } => (
            None,
            None,
            discovery_native_ref(
                category,
                native_event_id.as_deref().or(Some(event_id)),
                Some(title),
                provider_payload.as_ref(),
            ),
            UniversalEventKind::ProviderNotification {
                notification: ProviderNotification {
                    category: category.clone(),
                    title: title.clone(),
                    detail: detail.clone(),
                    status: status.clone(),
                    severity: Some(ProviderNotificationSeverity::Info),
                    subject: None,
                },
            },
        ),
    }
}

fn text_item(
    session_id: SessionId,
    item_id: agenter_core::ItemId,
    role: ItemRole,
    text: String,
) -> ItemState {
    ItemState {
        item_id,
        session_id,
        turn_id: None,
        role,
        status: ItemStatus::Completed,
        content: vec![ContentBlock {
            block_id: format!("text-{item_id}"),
            kind: ContentBlockKind::Text,
            text: Some(text),
            mime_type: None,
            artifact_id: None,
        }],
        tool: None,
        native: None,
    }
}

fn discovery_native_ref(
    method: &str,
    native_id: Option<&str>,
    summary: Option<&str>,
    raw_payload: Option<&serde_json::Value>,
) -> Option<NativeRef> {
    Some(NativeRef {
        protocol: "provider.discovery".to_owned(),
        method: Some(method.to_owned()),
        kind: Some("history".to_owned()),
        native_id: native_id.map(str::to_owned),
        summary: summary.map(str::to_owned),
        hash: None,
        pointer: None,
        raw_payload: raw_payload.cloned(),
    })
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
        let (sender, _) = broadcast::channel(SESSION_EVENT_BROADCAST_LIMIT);
        Self {
            sender,
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
    use super::*;
    use agenter_core::{ApprovalKind, ApprovalStatus as UniversalApprovalStatus, ItemId, TurnId};

    async fn test_pool() -> sqlx::PgPool {
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

    fn envelope(session_id: SessionId, event: UniversalEventKind) -> UniversalEventEnvelope {
        UniversalEventEnvelope {
            event_id: Uuid::new_v4().to_string(),
            seq: UniversalSeq::new(1),
            session_id,
            turn_id: None,
            item_id: None,
            ts: Utc::now(),
            source: UniversalEventSource::Runner,
            native: None,
            event,
        }
    }

    #[test]
    fn discovered_history_imports_universal_events() {
        let session_id = SessionId::new();
        let events = discovered_history_events(
            session_id,
            UserId::new(),
            &[
                DiscoveredSessionHistoryItem::UserMessage {
                    message_id: Some("u1".to_owned()),
                    content: "hello".to_owned(),
                },
                DiscoveredSessionHistoryItem::AgentMessage {
                    message_id: "a1".to_owned(),
                    content: "hi".to_owned(),
                },
            ],
        );

        assert_eq!(events.len(), 2);
        assert!(matches!(
            events[0].event,
            UniversalEventKind::ItemCreated { .. }
        ));
        assert_eq!(events[0].session_id, session_id);
        assert_eq!(events[0].source, UniversalEventSource::Native);
    }

    #[test]
    fn discovered_history_preserves_provider_payload_in_native_refs() {
        let session_id = SessionId::new();
        let payload = serde_json::json!({
            "method": "rawResponseItem/completed",
            "params": { "item": { "id": "native-1" } }
        });
        let events = discovered_history_events(
            session_id,
            UserId::new(),
            &[DiscoveredSessionHistoryItem::NativeNotification {
                event_id: Some("native-1".to_owned()),
                category: "raw".to_owned(),
                title: "Raw provider event".to_owned(),
                detail: None,
                status: None,
                provider_payload: Some(payload.clone()),
            }],
        );

        assert_eq!(events.len(), 1);
        assert_eq!(
            events[0]
                .native
                .as_ref()
                .and_then(|native| native.raw_payload.as_ref()),
            Some(&payload)
        );
    }

    #[test]
    fn reducer_sets_missing_structured_request_timestamps_from_envelope() {
        let session_id = SessionId::new();
        let ts = DateTime::parse_from_rfc3339("2026-05-06T10:05:00Z")
            .expect("valid timestamp")
            .with_timezone(&Utc);
        let approval_id = ApprovalId::new();
        let question_id = QuestionId::new();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };

        apply_universal_event_to_snapshot(
            &mut snapshot,
            &UniversalEventEnvelope {
                ts,
                event: UniversalEventKind::ApprovalRequested {
                    approval: Box::new(ApprovalRequest {
                        approval_id,
                        session_id,
                        turn_id: None,
                        item_id: None,
                        kind: ApprovalKind::Command,
                        title: "Run command".to_owned(),
                        details: None,
                        options: Vec::new(),
                        status: UniversalApprovalStatus::Pending,
                        risk: None,
                        subject: None,
                        native_request_id: None,
                        native_blocking: true,
                        policy: None,
                        native: None,
                        requested_at: None,
                        resolved_at: None,
                        resolving_decision: None,
                    }),
                },
                ..envelope(
                    session_id,
                    UniversalEventKind::ProviderNotification {
                        notification: ProviderNotification {
                            category: "test".to_owned(),
                            title: "placeholder".to_owned(),
                            detail: None,
                            status: None,
                            severity: None,
                            subject: None,
                        },
                    },
                )
            },
        );

        apply_universal_event_to_snapshot(
            &mut snapshot,
            &UniversalEventEnvelope {
                ts,
                event: UniversalEventKind::QuestionRequested {
                    question: Box::new(QuestionState {
                        question_id,
                        session_id,
                        turn_id: None,
                        title: "Input".to_owned(),
                        description: None,
                        fields: Vec::new(),
                        status: QuestionStatus::Pending,
                        answer: None,
                        native_request_id: None,
                        native_blocking: true,
                        native: None,
                        requested_at: None,
                        answered_at: None,
                    }),
                },
                ..envelope(
                    session_id,
                    UniversalEventKind::ProviderNotification {
                        notification: ProviderNotification {
                            category: "test".to_owned(),
                            title: "placeholder".to_owned(),
                            detail: None,
                            status: None,
                            severity: None,
                            subject: None,
                        },
                    },
                )
            },
        );

        assert_eq!(
            snapshot.approvals[&approval_id].requested_at,
            Some(ts),
            "approval.requested should get a durable ordering timestamp"
        );
        assert_eq!(
            snapshot.questions[&question_id].requested_at,
            Some(ts),
            "question.requested should get a durable ordering timestamp"
        );
    }

    #[tokio::test]
    async fn approval_lifecycle_lists_resolving_universal_request() {
        let state = AppState::new(
            "runner-token".to_owned(),
            CookieSecurity::DevelopmentInsecure,
        );
        let session_id = SessionId::new();
        let approval_id = ApprovalId::new();
        let request = ApprovalRequest {
            approval_id,
            session_id,
            turn_id: Some(TurnId::new()),
            item_id: None,
            kind: ApprovalKind::Command,
            title: "Run command".to_owned(),
            details: Some("cargo test".to_owned()),
            options: Vec::new(),
            status: UniversalApprovalStatus::Pending,
            risk: None,
            subject: Some("cargo test".to_owned()),
            native_request_id: None,
            native_blocking: true,
            policy: None,
            native: None,
            requested_at: Some(Utc::now()),
            resolved_at: None,
            resolving_decision: None,
        };

        let stored = state
            .accept_runner_agent_event(AgentUniversalEvent {
                protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
                session_id,
                event_id: None,
                turn_id: request.turn_id,
                item_id: None,
                ts: None,
                source: UniversalEventSource::Runner,
                native: None,
                event: UniversalEventKind::ApprovalRequested {
                    approval: Box::new(request),
                },
            })
            .await
            .expect("stored approval");

        assert!(matches!(
            stored.event,
            UniversalEventKind::ApprovalRequested { .. }
        ));
        assert_eq!(state.pending_approval_requests(session_id).await.len(), 1);
        assert!(matches!(
            state
                .begin_approval_resolution(approval_id, ApprovalDecision::Accept)
                .await,
            ApprovalResolutionStart::Started
        ));
        let requests = state.pending_approval_requests(session_id).await;
        assert_eq!(requests[0].status, UniversalApprovalStatus::Resolving);
        assert_eq!(
            requests[0].resolving_decision,
            Some(ApprovalDecision::Accept)
        );
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at a disposable Postgres database"]
    async fn runner_event_after_restart_uses_persisted_session_workspace_for_receipt_append() {
        let pool = test_pool().await;
        let suffix = Uuid::new_v4();
        let user = agenter_db::create_user(
            &pool,
            &format!("restart-{suffix}@example.test"),
            Some("Restart Test"),
        )
        .await
        .expect("create user");
        let runner_id = RunnerId::new();
        agenter_db::upsert_runner_with_id(&pool, runner_id, "restart-runner", Some("test"))
            .await
            .expect("upsert runner");
        let workspace_id = WorkspaceId::from_uuid(Uuid::new_v4());
        agenter_db::upsert_workspace_with_id(
            &pool,
            workspace_id,
            runner_id,
            &format!("/tmp/agenter-restart-{suffix}"),
            Some("Restart Workspace"),
        )
        .await
        .expect("upsert workspace");
        let session_id = SessionId::new();
        agenter_db::create_session_with_id(
            &pool,
            agenter_db::CreateSessionRecord {
                session_id,
                owner_user_id: user.user_id,
                runner_id,
                workspace_id,
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                external_session_id: Some("thread-after-restart".to_owned()),
                title: Some("Restarted Codex".to_owned()),
                status: SessionStatus::Idle,
                usage_snapshot: None,
                turn_settings: None,
            },
        )
        .await
        .expect("create session");

        let restarted_state = AppState::new_with_test_db_pool(pool.clone());
        let stored = restarted_state
            .accept_runner_agent_event_with_receipt(
                runner_id,
                Some(1),
                AgentUniversalEvent {
                    protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
                    session_id,
                    event_id: None,
                    turn_id: None,
                    item_id: None,
                    ts: None,
                    source: UniversalEventSource::Native,
                    native: None,
                    event: UniversalEventKind::SessionStatusChanged {
                        status: SessionStatus::Running,
                        reason: Some("Codex thread resumed for send".to_owned()),
                    },
                },
            )
            .await
            .expect("runner event should append using DB session workspace");

        assert!(stored.is_some());
        assert_eq!(
            restarted_state.workspace_id_for_session(session_id).await,
            Some(workspace_id)
        );
    }

    #[tokio::test]
    #[ignore = "requires DATABASE_URL pointing at a disposable Postgres database"]
    async fn db_snapshot_replay_with_snapshot_is_not_truncated_at_live_replay_limit() {
        let pool = test_pool().await;
        let suffix = Uuid::new_v4();
        let user = agenter_db::create_user(
            &pool,
            &format!("snapshot-replay-{suffix}@example.test"),
            Some("Snapshot Replay Test"),
        )
        .await
        .expect("create user");
        let runner_id = RunnerId::new();
        agenter_db::upsert_runner_with_id(&pool, runner_id, "snapshot-replay-runner", Some("test"))
            .await
            .expect("upsert runner");
        let workspace_id = WorkspaceId::from_uuid(Uuid::new_v4());
        agenter_db::upsert_workspace_with_id(
            &pool,
            workspace_id,
            runner_id,
            &format!("/tmp/agenter-snapshot-replay-{suffix}"),
            Some("Snapshot Replay Workspace"),
        )
        .await
        .expect("upsert workspace");
        let session_id = SessionId::new();
        agenter_db::create_session_with_id(
            &pool,
            agenter_db::CreateSessionRecord {
                session_id,
                owner_user_id: user.user_id,
                runner_id,
                workspace_id,
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                external_session_id: Some("thread-snapshot-replay".to_owned()),
                title: Some("Snapshot Replay".to_owned()),
                status: SessionStatus::Idle,
                usage_snapshot: None,
                turn_settings: None,
            },
        )
        .await
        .expect("create session");

        for index in 0..=UNIVERSAL_EVENT_REPLAY_LIMIT {
            let envelope = UniversalEventEnvelope {
                event_id: Uuid::new_v4().to_string(),
                seq: UniversalSeq::zero(),
                session_id,
                turn_id: None,
                item_id: None,
                ts: Utc::now(),
                source: UniversalEventSource::Runner,
                native: None,
                event: UniversalEventKind::ProviderNotification {
                    notification: ProviderNotification {
                        category: "replay-test".to_owned(),
                        title: format!("event {index}"),
                        detail: None,
                        status: None,
                        severity: None,
                        subject: None,
                    },
                },
            };
            agenter_db::append_universal_event_reducing_snapshot(
                &pool,
                workspace_id,
                envelope,
                None,
                apply_universal_event_to_snapshot,
            )
            .await
            .expect("append universal event");
        }

        let state = AppState::new_with_test_db_pool(pool);
        let replay = state
            .session_snapshot_replay(session_id, Some(UniversalSeq::zero()), true)
            .await
            .expect("snapshot replay");

        assert!(replay.replay_complete);
        assert_eq!(replay.events.len(), UNIVERSAL_EVENT_REPLAY_LIMIT + 1);
        assert_eq!(
            replay.replay_through_seq, replay.snapshot_seq,
            "full snapshot reload should replay through the durable snapshot cursor"
        );
    }

    #[test]
    fn universal_reducer_reconstructs_snapshot_from_ordered_events() {
        let session_id = SessionId::new();
        let item_id = ItemId::new();
        let mut snapshot = SessionSnapshot {
            session_id,
            ..SessionSnapshot::default()
        };

        let created = envelope(
            session_id,
            UniversalEventKind::ItemCreated {
                item: Box::new(ItemState {
                    item_id,
                    session_id,
                    turn_id: None,
                    role: ItemRole::Assistant,
                    status: ItemStatus::Created,
                    content: Vec::new(),
                    tool: None,
                    native: None,
                }),
            },
        );
        let mut delta = envelope(
            session_id,
            UniversalEventKind::ContentDelta {
                block_id: "text-1".to_owned(),
                kind: Some(ContentBlockKind::Text),
                delta: "hello".to_owned(),
            },
        );
        delta.seq = UniversalSeq::new(2);
        delta.item_id = Some(item_id);

        apply_universal_event_to_snapshot(&mut snapshot, &created);
        apply_universal_event_to_snapshot(&mut snapshot, &delta);

        let item = snapshot.items.get(&item_id).expect("item");
        assert_eq!(item.status, ItemStatus::Streaming);
        assert_eq!(item.content[0].text.as_deref(), Some("hello"));
    }
}
