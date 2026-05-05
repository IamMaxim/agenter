use agenter_core::{SessionStatus, TurnStatus};
use serde_json::Value;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum CodexTurnDriverState {
    Idle,
    Starting,
    Running,
    WaitingForApproval,
    WaitingForInput,
    Interrupting,
    Completed,
    Failed,
    Cancelled,
    Interrupted,
    Detached,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) struct CodexTurnStateTransition {
    pub previous: CodexTurnDriverState,
    pub current: CodexTurnDriverState,
    pub legal: bool,
}

#[derive(Debug)]
pub(crate) struct CodexTurnDriver {
    state: CodexTurnDriverState,
    session_id: agenter_core::SessionId,
    provider_thread_id: Option<String>,
    provider_turn_id: Option<String>,
}

impl CodexTurnDriverState {
    pub(crate) fn session_status(self) -> Option<SessionStatus> {
        match self {
            Self::Idle => Some(SessionStatus::Idle),
            Self::Starting => Some(SessionStatus::Starting),
            Self::Running => Some(SessionStatus::Running),
            Self::WaitingForApproval => Some(SessionStatus::WaitingForApproval),
            Self::WaitingForInput => Some(SessionStatus::WaitingForInput),
            Self::Interrupting => Some(SessionStatus::Running),
            Self::Completed => Some(SessionStatus::Idle),
            Self::Failed => Some(SessionStatus::Failed),
            Self::Cancelled | Self::Interrupted => Some(SessionStatus::Interrupted),
            Self::Detached => Some(SessionStatus::Degraded),
        }
    }

    fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Cancelled | Self::Interrupted | Self::Detached
        )
    }

    pub(crate) fn turn_status(self) -> Option<TurnStatus> {
        match self {
            Self::Idle => None,
            Self::Starting => Some(TurnStatus::Starting),
            Self::Running => Some(TurnStatus::Running),
            Self::WaitingForApproval => Some(TurnStatus::WaitingForApproval),
            Self::WaitingForInput => Some(TurnStatus::WaitingForInput),
            Self::Interrupting => Some(TurnStatus::Interrupting),
            Self::Completed => Some(TurnStatus::Completed),
            Self::Failed => Some(TurnStatus::Failed),
            Self::Cancelled => Some(TurnStatus::Cancelled),
            Self::Interrupted => Some(TurnStatus::Interrupted),
            Self::Detached => Some(TurnStatus::Detached),
        }
    }
}

impl CodexTurnDriver {
    pub(crate) fn new(session_id: agenter_core::SessionId) -> Self {
        Self {
            state: CodexTurnDriverState::Idle,
            session_id,
            provider_thread_id: None,
            provider_turn_id: None,
        }
    }

    #[cfg(test)]
    pub(crate) fn state(&self) -> CodexTurnDriverState {
        self.state
    }

    pub(crate) fn provider_turn_id(&self) -> Option<&str> {
        self.provider_turn_id.as_deref()
    }

    pub(crate) fn observe_targets(
        &mut self,
        provider_thread_id: Option<&str>,
        provider_turn_id: Option<&str>,
    ) {
        if let Some(thread_id) = provider_thread_id {
            self.provider_thread_id = Some(thread_id.to_owned());
        }
        if let Some(turn_id) = provider_turn_id {
            self.provider_turn_id = Some(turn_id.to_owned());
        }
    }

    pub(crate) fn turn_start_requested(
        &mut self,
        payload: Option<&Value>,
    ) -> CodexTurnStateTransition {
        self.transition(
            CodexTurnDriverState::Starting,
            "turn/start request sent",
            payload,
            |previous| matches!(previous, CodexTurnDriverState::Idle),
        )
    }

    pub(crate) fn turn_started(&mut self, payload: Option<&Value>) -> CodexTurnStateTransition {
        self.transition(
            CodexTurnDriverState::Running,
            "turn/started notification",
            payload,
            |previous| {
                matches!(
                    previous,
                    CodexTurnDriverState::Starting | CodexTurnDriverState::Running
                )
            },
        )
    }

    pub(crate) fn approval_requested(
        &mut self,
        payload: Option<&Value>,
    ) -> CodexTurnStateTransition {
        self.transition(
            CodexTurnDriverState::WaitingForApproval,
            "approval request",
            payload,
            |previous| {
                matches!(
                    previous,
                    CodexTurnDriverState::Starting
                        | CodexTurnDriverState::Running
                        | CodexTurnDriverState::WaitingForInput
                )
            },
        )
    }

    pub(crate) fn input_requested(&mut self, payload: Option<&Value>) -> CodexTurnStateTransition {
        self.transition(
            CodexTurnDriverState::WaitingForInput,
            "question request",
            payload,
            |previous| {
                matches!(
                    previous,
                    CodexTurnDriverState::Starting
                        | CodexTurnDriverState::Running
                        | CodexTurnDriverState::WaitingForApproval
                )
            },
        )
    }

    pub(crate) fn request_resolved(&mut self, payload: Option<&Value>) -> CodexTurnStateTransition {
        self.transition(
            CodexTurnDriverState::Running,
            "serverRequest/resolved",
            payload,
            |previous| {
                matches!(
                    previous,
                    CodexTurnDriverState::WaitingForApproval
                        | CodexTurnDriverState::WaitingForInput
                        | CodexTurnDriverState::Running
                        | CodexTurnDriverState::Interrupting
                )
            },
        )
    }

    pub(crate) fn browser_answered(&mut self, payload: Option<&Value>) -> CodexTurnStateTransition {
        self.transition(
            CodexTurnDriverState::Running,
            "browser pending request answer",
            payload,
            |previous| {
                matches!(
                    previous,
                    CodexTurnDriverState::WaitingForApproval
                        | CodexTurnDriverState::WaitingForInput
                        | CodexTurnDriverState::Interrupting
                )
            },
        )
    }

    pub(crate) fn interrupt_requested(
        &mut self,
        payload: Option<&Value>,
    ) -> CodexTurnStateTransition {
        self.transition(
            CodexTurnDriverState::Interrupting,
            "turn/interrupt request",
            payload,
            |previous| {
                matches!(
                    previous,
                    CodexTurnDriverState::Starting
                        | CodexTurnDriverState::Running
                        | CodexTurnDriverState::WaitingForApproval
                        | CodexTurnDriverState::WaitingForInput
                        | CodexTurnDriverState::Interrupting
                )
            },
        )
    }

    pub(crate) fn terminal_completed(
        &mut self,
        status: Option<&str>,
        payload: Option<&Value>,
    ) -> CodexTurnStateTransition {
        let next = match status {
            Some("failed") => CodexTurnDriverState::Failed,
            Some("cancelled" | "canceled") => CodexTurnDriverState::Cancelled,
            Some("interrupted") => CodexTurnDriverState::Interrupted,
            Some("detached") => CodexTurnDriverState::Detached,
            _ => CodexTurnDriverState::Completed,
        };
        self.transition(next, "turn/completed", payload, |previous| {
            !matches!(previous, CodexTurnDriverState::Idle) && !previous.is_terminal()
        })
    }

    pub(crate) fn terminal_detached(
        &mut self,
        payload: Option<&Value>,
    ) -> CodexTurnStateTransition {
        self.transition(
            CodexTurnDriverState::Detached,
            "turn/detached",
            payload,
            |previous| !matches!(previous, CodexTurnDriverState::Idle) && !previous.is_terminal(),
        )
    }

    pub(crate) fn terminal_failed(&mut self, payload: Option<&Value>) -> CodexTurnStateTransition {
        self.transition(
            CodexTurnDriverState::Failed,
            "turn/failed",
            payload,
            |previous| !matches!(previous, CodexTurnDriverState::Idle) && !previous.is_terminal(),
        )
    }

    fn transition(
        &mut self,
        next: CodexTurnDriverState,
        event: &'static str,
        payload: Option<&Value>,
        legal: impl FnOnce(CodexTurnDriverState) -> bool,
    ) -> CodexTurnStateTransition {
        let previous = self.state;
        let legal = legal(previous);
        if !legal {
            tracing::warn!(
                session_id = %self.session_id,
                provider_thread_id = self.provider_thread_id.as_deref(),
                provider_turn_id = self.provider_turn_id.as_deref(),
                event,
                previous_state = ?previous,
                next_state = ?next,
                payload_preview = payload.and_then(|value| agenter_core::logging::payload_preview(
                    value,
                    agenter_core::logging::payload_logging_enabled()
                )).as_deref(),
                "illegal codex turn driver state transition"
            );
        }
        self.state = next;
        CodexTurnStateTransition {
            previous,
            current: next,
            legal,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::{CodexTurnDriver, CodexTurnDriverState};
    use agenter_core::{SessionId, SessionStatus};

    #[test]
    fn valid_codex_turn_state_transitions_follow_turn_lifecycle() {
        let mut driver = CodexTurnDriver::new(SessionId::new());

        assert_eq!(
            driver.turn_start_requested(None).current,
            CodexTurnDriverState::Starting
        );
        assert_eq!(
            driver.turn_started(None).current,
            CodexTurnDriverState::Running
        );
        assert_eq!(
            driver.approval_requested(None).current,
            CodexTurnDriverState::WaitingForApproval
        );
        assert_eq!(
            driver.browser_answered(None).current,
            CodexTurnDriverState::Running
        );
        assert_eq!(
            driver.input_requested(None).current,
            CodexTurnDriverState::WaitingForInput
        );
        assert_eq!(
            driver.request_resolved(None).current,
            CodexTurnDriverState::Running
        );
        assert_eq!(
            driver.terminal_completed(Some("completed"), None).current,
            CodexTurnDriverState::Completed
        );
    }

    #[test]
    fn illegal_codex_turn_transition_warns_without_panicking() {
        let mut driver = CodexTurnDriver::new(SessionId::new());

        let transition = driver.terminal_completed(Some("failed"), None);

        assert_eq!(transition.previous, CodexTurnDriverState::Idle);
        assert_eq!(transition.current, CodexTurnDriverState::Failed);
        assert!(!transition.legal);
        assert_eq!(driver.state(), CodexTurnDriverState::Failed);
    }

    #[test]
    fn codex_turn_terminal_statuses_map_to_driver_states() {
        for (status, expected) in [
            (Some("completed"), CodexTurnDriverState::Completed),
            (Some("failed"), CodexTurnDriverState::Failed),
            (Some("cancelled"), CodexTurnDriverState::Cancelled),
            (Some("canceled"), CodexTurnDriverState::Cancelled),
            (Some("interrupted"), CodexTurnDriverState::Interrupted),
            (Some("detached"), CodexTurnDriverState::Detached),
        ] {
            let mut driver = CodexTurnDriver::new(SessionId::new());
            driver.turn_start_requested(None);

            assert_eq!(driver.terminal_completed(status, None).current, expected);
        }
    }

    #[test]
    fn codex_turn_driver_states_map_to_session_statuses() {
        assert_eq!(
            CodexTurnDriverState::Idle.session_status(),
            Some(SessionStatus::Idle)
        );
        assert_eq!(
            CodexTurnDriverState::Starting.session_status(),
            Some(SessionStatus::Starting)
        );
        assert_eq!(
            CodexTurnDriverState::Running.session_status(),
            Some(SessionStatus::Running)
        );
        assert_eq!(
            CodexTurnDriverState::WaitingForApproval.session_status(),
            Some(SessionStatus::WaitingForApproval)
        );
        assert_eq!(
            CodexTurnDriverState::WaitingForInput.session_status(),
            Some(SessionStatus::WaitingForInput)
        );
        assert_eq!(
            CodexTurnDriverState::Interrupting.session_status(),
            Some(SessionStatus::Running)
        );
        assert_eq!(
            CodexTurnDriverState::Completed.session_status(),
            Some(SessionStatus::Idle)
        );
        assert_eq!(
            CodexTurnDriverState::Failed.session_status(),
            Some(SessionStatus::Failed)
        );
        assert_eq!(
            CodexTurnDriverState::Cancelled.session_status(),
            Some(SessionStatus::Interrupted)
        );
        assert_eq!(
            CodexTurnDriverState::Interrupted.session_status(),
            Some(SessionStatus::Interrupted)
        );
        assert_eq!(
            CodexTurnDriverState::Detached.session_status(),
            Some(SessionStatus::Degraded)
        );
    }
}
