#![allow(dead_code)]

use std::path::PathBuf;

use agenter_core::{AgentReasoningEffort, AgentTurnSettings, NativeRef, TurnId, UserInput};
use anyhow::Context;
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::{
    codec::{native_ref_for_frame, CodexClientResponseFrame, CODEX_APP_SERVER_PROTOCOL},
    id_map::CodexIdMap,
    transport::CodexTransport,
};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexTurnStartRequest {
    pub thread_id: String,
    pub input: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub responsesapi_client_metadata: Option<Map<String, Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub environments: Option<Vec<CodexTurnEnvironmentParams>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approval_policy: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub approvals_reviewer: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sandbox_policy: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub permissions: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub service_tier: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub personality: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output_schema: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub collaboration_mode: Option<Value>,
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexTurnEnvironmentParams {
    pub environment_id: String,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexTurnSteerRequest {
    pub thread_id: String,
    pub input: Vec<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub responsesapi_client_metadata: Option<Map<String, Value>>,
    pub expected_turn_id: String,
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexTurnInterruptRequest {
    pub thread_id: String,
    pub turn_id: String,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexTurnStartResult {
    pub method: String,
    pub native: NativeRef,
    pub raw_request: Value,
    pub raw_response: Value,
    pub native_thread_id: String,
    pub native_turn_id: String,
    pub turn_id: TurnId,
    pub turn: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexTurnSteerResult {
    pub method: String,
    pub native: NativeRef,
    pub raw_request: Value,
    pub raw_response: Value,
    pub native_thread_id: String,
    pub expected_native_turn_id: String,
    pub native_turn_id: String,
    pub turn_id: TurnId,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexTurnInterruptResult {
    pub method: String,
    pub native: NativeRef,
    pub raw_request: Value,
    pub raw_response: Value,
    pub native_thread_id: String,
    pub native_turn_id: String,
}

pub struct CodexTurnClient<'a> {
    transport: &'a mut CodexTransport,
}

impl<'a> CodexTurnClient<'a> {
    pub fn new(transport: &'a mut CodexTransport) -> Self {
        Self { transport }
    }

    pub async fn start_turn(
        &mut self,
        request: CodexTurnStartRequest,
        id_map: &mut CodexIdMap,
    ) -> anyhow::Result<CodexTurnStartResult> {
        let raw_request = serde_json::to_value(&request)?;
        let response = self
            .transport
            .request_response("turn/start", raw_request.clone())
            .await?;
        parse_start_response(raw_request, response, id_map)
    }

    pub async fn steer_turn(
        &mut self,
        request: CodexTurnSteerRequest,
        id_map: &mut CodexIdMap,
    ) -> anyhow::Result<CodexTurnSteerResult> {
        let raw_request = serde_json::to_value(&request)?;
        let response = self
            .transport
            .request_response("turn/steer", raw_request.clone())
            .await?;
        parse_steer_response(raw_request, response, id_map)
    }

    pub async fn interrupt_turn(
        &mut self,
        request: CodexTurnInterruptRequest,
    ) -> anyhow::Result<CodexTurnInterruptResult> {
        let raw_request = serde_json::to_value(&request)?;
        let response = self
            .transport
            .request_response("turn/interrupt", raw_request.clone())
            .await?;
        Ok(CodexTurnInterruptResult {
            method: "turn/interrupt".to_owned(),
            native: native_ref_for_frame(&response),
            raw_response: response.raw_payload,
            native_thread_id: request.thread_id,
            native_turn_id: request.turn_id,
            raw_request,
        })
    }
}

pub fn start_request_from_universal(
    thread_id: impl Into<String>,
    input: &UserInput,
    settings: Option<&AgentTurnSettings>,
    extra: Map<String, Value>,
) -> CodexTurnStartRequest {
    // Dynamic tool specs are intentionally not promoted to a supported field here.
    // Agenter does not yet have a client-tool execution contract; callers that
    // need to preserve native Codex dynamic tool payloads, modelProvider, or
    // other exact app-server params must pass them in `extra`.
    CodexTurnStartRequest {
        thread_id: thread_id.into(),
        input: codex_input_from_universal(input),
        model: settings.and_then(|settings| settings.model.clone()),
        effort: settings.and_then(|settings| {
            settings
                .reasoning_effort
                .as_ref()
                .map(codex_reasoning_effort)
        }),
        collaboration_mode: settings.and_then(codex_collaboration_mode),
        extra,
        ..Default::default()
    }
}

pub fn steer_request_from_universal(
    thread_id: impl Into<String>,
    expected_native_turn_id: impl Into<String>,
    input: &UserInput,
    extra: Map<String, Value>,
) -> CodexTurnSteerRequest {
    CodexTurnSteerRequest {
        thread_id: thread_id.into(),
        expected_turn_id: expected_native_turn_id.into(),
        input: codex_input_from_universal(input),
        extra,
        ..Default::default()
    }
}

pub fn interrupt_request_from_universal(
    thread_id: impl Into<String>,
    native_turn_id: impl Into<String>,
) -> CodexTurnInterruptRequest {
    CodexTurnInterruptRequest {
        thread_id: thread_id.into(),
        turn_id: native_turn_id.into(),
    }
}

#[must_use]
pub fn native_ref_for_turn_command_result(
    method: &str,
    native_id: Option<String>,
    raw_payload: Value,
) -> NativeRef {
    NativeRef {
        protocol: CODEX_APP_SERVER_PROTOCOL.to_owned(),
        method: Some(method.to_owned()),
        kind: Some("client_response".to_owned()),
        native_id,
        summary: Some("Codex turn command result".to_owned()),
        hash: None,
        pointer: None,
        raw_payload: Some(raw_payload),
    }
}

fn parse_start_response(
    raw_request: Value,
    response: CodexClientResponseFrame,
    id_map: &mut CodexIdMap,
) -> anyhow::Result<CodexTurnStartResult> {
    let native_thread_id = string_field(&raw_request, "threadId").context("missing threadId")?;
    let turn = response
        .result
        .get("turn")
        .cloned()
        .context("Codex turn/start response did not include `turn`")?;
    let native_turn_id = string_field(&turn, "id").context("Codex turn is missing `id`")?;
    Ok(CodexTurnStartResult {
        method: "turn/start".to_owned(),
        native: native_ref_for_frame(&response),
        raw_request,
        raw_response: response.raw_payload,
        turn_id: id_map.turn_id(&native_thread_id, &native_turn_id),
        native_thread_id,
        native_turn_id,
        turn,
    })
}

fn parse_steer_response(
    raw_request: Value,
    response: CodexClientResponseFrame,
    id_map: &mut CodexIdMap,
) -> anyhow::Result<CodexTurnSteerResult> {
    let native_thread_id = string_field(&raw_request, "threadId").context("missing threadId")?;
    let expected_native_turn_id =
        string_field(&raw_request, "expectedTurnId").context("missing expectedTurnId")?;
    let native_turn_id =
        string_field(&response.result, "turnId").unwrap_or_else(|| expected_native_turn_id.clone());
    Ok(CodexTurnSteerResult {
        method: "turn/steer".to_owned(),
        native: native_ref_for_frame(&response),
        raw_request,
        raw_response: response.raw_payload,
        turn_id: id_map.turn_id(&native_thread_id, &native_turn_id),
        native_thread_id,
        expected_native_turn_id,
        native_turn_id,
    })
}

fn codex_input_from_universal(input: &UserInput) -> Vec<Value> {
    match input {
        UserInput::Text { text } => vec![codex_text_input(text)],
        UserInput::Blocks { blocks } => {
            let mapped = blocks
                .iter()
                .filter_map(|block| match (&block.kind, block.text.as_deref()) {
                    (_, Some(text)) if text.trim().is_empty() => None,
                    (agenter_core::ContentBlockKind::Image, Some(url)) => Some(json!({
                        "type": "image",
                        "url": url,
                    })),
                    (_, Some(text)) => Some(codex_text_input(text)),
                    _ => None,
                })
                .collect::<Vec<_>>();
            if mapped.is_empty() {
                vec![codex_text_input("")]
            } else {
                mapped
            }
        }
    }
}

fn codex_text_input(text: &str) -> Value {
    json!({
        "type": "text",
        "text": text,
        "textElements": [],
    })
}

fn codex_reasoning_effort(effort: &AgentReasoningEffort) -> String {
    match effort {
        AgentReasoningEffort::None => "none",
        AgentReasoningEffort::Minimal => "minimal",
        AgentReasoningEffort::Low => "low",
        AgentReasoningEffort::Medium => "medium",
        AgentReasoningEffort::High => "high",
        AgentReasoningEffort::Xhigh => "xhigh",
    }
    .to_owned()
}

fn codex_collaboration_mode(settings: &AgentTurnSettings) -> Option<Value> {
    let mode = settings.collaboration_mode.as_deref()?;
    let model = settings.model.as_deref()?.trim();
    if mode.trim().is_empty() || model.is_empty() {
        return None;
    }
    Some(json!({
        "mode": mode,
        "settings": {
            "model": model,
            "reasoning_effort": settings
                .reasoning_effort
                .as_ref()
                .map(codex_reasoning_effort),
            "developer_instructions": null,
        },
    }))
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    value.get(field).and_then(Value::as_str).map(str::to_owned)
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agenter_core::{AgentReasoningEffort, ContentBlock, ContentBlockKind, SessionId};
    use serde_json::json;

    use super::*;
    use crate::agents::codex::transport::CodexTransportConfig;

    fn shell_transport(script: &str) -> CodexTransportConfig {
        CodexTransportConfig::command(
            "/bin/sh",
            ["-c", script],
            std::env::current_dir().expect("test process should have a current directory"),
        )
        .with_request_timeout(Duration::from_millis(500))
    }

    fn python_fake_server(assertions: &str, result: Value) -> String {
        let result = serde_json::to_string(&result).expect("fixture result should serialize");
        format!(
            r#"python3 -c 'import json, sys
req = json.loads(sys.stdin.readline())
params = req["params"]
{assertions}
result = json.loads({result:?})
print(json.dumps({{"id": req["id"], "result": result}}), flush=True)
'"#,
            assertions = assertions,
            result = result,
        )
    }

    #[tokio::test]
    async fn codex_turn_commands_start_payload_response_extra_and_raw_are_typed() {
        let script = python_fake_server(
            r#"assert req["method"] == "turn/start", req
assert params["threadId"] == "thread-1", params
assert params["input"][0]["type"] == "text", params
assert params["input"][0]["text"] == "implement this", params
assert params["model"] == "gpt-5", params
assert params["effort"] == "high", params
assert params["collaborationMode"] == {"mode":"plan","settings":{"model":"gpt-5","reasoning_effort":"high","developer_instructions":None}}, params
assert params["approvalPolicy"] == {"type": "never"}, params
assert params["modelProvider"] == "openai", params
assert params["dynamicTools"] == [{"name": "native-only"}], params
"#,
            json!({
                "turn": {
                    "id": "turn-1",
                    "items": [],
                    "status": "inProgress",
                    "error": null,
                    "startedAt": 1_700_000_000,
                    "completedAt": null,
                    "durationMs": null
                },
                "debugEcho": {"raw": true}
            }),
        );
        let mut extra = Map::new();
        extra.insert("approvalPolicy".to_owned(), json!({"type": "never"}));
        extra.insert("modelProvider".to_owned(), json!("openai"));
        extra.insert(
            "dynamicTools".to_owned(),
            json!([{ "name": "native-only" }]),
        );
        let request = start_request_from_universal(
            "thread-1",
            &UserInput::Text {
                text: "implement this".to_owned(),
            },
            Some(&AgentTurnSettings {
                model: Some("gpt-5".to_owned()),
                reasoning_effort: Some(AgentReasoningEffort::High),
                collaboration_mode: Some("plan".to_owned()),
            }),
            extra,
        );
        let mut transport = CodexTransport::spawn(shell_transport(&script)).unwrap();
        let mut id_map = CodexIdMap::for_session(SessionId::new());
        let mut client = CodexTurnClient::new(&mut transport);

        let result = client.start_turn(request, &mut id_map).await.unwrap();

        assert_eq!(result.method, "turn/start");
        assert_eq!(result.native_thread_id, "thread-1");
        assert_eq!(result.native_turn_id, "turn-1");
        assert_eq!(result.raw_request["approvalPolicy"]["type"], "never");
        assert_eq!(result.raw_request["dynamicTools"][0]["name"], "native-only");
        assert_eq!(result.raw_response["result"]["debugEcho"]["raw"], true);
        assert_eq!(
            result.native.raw_payload.as_ref().unwrap()["result"]["turn"]["id"],
            "turn-1"
        );
        transport.shutdown().await.ok();
    }

    #[tokio::test]
    async fn codex_turn_commands_steer_payload_response_extra_and_raw_are_typed() {
        let script = python_fake_server(
            r#"assert req["method"] == "turn/steer", req
assert params["threadId"] == "thread-1", params
assert params["expectedTurnId"] == "turn-1", params
assert params["input"][0]["type"] == "text", params
assert params["input"][0]["text"] == "steer now", params
assert params["responsesapiClientMetadata"] == {"source": "test"}, params
"#,
            json!({
                "turnId": "turn-1",
                "accepted": true
            }),
        );
        let mut extra = Map::new();
        extra.insert(
            "responsesapiClientMetadata".to_owned(),
            json!({"source": "test"}),
        );
        let request = steer_request_from_universal(
            "thread-1",
            "turn-1",
            &UserInput::Blocks {
                blocks: vec![ContentBlock {
                    block_id: "block-1".to_owned(),
                    kind: ContentBlockKind::Text,
                    text: Some("steer now".to_owned()),
                    mime_type: None,
                    artifact_id: None,
                }],
            },
            extra,
        );
        let mut transport = CodexTransport::spawn(shell_transport(&script)).unwrap();
        let mut id_map = CodexIdMap::for_session(SessionId::new());
        let mut client = CodexTurnClient::new(&mut transport);

        let result = client.steer_turn(request, &mut id_map).await.unwrap();

        assert_eq!(result.method, "turn/steer");
        assert_eq!(result.expected_native_turn_id, "turn-1");
        assert_eq!(result.native_turn_id, "turn-1");
        assert_eq!(
            result.raw_request["responsesapiClientMetadata"]["source"],
            "test"
        );
        assert_eq!(result.raw_response["result"]["accepted"], true);
        assert_eq!(
            result.native.raw_payload.as_ref().unwrap()["result"]["turnId"],
            "turn-1"
        );
        transport.shutdown().await.ok();
    }

    #[tokio::test]
    async fn codex_turn_commands_interrupt_payload_response_and_raw_are_typed() {
        let script = python_fake_server(
            r#"assert req["method"] == "turn/interrupt", req
assert params["threadId"] == "thread-1", params
assert params["turnId"] == "turn-1", params
"#,
            json!({
                "interrupted": true
            }),
        );
        let request = interrupt_request_from_universal("thread-1", "turn-1");
        let mut transport = CodexTransport::spawn(shell_transport(&script)).unwrap();
        let mut client = CodexTurnClient::new(&mut transport);

        let result = client.interrupt_turn(request).await.unwrap();

        assert_eq!(result.method, "turn/interrupt");
        assert_eq!(result.native_thread_id, "thread-1");
        assert_eq!(result.native_turn_id, "turn-1");
        assert_eq!(result.raw_request["turnId"], "turn-1");
        assert_eq!(result.raw_response["result"]["interrupted"], true);
        assert_eq!(
            result.native.raw_payload.as_ref().unwrap()["result"]["interrupted"],
            true
        );
        transport.shutdown().await.ok();
    }

    #[test]
    fn codex_turn_commands_dynamic_tools_are_deferred_not_a_typed_field() {
        let request = start_request_from_universal(
            "thread-1",
            &UserInput::Text {
                text: "hello".to_owned(),
            },
            None,
            Map::new(),
        );
        let value = serde_json::to_value(request).unwrap();
        assert!(value.get("dynamicTools").is_none());

        let mut extra = Map::new();
        extra.insert("dynamicTools".to_owned(), json!([{ "name": "preserved" }]));
        let preserved = serde_json::to_value(start_request_from_universal(
            "thread-1",
            &UserInput::Text {
                text: "hello".to_owned(),
            },
            None,
            extra,
        ))
        .unwrap();
        assert_eq!(preserved["dynamicTools"][0]["name"], "preserved");
    }

    #[test]
    fn codex_turn_commands_native_ref_helper_preserves_raw_payload() {
        let native = native_ref_for_turn_command_result(
            "turn/start",
            Some("1".to_owned()),
            json!({"id": 1, "result": {"turn": {"id": "turn-1"}}}),
        );
        assert_eq!(native.protocol, CODEX_APP_SERVER_PROTOCOL);
        assert_eq!(native.method.as_deref(), Some("turn/start"));
        assert_eq!(native.native_id.as_deref(), Some("1"));
        assert_eq!(
            native.raw_payload.unwrap()["result"]["turn"]["id"],
            "turn-1"
        );
    }
}
