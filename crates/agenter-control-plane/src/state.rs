use std::{collections::HashMap, sync::Arc, time::Duration};

use agenter_core::{
    AgentProviderId, AgentQuestionAnswer, AgentTurnSettings, AppEvent, ApprovalId, CommandAction,
    CommandOutputStream, QuestionId, RunnerId, SessionId, SessionInfo, SessionStatus, UserId,
    WorkspaceId, WorkspaceRef,
};
use agenter_protocol::{
    browser::BrowserEventEnvelope,
    runner::{
        DiscoveredFileChangeStatus, DiscoveredSessionHistoryItem, DiscoveredSessions,
        DiscoveredToolStatus, RunnerCapabilities, RunnerResponseOutcome, RunnerServerMessage,
    },
    RequestId,
};
use tokio::{
    sync::{broadcast, mpsc, oneshot, Mutex},
    time::timeout,
};
use uuid::Uuid;

use crate::auth::CookieSecurity;
use crate::auth::{self, AuthenticatedUser, BootstrapAdmin};

const SESSION_EVENT_CACHE_LIMIT: usize = 128;

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

#[derive(Clone, Debug)]
enum ApprovalStatus {
    Pending,
    Resolving,
    Resolved(Box<BrowserEventEnvelope>),
}

#[derive(Clone, Debug)]
pub enum ApprovalResolutionStart {
    Missing,
    InProgress,
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
            let token = Uuid::new_v4().to_string();
            self.inner
                .auth_sessions
                .lock()
                .await
                .insert(token.clone(), user);
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
        self.inner.auth_sessions.lock().await.get(token).cloned()
    }

    pub async fn create_authenticated_session(&self, user: AuthenticatedUser) -> String {
        let token = Uuid::new_v4().to_string();
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
        let mut connections = self.inner.runner_connections.lock().await;
        if connections
            .get(&runner_id)
            .is_some_and(|connection| connection.connection_id == connection_id)
        {
            connections.remove(&runner_id);
            tracing::info!(%runner_id, %connection_id, "runner disconnected");
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
        let (response_sender, response_receiver) = oneshot::channel();
        self.inner
            .pending_runner_responses
            .lock()
            .await
            .insert((runner_id, request_id.clone()), response_sender);

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
        })
        .await
    }

    pub async fn register_session(&self, registration: SessionRegistration) -> RegisteredSession {
        let mut session = RegisteredSession {
            session_id: registration.session_id,
            owner_user_id: registration.owner_user_id,
            runner_id: registration.runner_id,
            workspace: registration.workspace,
            provider_id: registration.provider_id,
            status: SessionStatus::Running,
            title: registration.title,
            external_session_id: registration.external_session_id,
            turn_settings: registration.turn_settings,
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
                        .map(|session| SessionInfo {
                            session_id: session.session.session_id,
                            owner_user_id: session.session.owner_user_id,
                            runner_id: session.session.runner_id,
                            workspace_id: session.session.workspace_id,
                            provider_id: session.session.provider_id.clone(),
                            status: session.session.status.clone(),
                            external_session_id: session.session.external_session_id.clone(),
                            title: session.session.title.clone(),
                        })
                        .collect()
                })
                .unwrap_or_else(|error| {
                    tracing::warn!(%user_id, %error, "failed to list persisted sessions");
                    Vec::new()
                });
        }
        self.inner
            .registry
            .lock()
            .await
            .sessions
            .values()
            .filter(|session| session.owner_user_id == user_id)
            .map(session_info)
            .collect()
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
        let mut sessions = self.inner.sessions.lock().await;
        let events = sessions
            .entry(session_id)
            .or_insert_with(SessionEvents::new);
        let receiver = events.sender.subscribe();
        (events.cache.clone(), receiver)
    }

    pub async fn session_history(
        &self,
        user_id: UserId,
        session_id: SessionId,
    ) -> Option<Vec<BrowserEventEnvelope>> {
        if !self.can_access_session(user_id, session_id).await {
            return None;
        }

        if let Some(pool) = &self.inner.db_pool {
            return Some(
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
                    }),
            );
        }

        Some(
            self.inner
                .sessions
                .lock()
                .await
                .get(&session_id)
                .map(|events| events.cache.clone())
                .unwrap_or_default(),
        )
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
                        status: ApprovalStatus::Pending,
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
                        ApprovalStatus::Pending | ApprovalStatus::Resolving => {
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
            _ => {}
        }

        self.store_event(session_id, envelope).await
    }

    pub async fn begin_approval_resolution(
        &self,
        approval_id: ApprovalId,
    ) -> ApprovalResolutionStart {
        let mut registry = self.inner.registry.lock().await;
        let Some(approval) = registry.approvals.get_mut(&approval_id) else {
            return ApprovalResolutionStart::Missing;
        };
        match &approval.status {
            ApprovalStatus::Pending => {
                approval.status = ApprovalStatus::Resolving;
                tracing::debug!(%approval_id, session_id = %approval.session_id, "approval resolution started");
                ApprovalResolutionStart::Started {
                    session_id: approval.session_id,
                }
            }
            ApprovalStatus::Resolving => ApprovalResolutionStart::InProgress,
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
        if matches!(approval.status, ApprovalStatus::Resolving) {
            approval.status = ApprovalStatus::Pending;
        }
    }

    pub async fn approval_is_resolving(&self, approval_id: ApprovalId) -> bool {
        self.inner
            .registry
            .lock()
            .await
            .approvals
            .get(&approval_id)
            .is_some_and(|approval| matches!(approval.status, ApprovalStatus::Resolving))
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
                ApprovalStatus::Pending => return None,
                ApprovalStatus::Resolving => {
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
    ) {
        let Some(owner_user_id) = self
            .inner
            .bootstrap_admin
            .as_ref()
            .map(|admin| admin.user.user_id)
        else {
            tracing::warn!(%runner_id, "cannot import discovered sessions without bootstrap admin user");
            return;
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
                return;
            }
        }

        for discovered_session in discovered.sessions {
            let session = if let Some(pool) = &self.inner.db_pool {
                match agenter_db::upsert_session_by_external_id(
                    pool,
                    owner_user_id,
                    runner_id,
                    discovered.workspace.workspace_id,
                    discovered.provider_id.clone(),
                    &discovered_session.external_session_id,
                    discovered_session.title.as_deref(),
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
                RegisteredSession {
                    session_id: SessionId::new(),
                    owner_user_id,
                    runner_id,
                    workspace: discovered.workspace.clone(),
                    provider_id: discovered.provider_id.clone(),
                    status: SessionStatus::Running,
                    title: discovered_session.title.clone(),
                    external_session_id: Some(discovered_session.external_session_id.clone()),
                    turn_settings: None,
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
            if !discovered_events.is_empty() {
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
        })
        .collect()
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
    SessionInfo {
        session_id: session.session_id,
        owner_user_id: session.owner_user_id,
        runner_id: session.runner_id,
        workspace_id: session.workspace.workspace_id,
        provider_id: session.provider_id.clone(),
        status: session.status.clone(),
        external_session_id: session.external_session_id.clone(),
        title: session.title.clone(),
    }
}

#[cfg(test)]
mod tests {
    use agenter_core::{AppEvent, RunnerId, SessionId, UserMessageEvent};
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
