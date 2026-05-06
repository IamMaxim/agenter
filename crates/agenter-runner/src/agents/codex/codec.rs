#![allow(dead_code)]

use std::collections::HashMap;
use std::fmt;

use agenter_core::NativeRef;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

pub const CODEX_APP_SERVER_PROTOCOL: &str = "codex/app-server/v2";

#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Deserialize, Serialize)]
#[serde(untagged)]
pub enum RequestId {
    String(String),
    Integer(i64),
}

impl fmt::Display for RequestId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::String(value) => f.write_str(value),
            Self::Integer(value) => write!(f, "{value}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
#[serde(untagged)]
enum JSONRPCMessage {
    Request(JSONRPCRequest),
    Notification(JSONRPCNotification),
    Response(JSONRPCResponse),
    Error(JSONRPCError),
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
struct JSONRPCRequest {
    pub id: RequestId,
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub trace: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
struct JSONRPCNotification {
    pub method: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
struct JSONRPCResponse {
    pub id: RequestId,
    pub result: Value,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
struct JSONRPCError {
    pub error: JSONRPCErrorError,
    pub id: RequestId,
}

#[derive(Debug, Clone, PartialEq, Deserialize, Serialize)]
pub struct JSONRPCErrorError {
    pub code: i64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexKnownClientResponse {
    pub method: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexKnownServerRequest {
    pub method: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CodexKnownServerNotification {
    pub method: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum CodexDecodedFrame {
    ClientResponse(CodexClientResponseFrame),
    ClientError(CodexClientErrorFrame),
    ServerRequest(CodexServerRequestFrame),
    ServerNotification(CodexServerNotificationFrame),
    Malformed(CodexMalformedFrame),
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexClientResponseFrame {
    pub request_id: RequestId,
    pub method: Option<String>,
    pub result: Value,
    pub decoded: Option<CodexKnownClientResponse>,
    pub raw_payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexClientErrorFrame {
    pub request_id: RequestId,
    pub method: Option<String>,
    pub error: JSONRPCErrorError,
    pub raw_payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexServerRequestFrame {
    pub request_id: RequestId,
    pub method: String,
    pub decoded: Option<CodexKnownServerRequest>,
    pub raw_payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexServerNotificationFrame {
    pub method: String,
    pub decoded: Option<CodexKnownServerNotification>,
    pub raw_payload: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CodexMalformedFrame {
    pub line: String,
    pub error: String,
}

#[derive(Debug, Default)]
pub struct CodexCodec {
    next_request_id: i64,
    pending_methods: HashMap<RequestId, String>,
}

impl CodexCodec {
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_request_id: 1,
            pending_methods: HashMap::new(),
        }
    }

    pub fn encode_request(
        &mut self,
        method: impl Into<String>,
        params: Value,
    ) -> (RequestId, Value) {
        let request_id = RequestId::Integer(self.next_request_id);
        self.next_request_id += 1;
        self.encode_request_with_id(request_id, method, params)
    }

    pub fn encode_request_with_id(
        &mut self,
        request_id: RequestId,
        method: impl Into<String>,
        params: Value,
    ) -> (RequestId, Value) {
        let method = method.into();
        self.pending_methods
            .insert(request_id.clone(), method.clone());
        (
            request_id.clone(),
            json!({
                "id": request_id,
                "method": method,
                "params": params,
            }),
        )
    }

    #[must_use]
    pub fn encode_response(request_id: RequestId, result: Value) -> Value {
        json!({
            "id": request_id,
            "result": result,
        })
    }

    pub fn decode_line(&mut self, line: &str) -> CodexDecodedFrame {
        let trimmed = line.trim_end_matches(['\r', '\n']);
        let raw_payload = match serde_json::from_str::<Value>(trimmed) {
            Ok(value) => value,
            Err(error) => {
                return CodexDecodedFrame::Malformed(CodexMalformedFrame {
                    line: line.to_owned(),
                    error: error.to_string(),
                });
            }
        };
        let message = match serde_json::from_value::<JSONRPCMessage>(raw_payload.clone()) {
            Ok(message) => message,
            Err(error) => {
                return CodexDecodedFrame::Malformed(CodexMalformedFrame {
                    line: line.to_owned(),
                    error: error.to_string(),
                });
            }
        };
        match message {
            JSONRPCMessage::Response(response) => self.decode_response(response, raw_payload),
            JSONRPCMessage::Error(error) => self.decode_error(error, raw_payload),
            JSONRPCMessage::Request(request) => decode_server_request(request, raw_payload),
            JSONRPCMessage::Notification(notification) => {
                decode_server_notification(notification, raw_payload)
            }
        }
    }

    fn decode_response(
        &mut self,
        response: JSONRPCResponse,
        raw_payload: Value,
    ) -> CodexDecodedFrame {
        let method = self.pending_methods.remove(&response.id);
        let decoded = method
            .as_ref()
            .filter(|method| is_known_client_request_method(method))
            .map(|method| CodexKnownClientResponse {
                method: method.clone(),
            });
        CodexDecodedFrame::ClientResponse(CodexClientResponseFrame {
            request_id: response.id,
            method,
            result: response.result,
            decoded,
            raw_payload,
        })
    }

    fn decode_error(&mut self, error: JSONRPCError, raw_payload: Value) -> CodexDecodedFrame {
        let method = self.pending_methods.remove(&error.id);
        CodexDecodedFrame::ClientError(CodexClientErrorFrame {
            request_id: error.id,
            method,
            error: error.error,
            raw_payload,
        })
    }
}

fn decode_server_request(request: JSONRPCRequest, raw_payload: Value) -> CodexDecodedFrame {
    let decoded =
        is_known_server_request_method(&request.method).then(|| CodexKnownServerRequest {
            method: request.method.clone(),
        });
    CodexDecodedFrame::ServerRequest(CodexServerRequestFrame {
        request_id: request.id,
        method: request.method,
        decoded,
        raw_payload,
    })
}

fn decode_server_notification(
    notification: JSONRPCNotification,
    raw_payload: Value,
) -> CodexDecodedFrame {
    let method = notification.method.clone();
    let decoded =
        is_known_server_notification_method(&method).then(|| CodexKnownServerNotification {
            method: method.clone(),
        });
    CodexDecodedFrame::ServerNotification(CodexServerNotificationFrame {
        method,
        decoded,
        raw_payload,
    })
}

#[must_use]
pub fn is_known_client_request_method(method: &str) -> bool {
    matches!(
        method,
        "initialize"
            | "thread/start"
            | "thread/resume"
            | "thread/fork"
            | "thread/archive"
            | "thread/unsubscribe"
            | "thread/increment_elicitation"
            | "thread/decrement_elicitation"
            | "thread/name/set"
            | "thread/goal/set"
            | "thread/goal/get"
            | "thread/goal/clear"
            | "thread/metadata/update"
            | "thread/memoryMode/set"
            | "memory/reset"
            | "thread/unarchive"
            | "thread/compact/start"
            | "thread/shellCommand"
            | "thread/approveGuardianDeniedAction"
            | "thread/backgroundTerminals/clean"
            | "thread/rollback"
            | "thread/list"
            | "thread/loaded/list"
            | "thread/read"
            | "thread/turns/list"
            | "thread/inject_items"
            | "skills/list"
            | "hooks/list"
            | "marketplace/add"
            | "marketplace/remove"
            | "marketplace/upgrade"
            | "plugin/list"
            | "plugin/read"
            | "app/list"
            | "device/key/create"
            | "device/key/public"
            | "device/key/sign"
            | "fs/readFile"
            | "fs/writeFile"
            | "fs/createDirectory"
            | "fs/getMetadata"
            | "fs/readDirectory"
            | "fs/remove"
            | "fs/copy"
            | "fs/watch"
            | "fs/unwatch"
            | "skills/config/write"
            | "plugin/install"
            | "plugin/uninstall"
            | "turn/start"
            | "turn/steer"
            | "turn/interrupt"
            | "thread/realtime/start"
            | "thread/realtime/appendAudio"
            | "thread/realtime/appendText"
            | "thread/realtime/stop"
            | "thread/realtime/listVoices"
            | "review/start"
            | "model/list"
            | "modelProvider/capabilities/read"
            | "experimentalFeature/list"
            | "experimentalFeature/enablement/set"
            | "collaborationMode/list"
            | "mock/experimentalMethod"
            | "mcpServer/oauth/login"
            | "config/mcpServer/reload"
            | "mcpServerStatus/list"
            | "mcpServer/resource/read"
            | "mcpServer/tool/call"
            | "windowsSandbox/setupStart"
            | "account/login/start"
            | "account/login/cancel"
            | "account/logout"
            | "account/rateLimits/read"
            | "account/sendAddCreditsNudgeEmail"
            | "feedback/upload"
            | "command/exec"
            | "command/exec/write"
            | "command/exec/terminate"
            | "command/exec/resize"
            | "config/read"
            | "externalAgentConfig/detect"
            | "externalAgentConfig/import"
            | "config/value/write"
            | "config/batchWrite"
            | "configRequirements/read"
            | "account/read"
            | "GetConversationSummary"
            | "GitDiffToRemote"
            | "GetAuthStatus"
            | "FuzzyFileSearch"
            | "fuzzyFileSearch/sessionStart"
            | "fuzzyFileSearch/sessionUpdate"
            | "fuzzyFileSearch/sessionStop"
    )
}

#[must_use]
pub fn is_known_server_request_method(method: &str) -> bool {
    matches!(
        method,
        "item/commandExecution/requestApproval"
            | "item/fileChange/requestApproval"
            | "item/tool/requestUserInput"
            | "mcpServer/elicitation/request"
            | "item/permissions/requestApproval"
            | "item/tool/call"
            | "account/chatgptAuthTokens/refresh"
            | "ApplyPatchApproval"
            | "ExecCommandApproval"
    )
}

#[must_use]
pub fn is_known_server_notification_method(method: &str) -> bool {
    matches!(
        method,
        "error"
            | "thread/started"
            | "thread/status/changed"
            | "thread/archived"
            | "thread/unarchived"
            | "thread/closed"
            | "skills/changed"
            | "thread/name/updated"
            | "thread/goal/updated"
            | "thread/goal/cleared"
            | "thread/tokenUsage/updated"
            | "turn/started"
            | "hook/started"
            | "turn/completed"
            | "hook/completed"
            | "turn/diff/updated"
            | "turn/plan/updated"
            | "item/started"
            | "item/autoApprovalReview/started"
            | "item/autoApprovalReview/completed"
            | "item/completed"
            | "rawResponseItem/completed"
            | "item/agentMessage/delta"
            | "item/plan/delta"
            | "command/exec/outputDelta"
            | "item/commandExecution/outputDelta"
            | "item/commandExecution/terminalInteraction"
            | "item/fileChange/outputDelta"
            | "item/fileChange/patchUpdated"
            | "serverRequest/resolved"
            | "item/mcpToolCall/progress"
            | "mcpServer/oauthLogin/completed"
            | "mcpServer/startupStatus/updated"
            | "account/updated"
            | "account/rateLimits/updated"
            | "app/list/updated"
            | "remoteControl/status/changed"
            | "externalAgentConfig/import/completed"
            | "fs/changed"
            | "item/reasoning/summaryTextDelta"
            | "item/reasoning/summaryPartAdded"
            | "item/reasoning/textDelta"
            | "thread/compacted"
            | "model/rerouted"
            | "model/verification"
            | "warning"
            | "guardianWarning"
            | "deprecationNotice"
            | "configWarning"
            | "fuzzyFileSearch/sessionUpdated"
            | "fuzzyFileSearch/sessionCompleted"
            | "thread/realtime/started"
            | "thread/realtime/itemAdded"
            | "thread/realtime/transcript/delta"
            | "thread/realtime/transcript/done"
            | "thread/realtime/outputAudio/delta"
            | "thread/realtime/sdp"
            | "thread/realtime/error"
            | "thread/realtime/closed"
            | "windows/worldWritableWarning"
            | "windowsSandbox/setupCompleted"
            | "account/login/completed"
    )
}

pub trait CodexNativeFrame {
    fn native_method(&self) -> Option<&str>;
    fn native_kind(&self) -> &'static str;
    fn native_id(&self) -> Option<String>;
    fn raw_payload(&self) -> Option<&Value>;
}

impl CodexNativeFrame for CodexDecodedFrame {
    fn native_method(&self) -> Option<&str> {
        match self {
            Self::ClientResponse(frame) => frame.method.as_deref(),
            Self::ClientError(frame) => frame.method.as_deref(),
            Self::ServerRequest(frame) => Some(&frame.method),
            Self::ServerNotification(frame) => Some(&frame.method),
            Self::Malformed(_) => None,
        }
    }

    fn native_kind(&self) -> &'static str {
        match self {
            Self::ClientResponse(_) => "client_response",
            Self::ClientError(_) => "client_error",
            Self::ServerRequest(_) => "server_request",
            Self::ServerNotification(_) => "server_notification",
            Self::Malformed(_) => "malformed",
        }
    }

    fn native_id(&self) -> Option<String> {
        match self {
            Self::ClientResponse(frame) => Some(frame.request_id.to_string()),
            Self::ClientError(frame) => Some(frame.request_id.to_string()),
            Self::ServerRequest(frame) => Some(frame.request_id.to_string()),
            Self::ServerNotification(_) | Self::Malformed(_) => None,
        }
    }

    fn raw_payload(&self) -> Option<&Value> {
        match self {
            Self::ClientResponse(frame) => Some(&frame.raw_payload),
            Self::ClientError(frame) => Some(&frame.raw_payload),
            Self::ServerRequest(frame) => Some(&frame.raw_payload),
            Self::ServerNotification(frame) => Some(&frame.raw_payload),
            Self::Malformed(_) => None,
        }
    }
}

#[must_use]
pub fn native_ref_for_frame(frame: &impl CodexNativeFrame) -> NativeRef {
    NativeRef {
        protocol: CODEX_APP_SERVER_PROTOCOL.to_owned(),
        method: frame.native_method().map(str::to_owned),
        kind: Some(frame.native_kind().to_owned()),
        native_id: frame.native_id(),
        summary: None,
        hash: None,
        pointer: None,
        raw_payload: frame.raw_payload().cloned(),
    }
}

#[must_use]
pub fn native_ref_for_decoded_frame(frame: &CodexDecodedFrame) -> NativeRef {
    let mut native = native_ref_for_frame(frame);
    if let CodexDecodedFrame::Malformed(malformed) = frame {
        native.raw_payload = Some(json!({
            "line": malformed.line,
            "error": malformed.error,
        }));
    }
    native
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn codex_codec_correlates_client_response_with_pending_request_method_and_raw_payload() {
        let mut codec = CodexCodec::new();
        let (request_id, request) =
            codec.encode_request("model/list", json!({ "onlyAvailable": false }));

        assert_eq!(request["method"], "model/list");

        let line = json!({
            "id": request_id,
            "result": {
                "models": []
            }
        })
        .to_string();
        let frame = codec.decode_line(&line);

        let CodexDecodedFrame::ClientResponse(response) = &frame else {
            panic!("expected client response frame");
        };
        assert_eq!(response.method.as_deref(), Some("model/list"));
        assert_eq!(response.raw_payload["result"]["models"], json!([]));
        assert_eq!(
            response
                .decoded
                .as_ref()
                .map(|decoded| decoded.method.as_str()),
            Some("model/list")
        );

        let native = native_ref_for_decoded_frame(&frame);
        assert_eq!(native.protocol, CODEX_APP_SERVER_PROTOCOL);
        assert_eq!(native.method.as_deref(), Some("model/list"));
        assert_eq!(native.kind.as_deref(), Some("client_response"));
        assert_eq!(native.native_id.as_deref(), Some("1"));
        assert_eq!(native.raw_payload, Some(response.raw_payload.clone()));
    }

    #[test]
    fn codex_codec_decodes_server_request_and_preserves_native_id() {
        let mut codec = CodexCodec::new();
        let line = json!({
            "id": "approval-1",
            "method": "item/tool/call",
            "params": {
                "threadId": "thread-1",
                "turnId": "turn-1",
                "itemId": "item-1",
                "toolName": "client_tool",
                "arguments": {}
            }
        })
        .to_string();

        let frame = codec.decode_line(&line);
        let CodexDecodedFrame::ServerRequest(request) = &frame else {
            panic!("expected server request frame");
        };

        assert_eq!(request.method, "item/tool/call");
        assert_eq!(request.request_id.to_string(), "approval-1");
        assert!(request.decoded.is_some());
        assert_eq!(request.raw_payload["params"]["itemId"], "item-1");
        assert_eq!(
            native_ref_for_frame(&frame).native_id.as_deref(),
            Some("approval-1")
        );
    }

    #[test]
    fn codex_codec_decodes_notification_without_request_id() {
        let mut codec = CodexCodec::new();
        let line = json!({
            "method": "serverRequest/resolved",
            "params": {
                "requestId": 7
            }
        })
        .to_string();

        let frame = codec.decode_line(&line);
        let CodexDecodedFrame::ServerNotification(notification) = &frame else {
            panic!("expected server notification frame");
        };

        assert_eq!(notification.method, "serverRequest/resolved");
        assert!(notification.decoded.is_some());
        let native = native_ref_for_frame(&frame);
        assert_eq!(native.method.as_deref(), Some("serverRequest/resolved"));
        assert_eq!(native.kind.as_deref(), Some("server_notification"));
        assert_eq!(native.native_id, None);
        assert_eq!(native.raw_payload, Some(notification.raw_payload.clone()));
    }

    #[test]
    fn codex_codec_keeps_undecoded_native_frame_raw_payload() {
        let mut codec = CodexCodec::new();
        let line = json!({
            "id": 42,
            "method": "codex/new-native-method",
            "params": {
                "shape": "future"
            }
        })
        .to_string();

        let frame = codec.decode_line(&line);
        let CodexDecodedFrame::ServerRequest(request) = &frame else {
            panic!("expected undecoded server request frame");
        };

        assert_eq!(request.method, "codex/new-native-method");
        assert!(request.decoded.is_none());
        assert_eq!(request.raw_payload["params"]["shape"], "future");
        assert_eq!(
            native_ref_for_frame(&frame).raw_payload,
            Some(request.raw_payload.clone())
        );
    }

    #[test]
    fn codex_codec_reports_malformed_json_without_panicking() {
        let mut codec = CodexCodec::new();
        let frame = codec.decode_line("{ not json\n");

        let CodexDecodedFrame::Malformed(malformed) = frame else {
            panic!("expected malformed frame");
        };
        assert!(!malformed.error.is_empty());
        assert_eq!(malformed.line, "{ not json\n");

        let frame = CodexDecodedFrame::Malformed(malformed);
        let native = native_ref_for_decoded_frame(&frame);
        assert_eq!(native.kind.as_deref(), Some("malformed"));
        assert_eq!(native.raw_payload.unwrap()["line"], "{ not json\n");
    }
}
