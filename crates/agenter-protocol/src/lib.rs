//! Protocol types shared by Agenter services and clients.

pub mod browser;
pub mod runner;

use serde::{Deserialize, Serialize};

pub use browser::{BrowserClientMessage, BrowserEventEnvelope, BrowserServerMessage};
pub use runner::{
    AgentEvent, AgentInput, AgentInputCommand, AgentProviderAdvertisement, ApprovalAnswerCommand,
    CreateSessionCommand, ResumeSessionCommand, RunnerCapabilities, RunnerClientMessage,
    RunnerCommand, RunnerCommandEnvelope, RunnerCommandResult, RunnerError, RunnerEvent,
    RunnerEventEnvelope, RunnerHeartbeat, RunnerHeartbeatAck, RunnerHello, RunnerResponseEnvelope,
    RunnerResponseOutcome, RunnerServerMessage, ShutdownSessionCommand,
};

macro_rules! string_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Deserialize, Eq, Hash, Ord, PartialEq, PartialOrd, Serialize)]
        #[serde(transparent)]
        pub struct $name(String);

        impl $name {
            #[must_use]
            pub fn new(value: impl Into<String>) -> Self {
                Self(value.into())
            }

            #[must_use]
            pub fn as_str(&self) -> &str {
                &self.0
            }
        }

        impl From<&str> for $name {
            fn from(value: &str) -> Self {
                Self::new(value)
            }
        }

        impl From<String> for $name {
            fn from(value: String) -> Self {
                Self::new(value)
            }
        }

        impl std::fmt::Display for $name {
            fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
                self.0.fmt(f)
            }
        }
    };
}

string_id!(RequestId);
string_id!(EventId);
