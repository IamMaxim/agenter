use std::{collections::HashMap, sync::Arc};

use agenter_core::{
    AgentProviderId, AppEvent, ApprovalId, RunnerId, SessionId, SessionInfo, SessionStatus, UserId,
    WorkspaceId, WorkspaceRef,
};
use agenter_protocol::{
    browser::BrowserEventEnvelope,
    runner::{RunnerCapabilities, RunnerServerMessage},
};
use tokio::sync::{broadcast, mpsc, Mutex};
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
    runner_connections: Mutex<HashMap<RunnerId, mpsc::UnboundedSender<RunnerServerMessage>>>,
}

#[derive(Debug, Default)]
struct Registry {
    runners: HashMap<RunnerId, RegisteredRunner>,
    sessions: HashMap<SessionId, RegisteredSession>,
    approvals: HashMap<ApprovalId, RegisteredApproval>,
}

#[derive(Clone, Debug)]
pub struct RegisteredApproval {
    pub session_id: SessionId,
    status: ApprovalStatus,
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
pub struct RegisteredSession {
    pub session_id: SessionId,
    pub owner_user_id: UserId,
    pub runner_id: RunnerId,
    pub workspace: WorkspaceRef,
    pub provider_id: AgentProviderId,
    pub status: SessionStatus,
    pub title: Option<String>,
    pub external_session_id: Option<String>,
}

#[derive(Debug)]
struct SessionEvents {
    sender: broadcast::Sender<BrowserEventEnvelope>,
    cache: Vec<BrowserEventEnvelope>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RunnerSendError {
    NotConnected,
    Closed,
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
        Some(token)
    }

    pub async fn authenticated_user(&self, token: &str) -> Option<AuthenticatedUser> {
        self.inner.auth_sessions.lock().await.get(token).cloned()
    }

    pub fn bootstrap_user_id(&self) -> Option<UserId> {
        self.inner
            .bootstrap_admin
            .as_ref()
            .map(|admin| admin.user.user_id)
    }

    pub async fn logout(&self, token: &str) {
        self.inner.auth_sessions.lock().await.remove(token);
    }

    pub async fn register_runner(
        &self,
        runner_id: RunnerId,
        capabilities: RunnerCapabilities,
        workspaces: Vec<WorkspaceRef>,
    ) -> RegisteredRunner {
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
        runner
    }

    pub async fn connect_runner(
        &self,
        runner_id: RunnerId,
        sender: mpsc::UnboundedSender<RunnerServerMessage>,
    ) {
        self.inner
            .runner_connections
            .lock()
            .await
            .insert(runner_id, sender);
    }

    pub async fn disconnect_runner(&self, runner_id: RunnerId) {
        self.inner
            .runner_connections
            .lock()
            .await
            .remove(&runner_id);
    }

    pub async fn send_runner_message(
        &self,
        runner_id: RunnerId,
        message: RunnerServerMessage,
    ) -> Result<(), RunnerSendError> {
        let Some(sender) = self
            .inner
            .runner_connections
            .lock()
            .await
            .get(&runner_id)
            .cloned()
        else {
            return Err(RunnerSendError::NotConnected);
        };

        sender.send(message).map_err(|_| RunnerSendError::Closed)
    }

    pub async fn list_runners(&self) -> Vec<RegisteredRunner> {
        self.inner
            .registry
            .lock()
            .await
            .runners
            .values()
            .cloned()
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

    pub async fn create_session_with_title(
        &self,
        session_id: SessionId,
        owner_user_id: UserId,
        runner_id: RunnerId,
        workspace: WorkspaceRef,
        provider_id: AgentProviderId,
        title: Option<String>,
    ) -> RegisteredSession {
        let session = RegisteredSession {
            session_id,
            owner_user_id,
            runner_id,
            workspace,
            provider_id,
            status: SessionStatus::Running,
            title,
            external_session_id: None,
        };
        self.inner
            .registry
            .lock()
            .await
            .sessions
            .insert(session_id, session.clone());
        session
    }

    pub async fn create_session_for_workspace(
        &self,
        owner_user_id: UserId,
        workspace_id: WorkspaceId,
        provider_id: AgentProviderId,
        title: Option<String>,
    ) -> Option<RegisteredSession> {
        let workspace = {
            let registry = self.inner.registry.lock().await;
            registry
                .runners
                .values()
                .flat_map(|runner| runner.workspaces.iter())
                .find(|workspace| workspace.workspace_id == workspace_id)
                .cloned()
        }?;

        Some(
            self.create_session_with_title(
                SessionId::new(),
                owner_user_id,
                workspace.runner_id,
                workspace,
                provider_id,
                title,
            )
            .await,
        )
    }

    pub async fn can_access_session(&self, user_id: UserId, session_id: SessionId) -> bool {
        self.inner
            .registry
            .lock()
            .await
            .sessions
            .get(&session_id)
            .is_some_and(|session| session.owner_user_id == user_id)
    }

    pub async fn list_sessions(&self, user_id: UserId) -> Vec<SessionInfo> {
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

    pub async fn publish_event(
        &self,
        session_id: SessionId,
        event: AppEvent,
    ) -> BrowserEventEnvelope {
        if let AppEvent::ApprovalRequested(request) = &event {
            self.inner.registry.lock().await.approvals.insert(
                request.approval_id,
                RegisteredApproval {
                    session_id,
                    status: ApprovalStatus::Pending,
                },
            );
        }

        let envelope = BrowserEventEnvelope {
            event_id: Some(Uuid::new_v4().to_string().into()),
            event,
        };
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
        envelope
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

    async fn store_event(
        &self,
        session_id: SessionId,
        envelope: BrowserEventEnvelope,
    ) -> BrowserEventEnvelope {
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
        envelope
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
    use agenter_core::{AppEvent, SessionId, UserMessageEvent};

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
    fn runner_token_uses_dev_secret() {
        let state = AppState::new("dev-token".to_owned(), CookieSecurity::DevelopmentInsecure);

        assert!(state.is_runner_token_valid("dev-token"));
        assert!(!state.is_runner_token_valid("wrong-token"));
    }
}
