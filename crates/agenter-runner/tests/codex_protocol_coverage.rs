use std::collections::BTreeSet;
use std::fs;
use std::path::Path;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy)]
struct CoverageRow {
    codex: &'static str,
    mapping: &'static str,
}

const CLIENT_REQUEST_COVERAGE: &[CoverageRow] = &[
    CoverageRow { codex: "initialize", mapping: "Runner transport startup, capability discovery, no browser command" },
    CoverageRow { codex: "thread/start", mapping: "UniversalCommand::StartSession" },
    CoverageRow { codex: "thread/resume", mapping: "UniversalCommand::LoadSession" },
    CoverageRow { codex: "thread/fork", mapping: "UniversalCommand::ForkSession" },
    CoverageRow { codex: "thread/archive", mapping: "Provider command, emits session.status_changed(Archived)" },
    CoverageRow { codex: "thread/unsubscribe", mapping: "Adapter close/unsubscribe path, not public chat command by default" },
    CoverageRow { codex: "thread/increment_elicitation", mapping: "Provider command for external helpers; updates provider notification/obligation timeout metadata" },
    CoverageRow { codex: "thread/decrement_elicitation", mapping: "Provider command for external helpers; updates provider notification/obligation timeout metadata" },
    CoverageRow { codex: "thread/name/set", mapping: "Provider command and session.metadata_changed" },
    CoverageRow { codex: "thread/goal/set", mapping: "Provider command, provider.notification category thread_goal" },
    CoverageRow { codex: "thread/goal/get", mapping: "Provider command result" },
    CoverageRow { codex: "thread/goal/clear", mapping: "Provider command, provider.notification category thread_goal" },
    CoverageRow { codex: "thread/metadata/update", mapping: "Provider command, mapped metadata fields plus raw payload" },
    CoverageRow { codex: "thread/memoryMode/set", mapping: "Provider command, capability memory_mode" },
    CoverageRow { codex: "memory/reset", mapping: "Provider command, global runner scope" },
    CoverageRow { codex: "thread/unarchive", mapping: "Provider command, emits session status update" },
    CoverageRow { codex: "thread/compact/start", mapping: "Provider command plus visible compaction item/notification" },
    CoverageRow { codex: "thread/shellCommand", mapping: "Provider command scoped to a thread" },
    CoverageRow { codex: "thread/approveGuardianDeniedAction", mapping: "Provider command resolving a guardian-denied action" },
    CoverageRow { codex: "thread/backgroundTerminals/clean", mapping: "Provider command" },
    CoverageRow { codex: "thread/rollback", mapping: "Provider command and diff/item refresh" },
    CoverageRow { codex: "thread/list", mapping: "Adapter discovery/import" },
    CoverageRow { codex: "thread/loaded/list", mapping: "Adapter loaded-session discovery" },
    CoverageRow { codex: "thread/read", mapping: "Adapter history/metadata reconciliation" },
    CoverageRow { codex: "thread/turns/list", mapping: "Adapter paged history import" },
    CoverageRow { codex: "thread/inject_items", mapping: "Provider command; not general UI by default" },
    CoverageRow { codex: "skills/list", mapping: "Provider command result" },
    CoverageRow { codex: "hooks/list", mapping: "Provider command result" },
    CoverageRow { codex: "marketplace/add", mapping: "Provider command, guarded" },
    CoverageRow { codex: "marketplace/remove", mapping: "Provider command, guarded" },
    CoverageRow { codex: "marketplace/upgrade", mapping: "Provider command, guarded" },
    CoverageRow { codex: "plugin/list", mapping: "Provider command result" },
    CoverageRow { codex: "plugin/read", mapping: "Provider command result" },
    CoverageRow { codex: "app/list", mapping: "Provider command result; updates app list notification" },
    CoverageRow { codex: "device/key/create", mapping: "Provider command, guarded" },
    CoverageRow { codex: "device/key/public", mapping: "Provider command" },
    CoverageRow { codex: "device/key/sign", mapping: "Provider command, guarded" },
    CoverageRow { codex: "fs/readFile", mapping: "Provider command, runner-local fs authorization" },
    CoverageRow { codex: "fs/writeFile", mapping: "Provider command, runner-local fs authorization" },
    CoverageRow { codex: "fs/createDirectory", mapping: "Provider command, runner-local fs authorization" },
    CoverageRow { codex: "fs/getMetadata", mapping: "Provider command, runner-local fs authorization" },
    CoverageRow { codex: "fs/readDirectory", mapping: "Provider command, runner-local fs authorization" },
    CoverageRow { codex: "fs/remove", mapping: "Provider command, runner-local fs authorization" },
    CoverageRow { codex: "fs/copy", mapping: "Provider command, runner-local fs authorization" },
    CoverageRow { codex: "fs/watch", mapping: "Provider command and provider notification stream" },
    CoverageRow { codex: "fs/unwatch", mapping: "Provider command" },
    CoverageRow { codex: "skills/config/write", mapping: "Provider command, guarded" },
    CoverageRow { codex: "plugin/install", mapping: "Provider command, guarded" },
    CoverageRow { codex: "plugin/uninstall", mapping: "Provider command, guarded" },
    CoverageRow { codex: "turn/start", mapping: "UniversalCommand::StartTurn" },
    CoverageRow { codex: "turn/steer", mapping: "UniversalCommand::SendUserInput" },
    CoverageRow { codex: "turn/interrupt", mapping: "UniversalCommand::CancelTurn" },
    CoverageRow { codex: "thread/realtime/start", mapping: "Provider command, capability realtime" },
    CoverageRow { codex: "thread/realtime/appendAudio", mapping: "Provider command, capability realtime" },
    CoverageRow { codex: "thread/realtime/appendText", mapping: "Provider command, capability realtime" },
    CoverageRow { codex: "thread/realtime/stop", mapping: "Provider command, capability realtime" },
    CoverageRow { codex: "thread/realtime/listVoices", mapping: "Provider command result" },
    CoverageRow { codex: "review/start", mapping: "Provider command, review-mode item/notification" },
    CoverageRow { codex: "model/list", mapping: "Provider command result and model capability data" },
    CoverageRow { codex: "modelProvider/capabilities/read", mapping: "Provider command result and capability update" },
    CoverageRow { codex: "experimentalFeature/list", mapping: "Provider command result" },
    CoverageRow { codex: "experimentalFeature/enablement/set", mapping: "Provider command, guarded" },
    CoverageRow { codex: "collaborationMode/list", mapping: "Provider command result and mode capability data" },
    CoverageRow { codex: "mock/experimentalMethod", mapping: "Test-only provider command, disabled in production manifest" },
    CoverageRow { codex: "mcpServer/oauth/login", mapping: "Provider command, guarded" },
    CoverageRow { codex: "config/mcpServer/reload", mapping: "Provider command" },
    CoverageRow { codex: "mcpServerStatus/list", mapping: "Provider command result" },
    CoverageRow { codex: "mcpServer/resource/read", mapping: "Provider command result, optional thread scope" },
    CoverageRow { codex: "mcpServer/tool/call", mapping: "Provider command; result can emit tool item if thread-scoped" },
    CoverageRow { codex: "windowsSandbox/setupStart", mapping: "Provider command, platform-gated" },
    CoverageRow { codex: "account/login/start", mapping: "Provider command, guarded" },
    CoverageRow { codex: "account/login/cancel", mapping: "Provider command" },
    CoverageRow { codex: "account/logout", mapping: "Provider command, guarded" },
    CoverageRow { codex: "account/rateLimits/read", mapping: "Provider command result and usage.updated when applicable" },
    CoverageRow { codex: "account/sendAddCreditsNudgeEmail", mapping: "Provider command, guarded" },
    CoverageRow { codex: "feedback/upload", mapping: "Provider command, guarded" },
    CoverageRow { codex: "command/exec", mapping: "Provider command plus one-off command item/stream if thread-scoped" },
    CoverageRow { codex: "command/exec/write", mapping: "Provider command" },
    CoverageRow { codex: "command/exec/terminate", mapping: "Provider command" },
    CoverageRow { codex: "command/exec/resize", mapping: "Provider command" },
    CoverageRow { codex: "config/read", mapping: "Provider command result" },
    CoverageRow { codex: "externalAgentConfig/detect", mapping: "Provider command result" },
    CoverageRow { codex: "externalAgentConfig/import", mapping: "Provider command, guarded" },
    CoverageRow { codex: "config/value/write", mapping: "Provider command, guarded" },
    CoverageRow { codex: "config/batchWrite", mapping: "Provider command, guarded" },
    CoverageRow { codex: "configRequirements/read", mapping: "Provider command result" },
    CoverageRow { codex: "account/read", mapping: "Provider command result and account notification" },
    CoverageRow { codex: "getConversationSummary", mapping: "Deprecated compatibility provider command or explicit unsupported response" },
    CoverageRow { codex: "gitDiffToRemote", mapping: "Deprecated compatibility provider command or explicit unsupported response" },
    CoverageRow { codex: "getAuthStatus", mapping: "Deprecated compatibility provider command mapped to account/read if native supports it" },
    CoverageRow { codex: "fuzzyFileSearch", mapping: "Provider command, local fs search authorization" },
    CoverageRow { codex: "fuzzyFileSearch/sessionStart", mapping: "Provider command" },
    CoverageRow { codex: "fuzzyFileSearch/sessionUpdate", mapping: "Provider command" },
    CoverageRow { codex: "fuzzyFileSearch/sessionStop", mapping: "Provider command" },
];

const SERVER_REQUEST_COVERAGE: &[CoverageRow] = &[
    CoverageRow { codex: "item/commandExecution/requestApproval", mapping: "approval.requested kind Command; ResolveApproval maps to Codex command decision" },
    CoverageRow { codex: "item/fileChange/requestApproval", mapping: "approval.requested kind FileChange; ResolveApproval maps to file-change decision" },
    CoverageRow { codex: "item/tool/requestUserInput", mapping: "question.requested with native tool request fields; AnswerQuestion maps to ToolRequestUserInputResponse" },
    CoverageRow { codex: "mcpServer/elicitation/request", mapping: "question.requested with MCP schema metadata; AnswerQuestion maps to elicitation response" },
    CoverageRow { codex: "item/permissions/requestApproval", mapping: "approval.requested kind Permission; ResolveApproval maps to permission response" },
    CoverageRow { codex: "item/tool/call", mapping: "Unsupported unless dynamic-tools capability is enabled; otherwise explicit unsupported error" },
    CoverageRow { codex: "account/chatgptAuthTokens/refresh", mapping: "Runner auth callback if configured; otherwise provider notification" },
    CoverageRow { codex: "applyPatchApproval", mapping: "Deprecated approval compatibility path; map to FileChange or reject visibly" },
    CoverageRow { codex: "execCommandApproval", mapping: "Deprecated approval compatibility path; map to Command or reject visibly" },
];

const SERVER_NOTIFICATION_COVERAGE: &[CoverageRow] = &[
    CoverageRow {
        codex: "error",
        mapping: "error.reported",
    },
    CoverageRow {
        codex: "thread/started",
        mapping: "session.created or metadata reconciliation",
    },
    CoverageRow {
        codex: "thread/status/changed",
        mapping: "session.status_changed and active turn status when applicable",
    },
    CoverageRow {
        codex: "thread/archived",
        mapping: "session.status_changed(Archived)",
    },
    CoverageRow {
        codex: "thread/unarchived",
        mapping: "session.status_changed(Idle or Running)",
    },
    CoverageRow {
        codex: "thread/closed",
        mapping: "session.status_changed(Stopped) and clear pending obligations",
    },
    CoverageRow {
        codex: "skills/changed",
        mapping: "provider.notification category skills",
    },
    CoverageRow {
        codex: "thread/name/updated",
        mapping: "session.metadata_changed",
    },
    CoverageRow {
        codex: "thread/goal/updated",
        mapping: "provider.notification category thread_goal",
    },
    CoverageRow {
        codex: "thread/goal/cleared",
        mapping: "provider.notification category thread_goal",
    },
    CoverageRow {
        codex: "thread/tokenUsage/updated",
        mapping: "usage.updated",
    },
    CoverageRow {
        codex: "turn/started",
        mapping: "turn.started",
    },
    CoverageRow {
        codex: "hook/started",
        mapping: "hook item or provider notification",
    },
    CoverageRow {
        codex: "turn/completed",
        mapping: "turn.completed, turn.interrupted, or turn.failed",
    },
    CoverageRow {
        codex: "hook/completed",
        mapping: "hook item completion or provider notification",
    },
    CoverageRow {
        codex: "turn/diff/updated",
        mapping: "diff.updated",
    },
    CoverageRow {
        codex: "turn/plan/updated",
        mapping: "plan.updated with structured entries",
    },
    CoverageRow {
        codex: "item/started",
        mapping: "item.created with streaming/running status",
    },
    CoverageRow {
        codex: "item/autoApprovalReview/started",
        mapping: "approval policy metadata plus provider notification",
    },
    CoverageRow {
        codex: "item/autoApprovalReview/completed",
        mapping: "approval policy/risk update plus provider notification",
    },
    CoverageRow {
        codex: "item/completed",
        mapping: "item-specific completion mapping",
    },
    CoverageRow {
        codex: "rawResponseItem/completed",
        mapping: "native.unknown or raw native row with expandable full payload",
    },
    CoverageRow {
        codex: "item/agentMessage/delta",
        mapping: "content.delta(Text)",
    },
    CoverageRow {
        codex: "item/plan/delta",
        mapping: "content.delta(Text) and partial plan.updated",
    },
    CoverageRow {
        codex: "command/exec/outputDelta",
        mapping: "one-off command content.delta(CommandOutput)",
    },
    CoverageRow {
        codex: "item/commandExecution/outputDelta",
        mapping: "command item content.delta(CommandOutput)",
    },
    CoverageRow {
        codex: "item/commandExecution/terminalInteraction",
        mapping: "command item content.delta(TerminalInput)",
    },
    CoverageRow {
        codex: "item/fileChange/outputDelta",
        mapping: "file-change item content.delta(CommandOutput or Native)",
    },
    CoverageRow {
        codex: "item/fileChange/patchUpdated",
        mapping: "diff.updated and file-change item update",
    },
    CoverageRow {
        codex: "serverRequest/resolved",
        mapping: "approval.resolved or question.answered when correlated",
    },
    CoverageRow {
        codex: "item/mcpToolCall/progress",
        mapping: "MCP tool item provider/status content delta",
    },
    CoverageRow {
        codex: "mcpServer/oauthLogin/completed",
        mapping: "provider.notification category mcp_oauth",
    },
    CoverageRow {
        codex: "mcpServer/startupStatus/updated",
        mapping: "provider.notification category mcp_status",
    },
    CoverageRow {
        codex: "account/updated",
        mapping: "provider.notification category account",
    },
    CoverageRow {
        codex: "account/rateLimits/updated",
        mapping: "usage.updated plus provider notification when user-visible",
    },
    CoverageRow {
        codex: "app/list/updated",
        mapping: "provider.notification category apps",
    },
    CoverageRow {
        codex: "remoteControl/status/changed",
        mapping: "provider.notification category remote_control",
    },
    CoverageRow {
        codex: "externalAgentConfig/import/completed",
        mapping: "provider.notification category external_agent_config",
    },
    CoverageRow {
        codex: "fs/changed",
        mapping: "provider.notification category fs_watch",
    },
    CoverageRow {
        codex: "item/reasoning/summaryTextDelta",
        mapping: "content.delta(Reasoning) summary block",
    },
    CoverageRow {
        codex: "item/reasoning/summaryPartAdded",
        mapping: "new reasoning summary block or block separator",
    },
    CoverageRow {
        codex: "item/reasoning/textDelta",
        mapping: "raw reasoning block, visible in early research builds",
    },
    CoverageRow {
        codex: "thread/compacted",
        mapping: "deprecated compaction notification mapped to compaction item",
    },
    CoverageRow {
        codex: "model/rerouted",
        mapping: "provider.notification category model",
    },
    CoverageRow {
        codex: "model/verification",
        mapping: "provider.notification category model",
    },
    CoverageRow {
        codex: "warning",
        mapping: "provider.notification severity warning",
    },
    CoverageRow {
        codex: "guardianWarning",
        mapping: "provider.notification severity warning plus approval policy context",
    },
    CoverageRow {
        codex: "deprecationNotice",
        mapping: "provider.notification severity warning",
    },
    CoverageRow {
        codex: "configWarning",
        mapping: "provider.notification severity warning",
    },
    CoverageRow {
        codex: "fuzzyFileSearch/sessionUpdated",
        mapping: "provider command result notification",
    },
    CoverageRow {
        codex: "fuzzyFileSearch/sessionCompleted",
        mapping: "provider command result notification",
    },
    CoverageRow {
        codex: "thread/realtime/started",
        mapping: "provider.notification category realtime",
    },
    CoverageRow {
        codex: "thread/realtime/itemAdded",
        mapping: "realtime provider notification or item if transcript-scoped",
    },
    CoverageRow {
        codex: "thread/realtime/transcript/delta",
        mapping: "realtime transcript content delta when enabled",
    },
    CoverageRow {
        codex: "thread/realtime/transcript/done",
        mapping: "realtime transcript content completion",
    },
    CoverageRow {
        codex: "thread/realtime/outputAudio/delta",
        mapping: "artifact/native notification with raw payload available",
    },
    CoverageRow {
        codex: "thread/realtime/sdp",
        mapping: "provider.notification/native row with raw payload available",
    },
    CoverageRow {
        codex: "thread/realtime/error",
        mapping: "error.reported and provider notification",
    },
    CoverageRow {
        codex: "thread/realtime/closed",
        mapping: "provider.notification category realtime",
    },
    CoverageRow {
        codex: "windows/worldWritableWarning",
        mapping: "provider.notification severity warning",
    },
    CoverageRow {
        codex: "windowsSandbox/setupCompleted",
        mapping: "provider.notification category windows_sandbox",
    },
    CoverageRow {
        codex: "account/login/completed",
        mapping: "provider.notification category account",
    },
];

const THREAD_ITEM_COVERAGE: &[CoverageRow] = &[
    CoverageRow {
        codex: "UserMessage",
        mapping: "item.created role User, status Completed, text/block content",
    },
    CoverageRow {
        codex: "HookPrompt",
        mapping: "system or hook tool item with fragments and hook run IDs",
    },
    CoverageRow {
        codex: "AgentMessage",
        mapping: "assistant item with text content, deltas completed by final text",
    },
    CoverageRow {
        codex: "Plan",
        mapping: "assistant plan item plus plan.updated",
    },
    CoverageRow {
        codex: "Reasoning",
        mapping: "assistant reasoning item; summary visible and raw content expandable",
    },
    CoverageRow {
        codex: "CommandExecution",
        mapping: "tool item subkind command, command/cwd/process/status/output/exit/duration",
    },
    CoverageRow {
        codex: "FileChange",
        mapping: "tool item subkind file_change, file diff blocks and diff.updated",
    },
    CoverageRow {
        codex: "McpToolCall",
        mapping: "tool item kind Mcp, server/tool/arguments/result/error/duration",
    },
    CoverageRow {
        codex: "DynamicToolCall",
        mapping: "unsupported notification unless dynamic tools are enabled",
    },
    CoverageRow {
        codex: "CollabAgentToolCall",
        mapping: "tool item kind Subagent, child thread IDs and agent states",
    },
    CoverageRow {
        codex: "WebSearch",
        mapping: "tool item subkind web_search, query/action/result status",
    },
    CoverageRow {
        codex: "ImageView",
        mapping: "artifact/image item with local path URI policy",
    },
    CoverageRow {
        codex: "ImageGeneration",
        mapping: "artifact/image item with revised prompt, result, saved path",
    },
    CoverageRow {
        codex: "EnteredReviewMode",
        mapping: "review-mode item and session mode/provider notification",
    },
    CoverageRow {
        codex: "ExitedReviewMode",
        mapping: "review-mode item and session mode/provider notification",
    },
    CoverageRow {
        codex: "ContextCompaction",
        mapping: "system item and provider.notification category compaction",
    },
];

#[test]
fn codex_protocol_coverage_matches_current_app_server_sources() {
    let repo = repo_root();
    let common = read_source(
        &repo,
        "tmp/codex/codex-rs/app-server-protocol/src/protocol/common.rs",
    );
    let v2 = read_source(
        &repo,
        "tmp/codex/codex-rs/app-server-protocol/src/protocol/v2.rs",
    );

    assert_covered(
        "ClientRequest",
        extract_macro_methods(&common, "client_request_definitions"),
        CLIENT_REQUEST_COVERAGE,
    );
    assert_covered(
        "ServerRequest",
        extract_macro_methods(&common, "server_request_definitions"),
        SERVER_REQUEST_COVERAGE,
    );
    assert_covered(
        "ServerNotification",
        extract_macro_methods(&common, "server_notification_definitions"),
        SERVER_NOTIFICATION_COVERAGE,
    );
    assert_covered(
        "ThreadItem",
        extract_enum_variants(&v2, "ThreadItem"),
        THREAD_ITEM_COVERAGE,
    );
}

#[test]
fn codex_protocol_coverage_reports_unlisted_source_variants() {
    let actual = BTreeSet::from(["known".to_string(), "newVariant".to_string()]);
    let covered = [CoverageRow {
        codex: "known",
        mapping: "fixture mapping",
    }];

    let error = coverage_error("FixtureProtocol", actual, coverage_names(&covered))
        .expect("fixture should report missing coverage");

    assert!(
        error.contains("missing coverage rows: newVariant"),
        "unexpected error: {error}"
    );
}

fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .ancestors()
        .nth(2)
        .expect("runner crate should be two levels below repo root")
        .to_path_buf()
}

fn read_source(repo: &Path, relative: &str) -> String {
    let path = repo.join(relative);
    fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("failed to read {}: {error}", path.display()))
}

fn assert_covered(protocol: &str, actual: BTreeSet<String>, coverage: &[CoverageRow]) {
    assert_no_duplicate_coverage(protocol, coverage);
    assert_no_empty_mappings(protocol, coverage);

    if let Some(error) = coverage_error(protocol, actual, coverage_names(coverage)) {
        panic!("{error}");
    }
}

fn assert_no_duplicate_coverage(protocol: &str, coverage: &[CoverageRow]) {
    let mut seen = BTreeSet::new();
    let duplicates = coverage
        .iter()
        .filter_map(|row| {
            if seen.insert(row.codex) {
                None
            } else {
                Some(row.codex)
            }
        })
        .collect::<Vec<_>>();

    assert!(
        duplicates.is_empty(),
        "{protocol} coverage has duplicate rows: {}",
        duplicates.join(", ")
    );
}

fn assert_no_empty_mappings(protocol: &str, coverage: &[CoverageRow]) {
    let empty = coverage
        .iter()
        .filter_map(|row| row.mapping.trim().is_empty().then_some(row.codex))
        .collect::<Vec<_>>();

    assert!(
        empty.is_empty(),
        "{protocol} coverage has empty mapping rows: {}",
        empty.join(", ")
    );
}

fn coverage_names(coverage: &[CoverageRow]) -> BTreeSet<String> {
    coverage.iter().map(|row| row.codex.to_string()).collect()
}

fn coverage_error(
    protocol: &str,
    actual: BTreeSet<String>,
    covered: BTreeSet<String>,
) -> Option<String> {
    let missing = actual.difference(&covered).cloned().collect::<Vec<_>>();
    let stale = covered.difference(&actual).cloned().collect::<Vec<_>>();

    if missing.is_empty() && stale.is_empty() {
        return None;
    }

    let mut message = format!("{protocol} coverage drifted from tmp/codex app-server source.");
    if !missing.is_empty() {
        message.push_str(&format!("\nmissing coverage rows: {}", missing.join(", ")));
    }
    if !stale.is_empty() {
        message.push_str(&format!(
            "\nstale coverage rows not found in source: {}",
            stale.join(", ")
        ));
    }
    Some(message)
}

fn extract_macro_methods(source: &str, macro_name: &str) -> BTreeSet<String> {
    split_top_level_entries(&strip_line_comments(extract_macro_body(source, macro_name)))
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
    split_top_level_entries(&strip_line_comments(extract_enum_body(source, enum_name)))
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

fn method_name_for_entry(entry: &str) -> String {
    explicit_wire_name(entry)
        .or_else(|| serde_rename(entry))
        .unwrap_or_else(|| lower_camel_case(&first_identifier(entry)))
}

fn explicit_wire_name(entry: &str) -> Option<String> {
    let arrow = entry.find("=>")?;
    let after_arrow = &entry[arrow + 2..];
    quoted_string(after_arrow)
}

fn serde_rename(entry: &str) -> Option<String> {
    let marker = "serde(rename =";
    let start = entry.find(marker)?;
    quoted_string(&entry[start + marker.len()..])
}

fn quoted_string(input: &str) -> Option<String> {
    let start = input.find('"')? + 1;
    let end = input[start..].find('"')? + start;
    Some(input[start..end].to_string())
}

fn first_identifier(entry: &str) -> String {
    for line in entry.lines() {
        let trimmed = line.trim();
        if trimmed.is_empty()
            || trimmed.starts_with('#')
            || trimmed.starts_with("///")
            || trimmed.starts_with("//")
        {
            continue;
        }

        let ident = trimmed
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || *ch == '_')
            .collect::<String>();
        if !ident.is_empty() {
            return ident;
        }
    }

    panic!("entry has no variant identifier: {entry}");
}

fn lower_camel_case(ident: &str) -> String {
    let mut chars = ident.chars();
    let Some(first) = chars.next() else {
        return String::new();
    };

    first.to_ascii_lowercase().to_string() + chars.as_str()
}

fn split_top_level_entries(body: &str) -> Vec<String> {
    let mut entries = Vec::new();
    let mut start = 0;
    let mut depth = Vec::new();
    let mut in_string = false;
    let mut escaped = false;

    for (index, ch) in body.char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }

        match ch {
            '"' => in_string = true,
            '{' | '(' | '[' => depth.push(ch),
            '}' => pop_delimiter(&mut depth, '{', ch),
            ')' => pop_delimiter(&mut depth, '(', ch),
            ']' => pop_delimiter(&mut depth, '[', ch),
            ',' if depth.is_empty() => {
                entries.push(body[start..index].to_string());
                start = index + ch.len_utf8();
            }
            _ => {}
        }
    }

    if start < body.len() {
        entries.push(body[start..].to_string());
    }

    entries
}

fn strip_line_comments(source: &str) -> String {
    let mut stripped = String::with_capacity(source.len());

    for line in source.lines() {
        let mut in_string = false;
        let mut escaped = false;
        let mut comment_start = line.len();
        let mut previous = '\0';

        for (index, ch) in line.char_indices() {
            if in_string {
                if escaped {
                    escaped = false;
                } else if ch == '\\' {
                    escaped = true;
                } else if ch == '"' {
                    in_string = false;
                }
            } else if ch == '"' {
                in_string = true;
            } else if previous == '/' && ch == '/' {
                comment_start = index - previous.len_utf8();
                break;
            }
            previous = ch;
        }

        stripped.push_str(&line[..comment_start]);
        stripped.push('\n');
    }

    stripped
}

fn matching_delimiter(source: &str, open_index: usize, open: char, close: char) -> Option<usize> {
    let mut depth = 0;
    let mut in_string = false;
    let mut escaped = false;

    for (offset, ch) in source[open_index..].char_indices() {
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
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
                return Some(open_index + offset);
            }
        }
    }

    None
}

fn pop_delimiter(depth: &mut Vec<char>, expected: char, actual: char) {
    let popped = depth
        .pop()
        .unwrap_or_else(|| panic!("unmatched delimiter {actual}"));
    assert_eq!(
        popped, expected,
        "mismatched delimiter: expected {expected}, got {actual}"
    );
}
