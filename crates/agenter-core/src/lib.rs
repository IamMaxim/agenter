//! Core domain types for Agenter.

pub mod approval;
pub mod events;
pub mod ids;
pub mod logging;
pub mod session;
pub mod slash_command;
pub mod workspace;

pub use approval::{ApprovalDecision, ApprovalKind, ApprovalRequestEvent, ApprovalResolvedEvent};
pub use events::{
    AgentErrorEvent, AgentMessageDeltaEvent, AppEvent, CommandAction, CommandCompletedEvent,
    CommandEvent, CommandOutputEvent, CommandOutputStream, FileChangeEvent, FileChangeKind,
    MessageCompletedEvent, PlanEntry, PlanEntryStatus, PlanEvent, ProviderEvent,
    SessionStatusChangedEvent, ToolEvent, UserMessageEvent,
};
pub use ids::{
    ApprovalId, ConnectorBindingId, QuestionId, RunnerId, SessionId, UserId, WorkspaceId,
};
pub use session::{
    AgentCapabilities, AgentCollaborationMode, AgentModelOption, AgentOptions, AgentProviderId,
    AgentQuestionAnswer, AgentQuestionChoice, AgentQuestionField, AgentReasoningEffort,
    AgentTurnSettings, QuestionAnsweredEvent, QuestionRequestedEvent, SessionInfo, SessionStatus,
};
pub use slash_command::{
    SlashCommandArgument, SlashCommandArgumentKind, SlashCommandDangerLevel,
    SlashCommandDefinition, SlashCommandRequest, SlashCommandResult, SlashCommandTarget,
};
pub use workspace::WorkspaceRef;
