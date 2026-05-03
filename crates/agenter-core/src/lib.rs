//! Core domain types for Agenter.

pub mod approval;
pub mod events;
pub mod ids;
pub mod logging;
pub mod session;
pub mod slash_command;
pub mod workspace;

pub use approval::{
    ApprovalDecision, ApprovalKind, ApprovalOption, ApprovalOptionKind, ApprovalPolicyMetadata,
    ApprovalRequest, ApprovalRequestEvent, ApprovalResolutionState, ApprovalResolvedEvent,
    ApprovalRisk, ApprovalStatus, PolicyAction,
};
pub use events::{
    AgentErrorEvent, AgentMessageDeltaEvent, AppEvent, ArtifactKind, ArtifactState, CommandAction,
    CommandCompletedEvent, CommandEvent, CommandOutputEvent, CommandOutputStream, ContentBlock,
    ContentBlockKind, DiffFile, DiffState, FileChangeEvent, FileChangeKind, ItemRole, ItemState,
    ItemStatus, MessageCompletedEvent, NativeRef, PlanEntry, PlanEntryStatus, PlanEvent,
    PlanSource, PlanState, PlanStatus, ProviderEvent, SessionStatusChangedEvent,
    ToolActionProjection, ToolCommandProjection, ToolEvent, ToolMcpProjection, ToolProjection,
    ToolProjectionKind, ToolSubagentOperation, ToolSubagentProjection, ToolSubagentStateProjection,
    UniversalCommand, UniversalCommandEnvelope, UniversalEventEnvelope, UniversalEventKind,
    UniversalEventSource, UniversalPlanEntry, UniversalSeq, UserInput, UserMessageEvent,
};
pub use ids::{
    ApprovalId, ArtifactId, CommandId, ConnectorBindingId, DiffId, ItemId, PlanId, QuestionId,
    RunnerId, SessionId, TurnId, UserId, WorkspaceId,
};
pub use session::{
    AgentCapabilities, AgentCollaborationMode, AgentModelOption, AgentOptions, AgentProviderId,
    AgentQuestionAnswer, AgentQuestionChoice, AgentQuestionField, AgentReasoningEffort,
    AgentTurnSettings, ApprovalCapabilities, CapabilitySet, ContentCapabilities,
    IntegrationCapabilities, ModeCapabilities, PlanCapabilities, ProtocolCapabilities,
    QuestionAnsweredEvent, QuestionRequestedEvent, SessionInfo, SessionSnapshot, SessionStatus,
    SessionUsageContext, SessionUsageSnapshot, SessionUsageWindow, ToolCapabilities, TurnState,
    TurnStatus,
};
pub use slash_command::{
    SlashCommandArgument, SlashCommandArgumentKind, SlashCommandDangerLevel,
    SlashCommandDefinition, SlashCommandRequest, SlashCommandResult, SlashCommandTarget,
};
pub use workspace::WorkspaceRef;
