use std::{collections::HashMap, sync::Arc};

use agenter_core::{AgentProviderId, AppEvent, RunnerId, SessionId, WorkspaceRef};
use agenter_protocol::runner::RunnerCapabilities;
use tokio::sync::{broadcast, Mutex};
use uuid::Uuid;

use crate::auth::{self, AuthenticatedUser, BootstrapAdmin};

const SESSION_EVENT_CACHE_LIMIT: usize = 128;

#[derive(Clone, Debug)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

#[derive(Debug)]
struct AppStateInner {
    runner_token: String,
    bootstrap_admin: Option<BootstrapAdmin>,
    auth_sessions: Mutex<HashMap<String, AuthenticatedUser>>,
    registry: Mutex<Registry>,
    sessions: Mutex<HashMap<SessionId, SessionEvents>>,
}

#[derive(Debug, Default)]
struct Registry {
    runners: HashMap<RunnerId, RegisteredRunner>,
    sessions: HashMap<SessionId, RegisteredSession>,
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
    pub runner_id: RunnerId,
    pub workspace: WorkspaceRef,
    pub provider_id: AgentProviderId,
}

#[derive(Debug)]
struct SessionEvents {
    sender: broadcast::Sender<AppEvent>,
    cache: Vec<AppEvent>,
}

impl AppState {
    #[must_use]
    pub fn new(runner_token: String) -> Self {
        Self {
            inner: Arc::new(AppStateInner {
                runner_token,
                bootstrap_admin: None,
                auth_sessions: Mutex::new(HashMap::new()),
                registry: Mutex::new(Registry::default()),
                sessions: Mutex::new(HashMap::new()),
            }),
        }
    }

    pub fn new_with_bootstrap_admin(
        runner_token: String,
        email: String,
        password: String,
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
                bootstrap_admin: Some(BootstrapAdmin {
                    user,
                    password_hash,
                }),
                auth_sessions: Mutex::new(HashMap::new()),
                registry: Mutex::new(Registry::default()),
                sessions: Mutex::new(HashMap::new()),
            }),
        })
    }

    #[must_use]
    pub fn is_runner_token_valid(&self, token: &str) -> bool {
        self.inner.runner_token == token
    }

    pub async fn login_password(&self, email: &str, password: &str) -> Option<String> {
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

    pub async fn create_session(
        &self,
        session_id: SessionId,
        runner_id: RunnerId,
        workspace: WorkspaceRef,
        provider_id: AgentProviderId,
    ) -> RegisteredSession {
        let session = RegisteredSession {
            session_id,
            runner_id,
            workspace,
            provider_id,
        };
        self.inner
            .registry
            .lock()
            .await
            .sessions
            .insert(session_id, session.clone());
        session
    }

    pub async fn subscribe_session(
        &self,
        session_id: SessionId,
    ) -> (Vec<AppEvent>, broadcast::Receiver<AppEvent>) {
        let mut sessions = self.inner.sessions.lock().await;
        let events = sessions
            .entry(session_id)
            .or_insert_with(SessionEvents::new);
        let receiver = events.sender.subscribe();
        (events.cache.clone(), receiver)
    }

    pub async fn publish_event(&self, session_id: SessionId, event: AppEvent) {
        let sender = {
            let mut sessions = self.inner.sessions.lock().await;
            let events = sessions
                .entry(session_id)
                .or_insert_with(SessionEvents::new);
            events.cache.push(event.clone());
            if events.cache.len() > SESSION_EVENT_CACHE_LIMIT {
                let overflow = events.cache.len() - SESSION_EVENT_CACHE_LIMIT;
                events.cache.drain(..overflow);
            }
            events.sender.clone()
        };

        let _ = sender.send(event);
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

#[cfg(test)]
mod tests {
    use agenter_core::{AppEvent, SessionId, UserMessageEvent};

    use super::*;

    #[tokio::test]
    async fn subscribers_receive_published_session_events() {
        let state = AppState::new("dev-token".to_owned());
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
        assert!(matches!(received, AppEvent::UserMessage(_)));
    }

    #[tokio::test]
    async fn subscribe_snapshots_cached_events_and_live_receiver_atomically() {
        let state = AppState::new("dev-token".to_owned());
        let session_id = SessionId::new();
        let event = AppEvent::UserMessage(UserMessageEvent {
            session_id,
            message_id: Some("cached".to_owned()),
            author_user_id: None,
            content: "cached".to_owned(),
        });
        state.publish_event(session_id, event.clone()).await;

        let (cached, mut subscription) = state.subscribe_session(session_id).await;
        assert_eq!(cached, vec![event]);

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
            subscription.recv().await.expect("live event"),
            AppEvent::UserMessage(_)
        ));
    }

    #[test]
    fn runner_token_uses_dev_secret() {
        let state = AppState::new("dev-token".to_owned());

        assert!(state.is_runner_token_valid("dev-token"));
        assert!(!state.is_runner_token_valid("wrong-token"));
    }
}
