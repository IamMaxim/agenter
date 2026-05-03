#![allow(dead_code)]

use std::{collections::HashMap, future::Future, pin::Pin};

use agenter_core::{
    AgentCapabilities, AgentOptions, AgentProviderId, AppEvent, ApprovalDecision, ApprovalId,
    ApprovalOption, ApprovalRequest, ApprovalRequestEvent, ApprovalStatus, CapabilitySet,
    CommandAction, CommandOutputStream, ContentBlock, ContentBlockKind, DiffFile, DiffState,
    FileChangeEvent, ItemId, ItemRole, ItemState, ItemStatus, NativeRef, PlanSource, PlanState,
    PlanStatus, ProviderEvent, SessionId, SlashCommandDefinition, SlashCommandRequest,
    SlashCommandResult, ToolActionProjection, ToolCommandProjection, ToolEvent, ToolMcpProjection,
    ToolProjection, ToolProjectionKind, ToolSubagentOperation, ToolSubagentProjection,
    ToolSubagentStateProjection, TurnId, TurnState, TurnStatus, UniversalCommandEnvelope,
    UniversalEventKind, UniversalEventSource, UserInput, WorkspaceRef,
};
use agenter_protocol::runner::{AgentInput, AgentUniversalEvent, RunnerCommandResult, RunnerError};
use serde_json::Value;
use tokio::sync::mpsc;
use uuid::Uuid;

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

    #[must_use]
    pub fn project_legacy_event(
        &self,
        provider_id: AgentProviderId,
        protocol: impl Into<String>,
        method: Option<&str>,
        legacy: AppEvent,
    ) -> AdapterEvent {
        AdapterEvent::from_legacy(provider_id, protocol, method, legacy)
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct AdapterEvent {
    pub universal: AdapterUniversalEvent,
    pub legacy: Option<AppEvent>,
}

impl AdapterEvent {
    #[must_use]
    pub fn from_legacy(
        provider_id: AgentProviderId,
        protocol: impl Into<String>,
        method: Option<&str>,
        legacy: AppEvent,
    ) -> Self {
        let session_id = app_event_session_id(&legacy);
        let native = NativeRef {
            protocol: protocol.into(),
            method: method.map(str::to_owned),
            kind: Some(provider_id.to_string()),
            native_id: app_event_native_id(&legacy),
            summary: Some(app_event_summary(&legacy)),
            hash: None,
            pointer: None,
        };
        let (turn_id, item_id, event) = universal_event_from_legacy(&legacy, &native);
        let universal = AdapterUniversalEvent {
            session_id,
            turn_id,
            item_id,
            source: UniversalEventSource::Native,
            native: Some(native),
            event,
        };
        Self {
            universal,
            legacy: Some(legacy),
        }
    }

    #[must_use]
    pub fn legacy_projection(&self) -> Option<&AppEvent> {
        self.legacy.as_ref()
    }

    #[must_use]
    pub fn universal_projection_for_wal(&self) -> Option<AgentUniversalEvent> {
        let session_id = self.universal.session_id?;
        Some(AgentUniversalEvent {
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
            | UniversalEventKind::ContentDelta { .. }
            | UniversalEventKind::ContentCompleted { .. }
            | UniversalEventKind::UsageUpdated { .. }
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

#[must_use]
pub fn legacy_events_to_adapter_events(
    provider_id: AgentProviderId,
    protocol: &'static str,
    method: Option<&str>,
    events: Vec<AppEvent>,
) -> Vec<AdapterEvent> {
    events
        .into_iter()
        .map(|event| AdapterEvent::from_legacy(provider_id.clone(), protocol, method, event))
        .collect()
}

fn universal_event_from_legacy(
    event: &AppEvent,
    native: &NativeRef,
) -> (Option<TurnId>, Option<ItemId>, UniversalEventKind) {
    match event {
        AppEvent::SessionStarted(info) => (
            None,
            None,
            UniversalEventKind::SessionCreated {
                session: Box::new(info.clone()),
            },
        ),
        AppEvent::AgentMessageDelta(event) => {
            let turn_id = turn_id_from_legacy_payload(event.provider_payload.as_ref())
                .or_else(|| native.native_id.as_deref().map(stable_turn_id));
            let item_id = Some(stable_item_id(&format!(
                "assistant:{}:{}",
                event.session_id, event.message_id
            )));
            (
                turn_id,
                item_id,
                UniversalEventKind::ContentDelta {
                    block_id: text_block_id(&event.message_id),
                    kind: Some(ContentBlockKind::Text),
                    delta: event.delta.clone(),
                },
            )
        }
        AppEvent::AgentMessageCompleted(event) => {
            let turn_id = turn_id_from_legacy_payload(event.provider_payload.as_ref())
                .or_else(|| native.native_id.as_deref().map(stable_turn_id));
            if legacy_method(event.provider_payload.as_ref()) == Some("turn/completed") {
                let turn_id = turn_id.unwrap_or_else(|| stable_turn_id(&event.message_id));
                return (
                    Some(turn_id),
                    None,
                    UniversalEventKind::TurnCompleted {
                        turn: turn_state(event.session_id, turn_id, TurnStatus::Completed),
                    },
                );
            }
            let item_id = Some(stable_item_id(&format!(
                "assistant:{}:{}",
                event.session_id, event.message_id
            )));
            (
                turn_id,
                item_id,
                UniversalEventKind::ContentCompleted {
                    block_id: text_block_id(&event.message_id),
                    kind: Some(ContentBlockKind::Text),
                    text: event.content.clone(),
                },
            )
        }
        AppEvent::PlanUpdated(event) => {
            let turn_id = event
                .plan_id
                .as_deref()
                .map(stable_turn_id)
                .or_else(|| turn_id_from_legacy_payload(event.provider_payload.as_ref()));
            let plan_id = stable_plan_id(&format!(
                "plan:{}:{}",
                event.session_id,
                event.plan_id.as_deref().unwrap_or("default")
            ));
            (
                turn_id,
                None,
                UniversalEventKind::PlanUpdated {
                    plan: PlanState {
                        plan_id,
                        session_id: event.session_id,
                        turn_id,
                        status: plan_status_from_payload(event.provider_payload.as_ref())
                            .unwrap_or(PlanStatus::Draft),
                        title: event.title.clone(),
                        content: event.content.clone(),
                        entries: event
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
                        source: plan_source_from_payload(event.provider_payload.as_ref()),
                        partial: event.append
                            || plan_partial_from_payload(event.provider_payload.as_ref()),
                        updated_at: None,
                    },
                },
            )
        }
        AppEvent::ToolStarted(event)
        | AppEvent::ToolUpdated(event)
        | AppEvent::ToolCompleted(event)
            if is_todo_tool(event) && has_todo_plan_entries(event) =>
        {
            let plan_id =
                stable_plan_id(&format!("todo:{}:{}", event.session_id, event.tool_call_id));
            (
                turn_id_from_legacy_payload(event.provider_payload.as_ref()),
                None,
                UniversalEventKind::PlanUpdated {
                    plan: PlanState {
                        plan_id,
                        session_id: event.session_id,
                        turn_id: turn_id_from_legacy_payload(event.provider_payload.as_ref()),
                        status: todo_plan_status(event),
                        title: event.title.clone().or_else(|| Some("Todo plan".to_owned())),
                        content: None,
                        entries: todo_plan_entries(event),
                        artifact_refs: Vec::new(),
                        source: PlanSource::TodoTool,
                        partial: plan_partial_from_payload(event.provider_payload.as_ref()),
                        updated_at: None,
                    },
                },
            )
        }
        AppEvent::ToolStarted(event) | AppEvent::ToolUpdated(event) => {
            let item_id =
                stable_item_id(&format!("tool:{}:{}", event.session_id, event.tool_call_id));
            (
                turn_id_from_legacy_payload(event.provider_payload.as_ref()),
                Some(item_id),
                UniversalEventKind::ItemCreated {
                    item: Box::new(tool_item(
                        event,
                        item_id,
                        ItemStatus::Streaming,
                        native.clone(),
                    )),
                },
            )
        }
        AppEvent::ToolCompleted(event) => {
            let item_id =
                stable_item_id(&format!("tool:{}:{}", event.session_id, event.tool_call_id));
            (
                turn_id_from_legacy_payload(event.provider_payload.as_ref()),
                Some(item_id),
                UniversalEventKind::ItemCreated {
                    item: Box::new(tool_item(
                        event,
                        item_id,
                        ItemStatus::Completed,
                        native.clone(),
                    )),
                },
            )
        }
        AppEvent::CommandStarted(event) => {
            let turn_id = turn_id_from_legacy_payload(event.provider_payload.as_ref());
            let item_id = stable_item_id(&format!(
                "command:{}:{}",
                event.session_id, event.command_id
            ));
            (
                turn_id,
                Some(item_id),
                UniversalEventKind::ItemCreated {
                    item: Box::new(ItemState {
                        item_id,
                        session_id: event.session_id,
                        turn_id,
                        role: ItemRole::Tool,
                        status: ItemStatus::Streaming,
                        content: vec![ContentBlock {
                            block_id: command_block_id(&event.command_id),
                            kind: ContentBlockKind::ToolCall,
                            text: Some(event.command.clone()),
                            mime_type: None,
                            artifact_id: None,
                        }],
                        tool: Some(command_tool_projection(event)),
                        native: Some(native.clone()),
                    }),
                },
            )
        }
        AppEvent::CommandOutputDelta(event) => {
            let turn_id = turn_id_from_legacy_payload(event.provider_payload.as_ref());
            let item_id = stable_item_id(&format!(
                "command:{}:{}",
                event.session_id, event.command_id
            ));
            (
                turn_id,
                Some(item_id),
                UniversalEventKind::ContentDelta {
                    block_id: command_output_block_id(&event.command_id, &event.stream),
                    kind: Some(ContentBlockKind::CommandOutput),
                    delta: event.delta.clone(),
                },
            )
        }
        AppEvent::CommandCompleted(event) => {
            let turn_id = turn_id_from_legacy_payload(event.provider_payload.as_ref());
            let item_id = stable_item_id(&format!(
                "command:{}:{}",
                event.session_id, event.command_id
            ));
            (
                turn_id,
                Some(item_id),
                UniversalEventKind::ContentCompleted {
                    block_id: command_status_block_id(&event.command_id),
                    kind: Some(ContentBlockKind::CommandOutput),
                    text: Some(if event.success {
                        "command completed".to_owned()
                    } else {
                        "command failed".to_owned()
                    }),
                },
            )
        }
        AppEvent::FileChangeProposed(event)
        | AppEvent::FileChangeApplied(event)
        | AppEvent::FileChangeRejected(event) => {
            let turn_id = turn_id_from_legacy_payload(event.provider_payload.as_ref());
            (
                turn_id,
                None,
                UniversalEventKind::DiffUpdated {
                    diff: file_change_diff(event, turn_id),
                },
            )
        }
        AppEvent::ApprovalRequested(event) => (
            turn_id_from_legacy_payload(event.provider_payload.as_ref()),
            None,
            UniversalEventKind::ApprovalRequested {
                approval: Box::new(approval_request(event, native.clone())),
            },
        ),
        AppEvent::TurnDiffUpdated(event) => {
            let turn_id = turn_id_from_legacy_payload(event.provider_payload.as_ref())
                .or_else(|| event.event_id.as_deref().map(stable_turn_id));
            (
                turn_id,
                None,
                UniversalEventKind::DiffUpdated {
                    diff: provider_diff(event, turn_id),
                },
            )
        }
        AppEvent::ItemReasoning(event) => {
            let turn_id = turn_id_from_legacy_payload(event.provider_payload.as_ref())
                .or_else(|| event.event_id.as_deref().map(stable_turn_id));
            let item_key = event.event_id.as_deref().unwrap_or("reasoning");
            let item_id = stable_item_id(&format!("reasoning:{}:{item_key}", event.session_id));
            (
                turn_id,
                Some(item_id),
                UniversalEventKind::ContentDelta {
                    block_id: format!("reasoning-{item_key}"),
                    kind: Some(ContentBlockKind::Reasoning),
                    delta: event.detail.clone().unwrap_or_else(|| event.title.clone()),
                },
            )
        }
        AppEvent::ProviderEvent(event) if event.method == "turn/started" => {
            let turn_id = event
                .event_id
                .as_deref()
                .map(stable_turn_id)
                .unwrap_or_else(|| stable_turn_id(&format!("{}:turn", event.session_id)));
            (
                Some(turn_id),
                None,
                UniversalEventKind::TurnStarted {
                    turn: turn_state(event.session_id, turn_id, TurnStatus::Running),
                },
            )
        }
        AppEvent::ProviderEvent(event) if event.category == "compaction" => {
            let turn_id = turn_id_from_legacy_payload(event.provider_payload.as_ref())
                .or_else(|| event.event_id.as_deref().map(stable_turn_id));
            let item_id = stable_item_id(&format!(
                "compaction:{}:{}",
                event.session_id,
                event.event_id.as_deref().unwrap_or("context")
            ));
            (
                turn_id,
                Some(item_id),
                UniversalEventKind::ItemCreated {
                    item: Box::new(ItemState {
                        item_id,
                        session_id: event.session_id,
                        turn_id,
                        role: ItemRole::System,
                        status: ItemStatus::Completed,
                        content: vec![ContentBlock {
                            block_id: "context-compaction".to_owned(),
                            kind: ContentBlockKind::Native,
                            text: event.detail.clone().or_else(|| Some(event.title.clone())),
                            mime_type: None,
                            artifact_id: None,
                        }],
                        tool: None,
                        native: Some(native.clone()),
                    }),
                },
            )
        }
        AppEvent::McpToolCallProgress(event) => {
            let turn_id = turn_id_from_legacy_payload(event.provider_payload.as_ref())
                .or_else(|| event.event_id.as_deref().map(stable_turn_id));
            let item_id = stable_item_id(&format!(
                "native-tool:{}:{}",
                event.session_id,
                event.event_id.as_deref().unwrap_or(&event.method)
            ));
            (
                turn_id,
                Some(item_id),
                UniversalEventKind::ItemCreated {
                    item: Box::new(ItemState {
                        item_id,
                        session_id: event.session_id,
                        turn_id,
                        role: ItemRole::Tool,
                        status: provider_item_status(event),
                        content: vec![ContentBlock {
                            block_id: format!("native-{}", event.category),
                            kind: ContentBlockKind::Native,
                            text: event.detail.clone().or_else(|| Some(event.title.clone())),
                            mime_type: None,
                            artifact_id: None,
                        }],
                        tool: Some(provider_tool_projection(event)),
                        native: Some(native.clone()),
                    }),
                },
            )
        }
        AppEvent::Error(event) if event.session_id.is_some() => (
            None,
            None,
            UniversalEventKind::TurnFailed {
                turn: turn_state(
                    event.session_id.expect("checked above"),
                    stable_turn_id(&format!(
                        "error:{}:{}",
                        event.session_id.expect("checked above"),
                        event.code.as_deref().unwrap_or("provider")
                    )),
                    TurnStatus::Failed,
                ),
            },
        ),
        _ => (
            None,
            None,
            UniversalEventKind::NativeUnknown {
                summary: Some(app_event_summary(event)),
            },
        ),
    }
}

fn turn_state(session_id: SessionId, turn_id: TurnId, status: TurnStatus) -> TurnState {
    TurnState {
        turn_id,
        session_id,
        status: status.clone(),
        started_at: None,
        completed_at: None,
        model: None,
        mode: None,
    }
}

fn stable_uuid(namespace: &str, value: &str) -> Uuid {
    Uuid::parse_str(value).unwrap_or_else(|_| {
        Uuid::new_v5(
            &Uuid::NAMESPACE_URL,
            format!("agenter:{namespace}:{value}").as_bytes(),
        )
    })
}

fn stable_turn_id(value: &str) -> TurnId {
    TurnId::from_uuid(stable_uuid("turn", value))
}

fn stable_item_id(value: &str) -> ItemId {
    ItemId::from_uuid(stable_uuid("item", value))
}

fn stable_plan_id(value: &str) -> agenter_core::PlanId {
    agenter_core::PlanId::from_uuid(stable_uuid("plan", value))
}

fn stable_diff_id(value: &str) -> agenter_core::DiffId {
    agenter_core::DiffId::from_uuid(stable_uuid("diff", value))
}

fn turn_id_from_legacy_payload(payload: Option<&Value>) -> Option<TurnId> {
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
    .map(stable_turn_id)
}

fn legacy_method(payload: Option<&Value>) -> Option<&str> {
    payload
        .and_then(|payload| payload.get("method"))
        .and_then(Value::as_str)
}

fn text_block_id(message_id: &str) -> String {
    format!("text-{message_id}")
}

fn command_block_id(command_id: &str) -> String {
    format!("command-{command_id}")
}

fn command_output_block_id(command_id: &str, stream: &CommandOutputStream) -> String {
    let stream = match stream {
        CommandOutputStream::Stdout => "stdout",
        CommandOutputStream::Stderr => "stderr",
    };
    format!("command-{command_id}-{stream}")
}

fn command_status_block_id(command_id: &str) -> String {
    format!("command-{command_id}-status")
}

fn tool_item(
    event: &ToolEvent,
    item_id: ItemId,
    status: ItemStatus,
    native: NativeRef,
) -> ItemState {
    let block_kind = if status == ItemStatus::Completed {
        ContentBlockKind::ToolResult
    } else {
        ContentBlockKind::ToolCall
    };
    ItemState {
        item_id,
        session_id: event.session_id,
        turn_id: turn_id_from_legacy_payload(event.provider_payload.as_ref()),
        role: ItemRole::Tool,
        status: status.clone(),
        content: vec![ContentBlock {
            block_id: format!("tool-{}", event.tool_call_id),
            kind: block_kind,
            text: event.title.clone().or_else(|| Some(event.name.clone())),
            mime_type: None,
            artifact_id: None,
        }],
        tool: Some(tool_projection(event, status)),
        native: Some(native),
    }
}

fn command_tool_projection(event: &agenter_core::CommandEvent) -> ToolProjection {
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
            actions: event.actions.iter().map(tool_action_projection).collect(),
            exit_code: None,
            duration_ms: None,
            success: None,
        }),
        subagent: None,
        mcp: None,
    }
}

fn tool_projection(event: &ToolEvent, status: ItemStatus) -> ToolProjection {
    if let Some(subagent) = subagent_projection(event) {
        return ToolProjection {
            kind: ToolProjectionKind::Subagent,
            name: event.name.clone(),
            title: subagent_title(&subagent.operation).to_owned(),
            status,
            detail: tool_projection_detail(event),
            input_summary: event.input.as_ref().and_then(json_summary),
            output_summary: event.output.as_ref().and_then(json_summary),
            command: None,
            subagent: Some(subagent),
            mcp: None,
        };
    }

    let mcp = mcp_projection(event);
    ToolProjection {
        kind: if mcp.is_some() {
            ToolProjectionKind::Mcp
        } else {
            ToolProjectionKind::Tool
        },
        name: event.name.clone(),
        title: event.title.clone().unwrap_or_else(|| event.name.clone()),
        status,
        detail: tool_projection_detail(event),
        input_summary: event.input.as_ref().and_then(json_summary),
        output_summary: event.output.as_ref().and_then(json_summary),
        command: None,
        subagent: None,
        mcp,
    }
}

fn provider_tool_projection(event: &ProviderEvent) -> ToolProjection {
    ToolProjection {
        kind: ToolProjectionKind::Mcp,
        name: event.title.clone(),
        title: event.title.clone(),
        status: provider_item_status(event),
        detail: event.detail.clone(),
        input_summary: None,
        output_summary: event.detail.clone(),
        command: None,
        subagent: None,
        mcp: Some(ToolMcpProjection {
            server: string_at_value_from_option(
                event.provider_payload.as_ref(),
                &["/params/server", "/params/serverInfo/name"],
            )
            .map(str::to_owned),
            tool: string_at_value_from_option(
                event.provider_payload.as_ref(),
                &["/params/tool", "/params/name"],
            )
            .unwrap_or(&event.title)
            .to_owned(),
            arguments_summary: event
                .provider_payload
                .as_ref()
                .and_then(|payload| payload.pointer("/params/arguments").and_then(json_summary)),
            result_summary: event.detail.clone(),
        }),
    }
}

fn subagent_projection(event: &ToolEvent) -> Option<ToolSubagentProjection> {
    let provider = event.provider_payload.as_ref().or(event.input.as_ref())?;
    if string_at_value(provider, &["/type"]) != Some("collabAgentToolCall") {
        return None;
    }
    let tool = string_at_value(provider, &["/tool"]).unwrap_or(&event.name);
    let operation = match tool {
        "spawnAgent" => ToolSubagentOperation::Spawn,
        "wait" => ToolSubagentOperation::Wait,
        "closeAgent" => ToolSubagentOperation::Close,
        _ => return None,
    };
    Some(ToolSubagentProjection {
        operation,
        agent_ids: string_array_at_value(provider, "/receiverThreadIds"),
        model: string_at_value(provider, &["/model"]).map(str::to_owned),
        reasoning_effort: string_at_value(provider, &["/reasoningEffort"]).map(str::to_owned),
        prompt: string_at_value(provider, &["/prompt"]).map(str::to_owned),
        states: subagent_state_projection(provider),
    })
}

fn subagent_state_projection(provider: &Value) -> Vec<ToolSubagentStateProjection> {
    let Some(states) = provider.pointer("/agentsStates").and_then(Value::as_object) else {
        return Vec::new();
    };
    states
        .iter()
        .filter_map(|(agent_id, state)| {
            Some(ToolSubagentStateProjection {
                agent_id: agent_id.clone(),
                status: string_at_value(state, &["/status"])?.to_owned(),
                message: string_at_value(state, &["/message"]).map(str::to_owned),
            })
        })
        .collect()
}

fn mcp_projection(event: &ToolEvent) -> Option<ToolMcpProjection> {
    let provider = event.provider_payload.as_ref().or(event.input.as_ref())?;
    let provider_type = string_at_value(provider, &["/type", "/params/type"]);
    if provider_type != Some("mcpToolCall") && !event.name.to_ascii_lowercase().contains("mcp") {
        return None;
    }
    let tool = string_at_value(
        provider,
        &["/tool", "/name", "/params/tool", "/params/name"],
    )
    .unwrap_or(&event.name)
    .to_owned();
    Some(ToolMcpProjection {
        server: string_at_value(provider, &["/server", "/serverInfo/name", "/params/server"])
            .map(str::to_owned),
        tool,
        arguments_summary: provider
            .pointer("/arguments")
            .or_else(|| provider.pointer("/params/arguments"))
            .and_then(json_summary),
        result_summary: event.output.as_ref().and_then(json_summary),
    })
}

fn subagent_title(operation: &ToolSubagentOperation) -> &'static str {
    match operation {
        ToolSubagentOperation::Spawn => "Spawn subagent",
        ToolSubagentOperation::Wait => "Wait for subagent",
        ToolSubagentOperation::Close => "Close subagent",
    }
}

fn command_projection_detail(event: &agenter_core::CommandEvent) -> Option<String> {
    let parts = [
        event.cwd.clone(),
        event.source.clone(),
        event.process_id.as_ref().map(|pid| format!("pid {pid}")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    (!parts.is_empty()).then(|| parts.join(" · "))
}

fn tool_projection_detail(event: &ToolEvent) -> Option<String> {
    event
        .output
        .as_ref()
        .or(event.input.as_ref())
        .and_then(json_summary)
}

fn tool_action_projection(action: &CommandAction) -> ToolActionProjection {
    let skill_name = action
        .path
        .as_deref()
        .and_then(|path| path.strip_suffix("/SKILL.md"))
        .and_then(|path| path.rsplit('/').next());
    let label = if let Some(skill_name) = skill_name {
        format!("Skill: {skill_name}")
    } else if action.kind == "read" {
        format!(
            "Read {}",
            action
                .name
                .as_deref()
                .or(action.path.as_deref())
                .unwrap_or("file")
        )
    } else if action.kind == "search" {
        format!(
            "Search {}",
            action
                .query
                .as_deref()
                .or(action.path.as_deref())
                .unwrap_or("workspace")
        )
    } else if action.kind == "listFiles" {
        format!("List {}", action.path.as_deref().unwrap_or("files"))
    } else {
        action
            .command
            .clone()
            .unwrap_or_else(|| action.kind.clone())
    };
    ToolActionProjection {
        kind: action.kind.clone(),
        label,
        detail: action
            .path
            .clone()
            .or_else(|| action.query.clone())
            .or_else(|| action.command.clone()),
        path: action.path.clone(),
    }
}

fn json_summary(value: &Value) -> Option<String> {
    if value.is_null() {
        return None;
    }
    serde_json::to_string_pretty(value).ok()
}

fn file_change_diff(event: &FileChangeEvent, turn_id: Option<TurnId>) -> DiffState {
    DiffState {
        diff_id: stable_diff_id(&format!("file:{}:{}", event.session_id, event.path)),
        session_id: event.session_id,
        turn_id,
        title: Some(event.path.clone()),
        files: vec![DiffFile {
            path: event.path.clone(),
            status: event.change_kind.clone(),
            diff: event.diff.clone(),
        }],
        updated_at: None,
    }
}

fn provider_diff(event: &ProviderEvent, turn_id: Option<TurnId>) -> DiffState {
    DiffState {
        diff_id: stable_diff_id(&format!(
            "provider:{}:{}",
            event.session_id,
            event.event_id.as_deref().unwrap_or(&event.method)
        )),
        session_id: event.session_id,
        turn_id,
        title: Some(event.title.clone()),
        files: Vec::new(),
        updated_at: None,
    }
}

fn approval_request(event: &ApprovalRequestEvent, native: NativeRef) -> ApprovalRequest {
    ApprovalRequest {
        approval_id: event.approval_id,
        session_id: event.session_id,
        turn_id: turn_id_from_legacy_payload(event.provider_payload.as_ref()),
        item_id: None,
        kind: event.kind.clone(),
        title: event.title.clone(),
        details: event.details.clone(),
        options: event
            .options
            .clone()
            .into_iter()
            .chain(if event.options.is_empty() {
                ApprovalOption::canonical_defaults()
            } else {
                Vec::new()
            })
            .collect(),
        status: ApprovalStatus::Pending,
        risk: event.risk.clone(),
        subject: event.subject.clone().or_else(|| event.details.clone()),
        native_request_id: event.native_request_id.clone(),
        native_blocking: event.native_blocking,
        policy: event.policy.clone(),
        native: Some(native),
        requested_at: None,
        resolved_at: None,
    }
}

fn provider_item_status(event: &ProviderEvent) -> ItemStatus {
    match event.status.as_deref() {
        Some("completed" | "complete" | "done" | "success") => ItemStatus::Completed,
        Some("failed" | "error") => ItemStatus::Failed,
        Some("cancelled" | "canceled") => ItemStatus::Cancelled,
        _ => ItemStatus::Streaming,
    }
}

fn plan_status_from_payload(payload: Option<&Value>) -> Option<PlanStatus> {
    let status = string_at_value(
        payload?,
        &[
            "/params/status",
            "/params/planStatus",
            "/params/phase",
            "/params/update/status",
            "/params/update/planStatus",
            "/params/update/phase",
            "/params/update/state",
        ],
    )?;
    Some(match status {
        "none" => PlanStatus::None,
        "discovering" | "planning" | "started" | "starting" => PlanStatus::Discovering,
        "draft" | "updated" | "ready" => PlanStatus::Draft,
        "awaiting_approval" | "awaitingApproval" | "approval_requested" | "approvalRequested"
        | "needs_approval" | "needsApproval" => PlanStatus::AwaitingApproval,
        "revision_requested" | "revisionRequested" | "needs_revision" | "needsRevision" => {
            PlanStatus::RevisionRequested
        }
        "approved" | "accepted" => PlanStatus::Approved,
        "implementing" | "implementation_started" | "implementationStarted" => {
            PlanStatus::Implementing
        }
        "completed" | "complete" | "done" => PlanStatus::Completed,
        "cancelled" | "canceled" => PlanStatus::Cancelled,
        "failed" | "error" => PlanStatus::Failed,
        _ => return None,
    })
}

fn plan_source_from_payload(payload: Option<&Value>) -> PlanSource {
    let Some(payload) = payload else {
        return PlanSource::NativeStructured;
    };
    match string_at_value(payload, &["/params/source", "/params/update/source"]) {
        Some("markdown_file" | "plan_file" | "file") => PlanSource::MarkdownFile,
        Some("todo_tool" | "todowrite" | "todo_write") => PlanSource::TodoTool,
        Some("synthetic") => PlanSource::Synthetic,
        _ => PlanSource::NativeStructured,
    }
}

fn plan_partial_from_payload(payload: Option<&Value>) -> bool {
    let Some(payload) = payload else {
        return false;
    };
    bool_at_value(
        payload,
        &[
            "/params/partial",
            "/params/isPartial",
            "/params/update/partial",
            "/params/update/isPartial",
        ],
    )
    .unwrap_or(false)
}

fn is_todo_tool(event: &ToolEvent) -> bool {
    matches!(
        event.name.to_ascii_lowercase().as_str(),
        "todowrite" | "todo_write" | "todo"
    )
}

fn todo_plan_status(event: &ToolEvent) -> PlanStatus {
    let entries = todo_plan_entries(event);
    if entries.is_empty() {
        return PlanStatus::Draft;
    }
    if entries
        .iter()
        .all(|entry| entry.status == agenter_core::PlanEntryStatus::Completed)
    {
        PlanStatus::Completed
    } else if entries
        .iter()
        .any(|entry| entry.status == agenter_core::PlanEntryStatus::InProgress)
    {
        PlanStatus::Implementing
    } else {
        PlanStatus::Draft
    }
}

fn has_todo_plan_entries(event: &ToolEvent) -> bool {
    !todo_plan_entries(event).is_empty()
}

fn todo_plan_entries(event: &ToolEvent) -> Vec<agenter_core::UniversalPlanEntry> {
    let Some(value) = event.output.as_ref().or(event.input.as_ref()) else {
        return Vec::new();
    };
    todo_entries_from_value(value)
}

fn todo_entries_from_value(value: &Value) -> Vec<agenter_core::UniversalPlanEntry> {
    let todos = value
        .as_array()
        .or_else(|| value.pointer("/todos").and_then(Value::as_array))
        .or_else(|| value.pointer("/items").and_then(Value::as_array))
        .or_else(|| value.pointer("/tasks").and_then(Value::as_array));
    todos
        .into_iter()
        .flatten()
        .enumerate()
        .filter_map(|(index, todo)| {
            let label = string_at_value(todo, &["/content", "/title", "/label", "/text"])?;
            Some(agenter_core::UniversalPlanEntry {
                entry_id: string_at_value(todo, &["/id"])
                    .map(str::to_owned)
                    .unwrap_or_else(|| format!("todo-{index}")),
                label: label.to_owned(),
                status: match string_at_value(todo, &["/status"]).unwrap_or("pending") {
                    "in_progress" | "inProgress" | "running" => {
                        agenter_core::PlanEntryStatus::InProgress
                    }
                    "completed" | "complete" | "done" => agenter_core::PlanEntryStatus::Completed,
                    "failed" | "error" => agenter_core::PlanEntryStatus::Failed,
                    "cancelled" | "canceled" => agenter_core::PlanEntryStatus::Cancelled,
                    _ => agenter_core::PlanEntryStatus::Pending,
                },
            })
        })
        .collect()
}

fn string_at_value<'a>(value: &'a Value, pointers: &[&str]) -> Option<&'a str> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_str))
}

fn bool_at_value(value: &Value, pointers: &[&str]) -> Option<bool> {
    pointers
        .iter()
        .find_map(|pointer| value.pointer(pointer).and_then(Value::as_bool))
}

fn string_at_value_from_option<'a>(value: Option<&'a Value>, pointers: &[&str]) -> Option<&'a str> {
    string_at_value(value?, pointers)
}

fn string_array_at_value(value: &Value, pointer: &str) -> Vec<String> {
    value
        .pointer(pointer)
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect()
}

fn app_event_session_id(event: &AppEvent) -> Option<SessionId> {
    match event {
        AppEvent::SessionStarted(info) => Some(info.session_id),
        AppEvent::SessionStatusChanged(event) => Some(event.session_id),
        AppEvent::UserMessage(event) => Some(event.session_id),
        AppEvent::AgentMessageDelta(event) => Some(event.session_id),
        AppEvent::AgentMessageCompleted(event) => Some(event.session_id),
        AppEvent::PlanUpdated(event) => Some(event.session_id),
        AppEvent::ToolStarted(event)
        | AppEvent::ToolUpdated(event)
        | AppEvent::ToolCompleted(event) => Some(event.session_id),
        AppEvent::CommandStarted(event) => Some(event.session_id),
        AppEvent::CommandOutputDelta(event) => Some(event.session_id),
        AppEvent::CommandCompleted(event) => Some(event.session_id),
        AppEvent::FileChangeProposed(event)
        | AppEvent::FileChangeApplied(event)
        | AppEvent::FileChangeRejected(event) => Some(event.session_id),
        AppEvent::ApprovalRequested(event) => Some(event.session_id),
        AppEvent::ApprovalResolved(event) => Some(event.session_id),
        AppEvent::QuestionRequested(event) => Some(event.session_id),
        AppEvent::QuestionAnswered(event) => Some(event.session_id),
        AppEvent::TurnDiffUpdated(event)
        | AppEvent::ItemReasoning(event)
        | AppEvent::ServerRequestResolved(event)
        | AppEvent::McpToolCallProgress(event)
        | AppEvent::ThreadRealtimeEvent(event)
        | AppEvent::ProviderEvent(event) => Some(event.session_id),
        AppEvent::Error(event) => event.session_id,
    }
}

fn app_event_native_id(event: &AppEvent) -> Option<String> {
    match event {
        AppEvent::AgentMessageDelta(event) => Some(event.message_id.clone()),
        AppEvent::AgentMessageCompleted(event) => Some(event.message_id.clone()),
        AppEvent::PlanUpdated(event) => event.plan_id.clone(),
        AppEvent::ToolStarted(event)
        | AppEvent::ToolUpdated(event)
        | AppEvent::ToolCompleted(event) => Some(event.tool_call_id.clone()),
        AppEvent::CommandStarted(event) => Some(event.command_id.clone()),
        AppEvent::CommandOutputDelta(event) => Some(event.command_id.clone()),
        AppEvent::CommandCompleted(event) => Some(event.command_id.clone()),
        AppEvent::ApprovalRequested(event) => Some(event.approval_id.to_string()),
        AppEvent::ApprovalResolved(event) => Some(event.approval_id.to_string()),
        AppEvent::QuestionRequested(event) => Some(event.question_id.to_string()),
        AppEvent::QuestionAnswered(event) => Some(event.answer.question_id.to_string()),
        AppEvent::TurnDiffUpdated(event)
        | AppEvent::ItemReasoning(event)
        | AppEvent::ServerRequestResolved(event)
        | AppEvent::McpToolCallProgress(event)
        | AppEvent::ThreadRealtimeEvent(event)
        | AppEvent::ProviderEvent(event) => event.event_id.clone(),
        _ => None,
    }
}

fn app_event_summary(event: &AppEvent) -> String {
    match event {
        AppEvent::SessionStarted(_) => "session started",
        AppEvent::SessionStatusChanged(_) => "session status changed",
        AppEvent::UserMessage(_) => "user message",
        AppEvent::AgentMessageDelta(_) => "assistant message delta",
        AppEvent::AgentMessageCompleted(_) => "assistant message completed",
        AppEvent::PlanUpdated(_) => "plan updated",
        AppEvent::ToolStarted(_) => "tool started",
        AppEvent::ToolUpdated(_) => "tool updated",
        AppEvent::ToolCompleted(_) => "tool completed",
        AppEvent::CommandStarted(_) => "command started",
        AppEvent::CommandOutputDelta(_) => "command output delta",
        AppEvent::CommandCompleted(_) => "command completed",
        AppEvent::FileChangeProposed(_) => "file change proposed",
        AppEvent::FileChangeApplied(_) => "file change applied",
        AppEvent::FileChangeRejected(_) => "file change rejected",
        AppEvent::ApprovalRequested(_) => "approval requested",
        AppEvent::ApprovalResolved(_) => "approval resolved",
        AppEvent::QuestionRequested(_) => "question requested",
        AppEvent::QuestionAnswered(_) => "question answered",
        AppEvent::TurnDiffUpdated(_) => "turn diff updated",
        AppEvent::ItemReasoning(_) => "item reasoning",
        AppEvent::ServerRequestResolved(_) => "server request resolved",
        AppEvent::McpToolCallProgress(_) => "mcp tool call progress",
        AppEvent::ThreadRealtimeEvent(_) => "thread realtime event",
        AppEvent::ProviderEvent(_) => "provider event",
        AppEvent::Error(_) => "error",
    }
    .to_owned()
}

#[cfg(test)]
mod tests {
    use agenter_core::{
        AgentCapabilities, AgentErrorEvent, AgentMessageDeltaEvent, AgentProviderId, AppEvent,
        CapabilitySet, CommandAction, CommandEvent, PlanEntry, PlanEntryStatus, PlanEvent,
        PlanSource, PlanStatus, SessionId, ToolEvent, UniversalEventKind,
    };

    #[test]
    fn adapter_registry_resolves_provider_by_session_then_provider_id() {
        let codex = AgentProviderId::from(AgentProviderId::CODEX);
        let qwen = AgentProviderId::from(AgentProviderId::QWEN);
        let session_id = SessionId::nil();
        let mut registry = super::AdapterRegistry::new();

        registry.register_provider(super::AdapterProviderRegistration {
            provider_id: codex.clone(),
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
                .resolve_provider(Some(session_id), Some(&codex))
                .map(|p| &p.provider_id),
            Some(&qwen)
        );
        assert_eq!(
            registry
                .resolve_provider(None, Some(&codex))
                .map(|p| &p.provider_id),
            Some(&codex)
        );
    }

    #[test]
    fn adapter_event_preserves_legacy_projection_and_adds_universal_content_delta() {
        let session_id = SessionId::nil();
        let legacy = AppEvent::AgentMessageDelta(AgentMessageDeltaEvent {
            session_id,
            message_id: "message-1".to_owned(),
            delta: "hello".to_owned(),
            provider_payload: Some(serde_json::json!({"raw": "do-not-copy"})),
        });

        let event = super::AdapterEvent::from_legacy(
            AgentProviderId::from(AgentProviderId::CODEX),
            "codex-app-server",
            Some("agentMessage/delta"),
            legacy.clone(),
        );

        assert_eq!(event.legacy_projection(), Some(&legacy));
        let wal_event = event.universal_projection_for_wal().expect("wal event");
        assert_eq!(wal_event.session_id, session_id);
        assert!(matches!(
            &wal_event.event,
            UniversalEventKind::ContentDelta { .. }
        ));
        assert!(event.universal.native.is_some());
        assert_eq!(event.universal.session_id, Some(session_id));
        let native = event.universal.native.as_ref().expect("native ref");
        assert_eq!(native.protocol, "codex-app-server");
        assert_eq!(native.method.as_deref(), Some("agentMessage/delta"));
        assert_eq!(native.kind.as_deref(), Some(AgentProviderId::CODEX));
        assert_eq!(native.native_id.as_deref(), Some("message-1"));
        assert_eq!(native.summary.as_deref(), Some("assistant message delta"));
        assert!(native.pointer.is_none());
        let UniversalEventKind::ContentDelta {
            block_id,
            kind,
            delta,
        } = &event.universal.event
        else {
            panic!("expected universal content delta");
        };
        assert_eq!(block_id, "text-message-1");
        assert_eq!(kind, &Some(agenter_core::ContentBlockKind::Text));
        assert_eq!(delta, "hello");
    }

    #[test]
    fn command_projection_serializes_semantic_tool_metadata() {
        let session_id = SessionId::new();
        let legacy = AppEvent::CommandStarted(CommandEvent {
            session_id,
            command_id: "cmd-1".to_owned(),
            command: "cargo test".to_owned(),
            cwd: Some("/work/agenter".to_owned()),
            source: Some("unifiedExecStartup".to_owned()),
            process_id: Some("123".to_owned()),
            actions: vec![CommandAction {
                kind: "read".to_owned(),
                command: Some("sed -n '1,20p' /tmp/skills/demo/SKILL.md".to_owned()),
                path: Some("/tmp/skills/demo/SKILL.md".to_owned()),
                name: Some("SKILL.md".to_owned()),
                query: None,
                provider_payload: None,
            }],
            provider_payload: Some(serde_json::json!({"raw": "do-not-copy"})),
        });

        let event = super::AdapterEvent::from_legacy(
            AgentProviderId::from(AgentProviderId::CODEX),
            "codex-app-server",
            Some("item/started"),
            legacy,
        );

        let UniversalEventKind::ItemCreated { item } = &event.universal.event else {
            panic!("expected universal item");
        };
        let item_json = serde_json::to_value(item).expect("item serializes");
        assert_eq!(item_json["tool"]["kind"], "command");
        assert_eq!(item_json["tool"]["title"], "cargo test");
        assert_eq!(item_json["tool"]["command"]["cwd"], "/work/agenter");
        assert_eq!(item_json["tool"]["command"]["process_id"], "123");
        assert_eq!(
            item_json["tool"]["command"]["actions"][0]["path"],
            "/tmp/skills/demo/SKILL.md"
        );
        assert!(item_json["native"].is_object());
        assert!(item_json.get("provider_payload").is_none());
    }

    #[test]
    fn codex_collab_tool_projection_serializes_subagent_metadata() {
        let session_id = SessionId::new();
        let legacy = AppEvent::ToolCompleted(ToolEvent {
            session_id,
            tool_call_id: "tool-1".to_owned(),
            name: "spawnAgent".to_owned(),
            title: Some("spawnAgent".to_owned()),
            input: None,
            output: None,
            provider_payload: Some(serde_json::json!({
                "type": "collabAgentToolCall",
                "id": "tool-1",
                "tool": "spawnAgent",
                "receiverThreadIds": ["agent-1"],
                "model": "gpt-5.5",
                "reasoningEffort": "medium",
                "prompt": "Implement task"
            })),
        });

        let event = super::AdapterEvent::from_legacy(
            AgentProviderId::from(AgentProviderId::CODEX),
            "codex-app-server",
            Some("item/completed"),
            legacy,
        );

        let UniversalEventKind::ItemCreated { item } = &event.universal.event else {
            panic!("expected universal item");
        };
        let item_json = serde_json::to_value(item).expect("item serializes");
        assert_eq!(item_json["tool"]["kind"], "subagent");
        assert_eq!(item_json["tool"]["title"], "Spawn subagent");
        assert_eq!(item_json["tool"]["subagent"]["operation"], "spawn");
        assert_eq!(item_json["tool"]["subagent"]["agent_ids"][0], "agent-1");
        assert_eq!(item_json["tool"]["subagent"]["model"], "gpt-5.5");
        assert!(item_json.get("provider_payload").is_none());
    }

    #[test]
    fn codex_mcp_tool_projection_serializes_safe_tool_metadata() {
        let session_id = SessionId::new();
        let legacy = AppEvent::ToolCompleted(ToolEvent {
            session_id,
            tool_call_id: "mcp-1".to_owned(),
            name: "read_file".to_owned(),
            title: Some("read_file".to_owned()),
            input: Some(serde_json::json!({
                "type": "mcpToolCall",
                "id": "mcp-1",
                "serverInfo": {"name": "filesystem"},
                "tool": "read_file",
                "arguments": {"path": "README.md"}
            })),
            output: Some(serde_json::json!({"content": "hello"})),
            provider_payload: Some(serde_json::json!({
                "type": "mcpToolCall",
                "id": "mcp-1",
                "serverInfo": {"name": "filesystem"},
                "tool": "read_file",
                "arguments": {"path": "README.md"}
            })),
        });

        let event = super::AdapterEvent::from_legacy(
            AgentProviderId::from(AgentProviderId::CODEX),
            "codex-app-server",
            Some("item/completed"),
            legacy,
        );

        let UniversalEventKind::ItemCreated { item } = &event.universal.event else {
            panic!("expected universal item");
        };
        let item_json = serde_json::to_value(item).expect("item serializes");
        assert_eq!(item_json["tool"]["kind"], "mcp");
        assert_eq!(item_json["tool"]["title"], "read_file");
        assert_eq!(item_json["tool"]["mcp"]["server"], "filesystem");
        assert_eq!(item_json["tool"]["mcp"]["tool"], "read_file");
        assert_eq!(
            item_json["tool"]["mcp"]["arguments_summary"],
            "{\n  \"path\": \"README.md\"\n}"
        );
        assert!(item_json.get("provider_payload").is_none());
    }

    #[test]
    fn adapter_event_does_not_fabricate_nil_session_for_sessionless_legacy_event() {
        let legacy = AppEvent::Error(AgentErrorEvent {
            session_id: None,
            code: Some("runner_error".to_owned()),
            message: "provider failed before session binding".to_owned(),
            provider_payload: Some(serde_json::json!({"raw": "do-not-copy"})),
        });

        let event = super::AdapterEvent::from_legacy(
            AgentProviderId::from(AgentProviderId::QWEN),
            "acp-stdio",
            Some("session/update"),
            legacy,
        );

        assert_eq!(event.universal.session_id, None);
        assert!(event.universal_projection_for_wal().is_none());
        let native = event.universal.native.as_ref().expect("native ref");
        assert_eq!(native.protocol, "acp-stdio");
        assert_eq!(native.kind.as_deref(), Some(AgentProviderId::QWEN));
    }

    #[test]
    fn plan_update_uses_native_status_and_partial_marker() {
        let session_id = SessionId::new();
        let legacy = AppEvent::PlanUpdated(PlanEvent {
            session_id,
            plan_id: Some("turn-1".to_owned()),
            title: Some("Implementation plan".to_owned()),
            content: Some("Draft the work".to_owned()),
            entries: vec![PlanEntry {
                label: "Inspect".to_owned(),
                status: PlanEntryStatus::InProgress,
            }],
            append: true,
            provider_payload: Some(serde_json::json!({
                "method": "session/update",
                "params": {
                    "update": {
                        "status": "awaiting_approval",
                        "partial": true
                    }
                }
            })),
        });

        let event = super::AdapterEvent::from_legacy(
            AgentProviderId::from(AgentProviderId::GEMINI),
            "acp-stdio",
            Some("session/update"),
            legacy,
        );

        let UniversalEventKind::PlanUpdated { plan } = &event.universal.event else {
            panic!("expected plan update");
        };
        assert_eq!(plan.status, PlanStatus::AwaitingApproval);
        assert_eq!(plan.source, PlanSource::NativeStructured);
        assert!(plan.partial);
        assert_eq!(plan.content.as_deref(), Some("Draft the work"));
        assert_eq!(plan.entries[0].label, "Inspect");
    }

    #[test]
    fn opencode_todowrite_tool_output_becomes_synthetic_plan_state() {
        let session_id = SessionId::new();
        let legacy = AppEvent::ToolCompleted(ToolEvent {
            session_id,
            tool_call_id: "todo-1".to_owned(),
            name: "todowrite".to_owned(),
            title: None,
            input: None,
            output: Some(serde_json::json!({
                "todos": [
                    {"id": "a", "content": "Inspect", "status": "completed"},
                    {"id": "b", "content": "Implement", "status": "in_progress"}
                ]
            })),
            provider_payload: Some(serde_json::json!({
                "method": "session/update",
                "params": {
                    "update": {
                        "source": "todo_tool"
                    }
                }
            })),
        });

        let event = super::AdapterEvent::from_legacy(
            AgentProviderId::from(AgentProviderId::OPENCODE),
            "acp-stdio",
            Some("session/update"),
            legacy,
        );

        assert!(matches!(
            event.legacy_projection(),
            Some(AppEvent::ToolCompleted(_))
        ));
        let UniversalEventKind::PlanUpdated { plan } = &event.universal.event else {
            panic!("expected plan update");
        };
        assert_eq!(plan.source, PlanSource::TodoTool);
        assert_eq!(plan.status, PlanStatus::Implementing);
        assert_eq!(plan.entries.len(), 2);
        assert_eq!(plan.entries[0].status, PlanEntryStatus::Completed);
        assert_eq!(plan.entries[1].label, "Implement");
    }

    #[test]
    fn todowrite_without_structured_todos_stays_tool_projection() {
        let session_id = SessionId::new();
        let legacy = AppEvent::ToolCompleted(ToolEvent {
            session_id,
            tool_call_id: "todo-1".to_owned(),
            name: "todowrite".to_owned(),
            title: None,
            input: None,
            output: Some(serde_json::json!({"message": "nothing structured"})),
            provider_payload: Some(serde_json::json!({
                "method": "session/update",
                "params": {
                    "update": {
                        "source": "todo_tool"
                    }
                }
            })),
        });

        let event = super::AdapterEvent::from_legacy(
            AgentProviderId::from(AgentProviderId::OPENCODE),
            "acp-stdio",
            Some("session/update"),
            legacy,
        );

        assert!(matches!(
            event.universal.event,
            UniversalEventKind::ItemCreated { .. }
        ));
    }
}
