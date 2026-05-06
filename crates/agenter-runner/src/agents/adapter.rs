#![allow(dead_code)]

use std::{collections::HashMap, future::Future, pin::Pin};

use agenter_core::{
    AgentCapabilities, AgentOptions, AgentProviderId, ApprovalDecision, ApprovalId, CapabilitySet,
    ItemId, NativeRef, SessionId, SlashCommandDefinition, SlashCommandRequest, SlashCommandResult,
    TurnId, UniversalCommandEnvelope, UniversalEventKind, UniversalEventSource, UserInput,
    WorkspaceRef, UNIVERSAL_PROTOCOL_VERSION,
};
use agenter_protocol::runner::{AgentInput, AgentUniversalEvent, RunnerCommandResult, RunnerError};
use tokio::sync::mpsc;

pub type AdapterBoxFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;
pub type AdapterEventSender = mpsc::UnboundedSender<AdapterEvent>;
pub type AdapterEventReceiver = mpsc::UnboundedReceiver<AdapterEvent>;

#[derive(Clone, Debug)]
pub struct AdapterProviderRegistration {
    pub provider_id: AgentProviderId,
    pub capabilities: CapabilitySet,
}

#[derive(Clone, Debug, Default)]
pub struct AdapterRegistry {
    providers: HashMap<AgentProviderId, AdapterProviderRegistration>,
    session_providers: HashMap<SessionId, AgentProviderId>,
}

impl AdapterRegistry {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_provider(&mut self, registration: AdapterProviderRegistration) {
        self.providers
            .insert(registration.provider_id.clone(), registration);
    }

    pub fn bind_session(&mut self, session_id: SessionId, provider_id: AgentProviderId) {
        self.session_providers.insert(session_id, provider_id);
    }

    pub fn unbind_session(&mut self, session_id: SessionId) {
        self.session_providers.remove(&session_id);
    }

    #[must_use]
    pub fn provider_for_session(
        &self,
        session_id: SessionId,
    ) -> Option<&AdapterProviderRegistration> {
        self.session_providers
            .get(&session_id)
            .and_then(|provider_id| self.providers.get(provider_id))
    }

    #[must_use]
    pub fn resolve_provider(
        &self,
        session_id: Option<SessionId>,
        provider_id: Option<&AgentProviderId>,
    ) -> Option<&AdapterProviderRegistration> {
        session_id
            .and_then(|session_id| self.provider_for_session(session_id))
            .or_else(|| provider_id.and_then(|provider_id| self.providers.get(provider_id)))
    }
}

#[derive(Clone, Debug, Default)]
pub struct AdapterRuntime {
    registry: AdapterRegistry,
}

impl AdapterRuntime {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn register_provider(&mut self, registration: AdapterProviderRegistration) {
        self.registry.register_provider(registration);
    }

    pub fn bind_session(&mut self, session_id: SessionId, provider_id: AgentProviderId) {
        self.registry.bind_session(session_id, provider_id);
    }

    pub fn unbind_session(&mut self, session_id: SessionId) {
        self.registry.unbind_session(session_id);
    }

    #[must_use]
    pub fn resolve_provider(
        &self,
        session_id: Option<SessionId>,
        provider_id: Option<&AgentProviderId>,
    ) -> Option<&AdapterProviderRegistration> {
        self.registry.resolve_provider(session_id, provider_id)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdapterEvent {
    pub universal: AdapterUniversalEvent,
}

impl AdapterEvent {
    #[must_use]
    pub fn new(
        session_id: Option<SessionId>,
        turn_id: Option<TurnId>,
        item_id: Option<ItemId>,
        source: UniversalEventSource,
        native: Option<NativeRef>,
        event: UniversalEventKind,
    ) -> Self {
        Self {
            universal: AdapterUniversalEvent {
                session_id,
                turn_id,
                item_id,
                source,
                native,
                event,
            },
        }
    }

    #[must_use]
    pub fn from_universal(
        session_id: SessionId,
        turn_id: Option<TurnId>,
        item_id: Option<ItemId>,
        native: Option<NativeRef>,
        event: UniversalEventKind,
    ) -> Self {
        Self::new(
            Some(session_id),
            turn_id,
            item_id,
            UniversalEventSource::Native,
            native,
            event,
        )
    }

    #[must_use]
    pub fn session_status(
        provider_id: AgentProviderId,
        protocol: impl Into<String>,
        method: Option<&str>,
        session_id: SessionId,
        status: agenter_core::SessionStatus,
        reason: Option<String>,
    ) -> Self {
        Self::new(
            Some(session_id),
            None,
            None,
            UniversalEventSource::Native,
            Some(NativeRef {
                protocol: protocol.into(),
                method: method.map(str::to_owned),
                kind: Some(provider_id.to_string()),
                native_id: None,
                summary: Some("session status changed".to_owned()),
                hash: None,
                pointer: None,
                raw_payload: None,
            }),
            UniversalEventKind::SessionStatusChanged { status, reason },
        )
    }

    #[must_use]
    pub fn error(
        provider_id: AgentProviderId,
        protocol: impl Into<String>,
        method: Option<&str>,
        session_id: SessionId,
        code: Option<String>,
        message: String,
    ) -> Self {
        Self::new(
            Some(session_id),
            None,
            None,
            UniversalEventSource::Native,
            Some(NativeRef {
                protocol: protocol.into(),
                method: method.map(str::to_owned),
                kind: Some(provider_id.to_string()),
                native_id: None,
                summary: Some("error reported".to_owned()),
                hash: None,
                pointer: None,
                raw_payload: None,
            }),
            UniversalEventKind::ErrorReported { code, message },
        )
    }

    #[must_use]
    pub fn universal_projection_for_wal(&self) -> Option<AgentUniversalEvent> {
        let session_id = self.universal.session_id?;
        Some(AgentUniversalEvent {
            protocol_version: UNIVERSAL_PROTOCOL_VERSION.to_owned(),
            session_id,
            event_id: None,
            turn_id: self.universal.turn_id,
            item_id: self.universal.item_id,
            ts: None,
            source: self.universal.source.clone(),
            native: self.universal.native.clone(),
            event: self.universal.event.clone(),
        })
    }

    #[must_use]
    pub fn with_turn_id(mut self, turn_id: TurnId) -> Self {
        self.universal.turn_id = Some(turn_id);
        match &mut self.universal.event {
            UniversalEventKind::TurnStarted { turn }
            | UniversalEventKind::TurnStatusChanged { turn }
            | UniversalEventKind::TurnCompleted { turn }
            | UniversalEventKind::TurnFailed { turn }
            | UniversalEventKind::TurnCancelled { turn }
            | UniversalEventKind::TurnInterrupted { turn }
            | UniversalEventKind::TurnDetached { turn } => {
                turn.turn_id = turn_id;
            }
            UniversalEventKind::ItemCreated { item } => {
                item.turn_id = Some(turn_id);
            }
            UniversalEventKind::ApprovalRequested { approval } => {
                approval.turn_id = Some(turn_id);
            }
            UniversalEventKind::ApprovalResolved { .. } => {}
            UniversalEventKind::QuestionRequested { question }
            | UniversalEventKind::QuestionAnswered { question } => {
                question.turn_id = Some(turn_id);
            }
            UniversalEventKind::PlanUpdated { plan } => {
                plan.turn_id = Some(turn_id);
            }
            UniversalEventKind::DiffUpdated { diff } => {
                diff.turn_id = Some(turn_id);
            }
            UniversalEventKind::ArtifactCreated { artifact } => {
                artifact.turn_id = Some(turn_id);
            }
            UniversalEventKind::SessionCreated { .. }
            | UniversalEventKind::SessionStatusChanged { .. }
            | UniversalEventKind::SessionMetadataChanged { .. }
            | UniversalEventKind::ContentDelta { .. }
            | UniversalEventKind::ContentCompleted { .. }
            | UniversalEventKind::UsageUpdated { .. }
            | UniversalEventKind::ErrorReported { .. }
            | UniversalEventKind::ProviderNotification { .. }
            | UniversalEventKind::NativeUnknown { .. } => {}
        }
        self
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdapterUniversalEvent {
    pub session_id: Option<SessionId>,
    pub turn_id: Option<TurnId>,
    pub item_id: Option<ItemId>,
    pub source: UniversalEventSource,
    pub native: Option<NativeRef>,
    pub event: UniversalEventKind,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdapterStartSessionRequest {
    pub session_id: SessionId,
    pub workspace: WorkspaceRef,
    pub provider_id: AgentProviderId,
    pub initial_input: Option<AgentInput>,
    pub universal_command: Option<UniversalCommandEnvelope>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdapterLoadSessionRequest {
    pub session_id: SessionId,
    pub workspace: WorkspaceRef,
    pub provider_id: AgentProviderId,
    pub external_session_id: String,
    pub universal_command: Option<UniversalCommandEnvelope>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdapterStartTurnRequest {
    pub session_id: SessionId,
    pub provider_id: AgentProviderId,
    pub external_session_id: Option<String>,
    pub input: UserInput,
    pub universal_command: Option<UniversalCommandEnvelope>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdapterUserInputRequest {
    pub session_id: SessionId,
    pub input: UserInput,
    pub universal_command: Option<UniversalCommandEnvelope>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdapterApprovalResolutionRequest {
    pub session_id: SessionId,
    pub approval_id: ApprovalId,
    pub decision: ApprovalDecision,
    pub universal_command: Option<UniversalCommandEnvelope>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterCancelTurnRequest {
    pub session_id: SessionId,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterSetModeRequest {
    pub session_id: SessionId,
    pub mode: String,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdapterProviderCommandRequest {
    pub session_id: SessionId,
    pub external_session_id: Option<String>,
    pub command: SlashCommandRequest,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct AdapterCloseSessionRequest {
    pub session_id: SessionId,
}

#[derive(Clone, Debug, PartialEq)]
pub enum AdapterCommandOutcome {
    Accepted,
    AgentOptions(AgentOptions),
    ProviderCommands(Vec<SlashCommandDefinition>),
    ProviderCommandExecuted(SlashCommandResult),
    SessionCreated {
        session_id: SessionId,
        external_session_id: String,
    },
    SessionLoaded {
        session_id: SessionId,
        external_session_id: String,
    },
}

impl From<AdapterCommandOutcome> for RunnerCommandResult {
    fn from(value: AdapterCommandOutcome) -> Self {
        match value {
            AdapterCommandOutcome::Accepted => Self::Accepted,
            AdapterCommandOutcome::AgentOptions(options) => Self::AgentOptions { options },
            AdapterCommandOutcome::ProviderCommands(commands) => {
                Self::ProviderCommands { commands }
            }
            AdapterCommandOutcome::ProviderCommandExecuted(result) => {
                Self::ProviderCommandExecuted { result }
            }
            AdapterCommandOutcome::SessionCreated {
                session_id,
                external_session_id,
            } => Self::SessionCreated {
                session_id,
                external_session_id,
            },
            AdapterCommandOutcome::SessionLoaded {
                session_id,
                external_session_id,
            } => Self::SessionResumed {
                session_id,
                external_session_id,
            },
        }
    }
}

pub trait HarnessAdapter: Send + Sync {
    fn provider_id(&self) -> AgentProviderId;

    fn capabilities(&self) -> CapabilitySet {
        CapabilitySet::from(AgentCapabilities::default())
    }

    fn start(
        &self,
        request: AdapterStartSessionRequest,
    ) -> AdapterBoxFuture<'_, anyhow::Result<AdapterCommandOutcome>>;

    fn load(
        &self,
        request: AdapterLoadSessionRequest,
    ) -> AdapterBoxFuture<'_, anyhow::Result<AdapterCommandOutcome>>;

    fn start_turn(
        &self,
        request: AdapterStartTurnRequest,
    ) -> AdapterBoxFuture<'_, anyhow::Result<AdapterCommandOutcome>>;

    fn send_user_input(
        &self,
        request: AdapterUserInputRequest,
    ) -> AdapterBoxFuture<'_, anyhow::Result<AdapterCommandOutcome>>;

    fn resolve_approval(
        &self,
        request: AdapterApprovalResolutionRequest,
    ) -> AdapterBoxFuture<'_, anyhow::Result<AdapterCommandOutcome>>;

    fn cancel_turn(
        &self,
        request: AdapterCancelTurnRequest,
    ) -> AdapterBoxFuture<'_, anyhow::Result<AdapterCommandOutcome>>;

    fn set_mode(
        &self,
        request: AdapterSetModeRequest,
    ) -> AdapterBoxFuture<'_, anyhow::Result<AdapterCommandOutcome>>;

    fn execute_provider_command(
        &self,
        request: AdapterProviderCommandRequest,
    ) -> AdapterBoxFuture<'_, anyhow::Result<AdapterCommandOutcome>>;

    fn close(
        &self,
        request: AdapterCloseSessionRequest,
    ) -> AdapterBoxFuture<'_, anyhow::Result<AdapterCommandOutcome>>;

    fn take_event_stream(
        &self,
        session_id: SessionId,
    ) -> AdapterBoxFuture<'_, anyhow::Result<AdapterEventReceiver>>;
}

#[must_use]
pub fn runner_error(code: impl Into<String>, error: anyhow::Error) -> RunnerError {
    RunnerError {
        code: code.into(),
        message: error.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use agenter_core::{
        AgentCapabilities, AgentProviderId, CapabilitySet, ContentBlock, ContentBlockKind, ItemId,
        ItemRole, ItemState, ItemStatus, NativeRef, SessionId, TurnId, UniversalEventKind,
    };

    #[test]
    fn adapter_registry_resolves_provider_by_session_then_provider_id() {
        let gemini = AgentProviderId::from(AgentProviderId::GEMINI);
        let qwen = AgentProviderId::from(AgentProviderId::QWEN);
        let session_id = SessionId::nil();
        let mut registry = super::AdapterRegistry::new();

        registry.register_provider(super::AdapterProviderRegistration {
            provider_id: gemini.clone(),
            capabilities: CapabilitySet::from(AgentCapabilities {
                streaming: true,
                ..AgentCapabilities::default()
            }),
        });
        registry.register_provider(super::AdapterProviderRegistration {
            provider_id: qwen.clone(),
            capabilities: CapabilitySet::from(AgentCapabilities {
                approvals: true,
                ..AgentCapabilities::default()
            }),
        });
        registry.bind_session(session_id, qwen.clone());

        assert_eq!(
            registry
                .provider_for_session(session_id)
                .map(|p| &p.provider_id),
            Some(&qwen)
        );
        assert_eq!(
            registry
                .resolve_provider(Some(session_id), Some(&gemini))
                .map(|p| &p.provider_id),
            Some(&qwen)
        );
        assert_eq!(
            registry
                .resolve_provider(None, Some(&gemini))
                .map(|p| &p.provider_id),
            Some(&gemini)
        );
    }

    #[test]
    fn direct_universal_event_projects_to_runner_wal_event() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item_id = ItemId::new();

        let event = super::AdapterEvent::from_universal(
            session_id,
            Some(turn_id),
            Some(item_id),
            Some(native_ref("session/update")),
            UniversalEventKind::ContentDelta {
                block_id: "text-1".to_owned(),
                kind: Some(ContentBlockKind::Text),
                delta: "hello".to_owned(),
            },
        );

        let wal_event = event.universal_projection_for_wal().expect("wal event");
        assert_eq!(wal_event.session_id, session_id);
        assert_eq!(wal_event.turn_id, Some(turn_id));
        assert_eq!(wal_event.item_id, Some(item_id));
        assert_eq!(wal_event.native.expect("native").protocol, "acp");
        assert!(matches!(
            wal_event.event,
            UniversalEventKind::ContentDelta { .. }
        ));
    }

    #[test]
    fn direct_event_without_session_is_not_written_to_wal() {
        let event = super::AdapterEvent::new(
            None,
            None,
            None,
            agenter_core::UniversalEventSource::Native,
            Some(native_ref("runner/error")),
            UniversalEventKind::ErrorReported {
                code: Some("provider_error".to_owned()),
                message: "provider failed before session binding".to_owned(),
            },
        );

        assert!(event.universal_projection_for_wal().is_none());
    }

    #[test]
    fn with_turn_id_updates_nested_universal_item() {
        let session_id = SessionId::new();
        let turn_id = TurnId::new();
        let item_id = ItemId::new();
        let event = super::AdapterEvent::from_universal(
            session_id,
            None,
            Some(item_id),
            Some(native_ref("item/created")),
            UniversalEventKind::ItemCreated {
                item: Box::new(ItemState {
                    item_id,
                    session_id,
                    turn_id: None,
                    role: ItemRole::Assistant,
                    status: ItemStatus::Streaming,
                    content: vec![ContentBlock {
                        block_id: "text-1".to_owned(),
                        kind: ContentBlockKind::Text,
                        text: None,
                        mime_type: None,
                        artifact_id: None,
                    }],
                    tool: None,
                    native: None,
                }),
            },
        )
        .with_turn_id(turn_id);

        assert_eq!(event.universal.turn_id, Some(turn_id));
        let UniversalEventKind::ItemCreated { item } = event.universal.event else {
            panic!("expected item");
        };
        assert_eq!(item.turn_id, Some(turn_id));
    }

    fn native_ref(method: &str) -> NativeRef {
        NativeRef {
            protocol: "acp".to_owned(),
            method: Some(method.to_owned()),
            kind: Some(AgentProviderId::QWEN.to_owned()),
            native_id: None,
            summary: None,
            hash: None,
            pointer: None,
            raw_payload: None,
        }
    }
}
