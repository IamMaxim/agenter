#![allow(dead_code)]

use agenter_core::{
    AgentQuestionAnswer, AgentQuestionChoice, AgentQuestionField, ApprovalDecision, ApprovalId,
    ApprovalKind, ApprovalOption, ApprovalOptionKind, ApprovalRequest, ApprovalStatus, ItemId,
    NativeRef, ProviderNotification, ProviderNotificationSeverity, QuestionId, QuestionState,
    QuestionStatus, SessionId, TurnId, UniversalEventKind,
};
use serde_json::{json, Map, Value};
use uuid::Uuid;

use super::{
    codec::{CodexServerRequestFrame, RequestId, CODEX_APP_SERVER_PROTOCOL},
    id_map::CodexIdMap,
};

#[derive(Debug, Clone, PartialEq)]
pub enum CodexServerRequestOutput {
    ApprovalRequested(ApprovalRequest),
    QuestionRequested(QuestionState),
    NativeUnsupported {
        title: String,
        detail: String,
        native: NativeRef,
        response: Value,
    },
}

#[derive(Debug, Clone)]
pub struct CodexObligationMapper {
    session_id: SessionId,
    id_map: CodexIdMap,
}

impl CodexObligationMapper {
    #[must_use]
    pub fn new(session_id: SessionId) -> Self {
        Self {
            session_id,
            id_map: CodexIdMap::for_session(session_id),
        }
    }

    #[must_use]
    pub fn map_server_request(
        &mut self,
        request: &CodexServerRequestFrame,
    ) -> CodexServerRequestOutput {
        let params = params_object(&request.raw_payload);
        match request.method.as_str() {
            "item/commandExecution/requestApproval" => {
                CodexServerRequestOutput::ApprovalRequested(self.command_approval(request, params))
            }
            "item/fileChange/requestApproval" => CodexServerRequestOutput::ApprovalRequested(
                self.file_change_approval(request, params),
            ),
            "item/permissions/requestApproval" => CodexServerRequestOutput::ApprovalRequested(
                self.permission_approval(request, params),
            ),
            "ApplyPatchApproval" => CodexServerRequestOutput::ApprovalRequested(
                self.legacy_file_change_approval(request, params),
            ),
            "ExecCommandApproval" => CodexServerRequestOutput::ApprovalRequested(
                self.legacy_command_approval(request, params),
            ),
            "item/tool/requestUserInput" => CodexServerRequestOutput::QuestionRequested(
                self.tool_user_input_question(request, params),
            ),
            "mcpServer/elicitation/request" => CodexServerRequestOutput::QuestionRequested(
                self.mcp_elicitation_question(request, params),
            ),
            "account/chatgptAuthTokens/refresh" => self.unsupported(
                request,
                "Codex account token refresh required",
                "Agenter does not manage Codex ChatGPT auth tokens yet.",
            ),
            "item/tool/call" => self.unsupported(
                request,
                "Codex dynamic tool call is unsupported",
                "Agenter has no client-tool contract for Codex dynamic tool calls yet.",
            ),
            _ => self.unsupported(
                request,
                "Unsupported Codex server request",
                "Agenter has no mapper for this Codex app-server request.",
            ),
        }
    }

    fn command_approval(
        &mut self,
        request: &CodexServerRequestFrame,
        params: &Map<String, Value>,
    ) -> ApprovalRequest {
        let turn_id = turn_id(&mut self.id_map, params);
        let item_id = item_id(&mut self.id_map, params);
        let approval_key = string_param(params, "approvalId")
            .or_else(|| string_param(params, "itemId"))
            .unwrap_or_else(|| request.request_id.to_string());
        let command = string_param(params, "command");
        let cwd = string_param(params, "cwd");
        let mut details = Vec::new();
        push_labeled(&mut details, "Command", command.as_deref());
        push_labeled(&mut details, "Cwd", cwd.as_deref());
        push_labeled(
            &mut details,
            "Reason",
            string_param(params, "reason").as_deref(),
        );
        push_json(
            &mut details,
            "Additional permissions",
            params.get("additionalPermissions"),
        );
        push_json(
            &mut details,
            "Proposed exec policy amendment",
            params.get("proposedExecpolicyAmendment"),
        );
        push_json(
            &mut details,
            "Proposed network policy amendments",
            params.get("proposedNetworkPolicyAmendments"),
        );

        ApprovalRequest {
            approval_id: stable_approval_id(self.session_id, "codex-command", &approval_key),
            session_id: self.session_id,
            turn_id,
            item_id,
            kind: ApprovalKind::Command,
            title: command.unwrap_or_else(|| "Approve Codex command".to_owned()),
            details: join_details(details),
            options: command_options(params),
            status: ApprovalStatus::Pending,
            risk: None,
            subject: cwd,
            native_request_id: Some(request.request_id.to_string()),
            native_blocking: native_blocking(&request.raw_payload),
            policy: None,
            native: Some(native_ref_for_server_request(request)),
            requested_at: None,
            resolved_at: None,
            resolving_decision: None,
        }
    }

    fn file_change_approval(
        &mut self,
        request: &CodexServerRequestFrame,
        params: &Map<String, Value>,
    ) -> ApprovalRequest {
        let item_key =
            string_param(params, "itemId").unwrap_or_else(|| request.request_id.to_string());
        let turn_id = turn_id(&mut self.id_map, params);
        let item_id = item_id(&mut self.id_map, params);
        let mut details = Vec::new();
        push_labeled(
            &mut details,
            "Reason",
            string_param(params, "reason").as_deref(),
        );
        push_labeled(
            &mut details,
            "Grant root",
            string_param(params, "grantRoot").as_deref(),
        );

        ApprovalRequest {
            approval_id: stable_approval_id(self.session_id, "codex-file-change", &item_key),
            session_id: self.session_id,
            turn_id,
            item_id,
            kind: ApprovalKind::FileChange,
            title: "Approve Codex file changes".to_owned(),
            details: join_details(details),
            options: ApprovalOption::canonical_defaults(),
            status: ApprovalStatus::Pending,
            risk: None,
            subject: string_param(params, "grantRoot"),
            native_request_id: Some(request.request_id.to_string()),
            native_blocking: native_blocking(&request.raw_payload),
            policy: None,
            native: Some(native_ref_for_server_request(request)),
            requested_at: None,
            resolved_at: None,
            resolving_decision: None,
        }
    }

    fn permission_approval(
        &mut self,
        request: &CodexServerRequestFrame,
        params: &Map<String, Value>,
    ) -> ApprovalRequest {
        let item_key =
            string_param(params, "itemId").unwrap_or_else(|| request.request_id.to_string());
        let turn_id = turn_id(&mut self.id_map, params);
        let item_id = item_id(&mut self.id_map, params);
        let cwd = string_param(params, "cwd");
        let mut details = Vec::new();
        push_labeled(&mut details, "Cwd", cwd.as_deref());
        push_labeled(
            &mut details,
            "Reason",
            string_param(params, "reason").as_deref(),
        );
        push_json(
            &mut details,
            "Requested permissions",
            params.get("permissions"),
        );

        ApprovalRequest {
            approval_id: stable_approval_id(self.session_id, "codex-permission", &item_key),
            session_id: self.session_id,
            turn_id,
            item_id,
            kind: ApprovalKind::Permission,
            title: "Approve Codex permission request".to_owned(),
            details: join_details(details),
            options: vec![
                ApprovalOption::approve_once(),
                ApprovalOption::approve_always(),
                ApprovalOption::deny(),
                ApprovalOption::cancel_turn(),
            ],
            status: ApprovalStatus::Pending,
            risk: None,
            subject: cwd,
            native_request_id: Some(request.request_id.to_string()),
            native_blocking: native_blocking(&request.raw_payload),
            policy: None,
            native: Some(native_ref_for_server_request(request)),
            requested_at: None,
            resolved_at: None,
            resolving_decision: None,
        }
    }

    fn legacy_file_change_approval(
        &mut self,
        request: &CodexServerRequestFrame,
        params: &Map<String, Value>,
    ) -> ApprovalRequest {
        let key = string_param(params, "callId").unwrap_or_else(|| request.request_id.to_string());
        let mut details = Vec::new();
        push_labeled(
            &mut details,
            "Reason",
            string_param(params, "reason").as_deref(),
        );
        push_labeled(
            &mut details,
            "Grant root",
            string_param(params, "grantRoot").as_deref(),
        );
        push_json(&mut details, "File changes", params.get("fileChanges"));

        ApprovalRequest {
            approval_id: stable_approval_id(self.session_id, "codex-legacy-patch", &key),
            session_id: self.session_id,
            turn_id: None,
            item_id: None,
            kind: ApprovalKind::FileChange,
            title: "Approve legacy Codex patch".to_owned(),
            details: join_details(details),
            options: ApprovalOption::canonical_defaults(),
            status: ApprovalStatus::Pending,
            risk: None,
            subject: string_param(params, "grantRoot"),
            native_request_id: Some(request.request_id.to_string()),
            native_blocking: native_blocking(&request.raw_payload),
            policy: None,
            native: Some(native_ref_for_server_request(request)),
            requested_at: None,
            resolved_at: None,
            resolving_decision: None,
        }
    }

    fn legacy_command_approval(
        &mut self,
        request: &CodexServerRequestFrame,
        params: &Map<String, Value>,
    ) -> ApprovalRequest {
        let approval_key = string_param(params, "approvalId")
            .or_else(|| string_param(params, "callId"))
            .unwrap_or_else(|| request.request_id.to_string());
        let command = params
            .get("command")
            .and_then(Value::as_array)
            .map(|parts| {
                parts
                    .iter()
                    .filter_map(Value::as_str)
                    .collect::<Vec<_>>()
                    .join(" ")
            });
        let cwd = string_param(params, "cwd");
        let mut details = Vec::new();
        push_labeled(&mut details, "Command", command.as_deref());
        push_labeled(&mut details, "Cwd", cwd.as_deref());
        push_labeled(
            &mut details,
            "Reason",
            string_param(params, "reason").as_deref(),
        );
        push_json(&mut details, "Parsed command", params.get("parsedCmd"));

        ApprovalRequest {
            approval_id: stable_approval_id(self.session_id, "codex-legacy-command", &approval_key),
            session_id: self.session_id,
            turn_id: None,
            item_id: None,
            kind: ApprovalKind::Command,
            title: command.unwrap_or_else(|| "Approve legacy Codex command".to_owned()),
            details: join_details(details),
            options: ApprovalOption::canonical_defaults(),
            status: ApprovalStatus::Pending,
            risk: None,
            subject: cwd,
            native_request_id: Some(request.request_id.to_string()),
            native_blocking: native_blocking(&request.raw_payload),
            policy: None,
            native: Some(native_ref_for_server_request(request)),
            requested_at: None,
            resolved_at: None,
            resolving_decision: None,
        }
    }

    fn tool_user_input_question(
        &mut self,
        request: &CodexServerRequestFrame,
        params: &Map<String, Value>,
    ) -> QuestionState {
        let key = format!(
            "{}:{}",
            request.request_id,
            string_param(params, "itemId").unwrap_or_default()
        );
        let fields = params
            .get("questions")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(tool_question_field)
            .collect::<Vec<_>>();

        QuestionState {
            question_id: stable_question_id(self.session_id, "codex-tool-input", &key),
            session_id: self.session_id,
            turn_id: turn_id(&mut self.id_map, params),
            title: "Codex needs input".to_owned(),
            description: None,
            fields,
            status: QuestionStatus::Pending,
            answer: None,
            native_request_id: Some(request.request_id.to_string()),
            native_blocking: native_blocking(&request.raw_payload),
            native: Some(native_ref_for_server_request(request)),
            requested_at: None,
            answered_at: None,
        }
    }

    fn mcp_elicitation_question(
        &mut self,
        request: &CodexServerRequestFrame,
        params: &Map<String, Value>,
    ) -> QuestionState {
        let server_name =
            string_param(params, "serverName").unwrap_or_else(|| "MCP server".to_owned());
        let message = string_param(params, "message");
        let key = format!("{server_name}:{}", request.request_id);
        let mut fields = vec![AgentQuestionField {
            id: "action".to_owned(),
            label: "Action".to_owned(),
            prompt: message.clone(),
            kind: "choice".to_owned(),
            required: true,
            secret: false,
            choices: vec![
                AgentQuestionChoice {
                    value: "accept".to_owned(),
                    label: "Accept".to_owned(),
                    description: None,
                },
                AgentQuestionChoice {
                    value: "decline".to_owned(),
                    label: "Decline".to_owned(),
                    description: None,
                },
                AgentQuestionChoice {
                    value: "cancel".to_owned(),
                    label: "Cancel".to_owned(),
                    description: None,
                },
            ],
            default_answers: Vec::new(),
            schema: None,
        }];

        if let Some(url) = string_param(params, "url") {
            fields.push(AgentQuestionField {
                id: "url".to_owned(),
                label: "URL".to_owned(),
                prompt: Some(url),
                kind: "url".to_owned(),
                required: false,
                secret: false,
                choices: Vec::new(),
                default_answers: Vec::new(),
                schema: params
                    .get("elicitationId")
                    .cloned()
                    .map(|id| json!({ "elicitationId": id })),
            });
        }

        if let Some(schema) = params.get("requestedSchema") {
            fields.push(AgentQuestionField {
                id: "content".to_owned(),
                label: "Response".to_owned(),
                prompt: message.clone(),
                kind: "object".to_owned(),
                required: false,
                secret: false,
                choices: Vec::new(),
                default_answers: Vec::new(),
                schema: Some(schema.clone()),
            });
        }

        QuestionState {
            question_id: stable_question_id(self.session_id, "codex-mcp-elicitation", &key),
            session_id: self.session_id,
            turn_id: turn_id(&mut self.id_map, params),
            title: format!("{server_name} requests input"),
            description: message,
            fields,
            status: QuestionStatus::Pending,
            answer: None,
            native_request_id: Some(request.request_id.to_string()),
            native_blocking: native_blocking(&request.raw_payload),
            native: Some(native_ref_for_server_request(request)),
            requested_at: None,
            answered_at: None,
        }
    }

    fn unsupported(
        &self,
        request: &CodexServerRequestFrame,
        title: &str,
        detail: &str,
    ) -> CodexServerRequestOutput {
        CodexServerRequestOutput::NativeUnsupported {
            title: title.to_owned(),
            detail: detail.to_owned(),
            native: native_ref_for_server_request(request),
            response: codex_unsupported_response_for_request(
                request,
                format!("{detail} Native request id: {}.", request.request_id),
            ),
        }
    }
}

#[must_use]
pub fn unsupported_notification(output: &CodexServerRequestOutput) -> Option<UniversalEventKind> {
    let CodexServerRequestOutput::NativeUnsupported {
        title,
        detail,
        native,
        ..
    } = output
    else {
        return None;
    };
    Some(UniversalEventKind::ProviderNotification {
        notification: ProviderNotification {
            category: "codex_unsupported_server_request".to_owned(),
            title: title.clone(),
            detail: Some(detail.clone()),
            status: Some("unsupported".to_owned()),
            severity: Some(ProviderNotificationSeverity::Warning),
            subject: native.method.clone(),
        },
    })
}

#[must_use]
pub fn codex_approval_response(
    request_id: RequestId,
    method: &str,
    decision: &ApprovalDecision,
    original_request_payload: Option<&Value>,
) -> Value {
    let result = match method {
        "item/commandExecution/requestApproval" => {
            json!({ "decision": command_decision_payload(decision) })
        }
        "item/fileChange/requestApproval" => {
            json!({ "decision": simple_decision_payload(decision) })
        }
        "item/permissions/requestApproval" => {
            permission_response_payload(decision, original_request_payload)
        }
        "ApplyPatchApproval" | "ExecCommandApproval" => {
            json!({ "decision": legacy_review_decision_payload(decision) })
        }
        _ => provider_specific_result(decision).unwrap_or_else(|| json!({ "decision": "decline" })),
    };
    json!({ "id": request_id, "result": result })
}

#[must_use]
pub fn codex_tool_user_input_response(
    request_id: RequestId,
    answer: &AgentQuestionAnswer,
) -> Value {
    let answers = answer
        .answers
        .iter()
        .map(|(field_id, values)| (field_id.clone(), json!({ "answers": values })))
        .collect::<Map<_, _>>();
    json!({
        "id": request_id,
        "result": {
            "answers": answers,
        }
    })
}

#[must_use]
pub fn codex_mcp_elicitation_response(
    request_id: RequestId,
    action: &str,
    content: Option<Value>,
    meta: Option<Value>,
) -> Value {
    let action = match action {
        "accept" => "accept",
        "cancel" => "cancel",
        _ => "decline",
    };
    json!({
        "id": request_id,
        "result": {
            "action": action,
            "content": content,
            "_meta": meta,
        }
    })
}

#[must_use]
pub fn codex_unsupported_response(request_id: RequestId, message: String) -> Value {
    json!({
        "id": request_id,
        "result": {
            "contentItems": [
                {
                    "type": "inputText",
                    "text": message,
                }
            ],
            "success": false,
        }
    })
}

#[must_use]
pub fn codex_unsupported_error_response(request_id: RequestId, message: String) -> Value {
    json!({
        "id": request_id,
        "error": {
            "code": -32601,
            "message": message,
        }
    })
}

fn codex_unsupported_response_for_request(
    request: &CodexServerRequestFrame,
    message: String,
) -> Value {
    if request.method == "item/tool/call" {
        codex_unsupported_response(request.request_id.clone(), message)
    } else {
        codex_unsupported_error_response(request.request_id.clone(), message)
    }
}

fn params_object(raw_payload: &Value) -> &Map<String, Value> {
    match raw_payload.get("params").and_then(Value::as_object) {
        Some(params) => params,
        None => empty_object(),
    }
}

fn empty_object() -> &'static Map<String, Value> {
    static EMPTY: std::sync::OnceLock<Map<String, Value>> = std::sync::OnceLock::new();
    EMPTY.get_or_init(Map::new)
}

fn string_param(params: &Map<String, Value>, key: &str) -> Option<String> {
    params.get(key).and_then(value_to_string)
}

fn value_to_string(value: &Value) -> Option<String> {
    match value {
        Value::String(value) if !value.is_empty() => Some(value.clone()),
        Value::Number(_) | Value::Bool(_) => Some(value.to_string()),
        _ => None,
    }
}

fn native_blocking(raw_payload: &Value) -> bool {
    raw_payload
        .get("blocking")
        .or_else(|| raw_payload.pointer("/params/blocking"))
        .and_then(Value::as_bool)
        .unwrap_or(true)
}

fn turn_id(id_map: &mut CodexIdMap, params: &Map<String, Value>) -> Option<TurnId> {
    let thread_id = string_param(params, "threadId")?;
    let native_turn_id = string_param(params, "turnId")?;
    Some(id_map.turn_id(&thread_id, &native_turn_id))
}

fn item_id(id_map: &mut CodexIdMap, params: &Map<String, Value>) -> Option<ItemId> {
    let thread_id = string_param(params, "threadId")?;
    let native_item_id = string_param(params, "itemId")?;
    Some(id_map.item_id(&thread_id, &native_item_id))
}

fn stable_approval_id(session_id: SessionId, kind: &str, key: &str) -> ApprovalId {
    ApprovalId::from_uuid(stable_uuid(session_id, kind, key))
}

fn stable_question_id(session_id: SessionId, kind: &str, key: &str) -> QuestionId {
    QuestionId::from_uuid(stable_uuid(session_id, kind, key))
}

fn stable_uuid(session_id: SessionId, kind: &str, key: &str) -> Uuid {
    Uuid::new_v5(
        &Uuid::NAMESPACE_URL,
        format!("agenter:codex:obligation:{session_id}:{kind}:{key}").as_bytes(),
    )
}

fn push_labeled(details: &mut Vec<String>, label: &str, value: Option<&str>) {
    if let Some(value) = value.filter(|value| !value.is_empty()) {
        details.push(format!("{label}: {value}"));
    }
}

fn push_json(details: &mut Vec<String>, label: &str, value: Option<&Value>) {
    if let Some(value) = value.filter(|value| !value.is_null()) {
        details.push(format!("{label}: {value}"));
    }
}

fn join_details(details: Vec<String>) -> Option<String> {
    (!details.is_empty()).then(|| details.join("\n"))
}

fn command_options(params: &Map<String, Value>) -> Vec<ApprovalOption> {
    let mut options = approval_options_from_native_decisions(params.get("availableDecisions"));
    if options.is_empty() {
        options = ApprovalOption::canonical_defaults();
    }
    if let Some(amendment) = params.get("proposedExecpolicyAmendment") {
        options.push(ApprovalOption {
            option_id: "accept_with_execpolicy_amendment".to_owned(),
            kind: ApprovalOptionKind::PersistApprovalRule,
            label: "Approve and remember command policy".to_owned(),
            description: Some("Apply Codex's proposed exec policy amendment.".to_owned()),
            scope: Some("policy".to_owned()),
            native_option_id: Some("acceptWithExecpolicyAmendment".to_owned()),
            policy_rule: None,
        });
        let _ = amendment;
    }
    if params
        .get("proposedNetworkPolicyAmendments")
        .and_then(Value::as_array)
        .is_some_and(|amendments| !amendments.is_empty())
    {
        options.push(ApprovalOption {
            option_id: "apply_network_policy_amendment".to_owned(),
            kind: ApprovalOptionKind::PersistApprovalRule,
            label: "Apply network policy".to_owned(),
            description: Some(
                "Apply one of Codex's proposed network policy amendments.".to_owned(),
            ),
            scope: Some("policy".to_owned()),
            native_option_id: Some("applyNetworkPolicyAmendment".to_owned()),
            policy_rule: None,
        });
    }
    options
}

fn approval_options_from_native_decisions(value: Option<&Value>) -> Vec<ApprovalOption> {
    value
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .filter_map(|decision| {
            decision
                .as_str()
                .or_else(|| decision.get("type").and_then(Value::as_str))
        })
        .filter_map(|decision| match decision {
            "accept" => Some(ApprovalOption::approve_once()),
            "acceptForSession" => Some(ApprovalOption::approve_always()),
            "decline" => Some(ApprovalOption::deny()),
            "cancel" => Some(ApprovalOption::cancel_turn()),
            _ => None,
        })
        .collect()
}

fn tool_question_field(value: &Value) -> Option<AgentQuestionField> {
    let object = value.as_object()?;
    let id = string_param(object, "id")?;
    let options = object
        .get("options")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .enumerate()
        .map(|(index, option)| AgentQuestionChoice {
            value: option
                .get("label")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| index.to_string()),
            label: option
                .get("label")
                .and_then(Value::as_str)
                .map(str::to_owned)
                .unwrap_or_else(|| format!("Option {}", index + 1)),
            description: option
                .get("description")
                .and_then(Value::as_str)
                .map(str::to_owned),
        })
        .collect::<Vec<_>>();
    Some(AgentQuestionField {
        id,
        label: string_param(object, "header").unwrap_or_else(|| "Input".to_owned()),
        prompt: string_param(object, "question"),
        kind: if options.is_empty() {
            "text".to_owned()
        } else {
            "choice".to_owned()
        },
        required: !object
            .get("isOther")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        secret: object
            .get("isSecret")
            .and_then(Value::as_bool)
            .unwrap_or(false),
        choices: options,
        default_answers: Vec::new(),
        schema: Some(value.clone()),
    })
}

fn command_decision_payload(decision: &ApprovalDecision) -> Value {
    if let Some(payload) = provider_specific_result(decision) {
        return payload.get("decision").cloned().unwrap_or(payload);
    }
    match decision {
        ApprovalDecision::Accept => json!("accept"),
        ApprovalDecision::AcceptForSession => json!("acceptForSession"),
        ApprovalDecision::Decline => json!("decline"),
        ApprovalDecision::Cancel => json!("cancel"),
        ApprovalDecision::ProviderSpecific { .. } => unreachable!(),
    }
}

fn simple_decision_payload(decision: &ApprovalDecision) -> Value {
    match decision {
        ApprovalDecision::ProviderSpecific { payload } => payload
            .get("decision")
            .cloned()
            .unwrap_or_else(|| payload.clone()),
        ApprovalDecision::Accept => json!("accept"),
        ApprovalDecision::AcceptForSession => json!("acceptForSession"),
        ApprovalDecision::Decline => json!("decline"),
        ApprovalDecision::Cancel => json!("cancel"),
    }
}

fn legacy_review_decision_payload(decision: &ApprovalDecision) -> Value {
    match decision {
        ApprovalDecision::ProviderSpecific { payload } => payload
            .get("decision")
            .cloned()
            .unwrap_or_else(|| payload.clone()),
        ApprovalDecision::Accept => json!("approved"),
        ApprovalDecision::AcceptForSession => json!("approved_for_session"),
        ApprovalDecision::Decline => json!("denied"),
        ApprovalDecision::Cancel => json!("abort"),
    }
}

fn permission_response_payload(
    decision: &ApprovalDecision,
    original_request_payload: Option<&Value>,
) -> Value {
    if let Some(payload) = provider_specific_result(decision) {
        return payload;
    }
    match decision {
        ApprovalDecision::Accept | ApprovalDecision::AcceptForSession => {
            let permissions = original_request_payload
                .and_then(|raw| raw.pointer("/params/permissions"))
                .cloned()
                .unwrap_or_else(|| json!({}));
            json!({
                "permissions": permissions,
                "scope": if matches!(decision, ApprovalDecision::AcceptForSession) {
                    "session"
                } else {
                    "turn"
                },
                "strictAutoReview": false,
            })
        }
        ApprovalDecision::Decline | ApprovalDecision::Cancel => json!({
            "permissions": {},
            "scope": "turn",
            "strictAutoReview": false,
        }),
        ApprovalDecision::ProviderSpecific { .. } => unreachable!(),
    }
}

fn provider_specific_result(decision: &ApprovalDecision) -> Option<Value> {
    match decision {
        ApprovalDecision::ProviderSpecific { payload } => Some(payload.clone()),
        _ => None,
    }
}

fn native_ref_for_server_request(request: &CodexServerRequestFrame) -> NativeRef {
    NativeRef {
        protocol: CODEX_APP_SERVER_PROTOCOL.to_owned(),
        method: Some(request.method.clone()),
        kind: Some("server_request".to_owned()),
        native_id: Some(request.request_id.to_string()),
        summary: None,
        hash: None,
        pointer: None,
        raw_payload: Some(request.raw_payload.clone()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agents::codex::codec::{CodexCodec, CodexDecodedFrame};
    use std::collections::BTreeMap;

    fn decode_request(raw: Value) -> CodexServerRequestFrame {
        let mut codec = CodexCodec::new();
        let frame = codec.decode_line(&raw.to_string());
        let CodexDecodedFrame::ServerRequest(request) = frame else {
            panic!("expected server request");
        };
        request
    }

    fn mapper() -> CodexObligationMapper {
        CodexObligationMapper::new(SessionId::nil())
    }

    #[test]
    fn codex_approval_requests_map_command_approval_with_raw_payload_and_response() {
        let raw = json!({
            "id": "req-command",
            "method": "item/commandExecution/requestApproval",
            "blocking": false,
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-1",
                "approvalId": "approval-bridge-1",
                "command": "npm test",
                "cwd": "/work",
                "reason": "Needs network",
                "additionalPermissions": { "network": { "hosts": ["registry.npmjs.org"] } },
                "proposedExecpolicyAmendment": { "matcher": { "program": "npm" } },
                "proposedNetworkPolicyAmendments": [{ "host": "registry.npmjs.org", "action": "allow" }],
                "availableDecisions": ["accept", "acceptForSession", "decline", "cancel"]
            }
        });
        let request = decode_request(raw.clone());
        let CodexServerRequestOutput::ApprovalRequested(approval) =
            mapper().map_server_request(&request)
        else {
            panic!("expected approval");
        };

        assert_eq!(approval.kind, ApprovalKind::Command);
        assert_eq!(approval.native_request_id.as_deref(), Some("req-command"));
        assert!(!approval.native_blocking);
        assert!(approval.item_id.is_some());
        assert!(approval.turn_id.is_some());
        assert!(approval
            .details
            .as_deref()
            .unwrap()
            .contains("Additional permissions"));
        assert!(approval
            .options
            .iter()
            .any(|option| option.native_option_id.as_deref()
                == Some("acceptWithExecpolicyAmendment")));
        assert_eq!(
            approval.native.as_ref().unwrap().raw_payload.as_ref(),
            Some(&raw)
        );

        let response = codex_approval_response(
            RequestId::String("req-command".to_owned()),
            &request.method,
            &ApprovalDecision::AcceptForSession,
            Some(&request.raw_payload),
        );
        assert_eq!(response["id"], "req-command");
        assert_eq!(response["result"]["decision"], "acceptForSession");
    }

    #[test]
    fn codex_approval_requests_use_item_id_fallback_for_command_approval_key() {
        let raw = json!({
            "id": 7,
            "method": "item/commandExecution/requestApproval",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-fallback",
                "command": "cargo test"
            }
        });
        let request = decode_request(raw);
        let mut first = mapper();
        let mut second = mapper();
        let CodexServerRequestOutput::ApprovalRequested(first_approval) =
            first.map_server_request(&request)
        else {
            panic!("expected approval");
        };
        let CodexServerRequestOutput::ApprovalRequested(second_approval) =
            second.map_server_request(&request)
        else {
            panic!("expected approval");
        };

        assert_eq!(first_approval.approval_id, second_approval.approval_id);
        assert_eq!(first_approval.native_request_id.as_deref(), Some("7"));
    }

    #[test]
    fn codex_approval_requests_map_file_change_and_permission_payloads() {
        let file_request = decode_request(json!({
            "id": "req-file",
            "method": "item/fileChange/requestApproval",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "file-item",
                "reason": "write",
                "grantRoot": "/work"
            }
        }));
        let permission_raw = json!({
            "id": "req-permission",
            "method": "item/permissions/requestApproval",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "perm-item",
                "cwd": "/work",
                "reason": "need fs and network",
                "permissions": {
                    "network": { "enabled": true },
                    "fileSystem": { "read": ["/work"], "write": ["/work/tmp"] }
                }
            }
        });
        let permission_request = decode_request(permission_raw.clone());
        let mut mapper = mapper();

        let CodexServerRequestOutput::ApprovalRequested(file_approval) =
            mapper.map_server_request(&file_request)
        else {
            panic!("expected approval");
        };
        let CodexServerRequestOutput::ApprovalRequested(permission_approval) =
            mapper.map_server_request(&permission_request)
        else {
            panic!("expected approval");
        };

        assert_eq!(file_approval.kind, ApprovalKind::FileChange);
        assert_eq!(
            file_approval.native.as_ref().unwrap().raw_payload.as_ref(),
            Some(&file_request.raw_payload)
        );
        assert_eq!(permission_approval.kind, ApprovalKind::Permission);
        assert_eq!(permission_approval.subject.as_deref(), Some("/work"));
        assert!(permission_approval
            .details
            .as_deref()
            .unwrap()
            .contains("Requested permissions"));
        assert_eq!(
            permission_approval
                .native
                .as_ref()
                .unwrap()
                .raw_payload
                .as_ref(),
            Some(&permission_raw)
        );

        let response = codex_approval_response(
            RequestId::String("req-permission".to_owned()),
            &permission_request.method,
            &ApprovalDecision::ProviderSpecific {
                payload: json!({
                    "permissions": { "network": { "enabled": true } },
                    "scope": "session",
                    "strictAutoReview": true
                }),
            },
            Some(&permission_request.raw_payload),
        );
        assert_eq!(response["id"], "req-permission");
        assert_eq!(response["result"]["scope"], "session");
        assert_eq!(response["result"]["strictAutoReview"], true);
        assert_eq!(
            response["result"]["permissions"]["network"]["enabled"],
            true
        );
    }

    #[test]
    fn codex_question_requests_map_tool_user_input_with_schema_and_response() {
        let raw = json!({
            "id": "req-tool-input",
            "method": "item/tool/requestUserInput",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "tool-item",
                "questions": [{
                    "id": "choice",
                    "header": "Mode",
                    "question": "Pick mode",
                    "isSecret": false,
                    "options": [{ "label": "Fast", "description": "Quick path" }]
                }, {
                    "id": "token",
                    "header": "Token",
                    "question": "Paste token",
                    "isSecret": true,
                    "options": null
                }]
            }
        });
        let request = decode_request(raw.clone());
        let CodexServerRequestOutput::QuestionRequested(question) =
            mapper().map_server_request(&request)
        else {
            panic!("expected question");
        };

        assert_eq!(
            question.native_request_id.as_deref(),
            Some("req-tool-input")
        );
        assert_eq!(question.fields.len(), 2);
        assert_eq!(question.fields[0].schema.as_ref().unwrap()["id"], "choice");
        assert!(question.fields[1].secret);
        assert_eq!(
            question.native.as_ref().unwrap().raw_payload.as_ref(),
            Some(&raw)
        );

        let answer = AgentQuestionAnswer {
            question_id: question.question_id,
            answers: BTreeMap::from([
                ("choice".to_owned(), vec!["Fast".to_owned()]),
                ("token".to_owned(), vec!["secret".to_owned()]),
            ]),
        };
        let response =
            codex_tool_user_input_response(RequestId::String("req-tool-input".to_owned()), &answer);
        assert_eq!(response["id"], "req-tool-input");
        assert_eq!(
            response["result"]["answers"]["choice"]["answers"][0],
            "Fast"
        );
        assert_eq!(
            response["result"]["answers"]["token"]["answers"][0],
            "secret"
        );
    }

    #[test]
    fn codex_question_requests_map_mcp_elicitation_with_schema_and_response() {
        let raw = json!({
            "id": "req-mcp",
            "method": "mcpServer/elicitation/request",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "serverName": "linear",
                "message": "Create issue?",
                "requestedSchema": {
                    "type": "object",
                    "properties": {
                        "title": { "type": "string", "title": "Title" }
                    },
                    "required": ["title"]
                }
            }
        });
        let request = decode_request(raw.clone());
        let CodexServerRequestOutput::QuestionRequested(question) =
            mapper().map_server_request(&request)
        else {
            panic!("expected question");
        };

        assert_eq!(question.title, "linear requests input");
        assert_eq!(question.description.as_deref(), Some("Create issue?"));
        assert_eq!(question.fields[1].id, "content");
        assert_eq!(
            question.fields[1].schema.as_ref().unwrap()["properties"]["title"]["type"],
            "string"
        );
        assert_eq!(
            question.native.as_ref().unwrap().raw_payload.as_ref(),
            Some(&raw)
        );

        let response = codex_mcp_elicitation_response(
            RequestId::String("req-mcp".to_owned()),
            "accept",
            Some(json!({ "title": "Ship Stage 6" })),
            Some(json!({ "client": "agenter" })),
        );
        assert_eq!(response["id"], "req-mcp");
        assert_eq!(response["result"]["action"], "accept");
        assert_eq!(response["result"]["content"]["title"], "Ship Stage 6");
        assert_eq!(response["result"]["_meta"]["client"], "agenter");
    }

    #[test]
    fn codex_approval_requests_map_legacy_approval_requests_and_review_decisions() {
        let patch_request = decode_request(json!({
            "id": "legacy-patch",
            "method": "ApplyPatchApproval",
            "params": {
                "conversationId": "thread-legacy",
                "callId": "patch-call",
                "fileChanges": { "src/lib.rs": { "type": "add" } },
                "reason": "legacy write",
                "grantRoot": "/work"
            }
        }));
        let exec_request = decode_request(json!({
            "id": "legacy-exec",
            "method": "ExecCommandApproval",
            "params": {
                "conversationId": "thread-legacy",
                "callId": "exec-call",
                "approvalId": "exec-approval",
                "command": ["cargo", "test"],
                "cwd": "/work",
                "reason": "legacy command",
                "parsedCmd": []
            }
        }));
        let mut mapper = mapper();
        let CodexServerRequestOutput::ApprovalRequested(patch) =
            mapper.map_server_request(&patch_request)
        else {
            panic!("expected approval");
        };
        let CodexServerRequestOutput::ApprovalRequested(exec) =
            mapper.map_server_request(&exec_request)
        else {
            panic!("expected approval");
        };

        assert_eq!(patch.kind, ApprovalKind::FileChange);
        assert_eq!(exec.kind, ApprovalKind::Command);
        assert_eq!(patch.native_request_id.as_deref(), Some("legacy-patch"));
        assert_eq!(exec.native_request_id.as_deref(), Some("legacy-exec"));

        let response = codex_approval_response(
            RequestId::String("legacy-exec".to_owned()),
            &exec_request.method,
            &ApprovalDecision::Cancel,
            Some(&exec_request.raw_payload),
        );
        assert_eq!(response["id"], "legacy-exec");
        assert_eq!(response["result"]["decision"], "abort");
    }

    #[test]
    fn codex_approval_requests_surface_token_refresh_and_dynamic_tool_as_unsupported() {
        let token = decode_request(json!({
            "id": "token-refresh",
            "method": "account/chatgptAuthTokens/refresh",
            "params": {
                "reason": "unauthorized",
                "previousAccountId": "acct_1"
            }
        }));
        let dynamic = decode_request(json!({
            "id": "dynamic-tool",
            "method": "item/tool/call",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "callId": "call-1",
                "namespace": "client",
                "tool": "open_browser",
                "arguments": { "url": "http://localhost:3000" }
            }
        }));
        let mut token_mapper = mapper();

        let token_output = token_mapper.map_server_request(&token);
        let mut mapper = mapper();
        let dynamic_output = mapper.map_server_request(&dynamic);

        let CodexServerRequestOutput::NativeUnsupported {
            native, response, ..
        } = token_output
        else {
            panic!("expected unsupported");
        };
        assert_eq!(
            native.raw_payload.as_ref().unwrap()["params"]["previousAccountId"],
            "acct_1"
        );
        assert_eq!(response["id"], "token-refresh");
        assert_eq!(response["error"]["code"], -32601);
        assert!(response["error"]["message"]
            .as_str()
            .unwrap()
            .contains("auth tokens"));

        let CodexServerRequestOutput::NativeUnsupported {
            native, response, ..
        } = dynamic_output
        else {
            panic!("expected unsupported");
        };
        assert_eq!(
            native.raw_payload.as_ref().unwrap()["params"]["tool"],
            "open_browser"
        );
        assert_eq!(response["id"], "dynamic-tool");
        assert_eq!(response["result"]["success"], false);
        assert!(
            unsupported_notification(&CodexServerRequestOutput::NativeUnsupported {
                title: "x".to_owned(),
                detail: "y".to_owned(),
                native,
                response,
            })
            .is_some()
        );
    }
}
