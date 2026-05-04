# Codex Protocol/TUI Parity Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development` for implementation. Assign one stage per worker unless a stage explicitly says it depends on another stage. Workers are not alone in this codebase: do not revert unrelated edits, and adjust to concurrent changes made by other workers.

Status: ready for implementation
Date: 2026-05-04
Reference: local `tmp/codex` app-server protocol/TUI snapshot at `637f7dd6d7`

**Goal:** Make Agenter's Codex adapter explicitly track Codex app-server/TUI parity for server requests, notifications, capabilities, and turn state transitions.

**Architecture:** Treat Codex app-server as the native source of truth. The runner owns protocol classification and native request/response handling, then projects supported behavior into universal events and browser snapshots. Unsupported or degraded Codex features must be visible as explicit capability gaps, not silent `native.unknown` rows or hidden JSON-RPC failures.

**Tech Stack:** Rust runner/control-plane/protocol crates, Svelte browser UI, Codex app-server protocol snapshot under `tmp/codex`, existing universal protocol event log.

---

## Current Evidence

Already implemented in the current checkout:

- `crates/agenter-runner/src/agents/codex.rs` handles approval/question requests without blocking the turn loop while waiting for browser answers.
- Pending Codex server requests are tracked by native JSON-RPC request id.
- `serverRequest/resolved` clears pending approval/question state.
- `turn/completed` with `failed`, `cancelled`, or `interrupted` now maps to terminal universal turn states.
- Question requested/answered events are projected into universal snapshots.
- Targeted tests exist for failed turn completion and native request-id matching.

Remaining risks:

- Coverage against Codex app-server protocol is manual.
- Unsupported server requests are still handled by a broad fallback.
- Provider capabilities do not describe Codex method families or degraded support.
- Turn lifecycle is implicit in control flow rather than modeled as a state machine.
- Many TUI-visible notifications are still generic native/provider events.
- Browser-facing commands expose only a small subset of Codex app-server client requests.

## Stage 0: Baseline Inventory And Guardrails

**Owner:** Explorer or worker in read-mostly mode.

**Files:**

- Read: `tmp/codex/codex-rs/app-server-protocol/src/protocol/common.rs`
- Read: `tmp/codex/codex-rs/tui/src/app/app_server_adapter.rs`
- Read: `tmp/codex/codex-rs/tui/src/app/app_server_requests.rs`
- Read: `crates/agenter-runner/src/agents/codex.rs`
- Read: `crates/agenter-runner/src/agents/adapter.rs`
- Read: `crates/agenter-core/src/session.rs`
- Read: `web/src/api/types.ts`
- Modify: this plan only if the inventory disproves a stage below

- [ ] Confirm the current uncommitted checkout state with `git status --short`.
- [ ] List every Codex `ServerRequest`, `ServerNotification`, and high-value `ClientRequest` from `common.rs`.
- [ ] Classify each method as `supported`, `degraded`, `unsupported`, `ignored`, or `not_applicable_to_remote_runner`.
- [ ] Compare the classification against current handlers in `codex.rs`.
- [ ] Record any mismatch in the relevant later stage before implementation begins.

**Verification:**

- Run: `git status --short`
- Run: `cargo test -p agenter-runner codex_`

**Exit Criteria:**

- The implementation workers know the exact current gaps.
- No source code is changed in this stage except this plan if the audit changes scope.

## Stage 1: Generated Codex Protocol Coverage Matrix

**Owner:** Worker 1.

**Files:**

- Create: `crates/agenter-runner/src/agents/codex_protocol_coverage.rs`
- Modify: `crates/agenter-runner/src/agents/mod.rs` or `crates/agenter-runner/src/agents/codex.rs`, following existing module style
- Modify: `docs/runbooks/codex-app-server-spike.md`
- Test: `crates/agenter-runner/src/agents/codex_protocol_coverage.rs`

- [ ] Add a checked-in classification table for Codex methods with fields: `direction`, `method`, `support`, `agenter_surface`, `notes`.
- [ ] Parse the local protocol snapshot in tests, or embed a minimal extracted method list generated from `tmp/codex/codex-rs/app-server-protocol/src/protocol/common.rs`.
- [ ] Add a test that fails when a protocol method exists in the snapshot but is missing from Agenter's classification table.
- [ ] Classify current server requests:
  - `item/commandExecution/requestApproval`: `supported`
  - `item/fileChange/requestApproval`: `supported`
  - `item/tool/requestUserInput`: `supported`
  - `mcpServer/elicitation/request`: `supported`
  - `item/permissions/requestApproval`: `supported`
  - `item/tool/call`: `degraded`
  - `account/chatgptAuthTokens/refresh`: `degraded`
  - `execCommandApproval`: `degraded` or `supported`, matching the current legacy approval decision
  - `applyPatchApproval`: `degraded` or `supported`, matching the current legacy approval decision
- [ ] Classify current notification families into typed support buckets instead of one-off notes.
- [ ] Document how to refresh the matrix when `tmp/codex` changes.

**Verification:**

- Run: `cargo test -p agenter-runner codex_protocol_coverage`
- Run: `cargo test -p agenter-runner codex_`

**Exit Criteria:**

- A new Codex protocol method cannot silently appear without forcing an Agenter classification update.
- The classification table is stable enough for later workers to use as the implementation contract.

## Stage 2: Provider Capability Metadata

**Owner:** Worker 2.

**Files:**

- Modify: `crates/agenter-core/src/session.rs`
- Modify: `crates/agenter-protocol/src/runner.rs`
- Modify: `crates/agenter-runner/src/main.rs`
- Modify: `crates/agenter-control-plane/src/state.rs`
- Modify: `web/src/api/types.ts`
- Modify: `web/src/lib/sessionSnapshot.ts`
- Test: relevant Rust serialization tests in `crates/agenter-core` and `crates/agenter-protocol`
- Test: relevant web snapshot/capability tests

- [ ] Extend capability representation with provider-specific method-family metadata without breaking existing boolean fields.
- [ ] Add a stable shape such as:

```rust
pub struct ProviderCapabilityDetail {
    pub key: String,
    pub status: ProviderCapabilityStatus,
    pub methods: Vec<String>,
    pub reason: Option<String>,
}

pub enum ProviderCapabilityStatus {
    Supported,
    Degraded,
    Unsupported,
    NotApplicable,
}
```

- [ ] Advertise Codex method families from the Stage 1 table, including dynamic tools, MCP, realtime, fuzzy search, account auth, filesystem/config/plugin operations, and one-off terminal sessions.
- [ ] Keep current generic booleans (`interrupt`, `approvals`, `tool_user_input`, `mcp_elicitation`) accurate.
- [ ] Make browser snapshot hydration preserve the provider-specific details.
- [ ] Add frontend typing for provider capability details.

**Verification:**

- Run: `cargo test -p agenter-core capabilities`
- Run: `cargo test -p agenter-protocol capabilities`
- Run: `cargo test -p agenter-runner capabilities`
- Run from `web/`: `npm run test -- sessionSnapshot`

**Exit Criteria:**

- UI/API consumers can tell that a Codex method family is supported, degraded, unsupported, or not applicable before triggering it.
- Existing capability consumers still work with the generic boolean fields.

## Stage 3: Explicit Codex Turn State Machine

**Owner:** Worker 3.

**Files:**

- Modify: `crates/agenter-runner/src/agents/codex.rs`
- Optionally create: `crates/agenter-runner/src/agents/codex_turn_state.rs`
- Test: `crates/agenter-runner/src/agents/codex.rs` or new module tests

- [ ] Introduce an explicit `CodexTurnDriverState` with at least:
  - `Idle`
  - `Starting`
  - `Running`
  - `WaitingForApproval`
  - `WaitingForInput`
  - `Interrupting`
  - `Completed`
  - `Failed`
  - `Cancelled`
  - `Interrupted`
  - `Detached`
- [ ] Route `turn/start` request, `turn/started`, approval/question requests, `serverRequest/resolved`, browser answers, `turn/interrupt`, and `turn/completed` through state transitions.
- [ ] Make illegal transitions log as warnings with native payload context rather than panic.
- [ ] Preserve current non-blocking approval/question delivery.
- [ ] Ensure interrupt before `turn/start` response sends startup interrupt with thread id and then continues reading until Codex returns a terminal state or start response.
- [ ] Ensure interrupt after `turn_id` is known sends `turn/interrupt` with both `threadId` and `turnId`.
- [ ] Ensure pending approval/question cleanup runs on all terminal states.
- [ ] Emit universal session status changes from state transitions rather than ad hoc branches.

**Verification:**

- Run: `cargo test -p agenter-runner codex_turn`
- Run: `cargo test -p agenter-runner interrupt_cancels_blocked_approval_for_same_session`
- Run: `cargo test -p agenter-runner interrupt_does_not_count_completed_approval_cancel_replay_as_new_cancel`

**Exit Criteria:**

- Turn state behavior is testable without reading the whole turn loop.
- The adapter cannot leave pending approvals/questions live after Codex reports a terminal turn state.

## Stage 4: Classified Server Request Dispatcher

**Owner:** Worker 4.

**Files:**

- Modify: `crates/agenter-runner/src/agents/codex.rs`
- Reuse or modify: `crates/agenter-runner/src/agents/codex_protocol_coverage.rs`
- Test: `crates/agenter-runner/src/agents/codex.rs`

- [ ] Replace the broad `unsupported_codex_server_request` fallback with a dispatcher that uses the Stage 1 classification table.
- [ ] For `supported` requests, call the existing approval/question handlers.
- [ ] For `degraded` requests, emit a stable `AgentErrorEvent` or `NativeNotification` with category `codex_capability_gap`, method, request id, thread id, and turn id.
- [ ] For `unsupported` known requests, reply with a deterministic JSON-RPC error and publish a visible native event.
- [ ] For unknown requests, publish a visible `codex_unknown_server_request` event and reply with a deterministic JSON-RPC error.
- [ ] Keep `account/chatgptAuthTokens/refresh` as a special degraded path with operator guidance to authenticate Codex on the runner host.
- [ ] Keep `item/tool/call` degraded until a remote executor design is approved.
- [ ] Add tests for supported, degraded, unsupported, and unknown request dispatch.

**Verification:**

- Run: `cargo test -p agenter-runner codex_server_request`
- Run: `cargo test -p agenter-runner codex_`

**Exit Criteria:**

- Every inbound Codex server request gets exactly one visible Agenter event and exactly one JSON-RPC response when required.
- New protocol drift is surfaced as a classification/test failure or an explicit unknown-request event, never a silent drop.

## Stage 5: High-Value Notification Projection

**Owner:** Worker 5.

**Files:**

- Modify: `crates/agenter-core/src/events.rs`
- Modify: `crates/agenter-runner/src/agents/adapter.rs`
- Modify: `crates/agenter-runner/src/agents/codex.rs`
- Modify: `crates/agenter-control-plane/src/state.rs`
- Modify: `web/src/api/types.ts`
- Modify: `web/src/lib/universalEvents.ts`
- Modify: `web/src/lib/sessionSnapshot.ts`
- Modify: `web/src/lib/chatEvents.ts` if row projection changes
- Test: Rust normalization/projection tests
- Test: web universal event/session snapshot tests

- [ ] Promote thread lifecycle and metadata:
  - `thread/started`
  - `thread/archived`
  - `thread/unarchived`
  - `thread/closed`
  - `thread/name/updated`
  - `thread/contextWindow/updated`
- [ ] Promote hook lifecycle:
  - `hook/started`
  - `hook/completed`
- [ ] Promote approval review and guardian flows:
  - `item/autoApprovalReview/started`
  - `item/autoApprovalReview/completed`
  - `guardianWarning`
- [ ] Promote terminal interaction:
  - `item/commandExecution/terminalInteraction`
- [ ] Promote MCP status/auth:
  - `item/mcpToolCall/progress`
  - `mcpServer/oauthLogin/completed`
  - `mcpServer/startupStatus/updated`
- [ ] Promote account/model/warning/config:
  - `account/updated`
  - `account/rateLimits/updated`
  - `model/rerouted`
  - `model/verification`
  - `warning`
  - `deprecationNotice`
  - `configWarning`
- [ ] Promote fuzzy search and filesystem/window sandbox notifications as native typed events if no richer universal state exists yet.
- [ ] Keep raw provider payload attached for debugging.
- [ ] Avoid over-modeling features not yet exposed in UI; a typed native notification category is acceptable when the state model is not ready.

**Verification:**

- Run: `cargo test -p agenter-runner codex_notification`
- Run: `cargo test -p agenter-control-plane snapshot`
- Run: `cargo test -p agenter-control-plane event`
- Run from `web/`: `npm run test -- sessionSnapshot`
- Run from `web/`: `npm run test -- universalEvents`

**Exit Criteria:**

- TUI-visible notification families no longer collapse indistinguishably into generic fallback rows.
- Browser replay and live events render the same normalized categories.

## Stage 6: Browser Rendering For Capability Gaps And Typed Notifications

**Owner:** Worker 6.

**Files:**

- Modify: `web/src/lib/chatEvents.ts`
- Modify: `web/src/routes/ChatRoute.svelte`
- Modify: relevant row components under `web/src/components/`
- Modify: `web/src/api/types.ts`
- Test: web chat event tests and component tests

- [ ] Render `codex_capability_gap` as a compact, readable event row.
- [ ] Render `codex_auth_refresh_required` as an operator-action error explaining that Codex auth must be refreshed on the runner host.
- [ ] Keep raw JSON available in existing detailed/debug transcript modes.
- [ ] Render hook, guardian/auto-approval review, terminal interaction, MCP status, fuzzy search, and warning/config events with stable category labels.
- [ ] Preserve transcript verbosity semantics: compact mode should not show raw protocol noise, debug mode should expose full payload.
- [ ] Confirm usage metrics still update from token/rate-limit events.

**Verification:**

- Run from `web/`: `npm run check`
- Run from `web/`: `npm run lint`
- Run from `web/`: `npm run test`
- Run from `web/`: `npm run build`

**Exit Criteria:**

- Browser UI makes degraded Codex capabilities and native lifecycle events visible without flooding normal chat mode.
- Debug/detail modes still expose raw payloads for protocol diagnosis.

## Stage 7: Incremental Provider Command Surface

**Owner:** Worker 7.

**Files:**

- Modify: `crates/agenter-runner/src/agents/codex.rs`
- Modify: `crates/agenter-runner/src/main.rs` if command routing changes
- Modify: `web/src/api/sessions.ts` only if API shape changes
- Test: runner command mapping tests

- [ ] Add only low-risk read/admin commands first:
  - `codex.rate_limits` -> `account/rateLimits/read`
  - `codex.mcp_status` -> `mcpServerStatus/list`
  - `codex.mcp_reload` -> `config/mcpServer/reload`
  - `codex.rename` -> `thread/name/set`
  - `codex.context_window` -> `thread/contextWindow/inspect`
- [ ] Gate commands by provider-specific capabilities from Stage 2.
- [ ] Keep dangerous or broad operations out of this stage:
  - filesystem write/remove/copy
  - plugin install/uninstall
  - realtime audio
  - arbitrary MCP tool execution
  - account login/logout
  - terminal write/resize/terminate
- [ ] Return structured `SlashCommandResult.provider_payload` for each command.
- [ ] Add tests for command id, method, parameter shape, missing active turn where relevant, and capability-disabled behavior.

**Verification:**

- Run: `cargo test -p agenter-runner codex_provider_command`
- Run: `cargo test -p agenter-runner codex_`

**Exit Criteria:**

- Agenter exposes a useful but conservative subset of Codex client requests.
- Risky command families remain explicitly classified but not wired.

## Stage 8: Live Provider Smoke And Runbook Update

**Owner:** Worker 8 or final integrator.

**Files:**

- Modify: `docs/runbooks/codex-app-server-spike.md`
- Modify: `docs/runbooks/universal-protocol-smoke.md`
- Modify: `docs/plans/2026-05-04-codex-protocol-tui-parity.md`

- [ ] Run an automated targeted gate:
  - `cargo fmt --all -- --check`
  - `cargo test -p agenter-runner codex_`
  - `cargo test -p agenter-control-plane snapshot`
  - `cargo test -p agenter-control-plane event`
  - `cargo test -p agenter-control-plane question`
  - `cargo test -p agenter-control-plane approval`
- [ ] Run the universal protocol focused smoke from `docs/harness/VERIFICATION.md` if the touched code intersects universal snapshots/replay.
- [ ] Run frontend gate if web files changed:
  - `cd web`
  - `npm run check`
  - `npm run lint`
  - `npm run test`
  - `npm run build`
- [ ] Manual Codex app-server smoke, when local auth allows:
  - create or resume a Codex thread
  - send a normal turn
  - trigger one command approval
  - trigger one question or MCP elicitation if feasible
  - interrupt an active turn
  - observe one unsupported/degraded request path if feasible
  - observe failed/interrupted/cancelled terminal handling
- [ ] Record exact provider command, Codex version, prompt, workspace path, and limitations in the runbook or plan.

**Verification:**

- Run the commands listed above.
- Run: `git diff --check`

**Exit Criteria:**

- The plan records what passed and what could not be run.
- Any unresolved provider behavior is documented as an open question, not left in chat history.

## Stage Dependencies

- Stage 0 should run first.
- Stage 1 should run before Stages 2, 4, 5, and 7.
- Stage 2 can run in parallel with Stage 3 after Stage 1 classification is available.
- Stage 4 depends on Stage 1 and should coordinate with Stage 3 if both touch the turn loop.
- Stage 5 can run after Stage 1 and mostly in parallel with Stage 3/4, but integration must reconcile event names.
- Stage 6 depends on Stages 2, 4, and 5.
- Stage 7 depends on Stage 2 and should not start until command capability keys are stable.
- Stage 8 is final integration.

## Open Decisions

- Whether `item/tool/call` should remain permanently degraded or become an Agenter-hosted remote executor feature.
- Whether provider-specific capabilities belong directly in `AgentCapabilities`, `CapabilitySet`, or a new provider metadata structure.
- Whether all high-value Codex notifications should become universal first-class variants or typed native notifications with stable categories.
- Whether account login/logout and plugin management should ever be exposed through Agenter's browser command surface.

## Completion Criteria

- Protocol coverage matrix exists and is enforced by tests.
- Codex provider capabilities describe supported/degraded/unsupported method families.
- Turn state transitions are explicit and tested.
- All Codex server requests are classified and visible.
- High-value TUI-visible notifications are typed enough for browser replay and debugging.
- Browser rendering shows capability gaps and auth-refresh guidance clearly.
- Verification from `docs/harness/VERIFICATION.md` has been run or limitations are recorded here.
