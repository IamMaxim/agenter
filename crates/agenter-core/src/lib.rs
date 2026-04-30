//! Core domain types for Agenter.

pub mod approval;
pub mod events;
pub mod ids;
pub mod session;
pub mod workspace;

pub use approval::{ApprovalDecision, ApprovalKind, ApprovalRequestEvent, ApprovalResolvedEvent};
pub use events::{
    AgentErrorEvent, AgentMessageDeltaEvent, AppEvent, CommandCompletedEvent, CommandEvent,
    CommandOutputEvent, CommandOutputStream, FileChangeEvent, FileChangeKind,
    MessageCompletedEvent, PlanEntry, PlanEntryStatus, PlanEvent, SessionStatusChangedEvent,
    ToolEvent, UserMessageEvent,
};
pub use ids::{ApprovalId, ConnectorBindingId, RunnerId, SessionId, UserId, WorkspaceId};
pub use session::{AgentCapabilities, AgentProviderId, SessionInfo, SessionStatus};
pub use workspace::WorkspaceRef;
