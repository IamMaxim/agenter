# Codex App-Server to UAP/2 Implementation Plan

Status: Stage 11 live runner wiring implemented; live browser smoke pending
Date: 2026-05-05

## Goal

Add a new Codex adapter from scratch, using the Codex checkout at
`tmp/codex` as the native source of truth, and map the complete Codex
app-server protocol surface into Agenter's `uap/2` universal protocol.

This plan intentionally does not depend on any previous Codex adapter design.
The reference inputs are:

- `tmp/codex/codex-rs/app-server-protocol/src/protocol/common.rs`
- `tmp/codex/codex-rs/app-server-protocol/src/protocol/v2.rs`
- `tmp/codex/codex-rs/tui/src/app_server_session.rs`
- `tmp/codex/codex-rs/tui/src/app_server_adapter.rs`
- `tmp/codex/codex-rs/tui/src/app/app_server_requests.rs`
- `tmp/codex/codex-rs/tui/src/app/pending_interactive_replay.rs`
- `tmp/codex/codex-rs/tui/src/chatwidget.rs`
- `docs/chatgpt/002_protocol.md`
- current `uap/2` types under `crates/agenter-core/src/`

## Architecture Target

Codex integration is a native app-server adapter that emits direct `uap/2`
events at the runner boundary. The browser and interaction connectors consume
only universal events and provider command manifests.

```text
Browser / connectors
  <-> Control plane uap/2 snapshot, replay, commands, obligations
  <-> Runner adapter API
  <-> Codex app-server JSON-RPC transport
  <-> Codex app-server process
```

The adapter owns Codex process supervision, JSON-RPC request correlation,
native thread/turn/item ID mapping, pending server requests, and native
reduction into universal events. The control plane owns Agenter session
registry, user authorization, event cache, replay, and durable approval/question
delivery state.

## Non-Goals

- No compatibility bridge for removed legacy normalized projection code.
- No `codex exec --json` interactive adapter path.
- No browser code that branches on `provider_id == "codex"` for core transcript
  rendering.
- No early filtering of native payloads for privacy, size, or sensitivity.
  This is a local personal research project; early Codex work optimizes for
  complete browser visibility and fast debugging. Payload reduction is a later
  cleanup pass, not part of this plan's first implementation.
- No arbitrary local filesystem surface as a first-class chat primitive. Codex
  app-server filesystem APIs are exposed through explicit provider commands,
  but their native request/response payloads may still be attached to debug UI.

## Protocol Changes And Reasons

These changes are acceptable because Codex app-server is richer than the current
minimal `uap/2` core and the missing concepts would otherwise become hidden
provider-specific branches.

1. Add `AgentProviderId::CODEX = "codex"`.
   Reason: registrations, manifests, and tests need a stable provider constant
   even though `AgentProviderId` remains an open string wrapper.

2. Add `UniversalCommand::ForkSession`.
   Reason: `thread/fork` is a first-class Codex lifecycle operation, and
   treating it as an opaque provider command would make forked history,
   workspace metadata, and title/status updates harder to make provider-neutral.

3. Add first-class `ApprovalKind::Permission`.
   Reason: `item/permissions/requestApproval` is neither a shell command nor a
   file change. It grants a native permission profile and scope, so presenting
   it as a generic tool approval loses security meaning.

4. Extend `QuestionState` with `native_request_id`, `native_blocking`, and
   optional field schema metadata.
   Reason: Codex `item/tool/requestUserInput` and
   `mcpServer/elicitation/request` are server-to-client obligations that must
   survive browser reload, runner reconnect, and out-of-order resolution.
   MCP elicitation forms also carry schema-like detail beyond the current
   simple choices/text fields.

5. Extend tool projection with either a `subkind` string or explicit kinds for
   `file_change`, `web_search`, `image_view`, `image_generation`,
   `dynamic_tool`, `review_mode`, `hook`, `context_compaction`, and
   `one_off_command`.
   Reason: Codex `ThreadItem` is richer than the current command/mcp/tool/
   subagent taxonomy. The frontend should render truthful rows from universal
   data rather than Codex-specific item names.

6. Add richer provider capability details for Codex features:
   `thread_fork`, `turn_steer`, `thread_rollback`, `thread_compaction`,
   `thread_goal`, `memory_mode`, `review`, `realtime`, `client_fs`,
   `one_off_command`, `skills_plugins`, `config_account`, and `dynamic_tools`.
   Reason: global and experimental Codex surfaces should be feature-gated by a
   manifest, not by hard-coded frontend assumptions.

7. Extend `NativeRef` or the universal envelope with `raw_payload:
   Option<serde_json::Value>` and preserve native request/response/notification
   payloads to the browser during the research phase.
   Reason: this project runs locally and bandwidth is not a constraint. The
   fastest way to make the adapter correct is to see undecoded and partially
   decoded Codex payloads in the browser immediately. Later productization can
   add redaction, size limits, hashes, or local-only pointers.

8. Treat `serverRequest/resolved`, auto-review notifications, model/account/rate
   warnings, and realtime status as explicit universal side effects.
   Reason: they change visible obligation state or user understanding. They
   must not disappear merely because they are not chat text.

## Mapping Rules

- Native Codex `thread_id` maps to `SessionInfo.external_session_id`.
- Universal session IDs are Agenter IDs. Native IDs are preserved in
  `NativeRef`.
- Native Codex `turn_id` maps to universal `TurnId` with a stable adapter ID
  map. If a notification references an unknown native turn, the adapter creates
  a degraded synthetic turn and immediately emits an `error.reported` or
  `provider.notification` explaining the gap.
- Native Codex item IDs map to universal `ItemId` with deterministic stable
  IDs inside the Agenter session. Replayed history must reuse the same mapping.
- All server requests create a durable Agenter obligation before the native
  request is answered.
- Unknown native events are never silently dropped. They become
  `native.unknown` or a categorized `provider.notification` with the full raw
  native payload attached for browser inspection. Coverage tests must force a
  deliberate mapping when Codex adds a known enum variant.
- Every decoded Codex event, request, and response should also carry its raw
  native payload while the adapter is under active research. Decoded universal
  fields are for normal rendering; the raw payload is for expandable debug
  inspection and bug reports.
- `ThreadStatus::Active` with `WaitingOnApproval` maps to
  `SessionStatus::WaitingForApproval`; with `WaitingOnUserInput` maps to
  `SessionStatus::WaitingForInput`; otherwise active maps to `Running`.
  `Idle` maps to `Idle`, `SystemError` maps to `Failed`, `NotLoaded` maps to
  `Stopped` or `Degraded` depending on whether Agenter expected it to be loaded.
- Codex `TurnStatus::Completed`, `Interrupted`, `Failed`, and `InProgress`
  map to the corresponding universal turn states.

## Early Raw Payload Browser Contract

During the first Codex adapter implementation, every native Codex frame that the
adapter observes should be reachable from the browser:

- decoded app-server notifications include the decoded universal projection and
  the original raw notification payload;
- decoded app-server server requests include the approval/question projection and
  the original raw request payload;
- decoded app-server client responses and provider command results include the
  original raw response payload;
- undecoded or unrecognized native frames become a visible native/unknown row
  with method, type, native ID when available, and full raw JSON;
- browser UI renders raw JSON in an expandable dropdown using a stable component
  shared by items, provider notifications, approvals, questions, and command
  results;
- tests must prove that raw payloads survive runner serialization,
  control-plane event cache, snapshot replay, and browser materialization.

This contract is intentionally permissive because the system runs locally and
the immediate priority is fast adapter debugging. A later plan may reduce,
redact, or move raw payload categories back to local logs after the mapping is
stable.

## Q&A And Provisional Decisions

Q: Is Codex app-server the only supported Codex surface for this adapter?

A: Yes for interactive control. Batch `codex exec --json` can be a separate
fixture or smoke helper later, but it is not the adapter path.

Q: Should every Codex app-server API become first-class `uap/2`?

A: No. First-class universal concepts are sessions, turns, items, content
blocks, approvals, questions, usage, diffs, plans, artifacts, errors, and
provider notifications. Account, config, skills, plugins, filesystem, device
keys, model lists, realtime controls, fuzzy search, and one-off commands are
provider commands unless they become cross-provider concepts later.

Q: Should deprecated app-server APIs be implemented?

A: Decode them enough to reject or mark degraded visibly. Do not use deprecated
legacy APIs for new Codex turns. If Codex emits legacy approval requests, map
them to approval obligations with `native.protocol = "codex/app-server/v1"` and
include a warning notification.

Q: What is the source of truth for imported Codex history?

A: Codex remains source of truth. Use `thread/turns/list` where possible for
paged import and `thread/read` for metadata plus compatibility. Agenter stores
registry metadata and lightweight event cache, not canonical Codex transcript
truth.

Q: What happens if an event arrives before `turn/started`?

A: Buffer by native thread/turn when possible. If a buffer cannot be attached by
the next relevant thread read or terminal turn event, synthesize a degraded turn
and emit a visible provider notification. Tests must cover this.

Q: How is a command approval keyed when Codex omits `approval_id`?

A: Use `approval_id.unwrap_or(item_id)`, matching the current Codex TUI
behavior. Store both the app-server JSON-RPC request ID and the native approval
key.

Q: Should `item/tool/requestUserInput` be resolved FIFO per turn?

A: No as the core model. Agenter should persist `question_id -> native_request_id`.
FIFO is only a presentation fallback if the native request lacks a better ID.

Q: How should dynamic tool calls work?

A: Do not advertise dynamic tools until Agenter has a client-tool contract.
If Codex sends `item/tool/call`, respond with an explicit unsupported error and
emit a visible provider notification. This is safer than silently pretending the
tool ran.

Q: How much native payload reaches the browser?

A: All of it during the early Codex adapter stages. If a payload decodes cleanly,
show the decoded universal projection and keep the original native payload in an
expandable debug dropdown. If it does not decode, show a compact unknown/native
row with the full raw JSON payload expanded on demand. Later reduction/redaction
is a separate cleanup project.

Q: How are account and config APIs exposed?

A: As provider commands owned by the runner and guarded by control-plane user
authorization. They are not Agenter authentication or global configuration APIs.

Q: How does `turn/steer` fit `SendUserInput`?

A: `SendUserInput` maps to `turn/steer` only when a native active turn exists
and the adapter can supply the expected native turn ID. If Codex rejects steer,
emit `error.reported` and keep the universal turn state truthful.

## Client Request Coverage

The adapter must account for every `ClientRequest` variant from
`common.rs`. The coverage test in Stage 0 must fail when this list drifts.

| Codex client request | Agenter mapping |
| --- | --- |
| `initialize` | Runner transport startup, capability discovery, no browser command |
| `thread/start` | `UniversalCommand::StartSession` |
| `thread/resume` | `UniversalCommand::LoadSession` |
| `thread/fork` | New `UniversalCommand::ForkSession` |
| `thread/archive` | Provider command, emits `session.status_changed(Archived)` |
| `thread/unsubscribe` | Adapter close/unsubscribe path, not public chat command by default |
| `thread/increment_elicitation` | Provider command for external helpers; updates provider notification/obligation timeout metadata |
| `thread/decrement_elicitation` | Provider command for external helpers; updates provider notification/obligation timeout metadata |
| `thread/name/set` | Provider command and `session.metadata_changed` |
| `thread/goal/set` | Provider command, `provider.notification` category `thread_goal` |
| `thread/goal/get` | Provider command result |
| `thread/goal/clear` | Provider command, `provider.notification` category `thread_goal` |
| `thread/metadata/update` | Provider command, mapped metadata fields plus raw payload |
| `thread/memoryMode/set` | Provider command, capability `memory_mode` |
| `memory/reset` | Provider command, global runner scope |
| `thread/unarchive` | Provider command, emits session status update |
| `thread/compact/start` | Provider command plus visible compaction item/notification |
| `thread/shellCommand` | Provider command scoped to a thread |
| `thread/approveGuardianDeniedAction` | Provider command resolving a guardian-denied action |
| `thread/backgroundTerminals/clean` | Provider command |
| `thread/rollback` | Provider command and diff/item refresh |
| `thread/list` | Adapter discovery/import |
| `thread/loaded/list` | Adapter loaded-session discovery |
| `thread/read` | Adapter history/metadata reconciliation |
| `thread/turns/list` | Adapter paged history import |
| `thread/inject_items` | Provider command; not general UI by default |
| `skills/list` | Provider command result |
| `hooks/list` | Provider command result |
| `marketplace/add` | Provider command, guarded |
| `marketplace/remove` | Provider command, guarded |
| `marketplace/upgrade` | Provider command, guarded |
| `plugin/list` | Provider command result |
| `plugin/read` | Provider command result |
| `app/list` | Provider command result; updates app list notification |
| `device/key/create` | Provider command, guarded |
| `device/key/public` | Provider command |
| `device/key/sign` | Provider command, guarded |
| `fs/readFile` | Provider command, runner-local fs authorization |
| `fs/writeFile` | Provider command, runner-local fs authorization |
| `fs/createDirectory` | Provider command, runner-local fs authorization |
| `fs/getMetadata` | Provider command, runner-local fs authorization |
| `fs/readDirectory` | Provider command, runner-local fs authorization |
| `fs/remove` | Provider command, runner-local fs authorization |
| `fs/copy` | Provider command, runner-local fs authorization |
| `fs/watch` | Provider command and provider notification stream |
| `fs/unwatch` | Provider command |
| `skills/config/write` | Provider command, guarded |
| `plugin/install` | Provider command, guarded |
| `plugin/uninstall` | Provider command, guarded |
| `turn/start` | `UniversalCommand::StartTurn` |
| `turn/steer` | `UniversalCommand::SendUserInput` |
| `turn/interrupt` | `UniversalCommand::CancelTurn` |
| `thread/realtime/start` | Provider command, capability `realtime` |
| `thread/realtime/appendAudio` | Provider command, capability `realtime` |
| `thread/realtime/appendText` | Provider command, capability `realtime` |
| `thread/realtime/stop` | Provider command, capability `realtime` |
| `thread/realtime/listVoices` | Provider command result |
| `review/start` | Provider command, review-mode item/notification |
| `model/list` | Provider command result and model capability data |
| `modelProvider/capabilities/read` | Provider command result and capability update |
| `experimentalFeature/list` | Provider command result |
| `experimentalFeature/enablement/set` | Provider command, guarded |
| `collaborationMode/list` | Provider command result and mode capability data |
| `mock/experimentalMethod` | Test-only provider command, disabled in production manifest |
| `mcpServer/oauth/login` | Provider command, guarded |
| `config/mcpServer/reload` | Provider command |
| `mcpServerStatus/list` | Provider command result |
| `mcpServer/resource/read` | Provider command result, optional thread scope |
| `mcpServer/tool/call` | Provider command; result can emit tool item if thread-scoped |
| `windowsSandbox/setupStart` | Provider command, platform-gated |
| `account/login/start` | Provider command, guarded |
| `account/login/cancel` | Provider command |
| `account/logout` | Provider command, guarded |
| `account/rateLimits/read` | Provider command result and `usage.updated` when applicable |
| `account/sendAddCreditsNudgeEmail` | Provider command, guarded |
| `feedback/upload` | Provider command, guarded |
| `command/exec` | Provider command plus one-off command item/stream if thread-scoped |
| `command/exec/write` | Provider command |
| `command/exec/terminate` | Provider command |
| `command/exec/resize` | Provider command |
| `config/read` | Provider command result |
| `externalAgentConfig/detect` | Provider command result |
| `externalAgentConfig/import` | Provider command, guarded |
| `config/value/write` | Provider command, guarded |
| `config/batchWrite` | Provider command, guarded |
| `configRequirements/read` | Provider command result |
| `account/read` | Provider command result and account notification |
| `GetConversationSummary` | Deprecated compatibility provider command or explicit unsupported response |
| `GitDiffToRemote` | Deprecated compatibility provider command or explicit unsupported response |
| `GetAuthStatus` | Deprecated compatibility provider command mapped to `account/read` if native supports it |
| `FuzzyFileSearch` | Provider command, local fs search authorization |
| `fuzzyFileSearch/sessionStart` | Provider command |
| `fuzzyFileSearch/sessionUpdate` | Provider command |
| `fuzzyFileSearch/sessionStop` | Provider command |

## Server Request Coverage

| Codex server request | Universal mapping | Resolution behavior |
| --- | --- | --- |
| `item/commandExecution/requestApproval` | `approval.requested` with kind `Command` | `ResolveApproval` maps to Codex command decision; use `approval_id` fallback to `item_id` |
| `item/fileChange/requestApproval` | `approval.requested` with kind `FileChange` | `ResolveApproval` maps to file-change decision; provider-specific exec/network amendments are invalid |
| `item/tool/requestUserInput` | `question.requested` with native tool request fields | `AnswerQuestion` maps to `ToolRequestUserInputResponse` |
| `mcpServer/elicitation/request` | `question.requested` with MCP schema metadata | `AnswerQuestion` maps to accept/decline/cancel elicitation response |
| `item/permissions/requestApproval` | `approval.requested` with kind `Permission` | `ResolveApproval` maps to permission profile, scope, and strict-auto-review response |
| `item/tool/call` | Unsupported unless dynamic-tools capability is enabled | Respond with explicit unsupported error and emit provider notification |
| `account/chatgptAuthTokens/refresh` | Runner auth callback if configured; otherwise provider notification | Never rendered as a chat item |
| `ApplyPatchApproval` | Deprecated approval compatibility path | Map to `FileChange` or reject visibly depending payload |
| `ExecCommandApproval` | Deprecated approval compatibility path | Map to `Command` or reject visibly depending payload |

Server-request pending state must match these TUI-derived invariants:

- command approval key is `approval_id` when present, otherwise `item_id`;
- file-change approval key is `item_id`;
- MCP elicitation key is native request ID plus server name;
- pending requests clear on outbound resolution, `serverRequest/resolved`,
  terminal item start/completion as applicable, `turn/completed`, and
  `thread/closed`;
- duplicate same resolution is idempotent;
- conflicting resolution after native acceptance returns a visible error.

## Server Notification Coverage

| Codex notification | Universal mapping |
| --- | --- |
| `error` | `error.reported` |
| `thread/started` | `session.created` or metadata reconciliation |
| `thread/status/changed` | `session.status_changed` and active turn status when applicable |
| `thread/archived` | `session.status_changed(Archived)` |
| `thread/unarchived` | `session.status_changed(Idle or Running)` |
| `thread/closed` | `session.status_changed(Stopped)` and clear pending obligations |
| `skills/changed` | `provider.notification` category `skills` |
| `thread/name/updated` | `session.metadata_changed` |
| `thread/goal/updated` | `provider.notification` category `thread_goal` |
| `thread/goal/cleared` | `provider.notification` category `thread_goal` |
| `thread/tokenUsage/updated` | `usage.updated` |
| `turn/started` | `turn.started` |
| `hook/started` | hook item or provider notification |
| `turn/completed` | `turn.completed`, `turn.interrupted`, or `turn.failed` |
| `hook/completed` | hook item completion or provider notification |
| `turn/diff/updated` | `diff.updated` |
| `turn/plan/updated` | `plan.updated` with structured entries |
| `item/started` | `item.created` with streaming/running status |
| `item/autoApprovalReview/started` | approval policy metadata plus provider notification |
| `item/autoApprovalReview/completed` | approval policy/risk update plus provider notification |
| `item/completed` | item-specific completion mapping |
| `rawResponseItem/completed` | `native.unknown` or raw native row with expandable full payload |
| `item/agentMessage/delta` | `content.delta(Text)` |
| `item/plan/delta` | `content.delta(Text)` and partial `plan.updated` |
| `command/exec/outputDelta` | one-off command `content.delta(CommandOutput)` |
| `item/commandExecution/outputDelta` | command item `content.delta(CommandOutput)` |
| `item/commandExecution/terminalInteraction` | command item `content.delta(TerminalInput)` |
| `item/fileChange/outputDelta` | file-change item `content.delta(CommandOutput or Native)` |
| `item/fileChange/patchUpdated` | `diff.updated` and file-change item update |
| `serverRequest/resolved` | `approval.resolved` or `question.answered` when correlated |
| `item/mcpToolCall/progress` | MCP tool item provider/status content delta |
| `mcpServer/oauthLogin/completed` | provider notification category `mcp_oauth` |
| `mcpServer/startupStatus/updated` | provider notification category `mcp_status` |
| `account/updated` | provider notification category `account` |
| `account/rateLimits/updated` | `usage.updated` plus provider notification when user-visible |
| `app/list/updated` | provider notification category `apps` |
| `remoteControl/status/changed` | provider notification category `remote_control` |
| `externalAgentConfig/import/completed` | provider notification category `external_agent_config` |
| `fs/changed` | provider notification category `fs_watch` |
| `item/reasoning/summaryTextDelta` | `content.delta(Reasoning)` summary block |
| `item/reasoning/summaryPartAdded` | new reasoning summary block or block separator |
| `item/reasoning/textDelta` | raw reasoning block, visible in early research builds and expandable/collapsible in the browser |
| `thread/compacted` | deprecated compaction notification mapped to compaction item |
| `model/rerouted` | provider notification category `model` |
| `model/verification` | provider notification category `model` |
| `warning` | provider notification severity warning |
| `guardianWarning` | provider notification severity warning plus approval policy context |
| `deprecationNotice` | provider notification severity warning |
| `configWarning` | provider notification severity warning |
| `fuzzyFileSearch/sessionUpdated` | provider command result notification |
| `fuzzyFileSearch/sessionCompleted` | provider command result notification |
| `thread/realtime/started` | provider notification category `realtime` |
| `thread/realtime/itemAdded` | realtime provider notification or item if transcript-scoped |
| `thread/realtime/transcript/delta` | realtime transcript content delta when enabled |
| `thread/realtime/transcript/done` | realtime transcript content completion |
| `thread/realtime/outputAudio/delta` | artifact/native notification with raw payload available in debug dropdown |
| `thread/realtime/sdp` | provider notification/native row with raw payload available |
| `thread/realtime/error` | `error.reported` and provider notification |
| `thread/realtime/closed` | provider notification category `realtime` |
| `windows/worldWritableWarning` | provider notification severity warning |
| `windowsSandbox/setupCompleted` | provider notification category `windows_sandbox` |
| `account/login/completed` | provider notification category `account` |

## Thread Item Coverage

| Codex `ThreadItem` | Universal mapping |
| --- | --- |
| `UserMessage` | `item.created` role `User`, status `Completed`, text/block content |
| `HookPrompt` | system or hook tool item with fragments and hook run IDs |
| `AgentMessage` | assistant item with text content, deltas completed by final text |
| `Plan` | assistant plan item plus `plan.updated` |
| `Reasoning` | assistant reasoning item; summary visible and raw content available behind expansion |
| `CommandExecution` | tool item subkind `command`, command/cwd/process/status/output/exit/duration |
| `FileChange` | tool item subkind `file_change`, file diff blocks and `diff.updated` |
| `McpToolCall` | tool item kind `Mcp`, server/tool/arguments/result/error/duration |
| `DynamicToolCall` | unsupported notification unless dynamic tools are enabled |
| `CollabAgentToolCall` | tool item kind `Subagent`, child thread IDs and agent states |
| `WebSearch` | tool item subkind `web_search`, query/action/result status |
| `ImageView` | artifact/image item with local path URI policy |
| `ImageGeneration` | artifact/image item with revised prompt, result, saved path |
| `EnteredReviewMode` | review-mode item and session mode/provider notification |
| `ExitedReviewMode` | review-mode item and session mode/provider notification |
| `ContextCompaction` | system item and provider notification category `compaction` |

## Stage 0: Protocol Audit And Drift Guard

Owner: protocol-audit subagent.

Likely files:

- `docs/plans/2026-05-05-codex-app-server-uap2.md`
- `docs/runbooks/codex-app-server-protocol-audit.md`
- `crates/agenter-runner/tests/codex_protocol_coverage.rs`

Work:

- Build a small fixture or test that enumerates current Codex app-server
  `ClientRequest`, `ServerRequest`, `ServerNotification`, and `ThreadItem`
  variant names from `tmp/codex`.
- Encode the expected coverage rows from this plan.
- Make the test fail with a helpful message when Codex adds, renames, or removes
  a variant.
- Record the exact Codex checkout revision used by the audit.

Verification:

```sh
rg -n "client_request_definitions|server_request_definitions|server_notification_definitions|pub enum ThreadItem" tmp/codex/codex-rs/app-server-protocol/src/protocol
cargo test -p agenter-runner codex_protocol_coverage
```

Definition of done:

- Every current app-server client request, server request, server notification,
  and `ThreadItem` variant has a mapping row.
- The drift guard fails on an intentionally unlisted fixture variant.
- The runbook explains how to refresh the audit after updating `tmp/codex`.

Stage 0 completion notes:

- Added `crates/agenter-runner/tests/codex_protocol_coverage.rs` with a
  source-based parser for the current `common.rs` macro definitions and
  `v2.rs` `ThreadItem` enum.
- Added `docs/runbooks/codex-app-server-protocol-audit.md` with the refresh
  procedure and audited Codex revision
  `e4310be51f617f5e60382038fa9cbf53a2429ca4`.
- Verification note: `cargo test -p agenter-runner codex_protocol_coverage`
  passes against the current checkout.
- Coordinator verification repeated after Stage 1 schema changes:
  `cargo test -p agenter-runner codex_protocol_coverage` passed.

Risks:

- Codex uses macros for request/notification definitions, so the guard may need
  source parsing instead of Rust enum reflection.

## Stage 1: UAP/2 Schema Gaps

Owner: core-protocol subagent.

Likely files:

- `crates/agenter-core/src/session.rs`
- `crates/agenter-core/src/events.rs`
- `crates/agenter-core/src/approval.rs`
- `crates/agenter-protocol/src/runner.rs`
- `web/src/lib/sessionSnapshot.ts`
- `web/src/lib/universalEvents.ts`
- new ADR under `docs/decisions/`

Work:

- Add the protocol changes listed above.
- Keep wire compatibility for existing ACP/fake providers.
- Update serialization/deserialization tests for new command, approval, question,
  capability, and tool projection fields.
- Add raw native payload carriage for universal events and provider command
  results. Prefer a single shape that works for decoded and undecoded Codex
  payloads, such as `native.raw_payload`, so browser rendering can always show
  the original app-server JSON.
- Update browser snapshot materialization so new fields round-trip without Codex
  rendering branches.
- Write an ADR explaining why the UAP changes are cross-provider semantic
  concepts or capability-gated provider details.

Verification:

```sh
cargo test -p agenter-core
cargo test -p agenter-protocol browser runner
cd web && npm run test -- sessionSnapshot universalEvents
```

Definition of done:

- Existing providers still serialize and replay their current events unchanged.
- New Codex-required fields are available without `provider_id == "codex"`
  checks in shared browser normalizers.
- A universal event with `native.raw_payload` survives runner serialization,
  control-plane cache/replay, snapshot materialization, and frontend parsing.
- The ADR states the reason for each protocol change and its compatibility
  impact.

Stage 1 completion notes:

- Added `AgentProviderId::CODEX`, `UniversalCommand::ForkSession`,
  `ApprovalKind::Permission`, `QuestionState.native_request_id`,
  `QuestionState.native_blocking`, `AgentQuestionField.schema`,
  `ToolProjection.subkind`, and `NativeRef.raw_payload`.
- Added `docs/decisions/2026-05-05-uap2-codex-schema-gaps.md`.
- Added `migrations/0013_permission_approval_kind.sql` for DB approval-kind
  constraints.
- Verification note: `cargo test -p agenter-core`,
  `cargo test -p agenter-protocol browser`,
  `cargo test -p agenter-protocol runner`,
  `cd web && npm run test -- sessionSnapshot universalEvents normalizers`,
  `cargo check --workspace`, and `git diff --check` passed.

Risks:

- Adding too many first-class enums can overfit Codex. Prefer capability details
  or provider command metadata for surfaces that are not core chat semantics.

## Stage 2: Codex Transport And Codec

Owner: codex-transport subagent.

Likely files:

- `crates/agenter-runner/src/agents/codex/mod.rs`
- `crates/agenter-runner/src/agents/codex/transport.rs`
- `crates/agenter-runner/src/agents/codex/codec.rs`
- `crates/agenter-runner/src/agents/mod.rs`
- `crates/agenter-runner/Cargo.toml`

Work:

- Launch and supervise Codex app-server in the target workspace.
- Send `initialize` with the adapter's supported experimental API posture.
- Implement JSON-RPC request, response, notification, and server-request
  correlation.
- Preserve native request IDs, method names, and full native request/response/
  notification payloads on adapter output.
- Handle process exit, malformed frames, request timeout, lag/backpressure, and
  app-server disconnect as visible universal degraded/error events.
- Keep raw wire logging local and opt-in as a supplement, but do not rely on it
  for browser debugging because raw payloads should already travel with events.

Verification:

```sh
cargo test -p agenter-runner codex_transport
cargo test -p agenter-runner codex_codec
```

Definition of done:

- Fake app-server tests cover client request/response, server request/response,
  notification dispatch, malformed JSON, request timeout, and process exit.
- A disconnected app-server marks sessions `Degraded` or `Stopped` truthfully.
- Transport fixtures prove decoded and undecoded native frames keep the full raw
  JSON payload through the adapter output.

Completion notes, 2026-05-05:

- Added `crates/agenter-runner/src/agents/codex/{codec,transport}.rs` and
  exposed the `agents::codex` module.
- `CodexCodec` now correlates client responses/errors with pending methods,
  classifies known app-server requests/notifications, and preserves decoded,
  undecoded, and malformed wire data for downstream `NativeRef.raw_payload`.
- `CodexTransport` now spawns `codex app-server`, initializes with client
  capabilities, queues unrelated server frames while waiting for a response,
  reports request timeouts, and exposes process exits with stderr excerpts.
- Verification note: `cargo test -p agenter-runner codex_codec`,
  `cargo test -p agenter-runner codex_transport`,
  `cargo check --workspace`, `cargo fmt --all -- --check`, and
  `git diff --check` passed after QA fixes.
- Deferred truthfulness check: session-level `Degraded`/`Stopped` projection is
  implemented in Stage 3 because Stage 2 owns only the process/codec surface.

Risks:

- App-server JSON-RPC may not match generic JSON-RPC assumptions in all fields.
  The codec should follow Codex protocol types, not a hand-rolled shape.

## Stage 3: Session Lifecycle And Discovery

Owner: codex-session subagent.

Likely files:

- `crates/agenter-runner/src/agents/codex/session.rs`
- `crates/agenter-runner/src/agents/codex/reducer.rs`
- `crates/agenter-runner/src/agents/codex/id_map.rs`
- control-plane import/discovery tests if needed

Work:

- Map `StartSession` to `thread/start`.
- Map `LoadSession` to `thread/resume`.
- Map new `ForkSession` to `thread/fork`.
- Use `thread/list`, `thread/loaded/list`, `thread/read`, and
  `thread/turns/list` for discovery and history import.
- Map archive, unarchive, close/unsubscribe, name, goal, metadata, status, and
  token usage changes into universal session events or provider notifications.
- Preserve Codex `Thread` metadata: preview/name/title, cwd/path, model provider,
  timestamps, agent role/nickname, git info, source, and native thread ID.

Verification:

```sh
cargo test -p agenter-runner codex_session_lifecycle
cargo test -p agenter-runner codex_history_import
cargo test -p agenter-control-plane universal_discovered_history
```

Definition of done:

- Create, resume, fork, archive, unarchive, close, list, and history import are
  covered by fake app-server tests.
- Imported items get stable universal IDs across repeated imports.
- Session status and titles never fall back to raw UUIDs when Codex provides a
  human-readable name or preview.

Completion notes, 2026-05-06:

- Added `crates/agenter-runner/src/agents/codex/{id_map,session}.rs` and
  exposed them from the Codex module.
- `CodexSessionClient` now maps `thread/start`, `thread/resume`,
  `thread/fork`, `thread/list`, `thread/loaded/list`, `thread/read`,
  `thread/turns/list`, archive/unarchive/unsubscribe, name, goal, and metadata
  helper requests through the Stage 2 transport.
- Typed thread/turn/item import structs preserve Codex native thread IDs,
  name/preview-derived titles, cwd/path, model/provider metadata, timestamps,
  git/sub-agent metadata, statuses, and raw response/thread/turn/item payloads.
- Lifecycle request structs include flattened extra native params so current or
  future Codex-specific request fields can pass through without being dropped.
- `CodexIdMap` creates deterministic per-session turn and item IDs so repeated
  history imports reuse the same Agenter IDs.
- Verification note: `cargo test -p agenter-runner codex_session_lifecycle`,
  `cargo test -p agenter-runner codex_history_import`,
  `cargo test -p agenter-runner codex_id_map`, `cargo fmt --all -- --check`,
  `git diff --check`, and `cargo check --workspace` passed.

Risks:

- Codex history may be large. Import must be paged and should not block runner
  heartbeat or browser snapshot requests.

## Stage 4: Turn Command Mapper

Owner: codex-turns subagent.

Likely files:

- `crates/agenter-runner/src/agents/codex/turns.rs`
- `crates/agenter-runner/src/agents/codex/settings.rs`
- `crates/agenter-core/src/events.rs`

Work:

- Map `StartTurn` to `turn/start` with cwd, model, effort, approval policy,
  sandbox policy, permissions, service tier, summary, personality, output
  schema, collaboration mode, and input blocks when available.
- Map `SendUserInput` to `turn/steer` with expected native turn ID.
- Map `CancelTurn` to `turn/interrupt`; support startup interrupt when native
  turn ID is unknown by following Codex TUI behavior only if the current
  app-server supports it.
- Map `SetModel`, `SetMode`, and `SetTurnSettings` into stored defaults and
  turn-start overrides; expose unsupported combinations as provider
  notifications.
- Map review, compaction, rollback, shell command, guardian approval, and
  background-terminal cleanup as provider commands.

Verification:

```sh
cargo test -p agenter-runner codex_turn_start
cargo test -p agenter-runner codex_turn_steer
cargo test -p agenter-runner codex_turn_interrupt
```

Definition of done:

- Start-turn tests verify all supported settings reach native `TurnStartParams`.
- Steering a live turn uses native expected-turn protection.
- Rejected steer, rejected interrupt, and unsupported settings produce visible
  `error.reported` or `provider.notification` events.
- Universal turn state remains truthful during running, waiting, interrupted,
  failed, and completed flows.

Completion notes, 2026-05-06:

- Added `crates/agenter-runner/src/agents/codex/turns.rs`.
- `CodexTurnClient` now maps `turn/start`, `turn/steer`, and
  `turn/interrupt` through the Stage 2 transport and retains raw request,
  raw response, native thread ID, native turn ID, and `NativeRef.raw_payload`.
- Universal input helpers encode Codex text/image input and preserve native-only
  turn fields through flattened `extra` params.
- Dynamic tool specs are intentionally not promoted to a typed supported field;
  callers can preserve native payloads through `extra` until the client-tool
  contract exists.
- QA fix: fake app-server tests now parse JSON fixtures instead of embedding
  JSON literals directly in Python source.
- Verification note: `cargo test -p agenter-runner codex_turn_commands`,
  `cargo fmt --all -- --check`, and `git diff --check` passed.

Risks:

- Some Codex settings are experimental or account-dependent. Capabilities must
  say degraded/unsupported instead of letting the UI assume availability.

## Stage 5: Notification And Item Reducer

Owner: codex-reducer subagent.

Likely files:

- `crates/agenter-runner/src/agents/codex/reducer.rs`
- `crates/agenter-runner/src/agents/codex/items.rs`
- `crates/agenter-runner/src/agents/codex/content.rs`
- `crates/agenter-runner/tests/fixtures/codex/`

Work:

- Implement the server notification and `ThreadItem` mapping tables above.
- Buffer deltas until their item exists; synthesize a degraded item if a terminal
  delta cannot be correlated after reconciliation.
- Map agent text, plan deltas, reasoning summary/raw deltas, command output,
  terminal input, file output, patch updates, MCP progress, usage, warnings,
  model reroute/verification, and realtime status.
- Attach the full raw Codex notification or `ThreadItem` payload to each
  universal event, including events that decode successfully into structured
  universal fields.
- Ensure completed `ThreadItem` payloads reconcile the streamed partial state
  rather than duplicate rows.
- Add golden fixtures for each `ThreadItem` variant and each notification family.

Verification:

```sh
cargo test -p agenter-runner codex_reducer
cargo test -p agenter-runner codex_item_fixtures
cargo test -p agenter-runner codex_notification_fixtures
```

Definition of done:

- Every current `ThreadItem` variant has a golden mapping.
- Every current `ServerNotification` variant has a golden mapping or explicit
  unsupported/degraded assertion.
- Replayed deltas plus terminal item completion produce one coherent universal
  item, not duplicate transcript rows.
- Golden fixtures assert that raw native payloads survive alongside decoded
  universal projections.
- Raw reasoning is visible in early research builds, with browser collapse/
  expansion to keep the transcript usable.

Completion notes, 2026-05-06:

- Added `crates/agenter-runner/src/agents/codex/reducer.rs`.
- The reducer now has source-backed mapping tables for every current Codex
  `ThreadItem` variant and every current server-notification method.
- History item and notification reduction emits `UniversalEventKind` outputs
  with stable turn/item IDs, native refs, full raw payloads, and native/degraded
  fallback rows for unknown or weakly decoded frames.
- Covered mappings include text, reasoning summary/raw deltas, plans, diffs,
  command/file output, MCP progress, realtime transcript/audio, warnings,
  account/config/model notifications, usage, and lifecycle/status updates.
- Tests prove current source coverage, raw payload preservation for all mapped
  variants/methods, unknown notification visibility, and delta/completion ID
  convergence.
- Verification note: `cargo test -p agenter-runner codex_reducer`,
  `cargo fmt --all -- --check`, `git diff --check`, and
  `cargo check --workspace` passed in the worker; focused reducer tests were
  rerun locally after review.

Risks:

- Some notifications are global rather than thread-scoped. The reducer must not
  attach global account/config/fs events to arbitrary active sessions.

## Stage 6: Approvals, Questions, And Server Requests

Owner: codex-obligations subagent.

Likely files:

- `crates/agenter-runner/src/agents/codex/obligations.rs`
- `crates/agenter-runner/src/agents/codex/server_requests.rs`
- `crates/agenter-control-plane/src/approvals.rs`
- `crates/agenter-control-plane/src/questions.rs`
- browser approval/question components if schema fields require rendering

Work:

- Map all server-request variants according to the server-request table.
- Persist enough pending native-request state to survive browser reload and
  control-plane/runner reconnect.
- Attach the original server-request payload to the emitted approval/question
  event so unresolved obligation bugs can be debugged from the browser.
- Clear pending state on outbound response, `serverRequest/resolved`, terminal
  item events, turn completion, and thread close.
- Preserve Codex approval decision semantics, including accept-once,
  accept-for-session, cancel, decline, proposed policy amendments, network
  amendments, permission scope, and strict auto-review fields.
- Render tool-user-input and MCP elicitation questions from universal
  `QuestionState` without Codex-specific UI branches.

Verification:

```sh
cargo test -p agenter-runner codex_approval_requests
cargo test -p agenter-runner codex_question_requests
cargo test -p agenter-control-plane approval question obligation
cd web && npm run test -- approvals questions sessionSnapshot
```

Definition of done:

- Command approvals use `approval_id` fallback to `item_id`.
- File-change, permission, tool-user-input, and MCP elicitation requests round
  trip with native response payloads.
- Approval/question rows expose the original native server-request payload in an
  expandable debug dropdown.
- Duplicate same resolution is idempotent; conflicting resolution is rejected
  visibly.
- Browser reload shows pending approvals/questions with enough detail to act.
- Dynamic tool calls are explicitly rejected unless a dynamic-tools capability
  and client-tool contract are implemented in the same stage.

Risks:

- Permission approval payloads are still security-sensitive semantically, even
  though this local research build sends them to the browser. The UI must display
  the actual native permission profile and scope before allowing a grant.

Stage 6 worker note:

- Runner-local Codex server request mapping exposes stable native request IDs
  and deterministic approval/question IDs. Duplicate same-resolution idempotency
  and conflicting-resolution rejection remain control-plane obligation lifecycle
  responsibilities.

Completion notes, 2026-05-06:

- Added `crates/agenter-runner/src/agents/codex/obligations.rs`.
- Command, file-change, permission, legacy exec/patch, tool-user-input, MCP
  elicitation, auth-token refresh, and dynamic-tool server requests now map to
  approval/question/native-unsupported outputs with stable IDs and full raw
  native payloads.
- Approval/question response builders preserve native request IDs and known
  Codex decision semantics, including accept-once, accept-for-session, decline,
  cancel, permission scope, and strict-auto-review provider payloads.
- Dynamic client-tool calls are explicitly rejected with a native
  `success: false` dynamic-tool response until the client-tool contract exists.
- QA fix: unsupported non-dynamic-tool requests now return JSON-RPC error
  payloads instead of incorrectly using the dynamic-tool response shape.
- Verification note: `cargo test -p agenter-runner codex_approval_requests`,
  `cargo test -p agenter-runner codex_question_requests`,
  `cargo fmt --all -- --check`, `git diff --check`, and
  `cargo check --workspace` passed in the worker; focused tests were rerun
  locally after review.

## Stage 7: Provider Command Manifest

Owner: codex-provider-commands subagent.

Likely files:

- `crates/agenter-runner/src/agents/codex/provider_commands.rs`
- `crates/agenter-runner/src/agents/codex/capabilities.rs`
- `crates/agenter-protocol/src/runner.rs`
- `web/src/lib/providerCommands.ts`

Work:

- Expose non-universal app-server client requests through
  `ListProviderCommands` and `ExecuteProviderCommand`.
- Group commands by feature family: thread maintenance, model/config/account,
  skills/plugins/apps, MCP, filesystem/device, realtime, one-off command,
  fuzzy search, platform setup, and feedback.
- Mark each command supported, degraded, unsupported, guarded, experimental, or
  platform-gated.
- Require control-plane authorization for guarded commands.
- Return structured provider command results and emit provider notifications for
  long-running or streaming command families.

Verification:

```sh
cargo test -p agenter-runner codex_provider_commands
cargo test -p agenter-protocol runner_provider_commands
cd web && npm run test -- providerCommands
```

Definition of done:

- Every non-universal `ClientRequest` variant has a provider-command manifest
  entry or a documented adapter-internal mapping.
- Unsupported commands return explicit unsupported results, never silent no-ops.
- Guarded commands cannot be invoked without an authorized Agenter user command.

Completion notes, 2026-05-06:

- Added `crates/agenter-runner/src/agents/codex/provider_commands.rs`.
- The Codex manifest now classifies every current app-server `ClientRequest`
  method as core, adapter-internal, deferred compatibility, or provider command.
- Provider command entries retain native method, category, human-readable label,
  params/response schema names or placeholders, response kind, availability
  policy, and `RawPayloadDisplay::Always` for the early research phase.
- Capability-detail helpers group manifest methods by feature family for later
  runner/browser surfaces.
- Verification note: `cargo test -p agenter-runner codex_provider_commands`,
  `cargo test -p agenter-runner codex_protocol_coverage`,
  `cargo fmt --all -- --check`, `git diff --check`, and
  `cargo check --workspace` passed in the worker; focused tests were rerun
  locally after review.
- Deferred implementation note: this stage is metadata-only. The actual
  `ListProviderCommands` / `ExecuteProviderCommand` runner and browser surfaces
  still belong to a later provider-command execution stage.

Risks:

- Exposing filesystem and account operations too broadly would expand Agenter's
  product surface. Keep these discoverable but guarded and capability-gated.

## Stage 8: Reliability, Replay, And Reconnect

Owner: codex-reliability subagent.

Likely files:

- `crates/agenter-runner/src/agents/codex/state.rs`
- `crates/agenter-runner/src/agents/codex/wal.rs`
- `crates/agenter-control-plane/src/runner_events.rs`
- `crates/agenter-control-plane/src/universal.rs`
- `web/src/lib/sessionSnapshot.ts`

Work:

- Rebuild native ID maps and pending obligation maps after runner restart using
  control-plane state plus Codex `thread/read`/`thread/turns/list`.
- Preserve event ordering with runner WAL/ack semantics and control-plane
  `snapshot_seq`, `replay_from_seq`, `replay_through_seq`, and
  `replay_complete`.
- Handle browser reload, browser WebSocket reconnect, control-plane/runner
  reconnect, app-server process crash, and missed native notifications.
- Mark turns `Detached`, `Interrupted`, `Failed`, or `Degraded` truthfully when
  native state cannot be proven.

Verification:

```sh
cargo test -p agenter-runner codex_reconnect
cargo test -p agenter-control-plane subscribe_snapshot runner_event approval question
cd web && npm run test -- sessionSnapshot universalEvents events
```

Definition of done:

- Pending command approval survives browser reload.
- Pending MCP elicitation survives control-plane/runner reconnect or is marked
  detached with a visible reason.
- Missed streamed deltas reconcile from `thread/read` without duplicating final
  items.
- App-server crash produces truthful degraded/stopped state and does not leave
  approvals forever pending.

Stage 8 worker note:

- Added runner-local reconnect helpers in
  `crates/agenter-runner/src/agents/codex/state.rs` for rebuilding Codex ID maps
  from imported thread/turn history, reconciling pending approval/question
  records against held native requests or imported waiting state, emitting
  detached approval/question events with preserved native raw payloads when
  native state cannot be proven, mapping app-server process exit into degraded,
  error, then stopped universal outputs, and deduping imported final items
  against already-acked streamed WAL deltas.
- Verification note: `cargo test -p agenter-runner codex_reconnect`,
  `cargo fmt --all -- --check`, `git diff --check`, and
  `cargo check --workspace` passed.

Risks:

- Codex native history may not contain enough information to reconstruct every
  transient request. In that case, Agenter must mark the obligation detached
  rather than inventing a successful state.

## Stage 9: Browser Rendering And UX

Owner: browser-uap subagent.

Likely files:

- `web/src/lib/sessionSnapshot.ts`
- `web/src/lib/universalEvents.ts`
- `web/src/lib/events.ts`
- `web/src/components/ChatRoute.svelte`
- `web/src/components/SessionTreeSidebar.svelte`
- approval/question/tool row components

Work:

- Render Codex-derived events through universal item, tool, plan, diff,
  approval, question, usage, artifact, and provider-notification fields.
- Add generic UI for new tool subkinds and permission approvals.
- Show context compaction, review mode, guardian warnings, model reroute, and
  account/rate-limit notifications as compact visible state.
- Add a generic expandable raw-payload dropdown for every event, provider
  command result, approval, and question that carries native JSON.
- Render undecoded events as compact native rows with method/type/id summary and
  the full raw payload in the dropdown.
- Preserve mobile list-first and full-screen chat behavior.

Verification:

```sh
cd web
npm run test -- sessionSnapshot universalEvents events approvals questions providerCommands
npm run check
npm run lint
```

Definition of done:

- No shared transcript renderer needs Codex-specific item branching for core
  chat behavior.
- Plan, diff, approval, question, command, reasoning, usage, compaction, and
  review-mode rows render from universal data.
- Decoded and undecoded native payloads are visible in the browser without using
  local log files.
- Pending approvals/questions remain actionable after reload.
- Long labels and raw native details do not overflow compact/mobile layouts.

Stage 9 completion notes, 2026-05-06:

- Browser chat materialization now carries universal native raw payloads into
  inline tool/command/file/event rows, subagent rows, approval rows, question
  rows, error rows, and provider/native artifact rows without provider-specific
  Codex branches.
- Added generic display support for tool `subkind`, permission approval kind and
  native request metadata, question schema/default metadata, provider
  notifications, compaction/review-mode tool rows, and native/unknown rows.
- Added a shared raw-payload dropdown component used by inline events,
  subagent/provider payloads, approval/question cards, and error/native rows.
- Provider-command-specific browser surface is still not present; existing
  slash/provider payload API normalization remains the only web command-result
  surface in this stage.
- Verification note: `cd web && npm run test -- sessionSnapshot
  universalEvents normalizers approvals questions`, `npm run check`,
  `npm run lint`, and repo-root `git diff --check` passed.

Risks:

- Provider notifications and raw native rows can become noisy. Browser rendering
  should make them easy to collapse/filter while keeping all payloads reachable
  during the early research phase.

## Stage 10: Live Smoke, Docs, And Completion Gate

Owner: integration subagent.

Likely files:

- `docs/runbooks/codex-app-server-smoke.md`
- `docs/decisions/`
- `docs/harness/VERIFICATION.md` if new repeatable commands are added
- this plan's status section

Work:

- Write a live Codex app-server smoke runbook.
- Record exact Codex command/version, workspace path, prompt, and observed
  events.
- Verify plan, approval/question, command output, file change/diff, reasoning,
  interrupt, browser reload, runner reconnect, app-server crash, and history
  import.
- Verify at least one decoded event and one intentionally undecoded/native event
  show their full raw payload in the browser.
- Update this plan with completion notes and any unresolved questions.
- Add ADRs for lasting protocol/security/storage changes.

Verification:

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
cd web
npm run check
npm run lint
npm run test
npm run build
git diff --check
```

Definition of done:

- Full automated gate passes, or each unavailable command has a recorded
  environment limitation and next step.
- Live smoke records the exact native Codex app-server revision and observed
  universal events.
- Live smoke records browser evidence that raw native payloads are visible for
  decoded and undecoded events.
- Documentation, ADRs, and runbooks match implemented behavior.
- No known app-server protocol variant is unmapped, silently dropped, or hidden
  behind an undocumented provider-specific branch.

Risks:

- Live Codex behavior can drift with the checked-out repo. Stage 0's drift guard
  must be run whenever `tmp/codex` changes.

Stage 10 checkpoint notes, 2026-05-06:

- Added `docs/runbooks/codex-app-server-smoke.md` with exact commands for
  recording the Codex checkout revision, installed CLI path/version, app-server
  help surface, protocol schema generation, focused automated helper tests, and
  future live Agenter browser smoke execution.
- Current Codex checkout revision recorded by
  `git -C tmp/codex rev-parse HEAD`:
  `e4310be51f617f5e60382038fa9cbf53a2429ca4`.
- Current installed CLI path recorded by `command -v codex`:
  `/Users/maxim/.nvm/versions/node/v20.19.2/bin/codex`.
- Current installed CLI version recorded by `codex --version`:
  `codex-cli 0.128.0`. The command also printed
  `WARNING: proceeding, even though we could not update PATH: Operation not
  permitted (os error 1)`.
- `codex app-server --help`, `codex app-server generate-json-schema
  --experimental --help`, and `codex app-server generate-ts --experimental
  --help` were available and printed the experimental app-server command
  surfaces.
- Current live limitation: `rg -n
  "AGENTER_RUNNER_MODE.*codex|starting .*codex|CodexTransport|CodexSessionClient|CodexTurnClient"
  crates/agenter-runner/src/main.rs crates/agenter-runner/src/agents` found
  Codex helper clients under `crates/agenter-runner/src/agents/codex/`, but no
  Codex runner mode in `crates/agenter-runner/src/main.rs`.
- Therefore the live Codex browser smoke was not executed in this stage. No
  claim is made that plan, approval, question, command, file-change, reasoning,
  interrupt, reload, reconnect, crash, history import, decoded raw payload, or
  unknown/native raw payload behavior passed against a live Codex app-server
  through Agenter.
- Automated verification known from previous stage notes covers focused helper
  modules and browser materialization only:
  `cargo test -p agenter-runner codex_protocol_coverage`,
  `cargo test -p agenter-runner codex_codec`,
  `cargo test -p agenter-runner codex_transport`,
  `cargo test -p agenter-runner codex_session_lifecycle`,
  `cargo test -p agenter-runner codex_history_import`,
  `cargo test -p agenter-runner codex_id_map`,
  `cargo test -p agenter-runner codex_turn_commands`,
  `cargo test -p agenter-runner codex_reducer`,
  `cargo test -p agenter-runner codex_approval_requests`,
  `cargo test -p agenter-runner codex_question_requests`,
  `cargo test -p agenter-runner codex_provider_commands`,
  `cargo test -p agenter-runner codex_reconnect`, and
  `cd web && npm run test -- sessionSnapshot universalEvents normalizers
  approvals questions`.
- Stage 10 verification run in this documentation pass:
  `git diff --check` passed. Source tests were intentionally not rerun because
  this worker owns docs/runbooks and plan status only and coordinator source
  verification is running separately.
- Follow-up integration requirement: add a Codex runner mode that wires the
  helper modules into live runner commands/events, then execute
  `docs/runbooks/codex-app-server-smoke.md` and record actual browser evidence.

## Stage 11: Live Runner Mode Wiring

Owner: integration pass.

Status: implemented; automated verification passed.

Files:

- `crates/agenter-runner/src/main.rs`
- `crates/agenter-runner/src/agents/codex/runtime.rs`
- `crates/agenter-runner/src/agents/codex/transport.rs`
- `justfile`
- `docs/runbooks/codex-app-server-smoke.md`

Work completed:

- Added `AGENTER_RUNNER_MODE=codex` and `--codex` runner mode selection.
- Added `CodexRunnerRuntime`, a single actor that owns the mutable Codex
  app-server transport for one runner workspace.
- The runtime spawns `codex app-server --listen stdio://`, initializes it, maps
  create/resume/send/steer/interrupt/approval/question/refresh/shutdown runner
  commands, and emits direct `uap/2` events through the existing runner WAL
  path.
- Server notifications go through `CodexReducer`; server requests go through
  `CodexObligationMapper`; unsupported requests are answered and surfaced as
  visible provider notifications.
- Provider command listing now exposes the Codex manifest. Execution is
  intentionally guarded: supported commands execute, confirmed guarded thread
  maintenance commands execute, and high-risk/global command families return an
  explicit unsupported result with provider payload.
- Runner restart follow-ups must rehydrate Codex native thread state with
  `thread/resume` before `turn/start`/`turn/steer`. Persisted Agenter
  `external_session_id` values identify the native thread, but a fresh Codex
  app-server process may not have that thread loaded in memory.
- Added `just codex-runner` and `just codex-runner-json`.
- Updated the live smoke runbook to use `AGENTER_LOG_PAYLOADS=1 just
  codex-runner /private/tmp/agenter-codex-smoke`.

Verification notes:

- `cargo test -p agenter-runner codex_runtime` passed.
- `cargo test -p agenter-runner codex_` passed.
- `cargo test -p agenter-runner fake` passed.
- `cargo test -p agenter-runner acp` passed.
- `cargo test -p agenter-protocol runner` passed.
- `cargo test -p agenter-control-plane approval` passed; the requested
  `runner_event` and `question` filters currently match zero control-plane
  tests, so full workspace verification remains the control-plane regression
  gate.
- Full gate passed after Stage 11 wiring: `cargo fmt --all -- --check`,
  `cargo check --workspace`, `cargo clippy --workspace -- -D warnings`,
  `cargo test --workspace`, `cd web && npm run check`, `cd web && npm run
  lint`, `cd web && npm run test`, `cd web && npm run build`, and
  `git diff --check`.

Live smoke status:

- Not yet executed in this implementation pass. The code and `just` recipe are
  now present so the runbook can be executed against a real local Codex account
  and disposable workspace.

## Stage 12: Reload Row Sequencing Hardening

Owner: live browser debugging pass.

Status: implemented; live re-smoke pending.

Work completed:

- Browser reload with `include_snapshot=true` must replay through the durable
  snapshot cursor. Local research bandwidth is intentionally not constrained;
  the browser needs complete event chronology to rebuild row order for plans,
  questions, approvals, diffs, and native rows.
- Cursor-only subscriptions without a full snapshot remain bounded by the
  live replay limit and report incomplete replay when the requested event range
  is too large.
- Structured obligations now receive durable ordering timestamps when the
  runner omits native timestamps: `question.requested` and `approval.requested`
  use the universal envelope timestamp as `requested_at`.
- `plan.updated` uses the envelope timestamp when missing, but preserves an
  existing plan's earlier anchor timestamp when a later resume/history import
  repeats the same content.
- Frontend snapshot-only fallback ordering is now deterministic and avoids
  assigning identical row indexes to multiple timestamp-anchored structured
  rows.

Verification notes:

- `cargo test -p agenter-control-plane
  reducer_sets_missing_structured_request_timestamps_from_envelope` passed.
- `cargo test -p agenter-control-plane
  db_snapshot_replay_with_snapshot_is_not_truncated_at_live_replay_limit`
  compiled and was ignored because it requires `DATABASE_URL` for disposable
  Postgres.
- `cd web && npm run test -- sessionSnapshot` passed.

## Subagent Coordination

Suggested parallelization after Stage 0:

- Stage 1 blocks most implementation and should be reviewed first.
- Stages 2 and 3 can run in parallel once the transport interface is sketched,
  but only Stage 2 owns JSON-RPC process code.
- Stages 4, 5, and 6 can run in parallel after Stage 1 if they write to
  separate modules: turns, reducer/items, and obligations.
- Stage 7 can run alongside Stages 4-6 because provider commands are outside
  core turn reduction.
- Stage 8 integrates the outputs of Stages 2-7.
- Stage 9 starts after universal event shapes are stable enough to render.
- Stage 10 is the final integration and documentation pass.

Review gates:

- After Stage 0: confirm the coverage matrix is exhaustive against current
  `tmp/codex`.
- After Stage 1: confirm every protocol change has an ADR-quality reason.
- After Stage 6: manually review that permission and filesystem payloads are
  visible in the browser, because local research debugging currently takes
  priority over redaction.
- After Stage 8: manually review reload/reconnect/orphaned obligation behavior.
- Before Stage 10: run protocol drift guard again against the current Codex
  checkout.

## Open Questions To Confirm

- Stage 11 adds `AGENTER_RUNNER_MODE=codex` and `just codex-runner` so the live
  smoke can run through the normal Agenter stack.
- What debug-only hook should be used to inject or surface an intentionally
  unknown Codex app-server frame so the browser can prove live unknown/native
  raw payload visibility without waiting for upstream protocol drift?
- Should Agenter expose Codex realtime controls in the browser's first Codex
  release, or keep them command-only and hidden behind capability status?
- Should one-off `command/exec` rows appear in the main transcript when invoked
  from provider commands, or only in a command console surface?
- Which Codex account/config commands should be available to non-admin Agenter
  users in shared deployments?
- When the adapter stabilizes, which raw payload categories should be reduced,
  redacted, or moved back to local-only logs?

Each question above has a conservative default in this plan: command-only
realtime, command-console one-off exec, admin-only account/config mutation, and
no raw-payload reduction during the early local research phase.
