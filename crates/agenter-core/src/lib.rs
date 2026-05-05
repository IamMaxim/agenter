//! Core domain types for Agenter.

pub mod approval;
pub mod events;
pub mod ids;
pub mod logging;
pub mod session;
pub mod slash_command;
pub mod workspace;

pub use approval::{
    AgentObligation, AgentObligationKind, AgentObligationStatus, ApprovalDecision, ApprovalKind,
    ApprovalOption, ApprovalOptionKind, ApprovalPolicyMetadata, ApprovalPolicyRulePreview,
    ApprovalRequest, ApprovalResolutionState, ApprovalRisk, ApprovalStatus, PolicyAction,
};
pub use events::{
    ArtifactKind, ArtifactState, ContentBlock, ContentBlockKind, DiffFile, DiffState,
    FileChangeKind, ItemRole, ItemState, ItemStatus, NativeRef, PlanEntry, PlanEntryStatus,
    PlanSource, PlanState, PlanStatus, ProviderNotification, ProviderNotificationSeverity,
    ToolActionProjection, ToolCommandProjection, ToolEvent, ToolMcpProjection, ToolProjection,
    ToolProjectionKind, ToolSubagentOperation, ToolSubagentProjection, ToolSubagentStateProjection,
    UniversalCommand, UniversalCommandEnvelope, UniversalEventEnvelope, UniversalEventKind,
    UniversalEventSource, UniversalPlanEntry, UniversalSeq, UserInput, UserMessageEvent,
    UNIVERSAL_PROTOCOL_VERSION,
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
    ProviderCapabilityDetail, ProviderCapabilityStatus, QuestionState, QuestionStatus, SessionInfo,
    SessionSnapshot, SessionStatus, SessionUsageContext, SessionUsageSnapshot, SessionUsageWindow,
    ToolCapabilities, TurnState, TurnStatus,
};
pub use slash_command::{
    SlashCommandArgument, SlashCommandArgumentKind, SlashCommandDangerLevel,
    SlashCommandDefinition, SlashCommandRequest, SlashCommandResult, SlashCommandTarget,
};
pub use workspace::WorkspaceRef;
