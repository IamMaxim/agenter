//! Checked-in coverage matrix for the local Codex app-server protocol snapshot.
//!
//! Request and notification coverage is exhaustive against the local snapshot.
#![allow(dead_code)]

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum CodexProtocolDirection {
    ServerRequest,
    ServerNotification,
    ClientRequest,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodexProtocolSupport {
    Supported,
    Degraded,
    Unsupported,
    Ignored,
    NotApplicable,
    Deferred,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CodexProtocolCoverage {
    pub direction: CodexProtocolDirection,
    pub method: &'static str,
    pub support: CodexProtocolSupport,
    pub agenter_surface: &'static str,
    pub notes: &'static str,
}

macro_rules! entry {
    ($direction:ident, $method:literal, $support:ident, $surface:literal, $notes:literal) => {
        CodexProtocolCoverage {
            direction: CodexProtocolDirection::$direction,
            method: $method,
            support: CodexProtocolSupport::$support,
            agenter_surface: $surface,
            notes: $notes,
        }
    };
}

pub const CODEX_PROTOCOL_COVERAGE: &[CodexProtocolCoverage] = &[
    entry!(
        ServerRequest,
        "item/commandExecution/requestApproval",
        Supported,
        "approval",
        "Turn/start command approvals are routed to Agenter approval state."
    ),
    entry!(
        ServerRequest,
        "item/fileChange/requestApproval",
        Supported,
        "approval",
        "Turn/start file-change approvals are routed to Agenter approval state."
    ),
    entry!(
        ServerRequest,
        "item/tool/requestUserInput",
        Supported,
        "question",
        "Tool user-input requests are projected as Agenter questions."
    ),
    entry!(
        ServerRequest,
        "mcpServer/elicitation/request",
        Supported,
        "question",
        "MCP elicitation requests are projected as Agenter questions."
    ),
    entry!(
        ServerRequest,
        "item/permissions/requestApproval",
        Supported,
        "approval",
        "Additional permission approvals are routed to Agenter approval state."
    ),
    entry!(
        ServerRequest,
        "item/tool/call",
        Degraded,
        "native capability gap",
        "Dynamic client-side tool execution is visible but not executed remotely."
    ),
    entry!(
        ServerRequest,
        "account/chatgptAuthTokens/refresh",
        Degraded,
        "native capability gap",
        "Runner-host ChatGPT auth refresh must be handled locally."
    ),
    entry!(
        ServerRequest,
        "applyPatchApproval",
        Supported,
        "approval",
        "Legacy patch approvals are still routed through approval handling."
    ),
    entry!(
        ServerRequest,
        "execCommandApproval",
        Supported,
        "approval",
        "Legacy command approvals are still routed through approval handling."
    ),
    entry!(
        ServerNotification,
        "error",
        Supported,
        "native event",
        "Codex errors are visible as native/provider events."
    ),
    entry!(
        ServerNotification,
        "thread/started",
        Supported,
        "session lifecycle",
        "Thread startup is consumed by the adapter lifecycle."
    ),
    entry!(
        ServerNotification,
        "thread/status/changed",
        Supported,
        "session lifecycle",
        "Thread status changes update Agenter session status and remain visible natively."
    ),
    entry!(
        ServerNotification,
        "thread/archived",
        Supported,
        "session lifecycle",
        "Thread archive updates Agenter session status and remains visible natively."
    ),
    entry!(
        ServerNotification,
        "thread/unarchived",
        Supported,
        "session lifecycle",
        "Thread unarchive updates Agenter session status and remains visible natively."
    ),
    entry!(
        ServerNotification,
        "thread/closed",
        Supported,
        "session lifecycle",
        "Thread close updates Agenter session status and remains visible natively."
    ),
    entry!(
        ServerNotification,
        "skills/changed",
        Deferred,
        "native event",
        "Skills inventory changes are not surfaced as typed state yet."
    ),
    entry!(
        ServerNotification,
        "thread/name/updated",
        Supported,
        "session metadata",
        "Thread rename updates Agenter session title and remains visible natively."
    ),
    entry!(
        ServerNotification,
        "thread/tokenUsage/updated",
        Supported,
        "usage snapshot",
        "Token usage updates feed compact browser usage state."
    ),
    entry!(
        ServerNotification,
        "thread/contextWindow/updated",
        Supported,
        "usage snapshot",
        "Context-window updates feed browser usage state."
    ),
    entry!(
        ServerNotification,
        "turn/started",
        Supported,
        "turn lifecycle",
        "Turn start is mapped into universal turn state."
    ),
    entry!(
        ServerNotification,
        "hook/started",
        Degraded,
        "native event",
        "Hook lifecycle is visible but not first-class."
    ),
    entry!(
        ServerNotification,
        "turn/completed",
        Supported,
        "turn lifecycle",
        "Terminal turn states are mapped into universal turn state."
    ),
    entry!(
        ServerNotification,
        "hook/completed",
        Degraded,
        "native event",
        "Hook lifecycle is visible but not first-class."
    ),
    entry!(
        ServerNotification,
        "turn/diff/updated",
        Supported,
        "diff",
        "Diff updates are projected into universal events."
    ),
    entry!(
        ServerNotification,
        "turn/plan/updated",
        Supported,
        "plan",
        "Plan updates are projected into universal events."
    ),
    entry!(
        ServerNotification,
        "item/started",
        Supported,
        "transcript",
        "Item lifecycle contributes to transcript projection."
    ),
    entry!(
        ServerNotification,
        "item/autoApprovalReview/started",
        Degraded,
        "native event",
        "Auto-approval review lifecycle is visible but not first-class."
    ),
    entry!(
        ServerNotification,
        "item/autoApprovalReview/completed",
        Degraded,
        "native event",
        "Auto-approval review lifecycle is visible but not first-class."
    ),
    entry!(
        ServerNotification,
        "item/completed",
        Supported,
        "transcript",
        "Item completion contributes to transcript projection."
    ),
    entry!(
        ServerNotification,
        "rawResponseItem/completed",
        Ignored,
        "none",
        "Internal Codex Cloud event; not part of remote runner UX."
    ),
    entry!(
        ServerNotification,
        "item/agentMessage/delta",
        Supported,
        "transcript",
        "Agent deltas stream into browser-visible transcript output."
    ),
    entry!(
        ServerNotification,
        "item/plan/delta",
        Degraded,
        "native event",
        "Experimental plan deltas remain provider-native until typed projection lands."
    ),
    entry!(
        ServerNotification,
        "command/exec/outputDelta",
        Deferred,
        "native event",
        "One-off command sessions are not exposed remotely yet."
    ),
    entry!(
        ServerNotification,
        "item/commandExecution/outputDelta",
        Supported,
        "command output",
        "Command execution output is visible in transcript events."
    ),
    entry!(
        ServerNotification,
        "item/commandExecution/terminalInteraction",
        Degraded,
        "native event",
        "Terminal interaction metadata is visible but not interactive."
    ),
    entry!(
        ServerNotification,
        "item/fileChange/outputDelta",
        Supported,
        "diff",
        "File-change output is visible in transcript/diff events."
    ),
    entry!(
        ServerNotification,
        "item/fileChange/patchUpdated",
        Supported,
        "diff",
        "Patch updates are projected as file-change/diff state."
    ),
    entry!(
        ServerNotification,
        "serverRequest/resolved",
        Supported,
        "approval/question state",
        "Resolution clears pending approval/question state."
    ),
    entry!(
        ServerNotification,
        "item/mcpToolCall/progress",
        Degraded,
        "native event",
        "MCP tool progress is visible but not first-class."
    ),
    entry!(
        ServerNotification,
        "mcpServer/oauthLogin/completed",
        Deferred,
        "native event",
        "MCP OAuth completion has no remote auth flow yet."
    ),
    entry!(
        ServerNotification,
        "mcpServer/startupStatus/updated",
        Deferred,
        "native event",
        "MCP server status is not typed in provider capabilities yet."
    ),
    entry!(
        ServerNotification,
        "account/updated",
        Deferred,
        "native event",
        "Account state remains runner-local/provider-native."
    ),
    entry!(
        ServerNotification,
        "account/rateLimits/updated",
        Supported,
        "usage snapshot",
        "Rate-limit updates feed compact browser usage state when present."
    ),
    entry!(
        ServerNotification,
        "app/list/updated",
        Deferred,
        "native event",
        "App connector inventory is not exposed by Agenter yet."
    ),
    entry!(
        ServerNotification,
        "externalAgentConfig/import/completed",
        Deferred,
        "native event",
        "External agent config import is not exposed by Agenter yet."
    ),
    entry!(
        ServerNotification,
        "fs/changed",
        Deferred,
        "native event",
        "Filesystem watch projection is not exposed remotely."
    ),
    entry!(
        ServerNotification,
        "item/reasoning/summaryTextDelta",
        Supported,
        "transcript",
        "Reasoning summary text is projected in transcript detail."
    ),
    entry!(
        ServerNotification,
        "item/reasoning/summaryPartAdded",
        Supported,
        "transcript",
        "Reasoning summary parts are projected in transcript detail."
    ),
    entry!(
        ServerNotification,
        "item/reasoning/textDelta",
        Supported,
        "transcript",
        "Reasoning text is projected in transcript detail."
    ),
    entry!(
        ServerNotification,
        "thread/compacted",
        Supported,
        "transcript",
        "Context compaction is surfaced in transcript projection."
    ),
    entry!(
        ServerNotification,
        "model/rerouted",
        Degraded,
        "native event",
        "Model routing changes are visible but not typed as session model state."
    ),
    entry!(
        ServerNotification,
        "model/verification",
        Degraded,
        "native event",
        "Model verification is visible but not first-class."
    ),
    entry!(
        ServerNotification,
        "warning",
        Supported,
        "native event",
        "Warnings are visible as provider-native events."
    ),
    entry!(
        ServerNotification,
        "guardianWarning",
        Degraded,
        "native event",
        "Guardian warnings are visible but not first-class approval review state."
    ),
    entry!(
        ServerNotification,
        "deprecationNotice",
        Supported,
        "native event",
        "Deprecation notices are visible as provider-native events."
    ),
    entry!(
        ServerNotification,
        "configWarning",
        Supported,
        "native event",
        "Config warnings are visible as provider-native events."
    ),
    entry!(
        ServerNotification,
        "fuzzyFileSearch/sessionUpdated",
        NotApplicable,
        "none",
        "Fuzzy file search is a local TUI affordance, not a remote runner surface."
    ),
    entry!(
        ServerNotification,
        "fuzzyFileSearch/sessionCompleted",
        NotApplicable,
        "none",
        "Fuzzy file search is a local TUI affordance, not a remote runner surface."
    ),
    entry!(
        ServerNotification,
        "thread/realtime/started",
        Deferred,
        "native event",
        "Realtime sessions are not exposed by Agenter yet."
    ),
    entry!(
        ServerNotification,
        "thread/realtime/itemAdded",
        Deferred,
        "native event",
        "Realtime sessions are not exposed by Agenter yet."
    ),
    entry!(
        ServerNotification,
        "thread/realtime/transcript/delta",
        Deferred,
        "native event",
        "Realtime sessions are not exposed by Agenter yet."
    ),
    entry!(
        ServerNotification,
        "thread/realtime/transcript/done",
        Deferred,
        "native event",
        "Realtime sessions are not exposed by Agenter yet."
    ),
    entry!(
        ServerNotification,
        "thread/realtime/outputAudio/delta",
        Deferred,
        "native event",
        "Realtime audio is not exposed by Agenter yet."
    ),
    entry!(
        ServerNotification,
        "thread/realtime/sdp",
        Deferred,
        "native event",
        "Realtime transport negotiation is not exposed by Agenter yet."
    ),
    entry!(
        ServerNotification,
        "thread/realtime/error",
        Deferred,
        "native event",
        "Realtime sessions are not exposed by Agenter yet."
    ),
    entry!(
        ServerNotification,
        "thread/realtime/closed",
        Deferred,
        "native event",
        "Realtime sessions are not exposed by Agenter yet."
    ),
    entry!(
        ServerNotification,
        "windows/worldWritableWarning",
        NotApplicable,
        "native event",
        "Windows sandbox warning is irrelevant on non-Windows runners and native when present."
    ),
    entry!(
        ServerNotification,
        "windowsSandbox/setupCompleted",
        NotApplicable,
        "native event",
        "Windows sandbox setup is runner-host-local."
    ),
    entry!(
        ServerNotification,
        "account/login/completed",
        NotApplicable,
        "native event",
        "Account login is runner-host-local."
    ),
    entry!(
        ClientRequest,
        "initialize",
        Supported,
        "adapter lifecycle",
        "Runner initializes the Codex app-server stdio session."
    ),
    entry!(
        ClientRequest,
        "thread/start",
        Supported,
        "adapter lifecycle",
        "Runner creates native Codex threads for Agenter sessions."
    ),
    entry!(
        ClientRequest,
        "thread/resume",
        Supported,
        "adapter lifecycle",
        "Runner resumes persisted Codex threads when needed."
    ),
    entry!(
        ClientRequest,
        "thread/fork",
        Supported,
        "browser command",
        "Exposed as a conservative provider command."
    ),
    entry!(
        ClientRequest,
        "thread/archive",
        Supported,
        "browser command",
        "Exposed as a conservative provider command."
    ),
    entry!(
        ClientRequest,
        "thread/unsubscribe",
        Deferred,
        "adapter lifecycle",
        "Thread unsubscribe is not needed by the persistent runner path yet."
    ),
    entry!(
        ClientRequest,
        "thread/increment_elicitation",
        Deferred,
        "turn lifecycle",
        "Out-of-band elicitation timeout accounting is not modeled yet."
    ),
    entry!(
        ClientRequest,
        "thread/decrement_elicitation",
        Deferred,
        "turn lifecycle",
        "Out-of-band elicitation timeout accounting is not modeled yet."
    ),
    entry!(
        ClientRequest,
        "thread/name/set",
        Supported,
        "browser command",
        "Exposed as a conservative provider command."
    ),
    entry!(
        ClientRequest,
        "thread/metadata/update",
        Deferred,
        "browser command",
        "Thread metadata mutation needs a product contract."
    ),
    entry!(
        ClientRequest,
        "thread/memoryMode/set",
        Deferred,
        "browser command",
        "Memory mode selection is not exposed in Agenter yet."
    ),
    entry!(
        ClientRequest,
        "memory/reset",
        Unsupported,
        "none",
        "Memory reset is destructive and requires an explicit remote safety design."
    ),
    entry!(
        ClientRequest,
        "thread/unarchive",
        Supported,
        "browser command",
        "Exposed as a conservative provider command."
    ),
    entry!(
        ClientRequest,
        "thread/compact/start",
        Supported,
        "browser command",
        "Exposed as native Codex compaction."
    ),
    entry!(
        ClientRequest,
        "thread/shellCommand",
        Supported,
        "browser command",
        "Exposed as the existing dangerous Codex shell provider command."
    ),
    entry!(
        ClientRequest,
        "thread/approveGuardianDeniedAction",
        Deferred,
        "browser command",
        "Guardian-denied action approval is not modeled as a remote command yet."
    ),
    entry!(
        ClientRequest,
        "thread/backgroundTerminals/clean",
        Supported,
        "browser command",
        "Exposed as a dangerous provider command."
    ),
    entry!(
        ClientRequest,
        "thread/rollback",
        Supported,
        "browser command",
        "Exposed as a dangerous provider command."
    ),
    entry!(
        ClientRequest,
        "thread/list",
        Supported,
        "adapter lifecycle",
        "Runner discovery lists native Codex threads."
    ),
    entry!(
        ClientRequest,
        "thread/loaded/list",
        Supported,
        "browser command",
        "Exposed as a read-only provider command for native loaded-thread inspection."
    ),
    entry!(
        ClientRequest,
        "thread/read",
        Supported,
        "adapter lifecycle",
        "Runner reads native thread history for discovery/import paths."
    ),
    entry!(
        ClientRequest,
        "thread/contextWindow/inspect",
        Supported,
        "browser command",
        "Exposed as a conservative provider command."
    ),
    entry!(
        ClientRequest,
        "thread/turns/list",
        Supported,
        "browser command",
        "Exposed as a read-only provider command."
    ),
    entry!(
        ClientRequest,
        "thread/inject_items",
        Unsupported,
        "none",
        "Raw history injection needs an explicit transcript and trust design."
    ),
    entry!(
        ClientRequest,
        "skills/list",
        Supported,
        "browser command",
        "Exposed as a read-only provider command."
    ),
    entry!(
        ClientRequest,
        "marketplace/add",
        Unsupported,
        "none",
        "Marketplace mutation requires an approved supply-chain design."
    ),
    entry!(
        ClientRequest,
        "marketplace/remove",
        Unsupported,
        "none",
        "Marketplace mutation requires an approved supply-chain design."
    ),
    entry!(
        ClientRequest,
        "marketplace/upgrade",
        Unsupported,
        "none",
        "Marketplace mutation requires an approved supply-chain design."
    ),
    entry!(
        ClientRequest,
        "plugin/list",
        Supported,
        "browser command",
        "Exposed as a read-only provider command."
    ),
    entry!(
        ClientRequest,
        "plugin/read",
        Supported,
        "browser command",
        "Exposed as a read-only provider command."
    ),
    entry!(
        ClientRequest,
        "app/list",
        Supported,
        "browser command",
        "Exposed as a read-only provider command."
    ),
    entry!(
        ClientRequest,
        "device/key/create",
        Unsupported,
        "none",
        "Device-key operations are not safe through the remote browser surface."
    ),
    entry!(
        ClientRequest,
        "device/key/public",
        Deferred,
        "browser command",
        "Device-key public reads are not exposed in Agenter yet."
    ),
    entry!(
        ClientRequest,
        "device/key/sign",
        Unsupported,
        "none",
        "Device-key signing is not safe through the remote browser surface."
    ),
    entry!(
        ClientRequest,
        "fs/readFile",
        Unsupported,
        "none",
        "Direct filesystem reads require a separate approved file-browser design."
    ),
    entry!(
        ClientRequest,
        "fs/writeFile",
        Unsupported,
        "none",
        "Risky filesystem mutation requires an approved remote file-write design."
    ),
    entry!(
        ClientRequest,
        "fs/createDirectory",
        Unsupported,
        "none",
        "Direct filesystem mutation requires an approved remote file-write design."
    ),
    entry!(
        ClientRequest,
        "fs/getMetadata",
        Unsupported,
        "none",
        "Direct filesystem reads require a separate approved file-browser design."
    ),
    entry!(
        ClientRequest,
        "fs/readDirectory",
        Unsupported,
        "none",
        "Direct filesystem reads require a separate approved file-browser design."
    ),
    entry!(
        ClientRequest,
        "fs/remove",
        Unsupported,
        "none",
        "Direct filesystem mutation requires an approved remote file-write design."
    ),
    entry!(
        ClientRequest,
        "fs/copy",
        Unsupported,
        "none",
        "Direct filesystem mutation requires an approved remote file-write design."
    ),
    entry!(
        ClientRequest,
        "fs/watch",
        Deferred,
        "browser command",
        "Filesystem watch needs a file-browser/event design."
    ),
    entry!(
        ClientRequest,
        "fs/unwatch",
        Deferred,
        "browser command",
        "Filesystem watch needs a file-browser/event design."
    ),
    entry!(
        ClientRequest,
        "skills/config/write",
        Unsupported,
        "none",
        "Skills configuration writes require an explicit remote configuration design."
    ),
    entry!(
        ClientRequest,
        "plugin/install",
        Unsupported,
        "none",
        "Risky plugin installation requires an approved trust and supply-chain design."
    ),
    entry!(
        ClientRequest,
        "plugin/uninstall",
        Unsupported,
        "none",
        "Risky plugin removal requires an approved trust and supply-chain design."
    ),
    entry!(
        ClientRequest,
        "turn/start",
        Supported,
        "turn lifecycle",
        "Runner starts turns from Agenter user messages."
    ),
    entry!(
        ClientRequest,
        "turn/steer",
        Supported,
        "browser command",
        "Exposed as Codex turn steering."
    ),
    entry!(
        ClientRequest,
        "turn/interrupt",
        Supported,
        "turn lifecycle",
        "Runner interrupts active Codex turns."
    ),
    entry!(
        ClientRequest,
        "thread/realtime/start",
        Deferred,
        "browser command",
        "Realtime sessions are not exposed by Agenter yet."
    ),
    entry!(
        ClientRequest,
        "thread/realtime/appendAudio",
        Deferred,
        "browser command",
        "Realtime sessions are not exposed by Agenter yet."
    ),
    entry!(
        ClientRequest,
        "thread/realtime/appendText",
        Deferred,
        "browser command",
        "Realtime sessions are not exposed by Agenter yet."
    ),
    entry!(
        ClientRequest,
        "thread/realtime/stop",
        Deferred,
        "browser command",
        "Realtime sessions are not exposed by Agenter yet."
    ),
    entry!(
        ClientRequest,
        "thread/realtime/listVoices",
        Deferred,
        "browser command",
        "Realtime sessions are not exposed by Agenter yet."
    ),
    entry!(
        ClientRequest,
        "review/start",
        Supported,
        "browser command",
        "Exposed as native Codex review."
    ),
    entry!(
        ClientRequest,
        "model/list",
        Supported,
        "agent options",
        "Runner reads model options from Codex."
    ),
    entry!(
        ClientRequest,
        "experimentalFeature/list",
        Deferred,
        "browser command",
        "Experimental feature inventory is not exposed in Agenter yet."
    ),
    entry!(
        ClientRequest,
        "experimentalFeature/enablement/set",
        Unsupported,
        "none",
        "Experimental feature mutation requires explicit product approval."
    ),
    entry!(
        ClientRequest,
        "collaborationMode/list",
        Supported,
        "agent options",
        "Runner reads collaboration mode options from Codex."
    ),
    entry!(
        ClientRequest,
        "mock/experimentalMethod",
        NotApplicable,
        "none",
        "Codex protocol test method."
    ),
    entry!(
        ClientRequest,
        "mcpServer/oauth/login",
        Deferred,
        "browser command",
        "MCP OAuth login is not modeled as a remote browser flow yet."
    ),
    entry!(
        ClientRequest,
        "config/mcpServer/reload",
        Supported,
        "browser command",
        "Exposed as a conservative provider command."
    ),
    entry!(
        ClientRequest,
        "mcpServerStatus/list",
        Supported,
        "browser command",
        "Exposed as a conservative provider command."
    ),
    entry!(
        ClientRequest,
        "mcpServer/resource/read",
        Supported,
        "browser command",
        "Exposed as a read-only provider command."
    ),
    entry!(
        ClientRequest,
        "mcpServer/tool/call",
        Unsupported,
        "none",
        "Arbitrary MCP tool execution needs a remote executor design."
    ),
    entry!(
        ClientRequest,
        "windowsSandbox/setupStart",
        NotApplicable,
        "none",
        "Windows sandbox setup is runner-host-local."
    ),
    entry!(
        ClientRequest,
        "account/login/start",
        NotApplicable,
        "none",
        "Provider account login belongs on the runner host, not in a remote browser command."
    ),
    entry!(
        ClientRequest,
        "account/login/cancel",
        NotApplicable,
        "none",
        "Provider account login belongs on the runner host, not in a remote browser command."
    ),
    entry!(
        ClientRequest,
        "account/logout",
        NotApplicable,
        "none",
        "Provider account logout belongs on the runner host."
    ),
    entry!(
        ClientRequest,
        "account/rateLimits/read",
        Supported,
        "browser command",
        "Exposed as a conservative provider command."
    ),
    entry!(
        ClientRequest,
        "account/sendAddCreditsNudgeEmail",
        NotApplicable,
        "none",
        "Account billing nudges are outside the remote runner scope."
    ),
    entry!(
        ClientRequest,
        "feedback/upload",
        NotApplicable,
        "none",
        "Provider feedback upload is outside the remote runner scope."
    ),
    entry!(
        ClientRequest,
        "command/exec",
        Unsupported,
        "none",
        "One-off command execution requires a separate remote execution design."
    ),
    entry!(
        ClientRequest,
        "command/exec/write",
        Unsupported,
        "none",
        "One-off command execution requires a separate remote execution design."
    ),
    entry!(
        ClientRequest,
        "command/exec/terminate",
        Unsupported,
        "none",
        "One-off command execution requires a separate remote execution design."
    ),
    entry!(
        ClientRequest,
        "command/exec/resize",
        Unsupported,
        "none",
        "One-off command execution requires a separate remote execution design."
    ),
    entry!(
        ClientRequest,
        "config/read",
        Supported,
        "browser command",
        "Exposed as a read-only provider command."
    ),
    entry!(
        ClientRequest,
        "externalAgentConfig/detect",
        Deferred,
        "browser command",
        "External agent config detection is not exposed in Agenter yet."
    ),
    entry!(
        ClientRequest,
        "externalAgentConfig/import",
        Unsupported,
        "none",
        "External agent config import mutates runner-local config."
    ),
    entry!(
        ClientRequest,
        "config/value/write",
        Unsupported,
        "none",
        "Remote config mutation requires an approved configuration design."
    ),
    entry!(
        ClientRequest,
        "config/batchWrite",
        Unsupported,
        "none",
        "Remote config mutation requires an approved configuration design."
    ),
    entry!(
        ClientRequest,
        "configRequirements/read",
        Supported,
        "browser command",
        "Exposed as a read-only provider command."
    ),
    entry!(
        ClientRequest,
        "account/read",
        NotApplicable,
        "none",
        "Provider account reads are runner-host-local."
    ),
    entry!(
        ClientRequest,
        "getConversationSummary",
        Deferred,
        "adapter lifecycle",
        "Deprecated summary API is not used by the current adapter."
    ),
    entry!(
        ClientRequest,
        "gitDiffToRemote",
        Deferred,
        "browser command",
        "Deprecated diff API is not exposed; newer diff notifications are used."
    ),
    entry!(
        ClientRequest,
        "getAuthStatus",
        NotApplicable,
        "none",
        "Deprecated auth status is runner-host-local."
    ),
    entry!(
        ClientRequest,
        "fuzzyFileSearch",
        NotApplicable,
        "none",
        "Deprecated fuzzy search is a local TUI affordance."
    ),
    entry!(
        ClientRequest,
        "fuzzyFileSearch/sessionStart",
        NotApplicable,
        "none",
        "Fuzzy file search is a local TUI affordance."
    ),
    entry!(
        ClientRequest,
        "fuzzyFileSearch/sessionUpdate",
        NotApplicable,
        "none",
        "Fuzzy file search is a local TUI affordance."
    ),
    entry!(
        ClientRequest,
        "fuzzyFileSearch/sessionStop",
        NotApplicable,
        "none",
        "Fuzzy file search is a local TUI affordance."
    ),
];

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeSet;

    const PROTOCOL_METHOD_FIXTURE: &str =
        include_str!("../../tests/fixtures/codex_app_server_protocol_methods.rs");

    #[test]
    fn covers_every_server_request_in_codex_protocol_fixture() {
        let snapshot = read_protocol_fixture();
        let expected = extract_methods_from_macro(&snapshot, "server_request_definitions!");
        let classified = classified_methods(CodexProtocolDirection::ServerRequest);

        assert_exact_methods("ServerRequest", &expected, &classified);
    }

    #[test]
    fn covers_every_server_notification_in_codex_protocol_fixture() {
        let snapshot = read_protocol_fixture();
        let expected = extract_methods_from_macro(&snapshot, "server_notification_definitions!");
        let classified = classified_methods(CodexProtocolDirection::ServerNotification);

        assert_exact_methods("ServerNotification", &expected, &classified);
    }

    #[test]
    fn covers_every_client_request_in_codex_protocol_fixture() {
        let snapshot = read_protocol_fixture();
        let expected = extract_methods_from_macro(&snapshot, "client_request_definitions!");
        let classified = classified_methods(CodexProtocolDirection::ClientRequest);

        assert_exact_methods("ClientRequest", &expected, &classified);
    }

    #[test]
    fn coverage_entries_are_unique_by_direction_and_method() {
        let mut seen = BTreeSet::new();
        let duplicates = CODEX_PROTOCOL_COVERAGE
            .iter()
            .filter_map(|entry| {
                let key = (entry.direction, entry.method);
                (!seen.insert(key)).then_some(format!("{:?}:{}", entry.direction, entry.method))
            })
            .collect::<Vec<_>>();

        assert!(
            duplicates.is_empty(),
            "duplicate Codex protocol coverage entries: {}",
            duplicates.join(", ")
        );
    }

    fn read_protocol_fixture() -> String {
        PROTOCOL_METHOD_FIXTURE.to_owned()
    }

    fn classified_methods(direction: CodexProtocolDirection) -> BTreeSet<String> {
        CODEX_PROTOCOL_COVERAGE
            .iter()
            .filter(|entry| entry.direction == direction)
            .map(|entry| entry.method.to_owned())
            .collect()
    }

    fn extract_methods_from_macro(snapshot: &str, macro_name: &str) -> BTreeSet<String> {
        let body_start = find_macro_invocation_body_start(snapshot, macro_name)
            .unwrap_or_else(|| panic!("missing invocation body for {macro_name}"));
        let body_end = find_matching_brace(snapshot, body_start - 1)
            .unwrap_or_else(|| panic!("unterminated body for {macro_name}"));
        let body = &snapshot[body_start..body_end];

        let mut methods = BTreeSet::new();
        let mut pending_serde_rename = None;
        for line in body.lines() {
            let trimmed = line.trim();
            if let Some(method) = extract_quoted_attribute(trimmed, "#[serde(rename = ") {
                pending_serde_rename = Some(method);
                continue;
            }
            if let Some(method) = extract_explicit_method(trimmed) {
                methods.insert(method);
                pending_serde_rename = None;
                continue;
            }
            if let Some(variant) = extract_variant_name(trimmed) {
                let method = pending_serde_rename
                    .take()
                    .unwrap_or_else(|| lower_camel_variant(variant));
                methods.insert(method);
            }
        }

        methods
    }

    fn find_macro_invocation_body_start(snapshot: &str, macro_name: &str) -> Option<usize> {
        let mut search_from = 0usize;
        while let Some(relative_start) = snapshot[search_from..].find(macro_name) {
            let macro_start = search_from + relative_start;
            let line_start = snapshot[..macro_start]
                .rfind('\n')
                .map_or(0, |index| index + 1);
            let line_prefix = &snapshot[line_start..macro_start];
            search_from = macro_start + macro_name.len();

            if line_prefix.contains("macro_rules!") {
                continue;
            }

            let after_macro = &snapshot[search_from..];
            let Some(body_offset) = after_macro.find('{') else {
                continue;
            };
            let before_body = &after_macro[..body_offset];
            if before_body.trim().is_empty() {
                return Some(search_from + body_offset + 1);
            }
        }

        None
    }

    fn find_matching_brace(snapshot: &str, open_index: usize) -> Option<usize> {
        let mut depth = 0usize;
        for (offset, ch) in snapshot[open_index..].char_indices() {
            match ch {
                '{' => depth += 1,
                '}' => {
                    depth -= 1;
                    if depth == 0 {
                        return Some(open_index + offset);
                    }
                }
                _ => {}
            }
        }
        None
    }

    fn extract_explicit_method(line: &str) -> Option<String> {
        let arrow = line.find("=>")?;
        let after_arrow = &line[arrow + 2..];
        let first_quote = after_arrow.find('"')?;
        let rest = &after_arrow[first_quote + 1..];
        let second_quote = rest.find('"')?;
        Some(rest[..second_quote].to_owned())
    }

    fn extract_quoted_attribute(line: &str, prefix: &str) -> Option<String> {
        let start = line.find(prefix)?;
        let after_prefix = &line[start + prefix.len()..];
        let first_quote = after_prefix.find('"')?;
        let rest = &after_prefix[first_quote + 1..];
        let second_quote = rest.find('"')?;
        Some(rest[..second_quote].to_owned())
    }

    fn extract_variant_name(line: &str) -> Option<&str> {
        if line.contains("=>") || line.is_empty() {
            return None;
        }

        let before_payload = line
            .split(['{', '('])
            .next()
            .unwrap_or(line)
            .trim()
            .trim_end_matches(',');
        before_payload
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || ch == '_')
            .then_some(before_payload)
            .filter(|variant| {
                variant
                    .chars()
                    .next()
                    .is_some_and(|first| first.is_ascii_alphabetic())
            })
    }

    fn lower_camel_variant(variant: &str) -> String {
        let mut chars = variant.chars();
        let Some(first) = chars.next() else {
            return String::new();
        };
        first.to_ascii_lowercase().to_string() + chars.as_str()
    }

    fn assert_missing_methods(kind: &str, missing: Vec<String>) {
        assert!(
            missing.is_empty(),
            "{kind} methods missing from Codex protocol coverage table: {}",
            missing.join(", ")
        );
    }

    fn assert_exact_methods(
        kind: &str,
        expected: &BTreeSet<String>,
        classified: &BTreeSet<String>,
    ) {
        let missing = expected.difference(classified).cloned().collect::<Vec<_>>();
        let extra = classified.difference(expected).cloned().collect::<Vec<_>>();

        assert!(
            missing.is_empty() && extra.is_empty(),
            "{kind} methods do not exactly match Codex protocol snapshot; missing: {}; extra: {}",
            missing.join(", "),
            extra.join(", ")
        );
    }
}
