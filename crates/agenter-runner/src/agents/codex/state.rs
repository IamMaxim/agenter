#![allow(dead_code)]

use std::collections::{HashMap, HashSet};

use agenter_core::{
    ApprovalRequest, ApprovalStatus, ItemId, ItemStatus, NativeRef, ProviderNotification,
    ProviderNotificationSeverity, QuestionState, QuestionStatus, SessionId, SessionStatus, TurnId,
    TurnStatus, UniversalEventKind,
};
use chrono::Utc;
use serde_json::{json, Value};

use super::{
    codec::CODEX_APP_SERVER_PROTOCOL,
    id_map::CodexIdMap,
    reducer::CodexReducerOutput,
    session::{CodexThread, CodexThreadTurnsPage},
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexPendingNativeRequest {
    pub request_id: String,
    pub method: String,
}

impl CodexPendingNativeRequest {
    #[must_use]
    pub fn new(request_id: impl Into<String>, method: impl Into<String>) -> Self {
        Self {
            request_id: request_id.into(),
            method: method.into(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexProcessExit {
    pub status: Option<String>,
    pub stderr_excerpt: String,
}

#[derive(Debug, Clone)]
pub struct CodexReconnectState {
    pub session_id: SessionId,
    pub id_map: CodexIdMap,
    pub known_native_turns: HashSet<(String, String)>,
    pub known_native_items: HashSet<(String, String)>,
    pub pending_approvals: Vec<ApprovalRequest>,
    pub pending_questions: Vec<QuestionState>,
    pub detached_approvals: Vec<ApprovalRequest>,
    pub detached_questions: Vec<QuestionState>,
    pub detached_outputs: Vec<CodexReducerOutput>,
}

impl CodexReconnectState {
    #[must_use]
    pub fn rebuild_id_map<'a>(
        session_id: SessionId,
        threads: impl IntoIterator<Item = &'a CodexThread>,
        turn_pages: impl IntoIterator<Item = &'a CodexThreadTurnsPage>,
    ) -> Self {
        Self::rebuild(session_id, threads, turn_pages, [], [], [])
    }

    #[must_use]
    pub fn rebuild<'a>(
        session_id: SessionId,
        threads: impl IntoIterator<Item = &'a CodexThread>,
        turn_pages: impl IntoIterator<Item = &'a CodexThreadTurnsPage>,
        approvals: impl IntoIterator<Item = ApprovalRequest>,
        questions: impl IntoIterator<Item = QuestionState>,
        pending_native_requests: impl IntoIterator<Item = CodexPendingNativeRequest>,
    ) -> Self {
        let mut id_map = CodexIdMap::for_session(session_id);
        let mut known_native_turns = HashSet::new();
        let mut known_native_items = HashSet::new();
        let mut turn_statuses = HashMap::new();
        let mut session_statuses = HashMap::new();

        for thread in threads {
            rebuild_thread(
                &mut id_map,
                &mut known_native_turns,
                &mut known_native_items,
                &mut turn_statuses,
                &mut session_statuses,
                thread,
            );
        }
        for page in turn_pages {
            for turn in &page.turns {
                let turn_id = id_map.turn_id(&page.thread_id, &turn.native_turn_id);
                known_native_turns.insert((page.thread_id.clone(), turn.native_turn_id.clone()));
                turn_statuses.insert(turn_id, native_turn_status(turn.status.as_deref()));
                for item in &turn.items {
                    id_map.item_id(&page.thread_id, &item.native_item_id);
                    known_native_items
                        .insert((page.thread_id.clone(), item.native_item_id.clone()));
                }
            }
        }

        let pending_native_request_ids = pending_native_requests
            .into_iter()
            .map(|request| request.request_id)
            .collect::<HashSet<_>>();

        let mut state = Self {
            session_id,
            id_map,
            known_native_turns,
            known_native_items,
            pending_approvals: Vec::new(),
            pending_questions: Vec::new(),
            detached_approvals: Vec::new(),
            detached_questions: Vec::new(),
            detached_outputs: Vec::new(),
        };

        for approval in approvals {
            if obligation_is_still_native_pending(
                approval.native_request_id.as_deref(),
                approval.turn_id,
                &pending_native_request_ids,
                &turn_statuses,
                &session_statuses,
                SessionStatus::WaitingForApproval,
            ) {
                state.pending_approvals.push(approval);
            } else {
                state.detach_approval(approval);
            }
        }
        for question in questions {
            if obligation_is_still_native_pending(
                question.native_request_id.as_deref(),
                question.turn_id,
                &pending_native_request_ids,
                &turn_statuses,
                &session_statuses,
                SessionStatus::WaitingForInput,
            ) {
                state.pending_questions.push(question);
            } else {
                state.detach_question(question);
            }
        }

        state
    }

    fn detach_approval(&mut self, mut approval: ApprovalRequest) {
        approval.status = ApprovalStatus::Detached;
        let native = detached_native_ref(
            "approval.detached",
            approval.native_request_id.clone(),
            approval
                .native
                .as_ref()
                .and_then(|native| native.raw_payload.clone()),
            "Native Codex approval state could not be proven after reconnect.",
        );
        approval.native = Some(native.clone());
        approval.resolved_at = Some(Utc::now());
        self.detached_outputs.push(CodexReducerOutput {
            session_id: self.session_id,
            turn_id: approval.turn_id,
            item_id: approval.item_id,
            ts: Utc::now(),
            native: native.clone(),
            event: UniversalEventKind::ApprovalResolved {
                approval_id: approval.approval_id,
                status: ApprovalStatus::Detached,
                resolved_at: approval.resolved_at.expect("set above"),
                resolved_by_user_id: None,
                native: Some(native),
            },
        });
        self.detached_approvals.push(approval);
    }

    fn detach_question(&mut self, mut question: QuestionState) {
        question.status = QuestionStatus::Detached;
        let native = detached_native_ref(
            "question.detached",
            question.native_request_id.clone(),
            question
                .native
                .as_ref()
                .and_then(|native| native.raw_payload.clone()),
            "Native Codex question state could not be proven after reconnect.",
        );
        question.native = Some(native.clone());
        question.answered_at = Some(Utc::now());
        self.detached_outputs.push(CodexReducerOutput {
            session_id: self.session_id,
            turn_id: question.turn_id,
            item_id: None,
            ts: Utc::now(),
            native,
            event: UniversalEventKind::QuestionAnswered {
                question: Box::new(question.clone()),
            },
        });
        self.detached_questions.push(question);
    }
}

fn rebuild_thread(
    id_map: &mut CodexIdMap,
    known_native_turns: &mut HashSet<(String, String)>,
    known_native_items: &mut HashSet<(String, String)>,
    turn_statuses: &mut HashMap<TurnId, TurnStatus>,
    session_statuses: &mut HashMap<String, SessionStatus>,
    thread: &CodexThread,
) {
    session_statuses.insert(thread.native_thread_id.clone(), thread.status.clone());
    for turn in &thread.turns {
        let turn_id = id_map.turn_id(&thread.native_thread_id, &turn.native_turn_id);
        known_native_turns.insert((thread.native_thread_id.clone(), turn.native_turn_id.clone()));
        turn_statuses.insert(turn_id, native_turn_status(turn.status.as_deref()));
        for item in &turn.items {
            id_map.item_id(&thread.native_thread_id, &item.native_item_id);
            known_native_items
                .insert((thread.native_thread_id.clone(), item.native_item_id.clone()));
        }
    }
}

fn obligation_is_still_native_pending(
    native_request_id: Option<&str>,
    turn_id: Option<TurnId>,
    pending_native_request_ids: &HashSet<String>,
    turn_statuses: &HashMap<TurnId, TurnStatus>,
    session_statuses: &HashMap<String, SessionStatus>,
    waiting_status: SessionStatus,
) -> bool {
    if native_request_id.is_some_and(|id| pending_native_request_ids.contains(id)) {
        return true;
    }
    let Some(turn_id) = turn_id else {
        return false;
    };
    let turn_can_still_block = turn_statuses
        .get(&turn_id)
        .is_some_and(|status| !turn_status_is_terminal(status));
    let session_reports_waiting = session_statuses
        .values()
        .any(|status| *status == waiting_status);
    turn_can_still_block && session_reports_waiting
}

fn native_turn_status(status: Option<&str>) -> TurnStatus {
    match status {
        Some("completed") => TurnStatus::Completed,
        Some("failed") => TurnStatus::Failed,
        Some("interrupted") => TurnStatus::Interrupted,
        Some("cancelled") => TurnStatus::Cancelled,
        Some("waitingOnApproval") => TurnStatus::WaitingForApproval,
        Some("waitingOnUserInput") => TurnStatus::WaitingForInput,
        Some("detached") => TurnStatus::Detached,
        _ => TurnStatus::Running,
    }
}

fn turn_status_is_terminal(status: &TurnStatus) -> bool {
    matches!(
        status,
        TurnStatus::Completed
            | TurnStatus::Failed
            | TurnStatus::Cancelled
            | TurnStatus::Interrupted
            | TurnStatus::Detached
    )
}

#[must_use]
pub fn codex_process_exit_outputs(
    session_id: SessionId,
    exit: CodexProcessExit,
) -> Vec<CodexReducerOutput> {
    let raw_payload = json!({
        "type": "processExited",
        "status": exit.status,
        "stderrExcerpt": exit.stderr_excerpt,
    });
    let native = NativeRef {
        protocol: CODEX_APP_SERVER_PROTOCOL.to_owned(),
        method: Some("transport/process_exited".to_owned()),
        kind: Some("transport".to_owned()),
        native_id: None,
        summary: Some("Codex app-server process exited".to_owned()),
        hash: None,
        pointer: None,
        raw_payload: Some(raw_payload),
    };
    let message = native
        .raw_payload
        .as_ref()
        .and_then(|raw| raw.get("stderrExcerpt"))
        .and_then(Value::as_str)
        .filter(|stderr| !stderr.is_empty())
        .map(|stderr| format!("Codex app-server exited: {stderr}"))
        .unwrap_or_else(|| "Codex app-server exited before producing another frame.".to_owned());
    // Runner WAL order matters here: downstream replay should first see the
    // provider become degraded, then the visible error, then the terminal
    // stopped status that prevents reconnect readers from treating the turn as
    // still live past the acknowledged sequence.
    vec![
        CodexReducerOutput {
            session_id,
            turn_id: None,
            item_id: None,
            ts: Utc::now(),
            native: native.clone(),
            event: UniversalEventKind::SessionStatusChanged {
                status: SessionStatus::Degraded,
                reason: Some("Codex app-server process exited".to_owned()),
            },
        },
        CodexReducerOutput {
            session_id,
            turn_id: None,
            item_id: None,
            ts: Utc::now(),
            native: native.clone(),
            event: UniversalEventKind::ErrorReported {
                code: Some("codex_process_exited".to_owned()),
                message,
            },
        },
        CodexReducerOutput {
            session_id,
            turn_id: None,
            item_id: None,
            ts: Utc::now(),
            native,
            event: UniversalEventKind::SessionStatusChanged {
                status: SessionStatus::Stopped,
                reason: Some("Codex app-server process exited".to_owned()),
            },
        },
    ]
}

#[derive(Debug, Clone, Default)]
pub struct CodexReplayReconciler {
    item_statuses: HashMap<ItemId, ItemStatus>,
    completed_blocks: HashSet<String>,
}

impl CodexReplayReconciler {
    pub fn observe_streamed_outputs(&mut self, outputs: &[CodexReducerOutput]) {
        for output in outputs {
            self.observe_output(output);
        }
    }

    #[must_use]
    pub fn reconcile_imported_outputs(
        &mut self,
        outputs: Vec<CodexReducerOutput>,
    ) -> Vec<CodexReducerOutput> {
        // Imported `thread/read` or `thread/turns/list` items are reconciled
        // after already-acked streamed WAL deltas. We keep terminal facts that
        // advance the materialized item, but drop duplicate item creation and
        // replay-only deltas so `snapshot_seq` + replay cursors remain
        // idempotent for browser reconnects.
        let mut reconciled = Vec::new();
        for output in outputs {
            if self.should_keep_imported_output(&output) {
                self.observe_output(&output);
                reconciled.push(output);
            } else if let Some(item_id) = output.item_id {
                if matches!(output.event, UniversalEventKind::ItemCreated { .. }) {
                    self.item_statuses.insert(item_id, ItemStatus::Completed);
                }
            }
        }
        reconciled
    }

    #[must_use]
    pub fn has_item(&self, item_id: ItemId, status: ItemStatus) -> bool {
        self.item_statuses.get(&item_id) == Some(&status)
    }

    fn should_keep_imported_output(&self, output: &CodexReducerOutput) -> bool {
        match &output.event {
            UniversalEventKind::ItemCreated { .. } => output
                .item_id
                .map_or(true, |item_id| !self.item_statuses.contains_key(&item_id)),
            UniversalEventKind::ContentCompleted { block_id, .. } => {
                !self.completed_blocks.contains(block_id)
            }
            UniversalEventKind::ContentDelta { .. } => false,
            _ => true,
        }
    }

    fn observe_output(&mut self, output: &CodexReducerOutput) {
        match &output.event {
            UniversalEventKind::ItemCreated { item } => {
                if let Some(item_id) = output.item_id {
                    self.item_statuses.insert(item_id, item.status.clone());
                }
            }
            UniversalEventKind::ContentDelta { .. } => {
                if let Some(item_id) = output.item_id {
                    self.item_statuses
                        .entry(item_id)
                        .or_insert(ItemStatus::Streaming);
                }
            }
            UniversalEventKind::ContentCompleted { block_id, .. } => {
                self.completed_blocks.insert(block_id.clone());
                if let Some(item_id) = output.item_id {
                    self.item_statuses.insert(item_id, ItemStatus::Completed);
                }
            }
            _ => {}
        }
    }
}

fn detached_native_ref(
    method: &str,
    native_id: Option<String>,
    raw_payload: Option<Value>,
    reason: &str,
) -> NativeRef {
    NativeRef {
        protocol: CODEX_APP_SERVER_PROTOCOL.to_owned(),
        method: Some(method.to_owned()),
        kind: Some("reconnect".to_owned()),
        native_id,
        summary: Some(reason.to_owned()),
        hash: None,
        pointer: None,
        raw_payload: raw_payload.or_else(|| Some(json!({ "detachedReason": reason }))),
    }
}

#[must_use]
pub fn codex_reconnect_notification(
    session_id: SessionId,
    detail: impl Into<String>,
) -> CodexReducerOutput {
    let detail = detail.into();
    let native = detached_native_ref(
        "reconnect/reconciled",
        None,
        Some(json!({ "detail": detail })),
        "Codex reconnect reconciliation completed.",
    );
    CodexReducerOutput {
        session_id,
        turn_id: None,
        item_id: None,
        ts: Utc::now(),
        native,
        event: UniversalEventKind::ProviderNotification {
            notification: ProviderNotification {
                category: "codex_reconnect".to_owned(),
                title: "Codex reconnect reconciled".to_owned(),
                detail: Some(detail),
                status: Some("reconciled".to_owned()),
                severity: Some(ProviderNotificationSeverity::Info),
                subject: None,
            },
        },
    }
}

#[cfg(test)]
mod tests {
    use agenter_core::{
        ApprovalKind, ApprovalOption, ApprovalRequest, ApprovalStatus, ItemStatus, QuestionState,
        QuestionStatus, SessionId, SessionStatus, UniversalEventKind,
    };
    use serde_json::json;

    use super::*;
    use crate::agents::codex::{
        id_map::CodexIdMap,
        reducer::CodexReducer,
        session::{CodexHistoryItem, CodexThread, CodexTurn},
    };

    fn imported_thread(session_id: SessionId) -> (CodexThread, CodexIdMap) {
        let mut id_map = CodexIdMap::for_session(session_id);
        let turn_id = id_map.turn_id("thread-1", "turn-1");
        let item_1 = id_map.item_id("thread-1", "item-1");
        let item_2 = id_map.item_id("thread-1", "item-2");
        let thread = CodexThread {
            native_thread_id: "thread-1".to_owned(),
            title: Some("Reconnect fixture".to_owned()),
            preview: Some("Reconnect fixture".to_owned()),
            name: None,
            forked_from_id: None,
            cwd: None,
            path: None,
            model: None,
            model_provider: None,
            created_at: None,
            updated_at: None,
            status: SessionStatus::WaitingForApproval,
            source: None,
            agent_nickname: None,
            agent_role: None,
            git_info: None,
            raw_payload: json!({ "id": "thread-1" }),
            turns: vec![CodexTurn {
                native_turn_id: "turn-1".to_owned(),
                turn_id,
                status: Some("inProgress".to_owned()),
                started_at: None,
                completed_at: None,
                raw_payload: json!({ "id": "turn-1", "status": "inProgress" }),
                items: vec![
                    CodexHistoryItem {
                        native_item_id: "item-1".to_owned(),
                        item_id: item_1,
                        kind: Some("commandExecution".to_owned()),
                        raw_payload: json!({ "id": "item-1", "type": "commandExecution" }),
                    },
                    CodexHistoryItem {
                        native_item_id: "item-2".to_owned(),
                        item_id: item_2,
                        kind: Some("agentMessage".to_owned()),
                        raw_payload: json!({ "id": "item-2", "type": "agentMessage" }),
                    },
                ],
            }],
        };
        (thread, id_map)
    }

    #[test]
    fn codex_reconnect_rebuilds_id_map_from_imported_history() {
        let session_id = SessionId::new();
        let (thread, original_map) = imported_thread(session_id);
        let rebuilt = CodexReconnectState::rebuild_id_map(session_id, [&thread], []);

        assert_eq!(
            rebuilt.known_native_turns,
            HashSet::from([("thread-1".to_owned(), "turn-1".to_owned())])
        );
        assert_eq!(
            rebuilt.known_native_items,
            HashSet::from([
                ("thread-1".to_owned(), "item-1".to_owned()),
                ("thread-1".to_owned(), "item-2".to_owned())
            ])
        );
        let mut rebuilt_map = rebuilt.id_map.clone();
        let mut original_map = original_map;
        assert_eq!(
            rebuilt_map.turn_id("thread-1", "turn-1"),
            original_map.turn_id("thread-1", "turn-1")
        );
        assert_eq!(
            rebuilt_map.item_id("thread-1", "item-2"),
            original_map.item_id("thread-1", "item-2")
        );
    }

    #[test]
    fn codex_reconnect_keeps_pending_approval_when_native_request_is_still_held() {
        let session_id = SessionId::new();
        let (thread, mut id_map) = imported_thread(session_id);
        let approval = ApprovalRequest {
            approval_id: agenter_core::ApprovalId::new(),
            session_id,
            turn_id: Some(id_map.turn_id("thread-1", "turn-1")),
            item_id: Some(id_map.item_id("thread-1", "item-1")),
            kind: ApprovalKind::Command,
            title: "Approve".to_owned(),
            details: None,
            options: ApprovalOption::canonical_defaults(),
            status: ApprovalStatus::Pending,
            risk: None,
            subject: None,
            native_request_id: Some("req-1".to_owned()),
            native_blocking: true,
            policy: None,
            native: Some(test_native_ref(
                json!({ "id": "req-1", "params": { "itemId": "item-1" } }),
            )),
            requested_at: None,
            resolved_at: None,
            resolving_decision: None,
        };

        let rebuilt = CodexReconnectState::rebuild(
            session_id,
            [&thread],
            [],
            [approval.clone()],
            [],
            [CodexPendingNativeRequest::new(
                "req-1",
                "item/commandExecution/requestApproval",
            )],
        );

        assert_eq!(rebuilt.pending_approvals, vec![approval]);
        assert!(rebuilt.detached_outputs.is_empty());
    }

    #[test]
    fn codex_reconnect_detaches_pending_question_when_native_state_cannot_be_proven() {
        let session_id = SessionId::new();
        let (mut thread, mut id_map) = imported_thread(session_id);
        thread.status = SessionStatus::Idle;
        thread.turns[0].status = Some("completed".to_owned());
        let question = QuestionState {
            question_id: agenter_core::QuestionId::new(),
            session_id,
            turn_id: Some(id_map.turn_id("thread-1", "turn-1")),
            title: "Input".to_owned(),
            description: None,
            fields: Vec::new(),
            status: QuestionStatus::Pending,
            answer: None,
            native_request_id: Some("req-question".to_owned()),
            native_blocking: true,
            native: Some(test_native_ref(
                json!({ "id": "req-question", "method": "mcpServer/elicitation/request" }),
            )),
            requested_at: None,
            answered_at: None,
        };

        let rebuilt =
            CodexReconnectState::rebuild(session_id, [&thread], [], [], [question.clone()], []);

        assert!(rebuilt.pending_questions.is_empty());
        assert_eq!(
            rebuilt.detached_questions[0].status,
            QuestionStatus::Detached
        );
        assert_eq!(
            rebuilt.detached_questions[0]
                .native
                .as_ref()
                .and_then(|native| native.raw_payload.as_ref()),
            question
                .native
                .as_ref()
                .and_then(|native| native.raw_payload.as_ref())
        );
        assert!(matches!(
            rebuilt.detached_outputs[0].event,
            UniversalEventKind::QuestionAnswered { .. }
        ));
    }

    #[test]
    fn codex_reconnect_process_exit_maps_to_truthful_status_and_error_outputs() {
        let session_id = SessionId::new();
        let outputs = codex_process_exit_outputs(
            session_id,
            CodexProcessExit {
                status: None,
                stderr_excerpt: "panic: app-server crashed".to_owned(),
            },
        );

        assert!(matches!(
            outputs[0].event,
            UniversalEventKind::SessionStatusChanged {
                status: SessionStatus::Degraded,
                ..
            }
        ));
        assert!(matches!(
            outputs[1].event,
            UniversalEventKind::ErrorReported { ref message, .. } if message.contains("panic")
        ));
        assert!(matches!(
            outputs[2].event,
            UniversalEventKind::SessionStatusChanged {
                status: SessionStatus::Stopped,
                ..
            }
        ));
        assert!(outputs
            .iter()
            .all(|output| output.native.raw_payload.is_some()));
    }

    #[test]
    fn codex_reconnect_reconciles_imported_final_items_without_duplicate_streamed_items() {
        let session_id = SessionId::new();
        let mut id_map = CodexIdMap::for_session(session_id);
        let mut reducer = CodexReducer::new(session_id, id_map.clone());
        let item_id = id_map.item_id("thread-1", "item-2");
        let streamed = reducer.reduce_server_notification(&notification(json!({
            "jsonrpc": "2.0",
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-2",
                "delta": "final "
            }
        })));
        let imported = reducer.reduce_history_item(
            "thread-1",
            "turn-1",
            json!({ "id": "item-2", "type": "agentMessage", "text": "final answer" }),
        );
        let mut reconciler = CodexReplayReconciler::default();
        reconciler.observe_streamed_outputs(&streamed);
        let reconciled = reconciler.reconcile_imported_outputs(imported);

        assert_eq!(
            reconciled
                .iter()
                .filter(|output| matches!(output.event, UniversalEventKind::ItemCreated { .. }))
                .count(),
            0
        );
        assert!(reconciled
            .iter()
            .any(|output| matches!(output.event, UniversalEventKind::ContentCompleted { .. })));
        assert!(reconciler.has_item(item_id, ItemStatus::Completed));
    }

    fn notification(
        raw: serde_json::Value,
    ) -> crate::agents::codex::codec::CodexServerNotificationFrame {
        let mut codec = crate::agents::codex::codec::CodexCodec::new();
        let crate::agents::codex::codec::CodexDecodedFrame::ServerNotification(frame) =
            codec.decode_line(&raw.to_string())
        else {
            panic!("expected notification frame");
        };
        frame
    }

    fn test_native_ref(raw_payload: serde_json::Value) -> NativeRef {
        NativeRef {
            protocol: CODEX_APP_SERVER_PROTOCOL.to_owned(),
            method: Some("test".to_owned()),
            kind: Some("server_request".to_owned()),
            native_id: raw_payload
                .get("id")
                .and_then(serde_json::Value::as_str)
                .map(str::to_owned),
            summary: None,
            hash: None,
            pointer: None,
            raw_payload: Some(raw_payload),
        }
    }
}
