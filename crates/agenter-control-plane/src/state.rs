use std::{collections::HashMap, sync::Arc};

use agenter_core::{AppEvent, SessionId};
use tokio::sync::{broadcast, Mutex};

const SESSION_EVENT_CACHE_LIMIT: usize = 128;

#[derive(Clone, Debug)]
pub struct AppState {
    inner: Arc<AppStateInner>,
}

#[derive(Debug)]
struct AppStateInner {
    runner_token: String,
    sessions: Mutex<HashMap<SessionId, SessionEvents>>,
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
                sessions: Mutex::new(HashMap::new()),
            }),
        }
    }

    #[must_use]
    pub fn is_runner_token_valid(&self, token: &str) -> bool {
        self.inner.runner_token == token
    }

    pub async fn subscribe_session(&self, session_id: SessionId) -> broadcast::Receiver<AppEvent> {
        let mut sessions = self.inner.sessions.lock().await;
        sessions
            .entry(session_id)
            .or_insert_with(SessionEvents::new)
            .sender
            .subscribe()
    }

    pub async fn cached_events(&self, session_id: SessionId) -> Vec<AppEvent> {
        self.inner
            .sessions
            .lock()
            .await
            .get(&session_id)
            .map(|events| events.cache.clone())
            .unwrap_or_default()
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
        let mut subscription = state.subscribe_session(session_id).await;

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

    #[test]
    fn runner_token_uses_dev_secret() {
        let state = AppState::new("dev-token".to_owned());

        assert!(state.is_runner_token_valid("dev-token"));
        assert!(!state.is_runner_token_valid("wrong-token"));
    }
}
