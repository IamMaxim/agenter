#![allow(dead_code)]

use std::path::PathBuf;

use agenter_core::{ItemId, NativeRef, SessionStatus, TurnId};
use anyhow::Context;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Map, Value};

use super::{
    codec::{native_ref_for_frame, CodexClientResponseFrame, CODEX_APP_SERVER_PROTOCOL},
    id_map::CodexIdMap,
    transport::CodexTransport,
};

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadStartRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ephemeral: Option<bool>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub persist_extended_history: bool,
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadResumeRequest {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exclude_turns: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub persist_extended_history: bool,
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadForkRequest {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<PathBuf>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_provider: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<String>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub ephemeral: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub exclude_turns: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub persist_extended_history: bool,
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadListRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_key: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_direction: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model_providers: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub source_kinds: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub archived: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cwd: Option<Value>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub use_state_db_only: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub search_term: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CodexThreadTurnsListRequest {
    pub thread_id: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cursor: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub limit: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sort_direction: Option<String>,
    #[serde(flatten, default, skip_serializing_if = "Map::is_empty")]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexThreadOperation {
    pub method: String,
    pub native: NativeRef,
    pub raw_response: Value,
    pub thread: CodexThread,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexThreadList {
    pub native: NativeRef,
    pub raw_response: Value,
    pub threads: Vec<CodexThread>,
    pub next_cursor: Option<String>,
    pub backwards_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexThreadTurnsPage {
    pub native: NativeRef,
    pub raw_response: Value,
    pub thread_id: String,
    pub turns: Vec<CodexTurn>,
    pub next_cursor: Option<String>,
    pub backwards_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexLoadedThreads {
    pub native: NativeRef,
    pub raw_response: Value,
    pub thread_ids: Vec<String>,
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexCommandAck {
    pub method: String,
    pub native: NativeRef,
    pub raw_response: Value,
    pub status: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexThread {
    pub native_thread_id: String,
    pub title: Option<String>,
    pub preview: Option<String>,
    pub name: Option<String>,
    pub forked_from_id: Option<String>,
    pub cwd: Option<PathBuf>,
    pub path: Option<PathBuf>,
    pub model: Option<String>,
    pub model_provider: Option<String>,
    pub created_at: Option<DateTime<Utc>>,
    pub updated_at: Option<DateTime<Utc>>,
    pub status: SessionStatus,
    pub source: Option<String>,
    pub agent_nickname: Option<String>,
    pub agent_role: Option<String>,
    pub git_info: Option<CodexGitInfo>,
    pub raw_payload: Value,
    pub turns: Vec<CodexTurn>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexGitInfo {
    pub sha: Option<String>,
    pub branch: Option<String>,
    pub origin_url: Option<String>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexTurn {
    pub native_turn_id: String,
    pub turn_id: TurnId,
    pub status: Option<String>,
    pub started_at: Option<DateTime<Utc>>,
    pub completed_at: Option<DateTime<Utc>>,
    pub raw_payload: Value,
    pub items: Vec<CodexHistoryItem>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexHistoryItem {
    pub native_item_id: String,
    pub item_id: ItemId,
    pub kind: Option<String>,
    pub raw_payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexTokenUsageUpdate {
    pub thread_id: String,
    pub turn_id: String,
    pub raw_payload: Value,
    pub usage: Value,
}

pub struct CodexSessionClient<'a> {
    transport: &'a mut CodexTransport,
}

impl<'a> CodexSessionClient<'a> {
    pub fn new(transport: &'a mut CodexTransport) -> Self {
        Self { transport }
    }

    pub async fn start_thread(
        &mut self,
        request: CodexThreadStartRequest,
        id_map: &mut CodexIdMap,
    ) -> anyhow::Result<CodexThreadOperation> {
        let response = self
            .transport
            .request_response("thread/start", serde_json::to_value(request)?)
            .await?;
        parse_thread_operation("thread/start", response, id_map)
    }

    pub async fn resume_thread(
        &mut self,
        request: CodexThreadResumeRequest,
        id_map: &mut CodexIdMap,
    ) -> anyhow::Result<CodexThreadOperation> {
        let response = self
            .transport
            .request_response("thread/resume", serde_json::to_value(request)?)
            .await?;
        parse_thread_operation("thread/resume", response, id_map)
    }

    pub async fn fork_thread(
        &mut self,
        request: CodexThreadForkRequest,
        id_map: &mut CodexIdMap,
    ) -> anyhow::Result<CodexThreadOperation> {
        let response = self
            .transport
            .request_response("thread/fork", serde_json::to_value(request)?)
            .await?;
        parse_thread_operation("thread/fork", response, id_map)
    }

    pub async fn list_threads(
        &mut self,
        request: CodexThreadListRequest,
        id_map: &mut CodexIdMap,
    ) -> anyhow::Result<CodexThreadList> {
        let response = self
            .transport
            .request_response("thread/list", serde_json::to_value(request)?)
            .await?;
        parse_thread_list(response, id_map)
    }

    pub async fn list_loaded_threads(
        &mut self,
        cursor: Option<String>,
        limit: Option<u32>,
    ) -> anyhow::Result<CodexLoadedThreads> {
        let response = self
            .transport
            .request_response(
                "thread/loaded/list",
                json!({
                    "cursor": cursor,
                    "limit": limit,
                }),
            )
            .await?;
        parse_loaded_threads(response)
    }

    pub async fn read_thread(
        &mut self,
        thread_id: &str,
        include_turns: bool,
        id_map: &mut CodexIdMap,
    ) -> anyhow::Result<CodexThreadOperation> {
        let response = self
            .transport
            .request_response(
                "thread/read",
                json!({
                    "threadId": thread_id,
                    "includeTurns": include_turns,
                }),
            )
            .await?;
        parse_thread_operation("thread/read", response, id_map)
    }

    pub async fn list_turns(
        &mut self,
        request: CodexThreadTurnsListRequest,
        id_map: &mut CodexIdMap,
    ) -> anyhow::Result<CodexThreadTurnsPage> {
        let thread_id = request.thread_id.clone();
        let response = self
            .transport
            .request_response("thread/turns/list", serde_json::to_value(request)?)
            .await?;
        parse_turns_page(response, &thread_id, id_map)
    }

    pub async fn archive_thread(&mut self, thread_id: &str) -> anyhow::Result<CodexCommandAck> {
        self.thread_id_ack("thread/archive", thread_id).await
    }

    pub async fn unarchive_thread(
        &mut self,
        thread_id: &str,
        id_map: &mut CodexIdMap,
    ) -> anyhow::Result<CodexThreadOperation> {
        let response = self
            .transport
            .request_response("thread/unarchive", json!({ "threadId": thread_id }))
            .await?;
        parse_thread_operation("thread/unarchive", response, id_map)
    }

    pub async fn unsubscribe_thread(&mut self, thread_id: &str) -> anyhow::Result<CodexCommandAck> {
        self.thread_id_ack("thread/unsubscribe", thread_id).await
    }

    pub async fn close_thread(&mut self, thread_id: &str) -> anyhow::Result<CodexCommandAck> {
        self.unsubscribe_thread(thread_id).await
    }

    pub async fn set_thread_name(
        &mut self,
        thread_id: &str,
        name: &str,
    ) -> anyhow::Result<CodexCommandAck> {
        self.ack(
            "thread/name/set",
            json!({
                "threadId": thread_id,
                "name": name,
            }),
        )
        .await
    }

    pub async fn set_thread_goal(
        &mut self,
        thread_id: &str,
        goal_patch: Value,
    ) -> anyhow::Result<CodexCommandAck> {
        let mut params = object_or_empty(goal_patch);
        params.insert("threadId".to_owned(), Value::String(thread_id.to_owned()));
        self.ack("thread/goal/set", Value::Object(params)).await
    }

    pub async fn get_thread_goal(&mut self, thread_id: &str) -> anyhow::Result<CodexCommandAck> {
        self.thread_id_ack("thread/goal/get", thread_id).await
    }

    pub async fn clear_thread_goal(&mut self, thread_id: &str) -> anyhow::Result<CodexCommandAck> {
        self.thread_id_ack("thread/goal/clear", thread_id).await
    }

    pub async fn update_thread_metadata(
        &mut self,
        thread_id: &str,
        metadata_patch: Value,
        id_map: &mut CodexIdMap,
    ) -> anyhow::Result<CodexThreadOperation> {
        let mut params = object_or_empty(metadata_patch);
        params.insert("threadId".to_owned(), Value::String(thread_id.to_owned()));
        let response = self
            .transport
            .request_response("thread/metadata/update", Value::Object(params))
            .await?;
        parse_thread_operation("thread/metadata/update", response, id_map)
    }

    async fn thread_id_ack(
        &mut self,
        method: &str,
        thread_id: &str,
    ) -> anyhow::Result<CodexCommandAck> {
        self.ack(method, json!({ "threadId": thread_id })).await
    }

    async fn ack(&mut self, method: &str, params: Value) -> anyhow::Result<CodexCommandAck> {
        let response = self.transport.request_response(method, params).await?;
        Ok(CodexCommandAck {
            method: method.to_owned(),
            native: native_ref_for_frame(&response),
            status: response
                .result
                .get("status")
                .and_then(Value::as_str)
                .map(str::to_owned),
            raw_response: response.raw_payload,
        })
    }
}

impl super::codec::CodexNativeFrame for CodexClientResponseFrame {
    fn native_method(&self) -> Option<&str> {
        self.method.as_deref()
    }

    fn native_kind(&self) -> &'static str {
        "client_response"
    }

    fn native_id(&self) -> Option<String> {
        Some(self.request_id.to_string())
    }

    fn raw_payload(&self) -> Option<&Value> {
        Some(&self.raw_payload)
    }
}

pub fn parse_status_notification(raw_payload: Value) -> anyhow::Result<(String, SessionStatus)> {
    let params = raw_payload
        .get("params")
        .context("Codex status notification is missing params")?;
    let thread_id = string_field(params, "threadId")
        .context("Codex status notification is missing threadId")?;
    let status = params
        .get("status")
        .map(codex_status_to_session_status)
        .unwrap_or(SessionStatus::Degraded);
    Ok((thread_id, status))
}

pub fn parse_token_usage_notification(raw_payload: Value) -> anyhow::Result<CodexTokenUsageUpdate> {
    let params = raw_payload
        .get("params")
        .context("Codex token usage notification is missing params")?;
    Ok(CodexTokenUsageUpdate {
        thread_id: string_field(params, "threadId").context("missing threadId")?,
        turn_id: string_field(params, "turnId").context("missing turnId")?,
        usage: params.get("tokenUsage").cloned().unwrap_or(Value::Null),
        raw_payload,
    })
}

fn parse_thread_operation(
    method: &str,
    response: CodexClientResponseFrame,
    id_map: &mut CodexIdMap,
) -> anyhow::Result<CodexThreadOperation> {
    let thread_value = response
        .result
        .get("thread")
        .cloned()
        .context("Codex thread response did not include `thread`")?;
    let model = response
        .result
        .get("model")
        .and_then(Value::as_str)
        .map(str::to_owned);
    let model_provider = response
        .result
        .get("modelProvider")
        .and_then(Value::as_str)
        .map(str::to_owned);
    Ok(CodexThreadOperation {
        method: method.to_owned(),
        native: native_ref_for_frame(&response),
        raw_response: response.raw_payload,
        thread: parse_thread(thread_value, model, model_provider, id_map)?,
    })
}

fn parse_thread_list(
    response: CodexClientResponseFrame,
    id_map: &mut CodexIdMap,
) -> anyhow::Result<CodexThreadList> {
    let threads = response
        .result
        .get("data")
        .and_then(Value::as_array)
        .context("Codex thread/list response did not include `data`")?
        .iter()
        .cloned()
        .map(|thread| parse_thread(thread, None, None, id_map))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(CodexThreadList {
        native: native_ref_for_frame(&response),
        next_cursor: optional_string(response.result.get("nextCursor")),
        backwards_cursor: optional_string(response.result.get("backwardsCursor")),
        raw_response: response.raw_payload,
        threads,
    })
}

fn parse_loaded_threads(response: CodexClientResponseFrame) -> anyhow::Result<CodexLoadedThreads> {
    let thread_ids = response
        .result
        .get("data")
        .and_then(Value::as_array)
        .context("Codex thread/loaded/list response did not include `data`")?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    Ok(CodexLoadedThreads {
        native: native_ref_for_frame(&response),
        next_cursor: optional_string(response.result.get("nextCursor")),
        raw_response: response.raw_payload,
        thread_ids,
    })
}

fn parse_turns_page(
    response: CodexClientResponseFrame,
    thread_id: &str,
    id_map: &mut CodexIdMap,
) -> anyhow::Result<CodexThreadTurnsPage> {
    let turns = response
        .result
        .get("data")
        .and_then(Value::as_array)
        .context("Codex thread/turns/list response did not include `data`")?
        .iter()
        .cloned()
        .map(|turn| parse_turn(thread_id, turn, id_map))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(CodexThreadTurnsPage {
        native: native_ref_for_frame(&response),
        raw_response: response.raw_payload,
        thread_id: thread_id.to_owned(),
        turns,
        next_cursor: optional_string(response.result.get("nextCursor")),
        backwards_cursor: optional_string(response.result.get("backwardsCursor")),
    })
}

fn parse_thread(
    raw_payload: Value,
    response_model: Option<String>,
    response_model_provider: Option<String>,
    id_map: &mut CodexIdMap,
) -> anyhow::Result<CodexThread> {
    let native_thread_id =
        string_field(&raw_payload, "id").context("Codex thread is missing `id`")?;
    let name = non_empty_string(raw_payload.get("name"));
    let preview = non_empty_string(raw_payload.get("preview"));
    let title = name.clone().or_else(|| preview.clone());
    let turns = raw_payload
        .get("turns")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .cloned()
        .map(|turn| parse_turn(&native_thread_id, turn, id_map))
        .collect::<anyhow::Result<Vec<_>>>()?;
    Ok(CodexThread {
        native_thread_id,
        title,
        preview,
        name,
        forked_from_id: optional_string(raw_payload.get("forkedFromId")),
        cwd: optional_path(raw_payload.get("cwd")),
        path: optional_path(raw_payload.get("path")),
        model: response_model,
        model_provider: response_model_provider
            .or_else(|| optional_string(raw_payload.get("modelProvider"))),
        created_at: timestamp_seconds(raw_payload.get("createdAt")),
        updated_at: timestamp_seconds(raw_payload.get("updatedAt")),
        status: raw_payload
            .get("status")
            .map(codex_status_to_session_status)
            .unwrap_or(SessionStatus::Degraded),
        source: raw_payload.get("source").map(source_to_string),
        agent_nickname: optional_string(raw_payload.get("agentNickname")),
        agent_role: optional_string(raw_payload.get("agentRole")),
        git_info: parse_git_info(raw_payload.get("gitInfo")),
        raw_payload,
        turns,
    })
}

fn parse_turn(
    native_thread_id: &str,
    raw_payload: Value,
    id_map: &mut CodexIdMap,
) -> anyhow::Result<CodexTurn> {
    let native_turn_id = string_field(&raw_payload, "id").context("Codex turn is missing `id`")?;
    let items = raw_payload
        .get("items")
        .and_then(Value::as_array)
        .into_iter()
        .flatten()
        .cloned()
        .filter_map(|item| parse_history_item(native_thread_id, item, id_map))
        .collect();
    Ok(CodexTurn {
        turn_id: id_map.turn_id(native_thread_id, &native_turn_id),
        native_turn_id,
        status: raw_payload.get("status").map(source_to_string),
        started_at: timestamp_seconds(raw_payload.get("startedAt")),
        completed_at: timestamp_seconds(raw_payload.get("completedAt")),
        raw_payload,
        items,
    })
}

fn parse_history_item(
    native_thread_id: &str,
    raw_payload: Value,
    id_map: &mut CodexIdMap,
) -> Option<CodexHistoryItem> {
    let native_item_id = raw_payload.get("id")?.as_str()?.to_owned();
    Some(CodexHistoryItem {
        item_id: id_map.item_id(native_thread_id, &native_item_id),
        native_item_id,
        kind: optional_string(raw_payload.get("type")),
        raw_payload,
    })
}

fn codex_status_to_session_status(status: &Value) -> SessionStatus {
    match status.get("type").and_then(Value::as_str) {
        Some("idle") => SessionStatus::Idle,
        Some("notLoaded") => SessionStatus::Stopped,
        Some("systemError") => SessionStatus::Failed,
        Some("active") => {
            let flags = status
                .get("activeFlags")
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

fn parse_git_info(value: Option<&Value>) -> Option<CodexGitInfo> {
    let value = value?;
    Some(CodexGitInfo {
        sha: optional_string(value.get("sha")),
        branch: optional_string(value.get("branch")),
        origin_url: optional_string(value.get("originUrl")),
    })
}

fn timestamp_seconds(value: Option<&Value>) -> Option<DateTime<Utc>> {
    DateTime::from_timestamp(value?.as_i64()?, 0)
}

fn optional_path(value: Option<&Value>) -> Option<PathBuf> {
    optional_string(value).map(PathBuf::from)
}

fn optional_string(value: Option<&Value>) -> Option<String> {
    value.and_then(Value::as_str).map(str::to_owned)
}

fn non_empty_string(value: Option<&Value>) -> Option<String> {
    optional_string(value).filter(|value| !value.trim().is_empty())
}

fn string_field(value: &Value, field: &str) -> Option<String> {
    optional_string(value.get(field))
}

fn source_to_string(value: &Value) -> String {
    match value {
        Value::String(value) => value.clone(),
        Value::Object(object) => object
            .get("type")
            .and_then(Value::as_str)
            .map(str::to_owned)
            .unwrap_or_else(|| value.to_string()),
        other => other.to_string(),
    }
}

fn object_or_empty(value: Value) -> Map<String, Value> {
    match value {
        Value::Object(object) => object,
        _ => Map::new(),
    }
}

#[must_use]
pub fn native_ref_for_lifecycle_payload(method: &str, kind: &str, raw_payload: Value) -> NativeRef {
    NativeRef {
        protocol: CODEX_APP_SERVER_PROTOCOL.to_owned(),
        method: Some(method.to_owned()),
        kind: Some(kind.to_owned()),
        native_id: None,
        summary: None,
        hash: None,
        pointer: None,
        raw_payload: Some(raw_payload),
    }
}

#[cfg(test)]
mod tests {
    use std::time::Duration;

    use agenter_core::{SessionId, SessionStatus};
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

    fn sample_thread(id: &str, name: Option<&str>, preview: &str) -> Value {
        json!({
            "id": id,
            "forkedFromId": null,
            "preview": preview,
            "ephemeral": false,
            "modelProvider": "openai",
            "createdAt": 1_700_000_000,
            "updatedAt": 1_700_000_010,
            "status": { "type": "idle" },
            "path": "/tmp/rollout.jsonl",
            "cwd": "/workspace",
            "cliVersion": "0.0.0-test",
            "source": { "type": "appServer" },
            "agentNickname": "Planner",
            "agentRole": "implementer",
            "gitInfo": {
                "sha": "abc",
                "branch": "main",
                "originUrl": "https://example.invalid/repo.git"
            },
            "name": name,
            "turns": []
        })
    }

    fn sample_thread_with_turns(
        id: &str,
        name: Option<&str>,
        preview: &str,
        turns: Vec<Value>,
    ) -> Value {
        let mut thread = sample_thread(id, name, preview);
        thread["turns"] = Value::Array(turns);
        thread
    }

    #[tokio::test]
    async fn codex_session_lifecycle_create_resume_and_fork_preserve_metadata_and_raw_payloads() {
        let script = format!(
            r#"read line
printf '%s\n' '{}'
read line
printf '%s\n' '{}'
read line
printf '%s\n' '{}'
"#,
            json!({
                "id": 1,
                "result": {
                    "thread": sample_thread("thread-started", Some("Human title"), "UUID should lose"),
                    "model": "gpt-5",
                    "modelProvider": "openai",
                    "cwd": "/workspace"
                }
            }),
            json!({
                "id": 2,
                "result": {
                    "thread": sample_thread("thread-started", Some("Human title"), "UUID should lose"),
                    "model": "gpt-5",
                    "modelProvider": "openai",
                    "cwd": "/workspace"
                }
            }),
            json!({
                "id": 3,
                "result": {
                    "thread": sample_thread("thread-forked", None, "Readable preview"),
                    "model": "gpt-5-mini",
                    "modelProvider": "openai",
                    "cwd": "/workspace"
                }
            })
        );
        let mut transport = CodexTransport::spawn(shell_transport(&script)).unwrap();
        let mut id_map = CodexIdMap::for_session(SessionId::new());
        let mut client = CodexSessionClient::new(&mut transport);

        let started = client
            .start_thread(
                CodexThreadStartRequest {
                    cwd: Some("/workspace".to_owned()),
                    persist_extended_history: true,
                    ..Default::default()
                },
                &mut id_map,
            )
            .await
            .unwrap();
        assert_eq!(started.method, "thread/start");
        assert_eq!(started.thread.native_thread_id, "thread-started");
        assert_eq!(started.thread.title.as_deref(), Some("Human title"));
        assert_eq!(
            started.thread.cwd.as_deref(),
            Some(std::path::Path::new("/workspace"))
        );
        assert_eq!(started.thread.model.as_deref(), Some("gpt-5"));
        assert_eq!(started.thread.model_provider.as_deref(), Some("openai"));
        assert_eq!(
            started.raw_response["result"]["thread"]["name"],
            "Human title"
        );
        assert_eq!(
            started.native.raw_payload.as_ref().unwrap()["result"]["thread"]["id"],
            "thread-started"
        );

        let resumed = client
            .resume_thread(
                CodexThreadResumeRequest {
                    thread_id: "thread-started".to_owned(),
                    exclude_turns: true,
                    ..Default::default()
                },
                &mut id_map,
            )
            .await
            .unwrap();
        assert_eq!(resumed.method, "thread/resume");
        assert_eq!(resumed.thread.title.as_deref(), Some("Human title"));

        let forked = client
            .fork_thread(
                CodexThreadForkRequest {
                    thread_id: "thread-started".to_owned(),
                    exclude_turns: true,
                    ..Default::default()
                },
                &mut id_map,
            )
            .await
            .unwrap();
        assert_eq!(forked.method, "thread/fork");
        assert_eq!(forked.thread.native_thread_id, "thread-forked");
        assert_eq!(forked.thread.title.as_deref(), Some("Readable preview"));
        transport.shutdown().await.ok();
    }

    #[tokio::test]
    async fn codex_session_lifecycle_archive_unarchive_and_unsubscribe_are_typed() {
        let script = format!(
            r#"read line
printf '%s\n' '{{"id":1,"result":{{}}}}'
read line
printf '%s\n' '{}'
read line
printf '%s\n' '{{"id":3,"result":{{"status":"unsubscribed"}}}}'
"#,
            json!({
                "id": 2,
                "result": {
                    "thread": sample_thread("thread-1", Some("Restored"), "preview")
                }
            })
        );
        let mut transport = CodexTransport::spawn(shell_transport(&script)).unwrap();
        let mut id_map = CodexIdMap::for_session(SessionId::new());
        let mut client = CodexSessionClient::new(&mut transport);

        let archived = client.archive_thread("thread-1").await.unwrap();
        assert_eq!(archived.method, "thread/archive");
        assert_eq!(archived.native.method.as_deref(), Some("thread/archive"));

        let unarchived = client
            .unarchive_thread("thread-1", &mut id_map)
            .await
            .unwrap();
        assert_eq!(unarchived.method, "thread/unarchive");
        assert_eq!(unarchived.thread.title.as_deref(), Some("Restored"));

        let unsubscribed = client.unsubscribe_thread("thread-1").await.unwrap();
        assert_eq!(unsubscribed.method, "thread/unsubscribe");
        assert_eq!(unsubscribed.status.as_deref(), Some("unsubscribed"));
        transport.shutdown().await.ok();
    }

    #[tokio::test]
    async fn codex_history_import_lists_reads_pages_and_keeps_stable_ids_and_raw_payloads() {
        let turn = json!({
            "id": "turn-1",
            "items": [
                { "type": "userMessage", "id": "item-user", "content": [] },
                { "type": "agentMessage", "id": "item-agent", "text": "hi" }
            ],
            "status": { "type": "completed" },
            "error": null,
            "startedAt": 1_700_000_000,
            "completedAt": 1_700_000_001,
            "durationMs": 1000
        });
        let script = format!(
            r#"read line
printf '%s\n' '{}'
read line
printf '%s\n' '{}'
read line
printf '%s\n' '{}'
read line
printf '%s\n' '{{"id":4,"result":{{"data":["thread-1"],"nextCursor":null}}}}'
"#,
            json!({
                "id": 1,
                "result": {
                    "data": [sample_thread("thread-1", Some("List name"), "list preview")],
                    "nextCursor": "next",
                    "backwardsCursor": "back"
                }
            }),
            json!({
                "id": 2,
                "result": {
                    "thread": sample_thread_with_turns(
                        "thread-1",
                        Some("Read name"),
                        "read preview",
                        vec![turn.clone()]
                    )
                }
            }),
            json!({
                "id": 3,
                "result": {
                    "data": [turn],
                    "nextCursor": null,
                    "backwardsCursor": "older"
                }
            })
        );
        let session_id = SessionId::new();
        let mut transport = CodexTransport::spawn(shell_transport(&script)).unwrap();
        let mut id_map = CodexIdMap::for_session(session_id);
        let mut client = CodexSessionClient::new(&mut transport);

        let listed = client
            .list_threads(
                CodexThreadListRequest {
                    limit: Some(10),
                    ..Default::default()
                },
                &mut id_map,
            )
            .await
            .unwrap();
        assert_eq!(listed.threads[0].title.as_deref(), Some("List name"));
        assert_eq!(listed.next_cursor.as_deref(), Some("next"));
        assert_eq!(listed.raw_response["result"]["data"][0]["id"], "thread-1");

        let read = client
            .read_thread("thread-1", true, &mut id_map)
            .await
            .unwrap();
        let read_turn_id = read.thread.turns[0].turn_id;
        let read_item_id = read.thread.turns[0].items[0].item_id;
        assert_eq!(read.thread.turns[0].raw_payload["id"], "turn-1");
        assert_eq!(
            read.thread.turns[0].items[0].raw_payload["type"],
            "userMessage"
        );

        let page = client
            .list_turns(
                CodexThreadTurnsListRequest {
                    thread_id: "thread-1".to_owned(),
                    limit: Some(50),
                    ..Default::default()
                },
                &mut id_map,
            )
            .await
            .unwrap();
        assert_eq!(page.turns[0].turn_id, read_turn_id);
        assert_eq!(page.turns[0].items[0].item_id, read_item_id);

        let loaded = client.list_loaded_threads(None, None).await.unwrap();
        assert_eq!(loaded.thread_ids, vec!["thread-1"]);
        transport.shutdown().await.ok();
    }

    #[test]
    fn codex_session_status_and_token_usage_notifications_parse_lifecycle_payloads() {
        let (thread_id, status) = parse_status_notification(json!({
            "method": "thread/status/changed",
            "params": {
                "threadId": "thread-1",
                "status": {
                    "type": "active",
                    "activeFlags": ["waitingOnApproval"]
                }
            }
        }))
        .unwrap();
        assert_eq!(thread_id, "thread-1");
        assert_eq!(status, SessionStatus::WaitingForApproval);

        let usage = parse_token_usage_notification(json!({
            "method": "thread/tokenUsage/updated",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "tokenUsage": {
                    "total": { "totalTokens": 10 }
                }
            }
        }))
        .unwrap();
        assert_eq!(usage.thread_id, "thread-1");
        assert_eq!(usage.usage["total"]["totalTokens"], 10);
        assert_eq!(usage.raw_payload["method"], "thread/tokenUsage/updated");
    }

    #[test]
    fn codex_session_titles_prefer_name_then_preview_and_never_need_uuid_fallback() {
        let mut id_map = CodexIdMap::for_session(SessionId::new());
        let named = parse_thread(
            sample_thread(
                "00000000-0000-0000-0000-000000000001",
                Some("Readable name"),
                "Preview",
            ),
            None,
            None,
            &mut id_map,
        )
        .unwrap();
        assert_eq!(named.title.as_deref(), Some("Readable name"));

        let preview = parse_thread(
            sample_thread(
                "00000000-0000-0000-0000-000000000001",
                None,
                "Readable preview",
            ),
            None,
            None,
            &mut id_map,
        )
        .unwrap();
        assert_eq!(preview.title.as_deref(), Some("Readable preview"));

        let untitled = parse_thread(
            sample_thread("00000000-0000-0000-0000-000000000001", None, ""),
            None,
            None,
            &mut id_map,
        )
        .unwrap();
        assert_eq!(untitled.title, None);
    }

    #[test]
    fn codex_session_requests_preserve_extra_native_params() {
        let mut extra = Map::new();
        extra.insert("approvalPolicy".to_owned(), json!("never"));
        extra.insert(
            "permissions".to_owned(),
            json!({
                "profile": "custom"
            }),
        );
        let start = serde_json::to_value(CodexThreadStartRequest {
            model: Some("gpt-5".to_owned()),
            extra,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(start["model"], "gpt-5");
        assert_eq!(start["approvalPolicy"], "never");
        assert_eq!(start["permissions"]["profile"], "custom");

        let mut list_extra = Map::new();
        list_extra.insert("futureFilter".to_owned(), json!({"native": true}));
        let list = serde_json::to_value(CodexThreadListRequest {
            sort_key: Some("updated_at".to_owned()),
            sort_direction: Some("desc".to_owned()),
            cwd: Some(json!(["/workspace", "/tmp"])),
            use_state_db_only: true,
            extra: list_extra,
            ..Default::default()
        })
        .unwrap();
        assert_eq!(list["sortKey"], "updated_at");
        assert_eq!(list["sortDirection"], "desc");
        assert_eq!(list["cwd"][0], "/workspace");
        assert_eq!(list["useStateDbOnly"], true);
        assert_eq!(list["futureFilter"]["native"], true);
    }
}
