#![allow(dead_code)]

use agenter_core::{
    ContentBlock, ContentBlockKind, DiffFile, DiffId, DiffState, FileChangeKind, ItemId, ItemRole,
    ItemState, ItemStatus, NativeRef, PlanId, PlanSource, PlanState, PlanStatus,
    ProviderNotification, ProviderNotificationSeverity, SessionId, SessionStatus,
    SessionUsageContext, SessionUsageSnapshot, ToolActionProjection, ToolCommandProjection,
    ToolMcpProjection, ToolProjection, ToolProjectionKind, TurnId, TurnState, TurnStatus,
    UniversalEventKind, UniversalPlanEntry,
};
use chrono::{DateTime, Utc};
use serde_json::Value;
use uuid::Uuid;

use super::{
    codec::{CodexServerNotificationFrame, CODEX_APP_SERVER_PROTOCOL},
    id_map::CodexIdMap,
};

#[derive(Debug, Clone, Copy, Eq, PartialEq)]
pub struct CodexReducerMapping {
    pub codex: &'static str,
    pub mapping: &'static str,
}

pub const THREAD_ITEM_MAPPINGS: &[CodexReducerMapping] = &[
    CodexReducerMapping {
        codex: "AgentMessage",
        mapping: "item.created assistant text plus content.completed text",
    },
    CodexReducerMapping {
        codex: "CollabAgentToolCall",
        mapping: "item.created tool subkind collab_agent/subagent",
    },
    CodexReducerMapping {
        codex: "CommandExecution",
        mapping: "item.created command tool with command/output/status",
    },
    CodexReducerMapping {
        codex: "ContextCompaction",
        mapping: "item.created system native plus compaction notification",
    },
    CodexReducerMapping {
        codex: "DynamicToolCall",
        mapping: "item.created unsupported dynamic tool native row",
    },
    CodexReducerMapping {
        codex: "EnteredReviewMode",
        mapping: "item.created system native plus review notification",
    },
    CodexReducerMapping {
        codex: "ExitedReviewMode",
        mapping: "item.created system native plus review notification",
    },
    CodexReducerMapping {
        codex: "FileChange",
        mapping: "item.created file_change tool plus diff.updated",
    },
    CodexReducerMapping {
        codex: "HookPrompt",
        mapping: "item.created system native text",
    },
    CodexReducerMapping {
        codex: "ImageGeneration",
        mapping: "item.created native image_generation row",
    },
    CodexReducerMapping {
        codex: "ImageView",
        mapping: "item.created native image row",
    },
    CodexReducerMapping {
        codex: "McpToolCall",
        mapping: "item.created mcp tool with arguments/result/error",
    },
    CodexReducerMapping {
        codex: "Plan",
        mapping: "item.created assistant plan text plus plan.updated",
    },
    CodexReducerMapping {
        codex: "Reasoning",
        mapping: "item.created assistant reasoning summary/raw content",
    },
    CodexReducerMapping {
        codex: "UserMessage",
        mapping: "item.created user text/native content",
    },
    CodexReducerMapping {
        codex: "WebSearch",
        mapping: "item.created tool subkind web_search",
    },
];

pub const SERVER_NOTIFICATION_MAPPINGS: &[CodexReducerMapping] = &[
    CodexReducerMapping {
        codex: "account/login/completed",
        mapping: "provider.notification account",
    },
    CodexReducerMapping {
        codex: "account/rateLimits/updated",
        mapping: "usage.updated plus account rate notification",
    },
    CodexReducerMapping {
        codex: "account/updated",
        mapping: "provider.notification account",
    },
    CodexReducerMapping {
        codex: "app/list/updated",
        mapping: "provider.notification apps",
    },
    CodexReducerMapping {
        codex: "command/exec/outputDelta",
        mapping: "native.unknown one-off command output delta with raw payload",
    },
    CodexReducerMapping {
        codex: "configWarning",
        mapping: "provider.notification warning",
    },
    CodexReducerMapping {
        codex: "deprecationNotice",
        mapping: "provider.notification warning",
    },
    CodexReducerMapping {
        codex: "error",
        mapping: "error.reported",
    },
    CodexReducerMapping {
        codex: "externalAgentConfig/import/completed",
        mapping: "provider.notification external_agent_config",
    },
    CodexReducerMapping {
        codex: "fs/changed",
        mapping: "provider.notification fs_watch",
    },
    CodexReducerMapping {
        codex: "fuzzyFileSearch/sessionCompleted",
        mapping: "provider.notification fuzzy_file_search",
    },
    CodexReducerMapping {
        codex: "fuzzyFileSearch/sessionUpdated",
        mapping: "provider.notification fuzzy_file_search",
    },
    CodexReducerMapping {
        codex: "guardianWarning",
        mapping: "provider.notification warning",
    },
    CodexReducerMapping {
        codex: "hook/completed",
        mapping: "provider.notification hook",
    },
    CodexReducerMapping {
        codex: "hook/started",
        mapping: "provider.notification hook",
    },
    CodexReducerMapping {
        codex: "item/agentMessage/delta",
        mapping: "content.delta text",
    },
    CodexReducerMapping {
        codex: "item/autoApprovalReview/completed",
        mapping: "provider.notification approval_review",
    },
    CodexReducerMapping {
        codex: "item/autoApprovalReview/started",
        mapping: "provider.notification approval_review",
    },
    CodexReducerMapping {
        codex: "item/commandExecution/outputDelta",
        mapping: "content.delta command_output",
    },
    CodexReducerMapping {
        codex: "item/commandExecution/terminalInteraction",
        mapping: "content.delta terminal_input",
    },
    CodexReducerMapping {
        codex: "item/completed",
        mapping: "item completion mapping plus content.completed when applicable",
    },
    CodexReducerMapping {
        codex: "item/fileChange/outputDelta",
        mapping: "content.delta command_output",
    },
    CodexReducerMapping {
        codex: "item/fileChange/patchUpdated",
        mapping: "diff.updated",
    },
    CodexReducerMapping {
        codex: "item/mcpToolCall/progress",
        mapping: "content.delta provider_status",
    },
    CodexReducerMapping {
        codex: "item/plan/delta",
        mapping: "content.delta text plus partial plan.updated",
    },
    CodexReducerMapping {
        codex: "item/reasoning/summaryPartAdded",
        mapping: "provider.notification reasoning",
    },
    CodexReducerMapping {
        codex: "item/reasoning/summaryTextDelta",
        mapping: "content.delta reasoning",
    },
    CodexReducerMapping {
        codex: "item/reasoning/textDelta",
        mapping: "content.delta reasoning raw",
    },
    CodexReducerMapping {
        codex: "item/started",
        mapping: "item.created streaming",
    },
    CodexReducerMapping {
        codex: "mcpServer/oauthLogin/completed",
        mapping: "provider.notification mcp_oauth",
    },
    CodexReducerMapping {
        codex: "mcpServer/startupStatus/updated",
        mapping: "provider.notification mcp_status",
    },
    CodexReducerMapping {
        codex: "model/rerouted",
        mapping: "provider.notification model",
    },
    CodexReducerMapping {
        codex: "model/verification",
        mapping: "provider.notification model",
    },
    CodexReducerMapping {
        codex: "rawResponseItem/completed",
        mapping: "native.unknown raw response item",
    },
    CodexReducerMapping {
        codex: "remoteControl/status/changed",
        mapping: "provider.notification remote_control",
    },
    CodexReducerMapping {
        codex: "serverRequest/resolved",
        mapping: "provider.notification unresolved_server_request_resolved for Stage 6",
    },
    CodexReducerMapping {
        codex: "skills/changed",
        mapping: "provider.notification skills",
    },
    CodexReducerMapping {
        codex: "thread/archived",
        mapping: "session.status_changed archived",
    },
    CodexReducerMapping {
        codex: "thread/closed",
        mapping: "session.status_changed stopped",
    },
    CodexReducerMapping {
        codex: "thread/compacted",
        mapping: "provider.notification compaction",
    },
    CodexReducerMapping {
        codex: "thread/goal/cleared",
        mapping: "provider.notification thread_goal",
    },
    CodexReducerMapping {
        codex: "thread/goal/updated",
        mapping: "provider.notification thread_goal",
    },
    CodexReducerMapping {
        codex: "thread/name/updated",
        mapping: "session.metadata_changed",
    },
    CodexReducerMapping {
        codex: "thread/realtime/closed",
        mapping: "provider.notification realtime",
    },
    CodexReducerMapping {
        codex: "thread/realtime/error",
        mapping: "error.reported plus realtime notification",
    },
    CodexReducerMapping {
        codex: "thread/realtime/itemAdded",
        mapping: "native.unknown realtime item raw payload",
    },
    CodexReducerMapping {
        codex: "thread/realtime/outputAudio/delta",
        mapping: "native.unknown realtime audio raw payload",
    },
    CodexReducerMapping {
        codex: "thread/realtime/sdp",
        mapping: "provider.notification realtime",
    },
    CodexReducerMapping {
        codex: "thread/realtime/started",
        mapping: "provider.notification realtime",
    },
    CodexReducerMapping {
        codex: "thread/realtime/transcript/delta",
        mapping: "content.delta text",
    },
    CodexReducerMapping {
        codex: "thread/realtime/transcript/done",
        mapping: "content.completed text",
    },
    CodexReducerMapping {
        codex: "thread/started",
        mapping: "provider.notification thread_started metadata reconciliation",
    },
    CodexReducerMapping {
        codex: "thread/status/changed",
        mapping: "session.status_changed",
    },
    CodexReducerMapping {
        codex: "thread/tokenUsage/updated",
        mapping: "usage.updated",
    },
    CodexReducerMapping {
        codex: "thread/unarchived",
        mapping: "session.status_changed idle",
    },
    CodexReducerMapping {
        codex: "turn/completed",
        mapping: "turn.completed/interrupted/failed/status_changed",
    },
    CodexReducerMapping {
        codex: "turn/diff/updated",
        mapping: "diff.updated",
    },
    CodexReducerMapping {
        codex: "turn/plan/updated",
        mapping: "plan.updated",
    },
    CodexReducerMapping {
        codex: "turn/started",
        mapping: "turn.started",
    },
    CodexReducerMapping {
        codex: "warning",
        mapping: "provider.notification warning",
    },
    CodexReducerMapping {
        codex: "windows/worldWritableWarning",
        mapping: "provider.notification warning",
    },
    CodexReducerMapping {
        codex: "windowsSandbox/setupCompleted",
        mapping: "provider.notification windows_sandbox",
    },
];

#[derive(Debug, Clone, PartialEq)]
pub struct CodexReducerOutput {
    pub session_id: SessionId,
    pub turn_id: Option<TurnId>,
    pub item_id: Option<ItemId>,
    pub ts: DateTime<Utc>,
    pub native: NativeRef,
    pub event: UniversalEventKind,
}

#[derive(Debug, Clone)]
pub struct CodexReducer {
    session_id: SessionId,
    id_map: CodexIdMap,
}

struct ItemReduction {
    turn_id: Option<TurnId>,
    item_id: ItemId,
    native_item_id: String,
    kind: String,
    raw_item: Value,
    status: ItemStatus,
}

struct ProviderNotice {
    turn_id: Option<TurnId>,
    item_id: Option<ItemId>,
    native: NativeRef,
    category: String,
    title: String,
    detail: Option<String>,
    severity: Option<ProviderNotificationSeverity>,
}

impl CodexReducer {
    #[must_use]
    pub fn new(session_id: SessionId, id_map: CodexIdMap) -> Self {
        Self { session_id, id_map }
    }

    pub fn reduce_history_item(
        &mut self,
        native_thread_id: &str,
        native_turn_id: &str,
        raw_item: Value,
    ) -> Vec<CodexReducerOutput> {
        let Some(kind) = item_kind(&raw_item).map(str::to_owned) else {
            return vec![self.output(
                None,
                None,
                native_ref_for_item(None, None, raw_item),
                UniversalEventKind::NativeUnknown {
                    summary: Some("Codex thread item is missing type".to_owned()),
                },
            )];
        };
        let native_item_id =
            item_native_id(&raw_item).unwrap_or_else(|| synthetic_item_key(&raw_item));
        let turn_id = self.id_map.turn_id(native_thread_id, native_turn_id);
        let item_id = self.id_map.item_id(native_thread_id, &native_item_id);
        self.reduce_item_value(
            native_thread_id,
            ItemReduction {
                turn_id: Some(turn_id),
                item_id,
                native_item_id,
                kind,
                raw_item,
                status: ItemStatus::Completed,
            },
        )
    }

    pub fn reduce_server_notification(
        &mut self,
        frame: &CodexServerNotificationFrame,
    ) -> Vec<CodexReducerOutput> {
        let raw = frame.raw_payload.clone();
        let params = raw.get("params").unwrap_or(&Value::Null);
        let native = native_ref_for_notification(frame);
        match frame.method.as_str() {
            "error" => self.error_reported(params, native),
            "thread/status/changed" => self.session_status_changed(params, native),
            "thread/archived" => vec![self.output(
                None,
                None,
                native,
                UniversalEventKind::SessionStatusChanged {
                    status: SessionStatus::Archived,
                    reason: Some("Codex thread archived".to_owned()),
                },
            )],
            "thread/unarchived" => vec![self.output(
                None,
                None,
                native,
                UniversalEventKind::SessionStatusChanged {
                    status: SessionStatus::Idle,
                    reason: Some("Codex thread unarchived".to_owned()),
                },
            )],
            "thread/closed" => vec![self.output(
                None,
                None,
                native,
                UniversalEventKind::SessionStatusChanged {
                    status: SessionStatus::Stopped,
                    reason: Some("Codex thread closed".to_owned()),
                },
            )],
            "thread/name/updated" => vec![self.output(
                None,
                None,
                native,
                UniversalEventKind::SessionMetadataChanged {
                    title: string_at(params, &["threadName"]),
                },
            )],
            "thread/tokenUsage/updated" => self.usage_updated(params, native),
            "turn/started" => self.turn_started(params, native),
            "turn/completed" => self.turn_completed(params, native),
            "turn/diff/updated" => self.diff_updated(params, native),
            "turn/plan/updated" => self.structured_plan_updated(params, native),
            "item/started" => self.notification_item(params, native, ItemStatus::Streaming),
            "item/completed" => self.notification_item(params, native, ItemStatus::Completed),
            "item/agentMessage/delta" => {
                self.content_delta(params, native, "text", ContentBlockKind::Text, "delta")
            }
            "item/plan/delta" => self.plan_delta(params, native),
            "item/reasoning/summaryTextDelta" => self.content_delta(
                params,
                native,
                &format!(
                    "reasoning-summary-{}",
                    params
                        .get("summaryIndex")
                        .and_then(Value::as_i64)
                        .unwrap_or(0)
                ),
                ContentBlockKind::Reasoning,
                "delta",
            ),
            "item/reasoning/textDelta" => self.content_delta(
                params,
                native,
                &format!(
                    "reasoning-raw-{}",
                    params
                        .get("contentIndex")
                        .and_then(Value::as_i64)
                        .unwrap_or(0)
                ),
                ContentBlockKind::Reasoning,
                "delta",
            ),
            "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" => self
                .content_delta(
                    params,
                    native,
                    "output",
                    ContentBlockKind::CommandOutput,
                    "delta",
                ),
            "item/commandExecution/terminalInteraction" => self.content_delta(
                params,
                native,
                "terminal-input",
                ContentBlockKind::TerminalInput,
                "stdin",
            ),
            "item/mcpToolCall/progress" => self.content_delta(
                params,
                native,
                "progress",
                ContentBlockKind::ProviderStatus,
                "message",
            ),
            "item/fileChange/patchUpdated" => self.patch_updated(params, native),
            "thread/realtime/transcript/delta" => {
                self.realtime_transcript_delta(params, native, false)
            }
            "thread/realtime/transcript/done" => {
                self.realtime_transcript_delta(params, native, true)
            }
            "thread/realtime/error" => {
                let mut outputs = self.error_reported(params, native.clone());
                outputs.push(self.provider_notification(ProviderNotice {
                    turn_id: None,
                    item_id: None,
                    native,
                    category: "realtime".to_owned(),
                    title: "Realtime error".to_owned(),
                    detail: string_at(params, &["message"]),
                    severity: Some(ProviderNotificationSeverity::Error),
                }));
                outputs
            }
            "rawResponseItem/completed"
            | "thread/realtime/itemAdded"
            | "thread/realtime/outputAudio/delta"
            | "command/exec/outputDelta" => {
                let turn_id = self.turn_id_from_params(params);
                let item_id = self.item_id_from_params(params);
                vec![self.output(
                    turn_id,
                    item_id,
                    native,
                    UniversalEventKind::NativeUnknown {
                        summary: Some(format!("Codex native frame {}", frame.method)),
                    },
                )]
            }
            "serverRequest/resolved" => vec![self.provider_notification(ProviderNotice {
                turn_id: None,
                item_id: None,
                native,
                category: "server_request".to_owned(),
                title: "Server request resolved".to_owned(),
                detail: string_at(params, &["requestId"]).or_else(|| {
                    params
                        .get("requestId")
                        .map(|value| value.to_string())
                        .filter(|value| value != "null")
                }),
                severity: Some(ProviderNotificationSeverity::Info),
            })],
            method if is_known_reduced_notification(method) => {
                vec![self.generic_provider_notification(method, params, native)]
            }
            _ => {
                let turn_id = self.turn_id_from_params(params);
                let item_id = self.item_id_from_params(params);
                vec![self.output(
                    turn_id,
                    item_id,
                    native,
                    UniversalEventKind::NativeUnknown {
                        summary: Some(format!("Unknown Codex notification {}", frame.method)),
                    },
                )]
            }
        }
    }

    fn notification_item(
        &mut self,
        params: &Value,
        native: NativeRef,
        status: ItemStatus,
    ) -> Vec<CodexReducerOutput> {
        let Some(item) = params.get("item").cloned() else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex item notification missing item",
            )];
        };
        let Some(native_thread_id) = string_at(params, &["threadId"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex item notification missing threadId",
            )];
        };
        let Some(native_turn_id) = string_at(params, &["turnId"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex item notification missing turnId",
            )];
        };
        let native_item_id = item_native_id(&item).unwrap_or_else(|| synthetic_item_key(&item));
        let kind = item_kind(&item).unwrap_or("unknown").to_owned();
        let turn_id = self.id_map.turn_id(&native_thread_id, &native_turn_id);
        let item_id = self.id_map.item_id(&native_thread_id, &native_item_id);
        self.reduce_item_value_with_native(
            &native_thread_id,
            ItemReduction {
                turn_id: Some(turn_id),
                item_id,
                native_item_id,
                kind,
                raw_item: item,
                status,
            },
            native,
        )
    }

    fn reduce_item_value(
        &mut self,
        native_thread_id: &str,
        reduction: ItemReduction,
    ) -> Vec<CodexReducerOutput> {
        let native = native_ref_for_item(
            Some(&reduction.kind),
            Some(&reduction.native_item_id),
            reduction.raw_item.clone(),
        );
        self.reduce_item_value_with_native(native_thread_id, reduction, native)
    }

    fn reduce_item_value_with_native(
        &mut self,
        native_thread_id: &str,
        reduction: ItemReduction,
        native: NativeRef,
    ) -> Vec<CodexReducerOutput> {
        let ItemReduction {
            turn_id,
            item_id,
            native_item_id,
            kind,
            raw_item,
            status,
        } = reduction;
        if kind == "plan" {
            return vec![self.output(
                turn_id,
                Some(item_id),
                native,
                UniversalEventKind::PlanUpdated {
                    plan: PlanState {
                        plan_id: plan_id_for(self.session_id, item_id),
                        session_id: self.session_id,
                        turn_id,
                        status: if matches!(status, ItemStatus::Completed) {
                            PlanStatus::Draft
                        } else {
                            PlanStatus::Discovering
                        },
                        title: Some("Codex plan".to_owned()),
                        content: string_at(&raw_item, &["text"]),
                        entries: Vec::new(),
                        artifact_refs: Vec::new(),
                        source: PlanSource::NativeStructured,
                        partial: !matches!(status, ItemStatus::Completed),
                        updated_at: Some(Utc::now()),
                        handoff: None,
                    },
                },
            )];
        }
        let item = item_state(
            self.session_id,
            turn_id,
            item_id,
            &kind,
            &raw_item,
            status.clone(),
            native.clone(),
        );
        let mut outputs = vec![self.output(
            turn_id,
            Some(item_id),
            native.clone(),
            UniversalEventKind::ItemCreated {
                item: Box::new(item),
            },
        )];

        if matches!(status, ItemStatus::Completed) {
            if let Some((block_id, kind, text)) =
                completed_block_for_item(item_id, &kind, &raw_item)
            {
                outputs.push(self.output(
                    turn_id,
                    Some(item_id),
                    native.clone(),
                    UniversalEventKind::ContentCompleted {
                        block_id,
                        kind: Some(kind),
                        text: Some(text),
                    },
                ));
            }
        }

        match kind.as_str() {
            "fileChange" => outputs.push(self.output(
                turn_id,
                Some(item_id),
                native.clone(),
                UniversalEventKind::DiffUpdated {
                    diff: diff_state_for_changes(
                        self.session_id,
                        turn_id,
                        diff_id_for(self.session_id, item_id),
                        "Codex file changes",
                        raw_item.get("changes"),
                    ),
                },
            )),
            "contextCompaction" => outputs.push(self.provider_notification(ProviderNotice {
                turn_id,
                item_id: Some(item_id),
                native,
                category: "compaction".to_owned(),
                title: "Context compaction".to_owned(),
                detail: Some("Codex compacted thread context".to_owned()),
                severity: Some(ProviderNotificationSeverity::Info),
            })),
            "enteredReviewMode" | "exitedReviewMode" => {
                outputs.push(self.provider_notification(ProviderNotice {
                    turn_id,
                    item_id: Some(item_id),
                    native,
                    category: "review".to_owned(),
                    title: item_title(&kind),
                    detail: string_at(&raw_item, &["review"]),
                    severity: Some(ProviderNotificationSeverity::Info),
                }))
            }
            "dynamicToolCall" => outputs.push(self.provider_notification(ProviderNotice {
                turn_id,
                item_id: Some(item_id),
                native,
                category: "dynamic_tool".to_owned(),
                title: "Dynamic tool call".to_owned(),
                detail: Some(
                    "Dynamic client tools are not enabled in this reducer stage".to_owned(),
                ),
                severity: Some(ProviderNotificationSeverity::Warning),
            })),
            _ => {
                let _ = native_thread_id;
                let _ = native_item_id;
            }
        }
        outputs
    }

    fn content_delta(
        &mut self,
        params: &Value,
        native: NativeRef,
        block_suffix: &str,
        kind: ContentBlockKind,
        delta_field: &str,
    ) -> Vec<CodexReducerOutput> {
        let Some(native_thread_id) = string_at(params, &["threadId"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex delta missing threadId",
            )];
        };
        let Some(native_turn_id) = string_at(params, &["turnId"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex delta missing turnId",
            )];
        };
        let Some(native_item_id) = string_at(params, &["itemId"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex delta missing itemId",
            )];
        };
        let delta = string_at(params, &[delta_field]).unwrap_or_default();
        let turn_id = self.id_map.turn_id(&native_thread_id, &native_turn_id);
        let item_id = self.id_map.item_id(&native_thread_id, &native_item_id);
        vec![self.output(
            Some(turn_id),
            Some(item_id),
            native,
            UniversalEventKind::ContentDelta {
                block_id: block_id_for(item_id, block_suffix),
                kind: Some(kind),
                delta,
            },
        )]
    }

    fn realtime_transcript_delta(
        &mut self,
        params: &Value,
        native: NativeRef,
        completed: bool,
    ) -> Vec<CodexReducerOutput> {
        let native_thread_id =
            string_at(params, &["threadId"]).unwrap_or_else(|| "global".to_owned());
        let role = string_at(params, &["role"]).unwrap_or_else(|| "assistant".to_owned());
        let native_item_id = format!("realtime-transcript-{role}");
        let item_id = self.id_map.item_id(&native_thread_id, &native_item_id);
        if completed {
            vec![self.output(
                None,
                Some(item_id),
                native,
                UniversalEventKind::ContentCompleted {
                    block_id: block_id_for(item_id, "text"),
                    kind: Some(ContentBlockKind::Text),
                    text: string_at(params, &["text"]),
                },
            )]
        } else {
            vec![self.output(
                None,
                Some(item_id),
                native,
                UniversalEventKind::ContentDelta {
                    block_id: block_id_for(item_id, "text"),
                    kind: Some(ContentBlockKind::Text),
                    delta: string_at(params, &["delta"]).unwrap_or_default(),
                },
            )]
        }
    }

    fn plan_delta(&mut self, params: &Value, native: NativeRef) -> Vec<CodexReducerOutput> {
        let Some(native_thread_id) = string_at(params, &["threadId"]) else {
            return Vec::new();
        };
        let Some(native_turn_id) = string_at(params, &["turnId"]) else {
            return Vec::new();
        };
        let Some(native_item_id) = string_at(params, &["itemId"]) else {
            return Vec::new();
        };
        let turn_id = self.id_map.turn_id(&native_thread_id, &native_turn_id);
        let item_id = self.id_map.item_id(&native_thread_id, &native_item_id);
        vec![self.output(
            Some(turn_id),
            Some(item_id),
            native,
            UniversalEventKind::PlanUpdated {
                plan: PlanState {
                    plan_id: plan_id_for(self.session_id, item_id),
                    session_id: self.session_id,
                    turn_id: Some(turn_id),
                    status: PlanStatus::Discovering,
                    title: Some("Codex plan".to_owned()),
                    content: string_at(params, &["delta"]),
                    entries: Vec::new(),
                    artifact_refs: Vec::new(),
                    source: PlanSource::NativeStructured,
                    partial: true,
                    updated_at: Some(Utc::now()),
                    handoff: None,
                },
            },
        )]
    }

    fn structured_plan_updated(
        &mut self,
        params: &Value,
        native: NativeRef,
    ) -> Vec<CodexReducerOutput> {
        let Some(native_thread_id) = string_at(params, &["threadId"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex plan update missing threadId",
            )];
        };
        let Some(native_turn_id) = string_at(params, &["turnId"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex plan update missing turnId",
            )];
        };
        let turn_id = self.id_map.turn_id(&native_thread_id, &native_turn_id);
        let plan_item_id = self
            .id_map
            .item_id(&native_thread_id, &format!("{native_turn_id}:plan"));
        let entries = params
            .get("plan")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .map(|(index, step)| UniversalPlanEntry {
                entry_id: format!("codex-plan-step-{index}"),
                label: string_at(step, &["step"]).unwrap_or_else(|| step.to_string()),
                status: match string_at(step, &["status"]).as_deref() {
                    Some("completed") => agenter_core::PlanEntryStatus::Completed,
                    Some("inProgress") => agenter_core::PlanEntryStatus::InProgress,
                    _ => agenter_core::PlanEntryStatus::Pending,
                },
            })
            .collect();
        vec![self.output(
            Some(turn_id),
            Some(plan_item_id),
            native,
            UniversalEventKind::PlanUpdated {
                plan: PlanState {
                    plan_id: plan_id_for(self.session_id, plan_item_id),
                    session_id: self.session_id,
                    turn_id: Some(turn_id),
                    status: PlanStatus::Draft,
                    title: Some("Codex plan".to_owned()),
                    content: string_at(params, &["explanation"]),
                    entries,
                    artifact_refs: Vec::new(),
                    source: PlanSource::NativeStructured,
                    partial: false,
                    updated_at: Some(Utc::now()),
                    handoff: None,
                },
            },
        )]
    }

    fn patch_updated(&mut self, params: &Value, native: NativeRef) -> Vec<CodexReducerOutput> {
        let Some(native_thread_id) = string_at(params, &["threadId"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex patch update missing threadId",
            )];
        };
        let Some(native_turn_id) = string_at(params, &["turnId"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex patch update missing turnId",
            )];
        };
        let Some(native_item_id) = string_at(params, &["itemId"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex patch update missing itemId",
            )];
        };
        let turn_id = self.id_map.turn_id(&native_thread_id, &native_turn_id);
        let item_id = self.id_map.item_id(&native_thread_id, &native_item_id);
        vec![self.output(
            Some(turn_id),
            Some(item_id),
            native,
            UniversalEventKind::DiffUpdated {
                diff: diff_state_for_changes(
                    self.session_id,
                    Some(turn_id),
                    diff_id_for(self.session_id, item_id),
                    "Codex patch updated",
                    params.get("changes"),
                ),
            },
        )]
    }

    fn diff_updated(&mut self, params: &Value, native: NativeRef) -> Vec<CodexReducerOutput> {
        let Some(native_thread_id) = string_at(params, &["threadId"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex diff update missing threadId",
            )];
        };
        let Some(native_turn_id) = string_at(params, &["turnId"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex diff update missing turnId",
            )];
        };
        let turn_id = self.id_map.turn_id(&native_thread_id, &native_turn_id);
        let diff_item_id = self
            .id_map
            .item_id(&native_thread_id, &format!("{native_turn_id}:diff"));
        let diff = string_at(params, &["diff"]);
        vec![self.output(
            Some(turn_id),
            Some(diff_item_id),
            native,
            UniversalEventKind::DiffUpdated {
                diff: DiffState {
                    diff_id: diff_id_for(self.session_id, diff_item_id),
                    session_id: self.session_id,
                    turn_id: Some(turn_id),
                    title: Some("Codex turn diff".to_owned()),
                    files: vec![DiffFile {
                        path: "turn.diff".to_owned(),
                        status: FileChangeKind::Modify,
                        diff,
                    }],
                    updated_at: Some(Utc::now()),
                },
            },
        )]
    }

    fn turn_started(&mut self, params: &Value, native: NativeRef) -> Vec<CodexReducerOutput> {
        let Some(native_thread_id) = string_at(params, &["threadId"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex turn started missing threadId",
            )];
        };
        let Some(native_turn_id) = params.get("turn").and_then(|turn| string_at(turn, &["id"]))
        else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex turn started missing turn.id",
            )];
        };
        let turn_id = self.id_map.turn_id(&native_thread_id, &native_turn_id);
        vec![self.output(
            Some(turn_id),
            None,
            native,
            UniversalEventKind::TurnStarted {
                turn: self.turn_state(turn_id, TurnStatus::Running, params.get("turn")),
            },
        )]
    }

    fn turn_completed(&mut self, params: &Value, native: NativeRef) -> Vec<CodexReducerOutput> {
        let Some(native_thread_id) = string_at(params, &["threadId"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex turn completed missing threadId",
            )];
        };
        let Some(turn) = params.get("turn") else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex turn completed missing turn",
            )];
        };
        let Some(native_turn_id) = string_at(turn, &["id"]) else {
            return vec![self.native_unknown_from_params(
                params,
                native,
                "Codex turn completed missing turn.id",
            )];
        };
        let turn_id = self.id_map.turn_id(&native_thread_id, &native_turn_id);
        let status = codex_turn_status_to_universal(turn.get("status"));
        let turn_state = self.turn_state(turn_id, status.clone(), Some(turn));
        let event = match status {
            TurnStatus::Completed => UniversalEventKind::TurnCompleted { turn: turn_state },
            TurnStatus::Interrupted => UniversalEventKind::TurnInterrupted { turn: turn_state },
            TurnStatus::Failed => UniversalEventKind::TurnFailed { turn: turn_state },
            _ => UniversalEventKind::TurnStatusChanged { turn: turn_state },
        };
        vec![self.output(Some(turn_id), None, native, event)]
    }

    fn session_status_changed(
        &mut self,
        params: &Value,
        native: NativeRef,
    ) -> Vec<CodexReducerOutput> {
        vec![self.output(
            None,
            None,
            native,
            UniversalEventKind::SessionStatusChanged {
                status: codex_thread_status_to_universal(params.get("status")),
                reason: Some("Codex thread status changed".to_owned()),
            },
        )]
    }

    fn usage_updated(&mut self, params: &Value, native: NativeRef) -> Vec<CodexReducerOutput> {
        let usage = params.get("tokenUsage");
        let total_tokens = usage
            .and_then(|usage| usage.get("total"))
            .and_then(|total| total.get("totalTokens"))
            .and_then(Value::as_i64)
            .and_then(|value| u64::try_from(value).ok());
        let context_window = usage
            .and_then(|usage| usage.get("modelContextWindow"))
            .and_then(Value::as_i64)
            .and_then(|value| u64::try_from(value).ok());
        let used_percent = match (total_tokens, context_window) {
            (Some(used), Some(total)) if total > 0 => Some(used.saturating_mul(100) / total),
            _ => None,
        };
        let turn_id = self.turn_id_from_params(params);
        vec![self.output(
            turn_id,
            None,
            native,
            UniversalEventKind::UsageUpdated {
                usage: Box::new(SessionUsageSnapshot {
                    mode_label: None,
                    model: None,
                    reasoning_effort: None,
                    context: Some(SessionUsageContext {
                        used_percent,
                        used_tokens: total_tokens,
                        total_tokens: context_window,
                    }),
                    window_5h: None,
                    week: None,
                }),
            },
        )]
    }

    fn error_reported(&mut self, params: &Value, native: NativeRef) -> Vec<CodexReducerOutput> {
        let turn_id = self.turn_id_from_params(params);
        let item_id = self.item_id_from_params(params);
        vec![self.output(
            turn_id,
            item_id,
            native,
            UniversalEventKind::ErrorReported {
                code: params
                    .get("error")
                    .and_then(|error| string_at(error, &["codexErrorInfo", "code"]))
                    .or_else(|| string_at(params, &["code"])),
                message: params
                    .get("error")
                    .and_then(|error| string_at(error, &["message"]))
                    .or_else(|| string_at(params, &["message"]))
                    .unwrap_or_else(|| "Codex reported an error".to_owned()),
            },
        )]
    }

    fn generic_provider_notification(
        &mut self,
        method: &str,
        params: &Value,
        native: NativeRef,
    ) -> CodexReducerOutput {
        let (category, title, severity) = notification_category(method);
        let turn_id = self.turn_id_from_params(params);
        let item_id = self.item_id_from_params(params);
        self.provider_notification(ProviderNotice {
            turn_id,
            item_id,
            native,
            category: category.to_owned(),
            title: title.to_owned(),
            detail: notification_detail(params),
            severity,
        })
    }

    fn provider_notification(&self, notice: ProviderNotice) -> CodexReducerOutput {
        self.output(
            notice.turn_id,
            notice.item_id,
            notice.native,
            UniversalEventKind::ProviderNotification {
                notification: ProviderNotification {
                    category: notice.category,
                    title: notice.title,
                    detail: notice.detail,
                    status: None,
                    severity: notice.severity,
                    subject: notice.item_id.map(|id| id.to_string()),
                },
            },
        )
    }

    fn native_unknown_from_params(
        &mut self,
        params: &Value,
        native: NativeRef,
        summary: &str,
    ) -> CodexReducerOutput {
        let turn_id = self.turn_id_from_params(params);
        let item_id = self.item_id_from_params(params);
        self.output(
            turn_id,
            item_id,
            native,
            UniversalEventKind::NativeUnknown {
                summary: Some(summary.to_owned()),
            },
        )
    }

    fn turn_for(&mut self, params: &Value, native_turn_id: &str) -> TurnId {
        let native_thread_id =
            string_at(params, &["threadId"]).unwrap_or_else(|| "global".to_owned());
        self.id_map.turn_id(&native_thread_id, native_turn_id)
    }

    fn item_for(&mut self, params: &Value, native_item_id: &str) -> ItemId {
        let native_thread_id =
            string_at(params, &["threadId"]).unwrap_or_else(|| "global".to_owned());
        self.id_map.item_id(&native_thread_id, native_item_id)
    }

    fn turn_id_from_params(&mut self, params: &Value) -> Option<TurnId> {
        native_turn_id(params).map(|turn| self.turn_for(params, &turn))
    }

    fn item_id_from_params(&mut self, params: &Value) -> Option<ItemId> {
        native_item_id(params).map(|item| self.item_for(params, &item))
    }

    fn turn_state(&self, turn_id: TurnId, status: TurnStatus, turn: Option<&Value>) -> TurnState {
        TurnState {
            turn_id,
            session_id: self.session_id,
            status,
            started_at: turn.and_then(|turn| timestamp_seconds(turn.get("startedAt"))),
            completed_at: turn.and_then(|turn| timestamp_seconds(turn.get("completedAt"))),
            model: None,
            mode: None,
        }
    }

    fn output(
        &self,
        turn_id: Option<TurnId>,
        item_id: Option<ItemId>,
        native: NativeRef,
        event: UniversalEventKind,
    ) -> CodexReducerOutput {
        CodexReducerOutput {
            session_id: self.session_id,
            turn_id,
            item_id,
            ts: Utc::now(),
            native,
            event,
        }
    }
}

fn item_state(
    session_id: SessionId,
    turn_id: Option<TurnId>,
    item_id: ItemId,
    kind: &str,
    raw_item: &Value,
    status: ItemStatus,
    native: NativeRef,
) -> ItemState {
    let tool = item_tool(kind, raw_item, status.clone());
    ItemState {
        item_id,
        session_id,
        turn_id,
        role: item_role(kind),
        status,
        content: item_content(item_id, kind, raw_item),
        tool,
        native: Some(native),
    }
}

fn item_content(item_id: ItemId, kind: &str, raw_item: &Value) -> Vec<ContentBlock> {
    match kind {
        "userMessage" => user_message_text(raw_item)
            .map(|text| text_block(item_id, "text", ContentBlockKind::Text, text))
            .into_iter()
            .collect(),
        "agentMessage" | "plan" => string_at(raw_item, &["text"])
            .map(|text| text_block(item_id, "text", ContentBlockKind::Text, text))
            .into_iter()
            .collect(),
        "reasoning" => {
            let mut blocks = Vec::new();
            for (index, summary) in raw_item
                .get("summary")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .enumerate()
            {
                blocks.push(text_block(
                    item_id,
                    &format!("reasoning-summary-{index}"),
                    ContentBlockKind::Reasoning,
                    summary.to_owned(),
                ));
            }
            for (index, content) in raw_item
                .get("content")
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .enumerate()
            {
                blocks.push(text_block(
                    item_id,
                    &format!("reasoning-raw-{index}"),
                    ContentBlockKind::Reasoning,
                    content.to_owned(),
                ));
            }
            blocks
        }
        "commandExecution" => string_at(raw_item, &["aggregatedOutput"])
            .map(|output| text_block(item_id, "output", ContentBlockKind::CommandOutput, output))
            .into_iter()
            .collect(),
        "fileChange" => raw_item
            .get("changes")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .enumerate()
            .filter_map(|(index, change)| {
                string_at(change, &["diff"]).map(|diff| {
                    text_block(
                        item_id,
                        &format!("file-diff-{index}"),
                        ContentBlockKind::FileDiff,
                        diff,
                    )
                })
            })
            .collect(),
        "hookPrompt" => raw_item
            .get("fragments")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|fragment| string_at(fragment, &["text"]))
            .collect::<Vec<_>>()
            .join("")
            .pipe_non_empty()
            .map(|text| text_block(item_id, "text", ContentBlockKind::Native, text))
            .into_iter()
            .collect(),
        "enteredReviewMode" | "exitedReviewMode" => string_at(raw_item, &["review"])
            .map(|text| text_block(item_id, "text", ContentBlockKind::ProviderStatus, text))
            .into_iter()
            .collect(),
        "imageView" | "imageGeneration" => vec![ContentBlock {
            block_id: block_id_for(item_id, "image"),
            kind: ContentBlockKind::Image,
            text: string_at(raw_item, &["path"])
                .or_else(|| string_at(raw_item, &["savedPath"]))
                .or_else(|| string_at(raw_item, &["result"])),
            mime_type: None,
            artifact_id: None,
        }],
        _ => vec![ContentBlock {
            block_id: block_id_for(item_id, "native"),
            kind: ContentBlockKind::Native,
            text: Some(raw_item.to_string()),
            mime_type: Some("application/json".to_owned()),
            artifact_id: None,
        }],
    }
}

fn item_tool(kind: &str, raw_item: &Value, status: ItemStatus) -> Option<ToolProjection> {
    match kind {
        "commandExecution" => Some(ToolProjection {
            kind: ToolProjectionKind::Command,
            subkind: Some("command_execution".to_owned()),
            name: "command".to_owned(),
            title: string_at(raw_item, &["command"]).unwrap_or_else(|| "Command".to_owned()),
            status,
            detail: string_at(raw_item, &["status"]),
            input_summary: string_at(raw_item, &["command"]),
            output_summary: string_at(raw_item, &["aggregatedOutput"]),
            command: Some(ToolCommandProjection {
                command: string_at(raw_item, &["command"]).unwrap_or_default(),
                cwd: string_at(raw_item, &["cwd"]),
                source: string_at(raw_item, &["source"]),
                process_id: string_at(raw_item, &["processId"]),
                actions: raw_item
                    .get("commandActions")
                    .and_then(Value::as_array)
                    .into_iter()
                    .flatten()
                    .map(command_action_projection)
                    .collect(),
                exit_code: raw_item
                    .get("exitCode")
                    .and_then(Value::as_i64)
                    .and_then(|value| i32::try_from(value).ok()),
                duration_ms: raw_item
                    .get("durationMs")
                    .and_then(Value::as_i64)
                    .and_then(|value| u64::try_from(value).ok()),
                success: command_success(raw_item),
            }),
            subagent: None,
            mcp: None,
        }),
        "fileChange" => Some(ToolProjection {
            kind: ToolProjectionKind::Tool,
            subkind: Some("file_change".to_owned()),
            name: "file_change".to_owned(),
            title: "File change".to_owned(),
            status,
            detail: string_at(raw_item, &["status"]),
            input_summary: Some(format!(
                "{} changed file(s)",
                raw_item
                    .get("changes")
                    .and_then(Value::as_array)
                    .map_or(0, Vec::len)
            )),
            output_summary: None,
            command: None,
            subagent: None,
            mcp: None,
        }),
        "mcpToolCall" => Some(ToolProjection {
            kind: ToolProjectionKind::Mcp,
            subkind: None,
            name: string_at(raw_item, &["tool"]).unwrap_or_else(|| "mcp".to_owned()),
            title: format!(
                "{} {}",
                string_at(raw_item, &["server"]).unwrap_or_else(|| "MCP".to_owned()),
                string_at(raw_item, &["tool"]).unwrap_or_else(|| "tool".to_owned())
            ),
            status,
            detail: string_at(raw_item, &["status"])
                .or_else(|| string_at(raw_item, &["error", "message"])),
            input_summary: Some(
                raw_item
                    .get("arguments")
                    .cloned()
                    .unwrap_or(Value::Null)
                    .to_string(),
            ),
            output_summary: raw_item.get("result").map(Value::to_string),
            command: None,
            subagent: None,
            mcp: Some(ToolMcpProjection {
                server: string_at(raw_item, &["server"]),
                tool: string_at(raw_item, &["tool"]).unwrap_or_else(|| "tool".to_owned()),
                arguments_summary: raw_item.get("arguments").map(Value::to_string),
                result_summary: raw_item
                    .get("result")
                    .or_else(|| raw_item.get("error"))
                    .map(Value::to_string),
            }),
        }),
        "webSearch" => Some(generic_tool(
            "web_search",
            "Web search",
            status,
            string_at(raw_item, &["query"]),
        )),
        "dynamicToolCall" => Some(generic_tool(
            "dynamic_tool",
            string_at(raw_item, &["tool"]).unwrap_or_else(|| "Dynamic tool".to_owned()),
            status,
            raw_item.get("arguments").map(Value::to_string),
        )),
        "collabAgentToolCall" => Some(generic_tool(
            "collab_agent",
            string_at(raw_item, &["tool"]).unwrap_or_else(|| "Agent collaboration".to_owned()),
            status,
            string_at(raw_item, &["prompt"]),
        )),
        "imageView" => Some(generic_tool(
            "image_view",
            "Image view",
            status,
            string_at(raw_item, &["path"]),
        )),
        "imageGeneration" => Some(generic_tool(
            "image_generation",
            "Image generation",
            status,
            string_at(raw_item, &["revisedPrompt"]),
        )),
        _ => None,
    }
}

fn generic_tool(
    subkind: &str,
    title: impl Into<String>,
    status: ItemStatus,
    detail: Option<String>,
) -> ToolProjection {
    ToolProjection {
        kind: ToolProjectionKind::Tool,
        subkind: Some(subkind.to_owned()),
        name: subkind.to_owned(),
        title: title.into(),
        status,
        detail,
        input_summary: None,
        output_summary: None,
        command: None,
        subagent: None,
        mcp: None,
    }
}

fn command_action_projection(value: &Value) -> ToolActionProjection {
    ToolActionProjection {
        kind: string_at(value, &["type"]).unwrap_or_else(|| "native".to_owned()),
        label: string_at(value, &["cmd"])
            .or_else(|| string_at(value, &["command"]))
            .unwrap_or_else(|| value.to_string()),
        detail: string_at(value, &["reason"]),
        path: string_at(value, &["path"]),
    }
}

fn command_success(raw_item: &Value) -> Option<bool> {
    match string_at(raw_item, &["status"]).as_deref() {
        Some("completed") => Some(true),
        Some("failed" | "declined") => Some(false),
        _ => None,
    }
}

fn text_block(item_id: ItemId, suffix: &str, kind: ContentBlockKind, text: String) -> ContentBlock {
    ContentBlock {
        block_id: block_id_for(item_id, suffix),
        kind,
        text: Some(text),
        mime_type: None,
        artifact_id: None,
    }
}

fn completed_block_for_item(
    item_id: ItemId,
    kind: &str,
    raw_item: &Value,
) -> Option<(String, ContentBlockKind, String)> {
    match kind {
        "agentMessage" | "plan" => string_at(raw_item, &["text"])
            .map(|text| (block_id_for(item_id, "text"), ContentBlockKind::Text, text)),
        "reasoning" => raw_item
            .get("summary")
            .and_then(Value::as_array)
            .and_then(|summary| summary.first())
            .and_then(Value::as_str)
            .map(|text| {
                (
                    block_id_for(item_id, "reasoning-summary-0"),
                    ContentBlockKind::Reasoning,
                    text.to_owned(),
                )
            }),
        "commandExecution" => string_at(raw_item, &["aggregatedOutput"]).map(|text| {
            (
                block_id_for(item_id, "output"),
                ContentBlockKind::CommandOutput,
                text,
            )
        }),
        _ => None,
    }
}

fn diff_state_for_changes(
    session_id: SessionId,
    turn_id: Option<TurnId>,
    diff_id: DiffId,
    title: &str,
    changes: Option<&Value>,
) -> DiffState {
    let files = changes
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .map(|change| DiffFile {
            path: string_at(change, &["path"]).unwrap_or_else(|| "unknown".to_owned()),
            status: patch_kind_to_file_change_kind(change.get("kind")),
            diff: string_at(change, &["diff"]),
        })
        .collect();
    DiffState {
        diff_id,
        session_id,
        turn_id,
        title: Some(title.to_owned()),
        files,
        updated_at: Some(Utc::now()),
    }
}

fn patch_kind_to_file_change_kind(kind: Option<&Value>) -> FileChangeKind {
    match kind.and_then(Value::as_str) {
        Some("add") => FileChangeKind::Create,
        Some("delete") => FileChangeKind::Delete,
        _ => FileChangeKind::Modify,
    }
}

fn item_role(kind: &str) -> ItemRole {
    match kind {
        "userMessage" => ItemRole::User,
        "agentMessage" | "plan" | "reasoning" => ItemRole::Assistant,
        "hookPrompt" | "contextCompaction" | "enteredReviewMode" | "exitedReviewMode" => {
            ItemRole::System
        }
        _ => ItemRole::Tool,
    }
}

fn item_title(kind: &str) -> String {
    match kind {
        "userMessage" => "User message",
        "hookPrompt" => "Hook prompt",
        "agentMessage" => "Agent message",
        "plan" => "Plan",
        "reasoning" => "Reasoning",
        "commandExecution" => "Command",
        "fileChange" => "File change",
        "mcpToolCall" => "MCP tool",
        "dynamicToolCall" => "Dynamic tool",
        "collabAgentToolCall" => "Agent collaboration",
        "webSearch" => "Web search",
        "imageView" => "Image view",
        "imageGeneration" => "Image generation",
        "enteredReviewMode" => "Entered review mode",
        "exitedReviewMode" => "Exited review mode",
        "contextCompaction" => "Context compaction",
        _ => "Codex item",
    }
    .to_owned()
}

fn item_kind(raw_item: &Value) -> Option<&str> {
    raw_item.get("type").and_then(Value::as_str)
}

fn item_native_id(raw_item: &Value) -> Option<String> {
    string_at(raw_item, &["id"])
}

fn native_turn_id(params: &Value) -> Option<String> {
    string_at(params, &["turnId"])
        .or_else(|| params.get("turn").and_then(|turn| string_at(turn, &["id"])))
}

fn native_item_id(params: &Value) -> Option<String> {
    string_at(params, &["itemId"])
        .or_else(|| params.get("item").and_then(item_native_id))
        .or_else(|| {
            params
                .get("audio")
                .and_then(|audio| string_at(audio, &["itemId"]))
        })
}

fn string_at(value: &Value, path: &[&str]) -> Option<String> {
    let mut current = value;
    for key in path {
        current = current.get(*key)?;
    }
    match current {
        Value::String(value) => Some(value.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(value) => Some(value.to_string()),
        _ => None,
    }
}

fn user_message_text(raw_item: &Value) -> Option<String> {
    let content = raw_item.get("content")?.as_array()?;
    let text = content
        .iter()
        .filter_map(|block| match string_at(block, &["type"]).as_deref() {
            Some("text") => string_at(block, &["text"]),
            Some("image") => string_at(block, &["url"]).map(|url| format!("[image] {url}")),
            Some("localImage") => {
                string_at(block, &["path"]).map(|path| format!("[local image] {path}"))
            }
            Some("skill") => string_at(block, &["name"]).map(|name| format!("[skill] {name}")),
            Some("mention") => string_at(block, &["path"]).map(|path| format!("[mention] {path}")),
            _ => Some(block.to_string()),
        })
        .collect::<Vec<_>>()
        .join("\n");
    text.pipe_non_empty()
}

fn native_ref_for_item(
    kind: Option<&str>,
    native_id: Option<&str>,
    raw_payload: Value,
) -> NativeRef {
    NativeRef {
        protocol: CODEX_APP_SERVER_PROTOCOL.to_owned(),
        method: kind.map(|kind| format!("threadItem/{kind}")),
        kind: Some("thread_item".to_owned()),
        native_id: native_id.map(str::to_owned),
        summary: kind.map(item_title),
        hash: None,
        pointer: None,
        raw_payload: Some(raw_payload),
    }
}

fn native_ref_for_notification(frame: &CodexServerNotificationFrame) -> NativeRef {
    NativeRef {
        protocol: CODEX_APP_SERVER_PROTOCOL.to_owned(),
        method: Some(frame.method.clone()),
        kind: Some("server_notification".to_owned()),
        native_id: None,
        summary: Some(format!("Codex notification {}", frame.method)),
        hash: None,
        pointer: None,
        raw_payload: Some(frame.raw_payload.clone()),
    }
}

fn block_id_for(item_id: ItemId, suffix: &str) -> String {
    format!("{item_id}:{suffix}")
}

fn plan_id_for(session_id: SessionId, item_id: ItemId) -> PlanId {
    PlanId::from_uuid(stable_uuid("plan", session_id, item_id))
}

fn diff_id_for(session_id: SessionId, item_id: ItemId) -> DiffId {
    DiffId::from_uuid(stable_uuid("diff", session_id, item_id))
}

fn stable_uuid(kind: &str, session_id: SessionId, item_id: ItemId) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("agenter:codex:{kind}:{session_id}:{item_id}").as_bytes(),
    )
}

fn synthetic_item_key(raw_item: &Value) -> String {
    Uuid::new_v5(&Uuid::NAMESPACE_OID, raw_item.to_string().as_bytes()).to_string()
}

fn timestamp_seconds(value: Option<&Value>) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(value?.as_i64()?, 0)
}

fn codex_turn_status_to_universal(status: Option<&Value>) -> TurnStatus {
    match status.and_then(Value::as_str) {
        Some("completed") => TurnStatus::Completed,
        Some("interrupted") => TurnStatus::Interrupted,
        Some("failed") => TurnStatus::Failed,
        Some("inProgress") => TurnStatus::Running,
        _ => TurnStatus::Running,
    }
}

fn codex_thread_status_to_universal(status: Option<&Value>) -> SessionStatus {
    match status
        .and_then(|status| status.get("type"))
        .and_then(Value::as_str)
    {
        Some("idle") => SessionStatus::Idle,
        Some("notLoaded") => SessionStatus::Stopped,
        Some("systemError") => SessionStatus::Failed,
        Some("active") => {
            let flags = status
                .and_then(|status| status.get("activeFlags"))
                .and_then(Value::as_array)
                .into_iter()
                .flatten()
                .filter_map(Value::as_str)
                .collect::<Vec<_>>();
            if flags.contains(&"waitingOnApproval") {
                SessionStatus::WaitingForApproval
            } else if flags.contains(&"waitingOnUserInput") {
                SessionStatus::WaitingForInput
            } else {
                SessionStatus::Running
            }
        }
        _ => SessionStatus::Degraded,
    }
}

fn notification_detail(params: &Value) -> Option<String> {
    string_at(params, &["message"])
        .or_else(|| string_at(params, &["summary"]))
        .or_else(|| string_at(params, &["details"]))
        .or_else(|| string_at(params, &["error"]))
        .or_else(|| string_at(params, &["reason"]))
        .or_else(|| params.as_object().map(|_| params.to_string()))
}

fn notification_category(
    method: &str,
) -> (
    &'static str,
    &'static str,
    Option<ProviderNotificationSeverity>,
) {
    match method {
        "skills/changed" => (
            "skills",
            "Skills changed",
            Some(ProviderNotificationSeverity::Info),
        ),
        "thread/started" => (
            "thread",
            "Thread started",
            Some(ProviderNotificationSeverity::Info),
        ),
        "thread/goal/updated" | "thread/goal/cleared" => (
            "thread_goal",
            "Thread goal updated",
            Some(ProviderNotificationSeverity::Info),
        ),
        "hook/started" | "hook/completed" => (
            "hook",
            "Hook updated",
            Some(ProviderNotificationSeverity::Info),
        ),
        "item/autoApprovalReview/started" | "item/autoApprovalReview/completed" => (
            "approval_review",
            "Approval review",
            Some(ProviderNotificationSeverity::Info),
        ),
        "item/reasoning/summaryPartAdded" => (
            "reasoning",
            "Reasoning summary part",
            Some(ProviderNotificationSeverity::Debug),
        ),
        "mcpServer/oauthLogin/completed" => (
            "mcp_oauth",
            "MCP OAuth completed",
            Some(ProviderNotificationSeverity::Info),
        ),
        "mcpServer/startupStatus/updated" => (
            "mcp_status",
            "MCP server status",
            Some(ProviderNotificationSeverity::Info),
        ),
        "account/updated" | "account/login/completed" => (
            "account",
            "Account updated",
            Some(ProviderNotificationSeverity::Info),
        ),
        "account/rateLimits/updated" => (
            "account",
            "Rate limits updated",
            Some(ProviderNotificationSeverity::Info),
        ),
        "app/list/updated" => (
            "apps",
            "App list updated",
            Some(ProviderNotificationSeverity::Info),
        ),
        "remoteControl/status/changed" => (
            "remote_control",
            "Remote control status",
            Some(ProviderNotificationSeverity::Info),
        ),
        "externalAgentConfig/import/completed" => (
            "external_agent_config",
            "External agent import completed",
            Some(ProviderNotificationSeverity::Info),
        ),
        "fs/changed" => (
            "fs_watch",
            "Filesystem changed",
            Some(ProviderNotificationSeverity::Info),
        ),
        "thread/compacted" => (
            "compaction",
            "Thread compacted",
            Some(ProviderNotificationSeverity::Info),
        ),
        "model/rerouted" | "model/verification" => (
            "model",
            "Model update",
            Some(ProviderNotificationSeverity::Info),
        ),
        "warning"
        | "guardianWarning"
        | "deprecationNotice"
        | "configWarning"
        | "windows/worldWritableWarning" => (
            "warning",
            "Codex warning",
            Some(ProviderNotificationSeverity::Warning),
        ),
        "fuzzyFileSearch/sessionUpdated" | "fuzzyFileSearch/sessionCompleted" => (
            "fuzzy_file_search",
            "Fuzzy file search",
            Some(ProviderNotificationSeverity::Info),
        ),
        "thread/realtime/started" | "thread/realtime/sdp" | "thread/realtime/closed" => (
            "realtime",
            "Realtime status",
            Some(ProviderNotificationSeverity::Info),
        ),
        "windowsSandbox/setupCompleted" => (
            "windows_sandbox",
            "Windows sandbox setup",
            Some(ProviderNotificationSeverity::Info),
        ),
        _ => (
            "codex",
            "Codex notification",
            Some(ProviderNotificationSeverity::Info),
        ),
    }
}

fn is_known_reduced_notification(method: &str) -> bool {
    SERVER_NOTIFICATION_MAPPINGS
        .iter()
        .any(|mapping| mapping.codex == method)
}

trait NonEmptyString {
    fn pipe_non_empty(self) -> Option<String>;
}

impl NonEmptyString for String {
    fn pipe_non_empty(self) -> Option<String> {
        if self.trim().is_empty() {
            None
        } else {
            Some(self)
        }
    }
}

#[cfg(test)]
mod test_support {
    use std::{collections::BTreeSet, fs, path::Path};

    use serde_json::{json, Value};

    use super::{CodexReducerMapping, SERVER_NOTIFICATION_MAPPINGS, THREAD_ITEM_MAPPINGS};

    pub fn mapping_names(mappings: &[CodexReducerMapping]) -> Vec<String> {
        mappings
            .iter()
            .map(|mapping| mapping.codex.to_owned())
            .collect()
    }

    pub fn current_thread_item_variants() -> Vec<String> {
        extract_enum_variants(&read_v2(), "ThreadItem")
            .into_iter()
            .collect()
    }

    pub fn current_server_notification_methods() -> Vec<String> {
        extract_macro_methods(&read_common(), "server_notification_definitions")
            .into_iter()
            .collect()
    }

    pub fn assert_no_empty_mapping_labels() {
        assert!(THREAD_ITEM_MAPPINGS
            .iter()
            .all(|mapping| !mapping.mapping.trim().is_empty()));
        assert!(SERVER_NOTIFICATION_MAPPINGS
            .iter()
            .all(|mapping| !mapping.mapping.trim().is_empty()));
    }

    pub fn sample_thread_item(variant: &str) -> Value {
        match variant {
            "AgentMessage" => {
                json!({ "type": "agentMessage", "id": "agent-message", "text": "hello" })
            }
            "CollabAgentToolCall" => json!({
                "type": "collabAgentToolCall", "id": "collab", "tool": "spawnAgent",
                "status": "completed", "senderThreadId": "thread-1", "receiverThreadIds": ["child-1"],
                "prompt": "do work", "model": "gpt-5", "reasoningEffort": "medium", "agentsStates": {}
            }),
            "CommandExecution" => json!({
                "type": "commandExecution", "id": "cmd", "command": "echo hi", "cwd": "/tmp",
                "processId": "proc-1", "source": "agent", "status": "completed",
                "commandActions": [{ "type": "exec", "cmd": "echo" }],
                "aggregatedOutput": "hi\n", "exitCode": 0, "durationMs": 7
            }),
            "ContextCompaction" => json!({ "type": "contextCompaction", "id": "compact" }),
            "DynamicToolCall" => json!({
                "type": "dynamicToolCall", "id": "dynamic", "namespace": "client",
                "tool": "open", "arguments": {}, "status": "failed",
                "contentItems": null, "success": false, "durationMs": 3
            }),
            "EnteredReviewMode" => {
                json!({ "type": "enteredReviewMode", "id": "review-in", "review": "review started" })
            }
            "ExitedReviewMode" => {
                json!({ "type": "exitedReviewMode", "id": "review-out", "review": "review finished" })
            }
            "FileChange" => json!({
                "type": "fileChange", "id": "file", "status": "completed",
                "changes": [{ "path": "src/lib.rs", "kind": "update", "diff": "@@ diff" }]
            }),
            "HookPrompt" => json!({
                "type": "hookPrompt", "id": "hook",
                "fragments": [{ "text": "hook text", "hookRunId": "run-1" }]
            }),
            "ImageGeneration" => json!({
                "type": "imageGeneration", "id": "image-gen", "status": "completed",
                "revisedPrompt": "draw", "result": "ok", "savedPath": "/tmp/image.png"
            }),
            "ImageView" => {
                json!({ "type": "imageView", "id": "image-view", "path": "/tmp/image.png" })
            }
            "McpToolCall" => json!({
                "type": "mcpToolCall", "id": "mcp", "server": "server", "tool": "tool",
                "status": "completed", "arguments": { "x": 1 },
                "mcpAppResourceUri": null,
                "result": { "content": [{ "type": "text", "text": "ok" }], "structuredContent": null, "_meta": null },
                "error": null, "durationMs": 9
            }),
            "Plan" => json!({ "type": "plan", "id": "plan", "text": "1. test\n2. ship" }),
            "Reasoning" => {
                json!({ "type": "reasoning", "id": "reasoning", "summary": ["thinking"], "content": ["raw chain"] })
            }
            "UserMessage" => {
                json!({ "type": "userMessage", "id": "user", "content": [{ "type": "text", "text": "hello" }] })
            }
            "WebSearch" => {
                json!({ "type": "webSearch", "id": "web", "query": "codex", "action": { "type": "search" } })
            }
            other => panic!("missing sample for ThreadItem {other}"),
        }
    }

    pub fn sample_server_notification(method: &str) -> Value {
        json!({
            "method": method,
            "params": sample_server_notification_params(method)
        })
    }

    fn sample_server_notification_params(method: &str) -> Value {
        match method {
            "error" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "willRetry": false, "error": { "message": "bad", "codexErrorInfo": { "code": "bad" } } })
            }
            "thread/status/changed" => {
                json!({ "threadId": "thread-1", "status": { "type": "active", "activeFlags": [] } })
            }
            "thread/archived"
            | "thread/unarchived"
            | "thread/closed"
            | "thread/goal/cleared"
            | "thread/compacted"
            | "thread/realtime/closed" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "reason": "done" })
            }
            "thread/name/updated" => {
                json!({ "threadId": "thread-1", "threadName": "Readable title" })
            }
            "thread/tokenUsage/updated" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "tokenUsage": { "total": { "totalTokens": 20, "inputTokens": 10, "cachedInputTokens": 0, "outputTokens": 10, "reasoningOutputTokens": 2 }, "last": { "totalTokens": 3, "inputTokens": 1, "cachedInputTokens": 0, "outputTokens": 2, "reasoningOutputTokens": 0 }, "modelContextWindow": 100 } })
            }
            "turn/started" => {
                json!({ "threadId": "thread-1", "turn": { "id": "turn-1", "items": [], "status": "inProgress", "error": null, "startedAt": 1700000000, "completedAt": null, "durationMs": null } })
            }
            "turn/completed" => {
                json!({ "threadId": "thread-1", "turn": { "id": "turn-1", "items": [], "status": "completed", "error": null, "startedAt": 1700000000, "completedAt": 1700000001, "durationMs": 1000 } })
            }
            "turn/diff/updated" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "diff": "@@ diff" })
            }
            "turn/plan/updated" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "explanation": "plan", "plan": [{ "step": "test", "status": "inProgress" }] })
            }
            "item/started" | "item/completed" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "item": sample_thread_item("AgentMessage") })
            }
            "item/agentMessage/delta" | "item/plan/delta" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-1", "delta": "delta" })
            }
            "item/reasoning/summaryTextDelta" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-1", "summaryIndex": 0, "delta": "summary" })
            }
            "item/reasoning/summaryPartAdded" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-1", "summaryIndex": 1 })
            }
            "item/reasoning/textDelta" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-1", "contentIndex": 0, "delta": "raw" })
            }
            "item/commandExecution/outputDelta" | "item/fileChange/outputDelta" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-1", "delta": "out" })
            }
            "item/commandExecution/terminalInteraction" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-1", "processId": "proc-1", "stdin": "q" })
            }
            "item/fileChange/patchUpdated" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-1", "changes": [{ "path": "src/lib.rs", "kind": "update", "diff": "@@" }] })
            }
            "item/mcpToolCall/progress" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "itemId": "item-1", "message": "running" })
            }
            "serverRequest/resolved" => json!({ "threadId": "thread-1", "requestId": "request-1" }),
            "rawResponseItem/completed" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "item": { "type": "message", "id": "raw" } })
            }
            "command/exec/outputDelta" => {
                json!({ "processId": "proc-1", "stream": "stdout", "deltaBase64": "aGk=", "capReached": false })
            }
            "thread/realtime/transcript/delta" => {
                json!({ "threadId": "thread-1", "role": "assistant", "delta": "hi" })
            }
            "thread/realtime/transcript/done" => {
                json!({ "threadId": "thread-1", "role": "assistant", "text": "hi" })
            }
            "thread/realtime/itemAdded" => {
                json!({ "threadId": "thread-1", "item": { "type": "raw" } })
            }
            "thread/realtime/outputAudio/delta" => {
                json!({ "threadId": "thread-1", "audio": { "data": "abc", "sampleRate": 24000, "numChannels": 1, "samplesPerChannel": 1, "itemId": "audio-1" } })
            }
            "thread/realtime/error" => json!({ "threadId": "thread-1", "message": "audio failed" }),
            "thread/realtime/started" => {
                json!({ "threadId": "thread-1", "sessionId": "rt-1", "version": "v1" })
            }
            "thread/realtime/sdp" => json!({ "threadId": "thread-1", "sdp": "answer" }),
            "model/rerouted" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "fromModel": "a", "toModel": "b", "reason": "rateLimit" })
            }
            "model/verification" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "verifications": [] })
            }
            "warning" => json!({ "threadId": "thread-1", "message": "warn" }),
            "guardianWarning" => json!({ "threadId": "thread-1", "message": "guardian" }),
            "deprecationNotice" => json!({ "summary": "deprecated", "details": "later" }),
            "configWarning" => {
                json!({ "summary": "bad config", "details": "fix", "path": "/tmp/config.toml" })
            }
            "mcpServer/oauthLogin/completed" => {
                json!({ "name": "server", "success": true, "error": null })
            }
            "mcpServer/startupStatus/updated" => {
                json!({ "name": "server", "status": "ready", "error": null })
            }
            "account/rateLimits/updated" => {
                json!({ "rateLimits": { "limitId": "id", "limitName": "name", "primary": { "usedPercent": 10, "windowDurationMins": 300, "resetsAt": 1700000000 }, "secondary": null, "credits": null, "planType": null, "rateLimitReachedType": null } })
            }
            "windows/worldWritableWarning" => {
                json!({ "samplePaths": ["C:/tmp"], "extraCount": 0, "failedScan": false })
            }
            "windowsSandbox/setupCompleted" => {
                json!({ "mode": "elevated", "success": true, "error": null })
            }
            "app/list/updated" => json!({ "apps": [] }),
            "remoteControl/status/changed" => json!({ "status": "connected" }),
            "externalAgentConfig/import/completed" => json!({}),
            "fs/changed" => json!({ "watchId": "watch-1", "changes": [] }),
            "fuzzyFileSearch/sessionUpdated" => json!({ "sessionId": "search-1", "files": [] }),
            "fuzzyFileSearch/sessionCompleted" => json!({ "sessionId": "search-1" }),
            "skills/changed" => json!({}),
            "thread/started" => {
                json!({ "thread": { "id": "thread-1", "preview": "hello", "ephemeral": false, "modelProvider": "openai", "createdAt": 1700000000, "updatedAt": 1700000000, "status": { "type": "idle" }, "path": null, "cwd": "/tmp", "cliVersion": "test", "source": { "type": "appServer" }, "agentNickname": null, "agentRole": null, "gitInfo": null, "name": "Thread", "turns": [] } })
            }
            "thread/goal/updated" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "goal": { "objective": "ship" } })
            }
            "hook/started" | "hook/completed" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "run": { "id": "hook-1", "status": "completed" } })
            }
            "item/autoApprovalReview/started" | "item/autoApprovalReview/completed" => {
                json!({ "threadId": "thread-1", "turnId": "turn-1", "reviewId": "review-1", "targetItemId": "item-1", "review": { "status": "approved", "riskLevel": "low", "userAuthorization": "medium", "rationale": "ok" }, "action": { "type": "command", "command": "echo hi", "cwd": "/tmp", "source": "shell" } })
            }
            "account/updated" => json!({ "authMode": "chatgpt", "planType": null }),
            "account/login/completed" => {
                json!({ "loginId": "login-1", "success": true, "error": null })
            }
            other => panic!("missing sample for ServerNotification {other}"),
        }
    }

    fn read_common() -> String {
        read_source("tmp/codex/codex-rs/app-server-protocol/src/protocol/common.rs")
    }

    fn read_v2() -> String {
        read_source("tmp/codex/codex-rs/app-server-protocol/src/protocol/v2.rs")
    }

    fn read_source(relative: &str) -> String {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"))
            .ancestors()
            .nth(2)
            .expect("runner crate should live under repo root");
        let path = root.join(relative);
        fs::read_to_string(&path)
            .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
    }

    fn extract_macro_methods(source: &str, macro_name: &str) -> BTreeSet<String> {
        split_top_level_entries(strip_line_comments(extract_macro_body(source, macro_name)))
            .into_iter()
            .filter(|entry| !entry.trim().is_empty())
            .map(|entry| method_name_for_entry(&entry))
            .collect()
    }

    fn extract_macro_body<'a>(source: &'a str, macro_name: &str) -> &'a str {
        let marker = format!("{macro_name}! {{");
        let start = source
            .find(&marker)
            .unwrap_or_else(|| panic!("missing macro invocation {macro_name}!"));
        let body_start = start + marker.len();
        let body_end = matching_delimiter(source, body_start - 1, '{', '}')
            .unwrap_or_else(|| panic!("unterminated macro invocation {macro_name}!"));
        &source[body_start..body_end]
    }

    fn extract_enum_variants(source: &str, enum_name: &str) -> BTreeSet<String> {
        split_top_level_entries(strip_line_comments(extract_enum_body(source, enum_name)))
            .into_iter()
            .filter(|entry| !entry.trim().is_empty())
            .map(|entry| first_identifier(&entry))
            .collect()
    }

    fn extract_enum_body<'a>(source: &'a str, enum_name: &str) -> &'a str {
        let marker = format!("pub enum {enum_name}");
        let start = source
            .find(&marker)
            .unwrap_or_else(|| panic!("missing enum {enum_name}"));
        let open = source[start..]
            .find('{')
            .map(|offset| start + offset)
            .unwrap_or_else(|| panic!("missing enum body for {enum_name}"));
        let close = matching_delimiter(source, open, '{', '}')
            .unwrap_or_else(|| panic!("unterminated enum {enum_name}"));
        &source[open + 1..close]
    }

    fn split_top_level_entries(source: String) -> Vec<String> {
        let mut entries = Vec::new();
        let mut start = 0;
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escape = false;
        for (index, ch) in source.char_indices() {
            if in_string {
                if escape {
                    escape = false;
                } else if ch == '\\' {
                    escape = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            match ch {
                '"' => in_string = true,
                '(' | '{' | '[' => depth += 1,
                ')' | '}' | ']' => depth -= 1,
                ',' if depth == 0 => {
                    entries.push(source[start..index].trim().to_owned());
                    start = index + 1;
                }
                _ => {}
            }
        }
        if start < source.len() {
            entries.push(source[start..].trim().to_owned());
        }
        entries
    }

    fn matching_delimiter(
        source: &str,
        open_index: usize,
        open: char,
        close: char,
    ) -> Option<usize> {
        let mut depth = 0i32;
        let mut in_string = false;
        let mut escape = false;
        for (index, ch) in source
            .char_indices()
            .filter(|(index, _)| *index >= open_index)
        {
            if in_string {
                if escape {
                    escape = false;
                } else if ch == '\\' {
                    escape = true;
                } else if ch == '"' {
                    in_string = false;
                }
                continue;
            }
            if ch == '"' {
                in_string = true;
            } else if ch == open {
                depth += 1;
            } else if ch == close {
                depth -= 1;
                if depth == 0 {
                    return Some(index);
                }
            }
        }
        None
    }

    fn method_name_for_entry(entry: &str) -> String {
        entry
            .split('"')
            .nth(1)
            .unwrap_or_else(|| panic!("missing method string in macro entry: {entry}"))
            .to_owned()
    }

    fn first_identifier(entry: &str) -> String {
        let mut owned;
        let mut entry = entry.trim_start();
        loop {
            if let Some(rest) = entry.strip_prefix("#[") {
                let end = rest
                    .find(']')
                    .unwrap_or_else(|| panic!("unterminated attribute in enum entry: {entry}"));
                entry = rest[end + 1..].trim_start();
                continue;
            }
            if let Some(rest) = entry.strip_prefix("///") {
                owned = rest.lines().skip(1).collect::<Vec<_>>().join("\n");
                entry = owned.trim_start();
                continue;
            }
            break;
        }
        entry
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .collect()
    }

    fn strip_line_comments(source: &str) -> String {
        source
            .lines()
            .map(|line| line.split("//").next().unwrap_or_default())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

#[cfg(test)]
mod tests {
    use agenter_core::{ContentBlockKind, ItemRole, SessionId, UniversalEventKind};
    use serde_json::json;

    use crate::agents::codex::{
        codec::{CodexCodec, CodexDecodedFrame, CodexServerNotificationFrame},
        id_map::CodexIdMap,
    };

    #[test]
    fn codex_reducer_history_thread_item_mappings_cover_current_codex_source() {
        assert_eq!(
            super::test_support::current_thread_item_variants(),
            super::test_support::mapping_names(super::THREAD_ITEM_MAPPINGS)
        );
        super::test_support::assert_no_empty_mapping_labels();
    }

    #[test]
    fn codex_reducer_server_notification_mappings_cover_current_codex_source() {
        assert_eq!(
            super::test_support::current_server_notification_methods(),
            super::test_support::mapping_names(super::SERVER_NOTIFICATION_MAPPINGS)
        );
        super::test_support::assert_no_empty_mapping_labels();
    }

    #[test]
    fn codex_reducer_history_items_preserve_decoded_raw_payloads_for_every_current_variant() {
        let session_id = SessionId::new();
        let mut reducer = super::CodexReducer::new(session_id, CodexIdMap::for_session(session_id));

        for mapping in super::THREAD_ITEM_MAPPINGS {
            let raw_item = super::test_support::sample_thread_item(mapping.codex);
            let outputs = reducer.reduce_history_item("thread-1", "turn-1", raw_item.clone());

            assert!(
                !outputs.is_empty(),
                "{} produced no reducer output",
                mapping.codex
            );
            assert!(
                outputs
                    .iter()
                    .all(|output| output.native.raw_payload.as_ref() == Some(&raw_item)),
                "{} did not preserve decoded raw payload on every output",
                mapping.codex
            );
            assert!(
                outputs.iter().any(|output| matches!(
                    output.event,
                    UniversalEventKind::ItemCreated { .. }
                        | UniversalEventKind::PlanUpdated { .. }
                        | UniversalEventKind::ProviderNotification { .. }
                        | UniversalEventKind::NativeUnknown { .. }
                )),
                "{} did not produce a visible row or notification",
                mapping.codex
            );
        }
    }

    #[test]
    fn codex_reducer_server_notifications_preserve_raw_payloads_for_every_current_method() {
        let session_id = SessionId::new();
        let mut reducer = super::CodexReducer::new(session_id, CodexIdMap::for_session(session_id));

        for mapping in super::SERVER_NOTIFICATION_MAPPINGS {
            let raw = super::test_support::sample_server_notification(mapping.codex);
            let frame = decode_notification(raw.clone());
            let outputs = reducer.reduce_server_notification(&frame);

            assert!(
                !outputs.is_empty(),
                "{} produced no reducer output",
                mapping.codex
            );
            assert!(
                outputs
                    .iter()
                    .all(|output| output.native.raw_payload.as_ref() == Some(&raw)),
                "{} did not preserve notification raw payload on every output",
                mapping.codex
            );
        }
    }

    #[test]
    fn codex_reducer_undecoded_notifications_remain_visible_with_full_raw_payload() {
        let session_id = SessionId::new();
        let mut reducer = super::CodexReducer::new(session_id, CodexIdMap::for_session(session_id));
        let raw = json!({
            "method": "future/native",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-1",
                "opaque": { "answer": 42 }
            }
        });
        let frame = decode_notification(raw.clone());

        let outputs = reducer.reduce_server_notification(&frame);

        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0].native.method.as_deref(), Some("future/native"));
        assert_eq!(outputs[0].native.raw_payload.as_ref(), Some(&raw));
        assert!(matches!(
            outputs[0].event,
            UniversalEventKind::NativeUnknown { .. }
        ));
    }

    #[test]
    fn codex_reducer_deltas_and_terminal_completion_converge_on_same_item_and_block_ids() {
        let session_id = SessionId::new();
        let mut reducer = super::CodexReducer::new(session_id, CodexIdMap::for_session(session_id));
        let delta = json!({
            "method": "item/agentMessage/delta",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-1",
                "delta": "hel"
            }
        });
        let completed = json!({
            "method": "item/completed",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "item": {
                    "type": "agentMessage",
                    "id": "item-1",
                    "text": "hello"
                }
            }
        });

        let delta_outputs = reducer.reduce_server_notification(&decode_notification(delta));
        let completed_outputs = reducer.reduce_server_notification(&decode_notification(completed));

        let delta_output = delta_outputs
            .iter()
            .find(|output| matches!(output.event, UniversalEventKind::ContentDelta { .. }))
            .expect("delta output");
        let completed_output = completed_outputs
            .iter()
            .find(|output| matches!(output.event, UniversalEventKind::ContentCompleted { .. }))
            .expect("completed output");
        let created_output = completed_outputs
            .iter()
            .find(|output| matches!(output.event, UniversalEventKind::ItemCreated { .. }))
            .expect("item output");

        assert_eq!(delta_output.item_id, completed_output.item_id);
        assert_eq!(delta_output.item_id, created_output.item_id);
        assert_eq!(
            delta_output.turn_id, completed_output.turn_id,
            "turn mapping should also converge"
        );

        let UniversalEventKind::ContentDelta {
            block_id,
            kind,
            delta,
        } = &delta_output.event
        else {
            panic!("expected delta");
        };
        assert_eq!(kind, &Some(ContentBlockKind::Text));
        assert_eq!(delta, "hel");

        let UniversalEventKind::ContentCompleted {
            block_id: completed_block_id,
            kind,
            text,
        } = &completed_output.event
        else {
            panic!("expected completed");
        };
        assert_eq!(block_id, completed_block_id);
        assert_eq!(kind, &Some(ContentBlockKind::Text));
        assert_eq!(text.as_deref(), Some("hello"));
    }

    #[test]
    fn codex_reducer_native_user_message_still_emits_user_item() {
        let session_id = SessionId::new();
        let mut reducer = super::CodexReducer::new(session_id, CodexIdMap::for_session(session_id));
        let completed = json!({
            "method": "item/completed",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "item": {
                    "type": "userMessage",
                    "id": "user-1",
                    "content": [{ "type": "text", "text": "Implement the plan." }]
                }
            }
        });

        let outputs = reducer.reduce_server_notification(&decode_notification(completed));
        let created = outputs
            .iter()
            .find_map(|output| match &output.event {
                UniversalEventKind::ItemCreated { item } => Some(item),
                _ => None,
            })
            .expect("user item output");

        assert_eq!(created.role, ItemRole::User);
        assert_eq!(
            created.content[0].text.as_deref(),
            Some("Implement the plan.")
        );
    }

    #[test]
    fn codex_reducer_plan_delta_emits_only_plan_update() {
        let session_id = SessionId::new();
        let mut reducer = super::CodexReducer::new(session_id, CodexIdMap::for_session(session_id));
        let delta = json!({
            "method": "item/plan/delta",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "plan-1",
                "delta": "# Plan"
            }
        });

        let outputs = reducer.reduce_server_notification(&decode_notification(delta));

        assert_eq!(outputs.len(), 1);
        assert!(matches!(
            outputs[0].event,
            UniversalEventKind::PlanUpdated { .. }
        ));
    }

    #[test]
    fn codex_reducer_completed_plan_item_emits_only_plan_update() {
        let session_id = SessionId::new();
        let mut reducer = super::CodexReducer::new(session_id, CodexIdMap::for_session(session_id));
        let completed = json!({
            "method": "item/completed",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "item": {
                    "type": "plan",
                    "id": "plan-1",
                    "text": "# Plan"
                }
            }
        });

        let outputs = reducer.reduce_server_notification(&decode_notification(completed));

        assert_eq!(outputs.len(), 1);
        let UniversalEventKind::PlanUpdated { plan } = &outputs[0].event else {
            panic!("expected plan update");
        };
        assert_eq!(plan.content.as_deref(), Some("# Plan"));
        assert!(!plan.partial);
    }

    #[test]
    fn codex_reducer_history_plan_item_emits_only_plan_update() {
        let session_id = SessionId::new();
        let mut reducer = super::CodexReducer::new(session_id, CodexIdMap::for_session(session_id));
        let raw_item = json!({
            "type": "plan",
            "id": "plan-1",
            "text": "# Plan"
        });

        let outputs = reducer.reduce_history_item("thread-1", "turn-1", raw_item);

        assert_eq!(outputs.len(), 1);
        assert!(matches!(
            outputs[0].event,
            UniversalEventKind::PlanUpdated { .. }
        ));
    }

    fn decode_notification(raw: serde_json::Value) -> CodexServerNotificationFrame {
        let mut codec = CodexCodec::new();
        let CodexDecodedFrame::ServerNotification(frame) = codec.decode_line(&raw.to_string())
        else {
            panic!("expected server notification frame");
        };
        frame
    }
}
