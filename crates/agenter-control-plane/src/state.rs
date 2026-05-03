use std::{collections::HashMap, sync::Arc, time::Duration};

use agenter_core::{
    AgentProviderId, AgentQuestionAnswer, AgentTurnSettings, AppEvent, ApprovalDecision,
    ApprovalId, ApprovalResolutionState, CommandAction, CommandOutputStream, ProviderEvent,
    QuestionId, RunnerId, SessionId, SessionInfo, SessionStatus, SessionUsageContext,
    SessionUsageSnapshot, SessionUsageWindow, UserId, WorkspaceId, WorkspaceRef,
};
use agenter_protocol::{
    browser::BrowserEventEnvelope,
    runner::{
        DiscoveredFileChangeStatus, DiscoveredSessionHistoryItem, DiscoveredSessionHistoryStatus,
        DiscoveredSessions, DiscoveredToolStatus, RunnerCapabilities, RunnerResponseOutcome,
        RunnerServerMessage,
    },
    RequestId,
};
use chrono::{DateTime, Duration as ChronoDuration, Utc};
use serde_json::Value;
use tokio::{
    sync::{broadcast, mpsc, oneshot, Mutex},
    time::timeout,
};
use uuid::Uuid;

use crate::auth::CookieSecurity;
use crate::auth::{self, AuthenticatedUser, BootstrapAdmin};

const SESSION_EVENT_CACHE_LIMIT: usize = 128;
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
    pub resolved: bool,
}

/// Tracks client-visible approval lifecycle. Pending/Resolving carry the original
/// `approval_requested` envelope so reconnects still render cards after cache eviction.
#[derive(Clone, Debug)]
enum ApprovalStatus {
    Pending(Box<BrowserEventEnvelope>),
    Resolving {
        request: Box<BrowserEventEnvelope>,
        decision: ApprovalDecision,
    },
    Resolved(Box<BrowserEventEnvelope>),
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
    envelope: &BrowserEventEnvelope,
    approval_id: ApprovalId,
) -> bool {
    match &envelope.event {
        AppEvent::ApprovalRequested(req) => req.approval_id == approval_id,
        AppEvent::ApprovalResolved(res) => res.approval_id == approval_id,
        _ => false,
    }
}

fn approval_request_envelope_with_state(
    envelope: &BrowserEventEnvelope,
    state: ApprovalResolutionState,
    decision: Option<ApprovalDecision>,
) -> BrowserEventEnvelope {
    let mut envelope = envelope.clone();
    if let AppEvent::ApprovalRequested(request) = &mut envelope.event {
        request.resolution_state = Some(state);
        request.resolving_decision = decision;
    }
    envelope
}

fn merge_pending_approval_envelopes_for_session(
    session_id: SessionId,
    mut envelopes: Vec<BrowserEventEnvelope>,
    registry: &Registry,
) -> Vec<BrowserEventEnvelope> {
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
            ApprovalStatus::Resolving { request, decision } => {
                approval_request_envelope_with_state(
                    request,
                    ApprovalResolutionState::Resolving,
                    Some(decision.clone()),
                )
            }
            ApprovalStatus::Resolved(_) => continue,
        };
        match envelopes
            .iter()
            .position(|e| envelope_references_approval_id(e, approval_id))
        {
            Some(index) => {
                if matches!(envelopes[index].event, AppEvent::ApprovalRequested(_)) {
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
    InProgress {
        session_id: SessionId,
        envelope: Box<BrowserEventEnvelope>,
    },
    AlreadyResolved {
        session_id: SessionId,
        envelope: Box<BrowserEventEnvelope>,
    },
    Started {
        session_id: SessionId,
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
    sender: broadcast::Sender<BrowserEventEnvelope>,
    cache: Vec<BrowserEventEnvelope>,
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
}

#[derive(Clone, Debug, Default, Eq, PartialEq, serde::Serialize)]
pub struct WorkspaceSessionRefreshSummary {
    pub discovered_count: usize,
    pub refreshed_cache_count: usize,
    pub skipped_failed_count: usize,
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
            }),
        })
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
            self.mark_runner_sessions_stopped(runner_id).await;
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

    pub async fn subscribe_session(
        &self,
        session_id: SessionId,
    ) -> (
        Vec<BrowserEventEnvelope>,
        broadcast::Receiver<BrowserEventEnvelope>,
    ) {
        let (mut cached, receiver) = {
            let mut sessions = self.inner.sessions.lock().await;
            let events = sessions
                .entry(session_id)
                .or_insert_with(SessionEvents::new);
            let receiver = events.sender.subscribe();
            (events.cache.clone(), receiver)
        };
        let registry = self.inner.registry.lock().await;
        cached = merge_pending_approval_envelopes_for_session(session_id, cached, &registry);
        (cached, receiver)
    }

    pub async fn session_history(
        &self,
        user_id: UserId,
        session_id: SessionId,
    ) -> Option<Vec<BrowserEventEnvelope>> {
        if !self.can_access_session(user_id, session_id).await {
            return None;
        }

        let mut history = if let Some(pool) = &self.inner.db_pool {
            agenter_db::list_event_cache(pool, session_id)
                .await
                .map(|events| {
                    events
                        .into_iter()
                        .filter_map(|event| {
                            serde_json::from_value::<AppEvent>(event.payload)
                                .map(|app_event| BrowserEventEnvelope {
                                    event_id: Some(event.event_id.to_string().into()),
                                    event: app_event,
                                })
                                .map_err(|error| {
                                    tracing::warn!(
                                        %session_id,
                                        event_id = %event.event_id,
                                        %error,
                                        "failed to decode cached app event"
                                    );
                                })
                                .ok()
                        })
                        .collect()
                })
                .unwrap_or_else(|error| {
                    tracing::warn!(%session_id, %error, "failed to load persisted session history");
                    Vec::new()
                })
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
    ) -> Vec<BrowserEventEnvelope> {
        let registry = self.inner.registry.lock().await;
        let mut out = Vec::new();
        for approval in registry.approvals.values() {
            if approval.session_id != session_id {
                continue;
            }
            match &approval.status {
                ApprovalStatus::Pending(env) => {
                    out.push(approval_request_envelope_with_state(
                        env,
                        ApprovalResolutionState::Pending,
                        None,
                    ));
                }
                ApprovalStatus::Resolving { request, decision } => {
                    out.push(approval_request_envelope_with_state(
                        request,
                        ApprovalResolutionState::Resolving,
                        Some(decision.clone()),
                    ));
                }
                ApprovalStatus::Resolved(_) => {}
            }
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
        event: AppEvent,
    ) -> BrowserEventEnvelope {
        let envelope = BrowserEventEnvelope {
            event_id: Some(Uuid::new_v4().to_string().into()),
            event,
        };
        match &envelope.event {
            AppEvent::ApprovalRequested(request) => {
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
            }
            AppEvent::ApprovalResolved(resolved) => {
                tracing::info!(
                    approval_id = %resolved.approval_id,
                    %session_id,
                    "approval resolved event received"
                );
                let mut registry = self.inner.registry.lock().await;
                match registry.approvals.get_mut(&resolved.approval_id) {
                    Some(approval) => match &approval.status {
                        ApprovalStatus::Resolved(existing) => return *existing.clone(),
                        ApprovalStatus::Pending(_) | ApprovalStatus::Resolving { .. } => {
                            approval.status = ApprovalStatus::Resolved(Box::new(envelope.clone()));
                        }
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
            AppEvent::QuestionRequested(request) => {
                tracing::info!(
                    question_id = %request.question_id,
                    %session_id,
                    "question requested"
                );
                self.inner.registry.lock().await.questions.insert(
                    request.question_id,
                    RegisteredQuestion {
                        session_id,
                        resolved: false,
                    },
                );
            }
            AppEvent::QuestionAnswered(answered) => {
                let mut registry = self.inner.registry.lock().await;
                if let Some(question) = registry.questions.get_mut(&answered.question_id) {
                    question.resolved = true;
                }
            }
            AppEvent::SessionStatusChanged(status) => {
                self.apply_session_status(status.session_id, status.status.clone())
                    .await;
            }
            AppEvent::ProviderEvent(provider_event)
            | AppEvent::TurnDiffUpdated(provider_event)
            | AppEvent::ItemReasoning(provider_event)
            | AppEvent::ServerRequestResolved(provider_event)
            | AppEvent::McpToolCallProgress(provider_event)
            | AppEvent::ThreadRealtimeEvent(provider_event) => {
                if let Some(usage) = self
                    .apply_provider_usage_event(session_id, provider_event)
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

        self.store_event(session_id, envelope).await
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

    async fn mark_runner_sessions_stopped(&self, runner_id: RunnerId) {
        let sessions = {
            let registry = self.inner.registry.lock().await;
            registry
                .sessions
                .values()
                .filter(|session| {
                    session.runner_id == runner_id
                        && should_stop_on_runner_disconnect(&session.status)
                })
                .map(|session| session.session_id)
                .collect::<Vec<_>>()
        };
        for session_id in sessions {
            self.publish_event(
                session_id,
                AppEvent::SessionStatusChanged(agenter_core::SessionStatusChangedEvent {
                    session_id,
                    status: SessionStatus::Stopped,
                    reason: Some("Runner disconnected.".to_owned()),
                }),
            )
            .await;
        }
    }

    async fn apply_provider_usage_event(
        &self,
        session_id: SessionId,
        event: &ProviderEvent,
    ) -> Option<SessionUsageSnapshot> {
        let usage_update = usage_snapshot_from_provider_event(event)?;
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
        let mut registry = self.inner.registry.lock().await;
        let Some(approval) = registry.approvals.get_mut(&approval_id) else {
            return ApprovalResolutionStart::Missing;
        };
        match &approval.status {
            ApprovalStatus::Pending(request_env) => {
                approval.status = ApprovalStatus::Resolving {
                    request: request_env.clone(),
                    decision,
                };
                tracing::debug!(%approval_id, session_id = %approval.session_id, "approval resolution started");
                ApprovalResolutionStart::Started {
                    session_id: approval.session_id,
                }
            }
            ApprovalStatus::Resolving { request, decision } => {
                ApprovalResolutionStart::InProgress {
                    session_id: approval.session_id,
                    envelope: Box::new(approval_request_envelope_with_state(
                        request,
                        ApprovalResolutionState::Resolving,
                        Some(decision.clone()),
                    )),
                }
            }
            ApprovalStatus::Resolved(envelope) => ApprovalResolutionStart::AlreadyResolved {
                session_id: approval.session_id,
                envelope: envelope.clone(),
            },
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
        event: AppEvent,
    ) -> Option<BrowserEventEnvelope> {
        let envelope = BrowserEventEnvelope {
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
                ApprovalStatus::Resolved(existing) => return Some(*existing.clone()),
                ApprovalStatus::Pending(_) => return None,
                ApprovalStatus::Resolving { .. } => {
                    approval.status = ApprovalStatus::Resolved(Box::new(envelope.clone()));
                }
            }
        }

        self.store_event(session_id, envelope.clone()).await;
        Some(envelope)
    }

    pub async fn question_session(&self, question_id: QuestionId) -> Option<SessionId> {
        self.inner
            .registry
            .lock()
            .await
            .questions
            .get(&question_id)
            .filter(|question| !question.resolved)
            .map(|question| question.session_id)
    }

    pub async fn finish_question_answer(
        &self,
        session_id: SessionId,
        answer: AgentQuestionAnswer,
    ) -> BrowserEventEnvelope {
        let event = AppEvent::QuestionAnswered(agenter_core::QuestionAnsweredEvent {
            session_id,
            question_id: answer.question_id,
            answer,
            provider_payload: None,
        });
        self.publish_event(session_id, event).await
    }

    async fn store_event(
        &self,
        session_id: SessionId,
        mut envelope: BrowserEventEnvelope,
    ) -> BrowserEventEnvelope {
        if let Some(pool) = &self.inner.db_pool {
            match agenter_db::append_event_cache(pool, session_id, &envelope.event).await {
                Ok(cached) => {
                    envelope.event_id = Some(cached.event_id.to_string().into());
                }
                Err(error) => {
                    tracing::warn!(%session_id, %error, "failed to persist app event cache row");
                }
            }
        }
        let sender = {
            let mut sessions = self.inner.sessions.lock().await;
            let events = sessions
                .entry(session_id)
                .or_insert_with(SessionEvents::new);
            events.cache.push(envelope.clone());
            if events.cache.len() > SESSION_EVENT_CACHE_LIMIT {
                let overflow = events.cache.len() - SESSION_EVENT_CACHE_LIMIT;
                events.cache.drain(..overflow);
            }
            events.sender.clone()
        };

        let _ = sender.send(envelope.clone());
        tracing::debug!(
            %session_id,
            event_id = envelope.event_id.as_ref().map(ToString::to_string).as_deref(),
            event_type = app_event_name(&envelope.event),
            "stored and broadcast app event"
        );
        envelope
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
            let session = if let Some(pool) = &self.inner.db_pool {
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
                    Ok(session) => RegisteredSession {
                        session_id: session.session_id,
                        owner_user_id: session.owner_user_id,
                        runner_id: session.runner_id,
                        workspace: discovered.workspace.clone(),
                        provider_id: session.provider_id,
                        status: session.status,
                        title: session.title,
                        external_session_id: session.external_session_id,
                        turn_settings: session.turn_settings,
                        usage: session.usage_snapshot,
                        created_at: session.created_at,
                        updated_at: session.updated_at,
                    },
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
                summary.skipped_failed_count += 1;
                continue;
            }
            if !discovered_events.is_empty() || matches!(mode, SessionImportMode::Forced) {
                if let Some(pool) = &self.inner.db_pool {
                    if let Err(error) =
                        agenter_db::clear_event_cache(pool, session.session_id).await
                    {
                        tracing::warn!(%session.session_id, %error, "failed to clear discovered session event cache");
                    }
                }
                {
                    let mut sessions = self.inner.sessions.lock().await;
                    sessions
                        .entry(session.session_id)
                        .or_insert_with(SessionEvents::new)
                        .cache
                        .clear();
                }
                summary.refreshed_cache_count += 1;
                for event in discovered_events {
                    self.store_event(
                        session.session_id,
                        BrowserEventEnvelope {
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

    pub async fn take_refresh_summary(
        &self,
        request_id: &RequestId,
    ) -> Option<WorkspaceSessionRefreshSummary> {
        self.inner.refresh_summaries.lock().await.remove(request_id)
    }
}

fn discovered_history_events(
    session_id: SessionId,
    owner_user_id: UserId,
    history: &[DiscoveredSessionHistoryItem],
) -> Vec<AppEvent> {
    history
        .iter()
        .flat_map(|item| match item {
            DiscoveredSessionHistoryItem::UserMessage {
                message_id,
                content,
            } => vec![AppEvent::UserMessage(agenter_core::UserMessageEvent {
                session_id,
                message_id: message_id.clone(),
                author_user_id: Some(owner_user_id),
                content: content.clone(),
            })],
            DiscoveredSessionHistoryItem::AgentMessage {
                message_id,
                content,
            } => vec![AppEvent::AgentMessageCompleted(
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
            } => vec![AppEvent::PlanUpdated(agenter_core::PlanEvent {
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
                        vec![AppEvent::ToolCompleted(event)]
                    }
                    DiscoveredToolStatus::Running => vec![AppEvent::ToolStarted(event)],
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
                let mut events = vec![AppEvent::CommandStarted(agenter_core::CommandEvent {
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
                })];
                if let Some(output) = output {
                    if !output.is_empty() {
                        events.push(AppEvent::CommandOutputDelta(
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
                events.push(AppEvent::CommandCompleted(
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
                    DiscoveredFileChangeStatus::Applied => vec![AppEvent::FileChangeApplied(event)],
                    DiscoveredFileChangeStatus::Rejected => {
                        vec![AppEvent::FileChangeRejected(event)]
                    }
                    DiscoveredFileChangeStatus::Proposed => {
                        vec![AppEvent::FileChangeProposed(event)]
                    }
                }
            }
            DiscoveredSessionHistoryItem::ProviderEvent {
                event_id,
                category,
                title,
                detail,
                status,
                provider_payload,
            } => vec![AppEvent::ProviderEvent(agenter_core::ProviderEvent {
                session_id,
                provider_id: AgentProviderId::from(AgentProviderId::CODEX),
                method: category.clone(),
                event_id: event_id.clone(),
                category: category.clone(),
                title: title.clone(),
                detail: detail.clone(),
                status: status.clone(),
                provider_payload: provider_payload.clone(),
            })],
        })
        .collect()
}

fn should_stop_on_runner_disconnect(status: &SessionStatus) -> bool {
    matches!(
        status,
        SessionStatus::Starting
            | SessionStatus::Running
            | SessionStatus::WaitingForInput
            | SessionStatus::WaitingForApproval
            | SessionStatus::Idle
            | SessionStatus::Completed
    )
}

fn app_event_name(event: &AppEvent) -> &'static str {
    match event {
        AppEvent::SessionStarted(_) => "session_started",
        AppEvent::SessionStatusChanged(_) => "session_status_changed",
        AppEvent::UserMessage(_) => "user_message",
        AppEvent::AgentMessageDelta(_) => "agent_message_delta",
        AppEvent::AgentMessageCompleted(_) => "agent_message_completed",
        AppEvent::PlanUpdated(_) => "plan_updated",
        AppEvent::ToolStarted(_) => "tool_started",
        AppEvent::ToolUpdated(_) => "tool_updated",
        AppEvent::ToolCompleted(_) => "tool_completed",
        AppEvent::CommandStarted(_) => "command_started",
        AppEvent::CommandOutputDelta(_) => "command_output_delta",
        AppEvent::CommandCompleted(_) => "command_completed",
        AppEvent::FileChangeProposed(_) => "file_change_proposed",
        AppEvent::FileChangeApplied(_) => "file_change_applied",
        AppEvent::FileChangeRejected(_) => "file_change_rejected",
        AppEvent::ApprovalRequested(_) => "approval_requested",
        AppEvent::ApprovalResolved(_) => "approval_resolved",
        AppEvent::QuestionRequested(_) => "question_requested",
        AppEvent::QuestionAnswered(_) => "question_answered",
        AppEvent::TurnDiffUpdated(_) => "turn_diff_updated",
        AppEvent::ItemReasoning(_) => "item_reasoning",
        AppEvent::ServerRequestResolved(_) => "server_request_resolved",
        AppEvent::McpToolCallProgress(_) => "mcp_tool_call_progress",
        AppEvent::ThreadRealtimeEvent(_) => "thread_realtime_event",
        AppEvent::ProviderEvent(_) => "provider_event",
        AppEvent::Error(_) => "error",
    }
}

impl SessionEvents {
    fn new() -> Self {
        let (sender, _) = broadcast::channel(SESSION_EVENT_CACHE_LIMIT);
        Self {
            sender,
            cache: Vec::new(),
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

fn usage_snapshot_from_provider_event(event: &ProviderEvent) -> Option<SessionUsageSnapshot> {
    match event.category.as_str() {
        "token_usage" => usage_snapshot_from_token_usage(event.provider_payload.as_ref()),
        "rate_limits" => usage_snapshot_from_rate_limits(event.provider_payload.as_ref()),
        _ => None,
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
        ],
    );
    let total_tokens = integer_at(
        payload,
        &[
            "/params/tokenUsage/modelContextWindow",
            "/params/modelContextWindow",
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
        AgentProviderId, AppEvent, ApprovalDecision, ApprovalId, ApprovalKind,
        ApprovalRequestEvent, RunnerId, SessionId, UserId, UserMessageEvent, WorkspaceId,
        WorkspaceRef,
    };
    use agenter_protocol::runner::{RunnerHeartbeatAck, RunnerServerMessage};

    use super::*;

    #[tokio::test]
    async fn subscribers_receive_published_session_events() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);
        let session_id = SessionId::new();
        let (cached, mut subscription) = state.subscribe_session(session_id).await;
        assert!(cached.is_empty());

        state
            .publish_event(
                session_id,
                AppEvent::UserMessage(UserMessageEvent {
                    session_id,
                    message_id: Some("user-1".to_owned()),
                    author_user_id: None,
                    content: "hello".to_owned(),
                }),
            )
            .await;

        let received = subscription.recv().await.expect("event is broadcast");
        assert!(matches!(received.event, AppEvent::UserMessage(_)));
        assert!(received.event_id.is_some());
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
                AppEvent::ProviderEvent(agenter_core::ProviderEvent {
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
                AppEvent::ProviderEvent(agenter_core::ProviderEvent {
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
                AppEvent::SessionStatusChanged(agenter_core::SessionStatusChangedEvent {
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
    async fn runner_disconnect_marks_active_sessions_stopped() {
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
                AppEvent::SessionStatusChanged(agenter_core::SessionStatusChangedEvent {
                    session_id: active.session_id,
                    status: SessionStatus::Running,
                    reason: None,
                }),
            )
            .await;
        state
            .publish_event(
                failed.session_id,
                AppEvent::SessionStatusChanged(agenter_core::SessionStatusChangedEvent {
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
        assert_eq!(active_status, SessionStatus::Stopped);
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
                AppEvent::ApprovalRequested(ApprovalRequestEvent {
                    session_id: sid,
                    approval_id,
                    kind: ApprovalKind::FileChange,
                    title: "Approve change".to_owned(),
                    details: Some("files".to_owned()),
                    expires_at: None,
                    presentation: None,
                    resolution_state: None,
                    resolving_decision: None,
                    provider_payload: None,
                }),
            )
            .await;

        for i in 0..super::SESSION_EVENT_CACHE_LIMIT {
            state
                .publish_event(
                    sid,
                    AppEvent::UserMessage(UserMessageEvent {
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
                .any(|e| matches!(e.event, AppEvent::ApprovalRequested(_))),
            "oldest approval_requested should be evicted from ring buffer"
        );

        let history = state.session_history(owner, sid).await.expect("history");
        let found = history.iter().find_map(|env| match &env.event {
            AppEvent::ApprovalRequested(req) if req.approval_id == approval_id => Some(()),
            _ => None,
        });
        assert!(
            found.is_some(),
            "history should merge pending approval from registry"
        );

        let pending = state.pending_approval_request_envelopes(sid).await;
        assert_eq!(pending.len(), 1);
        assert!(matches!(pending[0].event, AppEvent::ApprovalRequested(_)));
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
                AppEvent::ApprovalRequested(ApprovalRequestEvent {
                    session_id: sid,
                    approval_id,
                    kind: ApprovalKind::FileChange,
                    title: "Approve change".to_owned(),
                    details: Some("files".to_owned()),
                    expires_at: None,
                    presentation: None,
                    resolution_state: None,
                    resolving_decision: None,
                    provider_payload: None,
                }),
            )
            .await;

        let started = state
            .begin_approval_resolution(approval_id, ApprovalDecision::Accept)
            .await;
        assert!(matches!(
            started,
            ApprovalResolutionStart::Started { session_id } if session_id == sid
        ));

        let history = state.session_history(owner, sid).await.expect("history");
        let approval = history
            .iter()
            .find_map(|env| match &env.event {
                AppEvent::ApprovalRequested(req) if req.approval_id == approval_id => Some(req),
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
        let event = AppEvent::UserMessage(UserMessageEvent {
            session_id,
            message_id: Some("cached".to_owned()),
            author_user_id: None,
            content: "cached".to_owned(),
        });
        state.publish_event(session_id, event).await;

        let (cached, mut subscription) = state.subscribe_session(session_id).await;
        assert_eq!(cached.len(), 1);
        assert!(matches!(cached[0].event, AppEvent::UserMessage(_)));

        state
            .publish_event(
                session_id,
                AppEvent::UserMessage(UserMessageEvent {
                    session_id,
                    message_id: Some("live".to_owned()),
                    author_user_id: None,
                    content: "live".to_owned(),
                }),
            )
            .await;
        assert!(matches!(
            subscription.recv().await.expect("live event").event,
            AppEvent::UserMessage(_)
        ));
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
                AppEvent::UserMessage(UserMessageEvent {
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
        assert_eq!(history.len(), 1);
        assert!(matches!(
            history[0].event,
            AppEvent::AgentMessageCompleted(_)
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
                AppEvent::UserMessage(UserMessageEvent {
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
        assert!(matches!(history[0].event, AppEvent::UserMessage(_)));
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
                DiscoveredSessionHistoryItem::ProviderEvent {
                    event_id: Some("compact-1".to_owned()),
                    category: "compaction".to_owned(),
                    title: "Context compacted".to_owned(),
                    detail: None,
                    status: Some("completed".to_owned()),
                    provider_payload: None,
                },
            ],
        );

        assert!(matches!(events[0], AppEvent::UserMessage(_)));
        assert!(matches!(events[1], AppEvent::AgentMessageCompleted(_)));
        assert!(matches!(events[2], AppEvent::CommandStarted(_)));
        assert!(matches!(events[3], AppEvent::CommandOutputDelta(_)));
        assert!(matches!(events[4], AppEvent::CommandCompleted(_)));
        assert!(matches!(events[5], AppEvent::ToolCompleted(_)));
        assert!(matches!(events[6], AppEvent::FileChangeApplied(_)));
        assert!(matches!(events[7], AppEvent::PlanUpdated(_)));
        assert!(matches!(events[8], AppEvent::ProviderEvent(_)));
    }

    #[test]
    fn discovered_history_codex_thread_item_maps_to_provider_app_event() {
        let session_id = SessionId::nil();
        let owner_user_id = UserId::nil();
        let events = discovered_history_events(
            session_id,
            owner_user_id,
            &[DiscoveredSessionHistoryItem::ProviderEvent {
                event_id: Some("og-1".to_owned()),
                category: "codex_thread_item".to_owned(),
                title: "orphanGadget".to_owned(),
                detail: Some("experimental row".to_owned()),
                status: Some("done".to_owned()),
                provider_payload: None,
            }],
        );

        assert_eq!(events.len(), 1);
        let AppEvent::ProviderEvent(pe) = &events[0] else {
            panic!("expected ProviderEvent app event");
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
}
