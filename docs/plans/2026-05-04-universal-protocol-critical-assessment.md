# Universal Protocol Critical Assessment And Hardening Plan

Status: proposed
Date: 2026-05-04
Related plans:

- `docs/plans/2026-05-03-universal-agent-protocol.md`
- `docs/plans/2026-05-04-codex-protocol-tui-parity.md`

## Goal

Critically assess the current `uap/1` protocol, state machines, and Codex adapter, then define concrete hardening work to remove fragile transitional architecture.

## Assessment Summary

The current direction is sound: Agenter should keep a universal, event-sourced, capability-gated browser/control-plane protocol, with Codex using the native app-server adapter and ACP-family providers using the generic ACP runtime.

The implementation is fragile because the migration is only partially complete. Universal events, legacy `NormalizedEvent` projection, in-memory caches, durable DB snapshots, runner WAL, runner pending maps, and control-plane approval registries all overlap. That makes correctness depend on cross-layer conventions instead of one clearly owned state machine.

The highest-risk area is not the browser replay path. Focused replay and runner-ack tests currently pass. The highest-risk area is live runner ownership across interruptions: the runner WebSocket loop still owns runtime channels and pending maps that should survive transport reconnects.

Verification performed during this assessment:

```sh
cargo test -p agenter-control-plane runner_event_ack_state_dedupes_replayed_sequences
cargo test -p agenter-control-plane seeded_runner_ack_marks_old_replay_as_duplicate
cargo test -p agenter-control-plane subscribe_snapshot_replays_after_seq_in_strict_order
cargo test -p agenter-runner codex_turn
```

All four focused checks passed.

## Findings And Solutions

### 1. Runner reconnect is not architecturally safe yet

Evidence:

- `run_codex_runner` creates the WebSocket, WAL, pending approval/question maps, event channels, and `CodexRunnerRuntime` after connecting, then consumes them inside the socket loop.
- If the control-plane WebSocket closes, the loop breaks and those receivers/maps are dropped while spawned turn tasks may still be running.
- The universal protocol plan already records this as incomplete: live adapter task survival still needs provider runtimes and pending maps hoisted outside transient socket lifetimes.

Impact:

- A control-plane disconnect during streaming can orphan native events.
- A disconnect while Codex is awaiting approval or input can drop the local waiter that would deliver the browser answer.
- A reconnect can replay WAL records, but it cannot reconstruct dropped live waiters.

Solutions:

- Preferred: introduce a reconnect-stable `RunnerSupervisor`. It owns provider runtimes, active turn handles, pending approval/question waiters, and a durable outbound event queue. A transport task only connects, sends hello, drains queued events, and delivers inbound commands to the supervisor.
- Smaller step: split current `run_codex_runner` and multi-provider runner loops into `RunnerRuntimeState` plus `RunnerTransportSession`, with all provider runtime state created before and outside each socket session.
- Avoid: adding more recovery branches inside the existing socket loop. That keeps transport lifetime coupled to provider lifetime.

### 2. Runner event dedupe is process-local

Evidence:

- Runner WAL records carry `runner_event_seq`.
- The control plane tracks `runner_event_acks` and `seen_runner_events` in memory.
- `agent_events` does not store `(runner_id, runner_event_seq)`.
- The existing plan explicitly says cross-control-plane-restart dedupe should move into durable storage.

Impact:

- DB-backed deployments are still vulnerable to duplicate append after a control-plane restart if a runner replays unacked WAL records.
- No-DB mode cannot honestly claim lossless restart behavior.

Solutions:

- Preferred: add `runner_event_receipts(runner_id, runner_event_seq, event_id, accepted_at)` with a unique key. Ack only after the same transaction commits the universal event and receipt.
- Alternative: add nullable `runner_id` and `runner_event_seq` columns to `agent_events` with a partial unique index.
- Recovery: on runner hello, derive the ack cursor from durable receipts, not from memory or runner-supplied hints alone.

### 3. `uap/1` is not versioned in universal envelopes

Evidence:

- `UniversalEventEnvelope` has ids, sequence, timestamp, source, native ref, and event, but no `protocol_version`.
- Runner transport has `PROTOCOL_VERSION = "0.1"`, which is not the same as the browser/control-plane universal schema version.

Impact:

- Future browser/control-plane compatibility becomes implicit.
- Snapshot replay cannot distinguish schema evolution from malformed old data.

Solutions:

- Add `protocol_version: "uap/1"` to universal event and command envelopes.
- Add browser subscription capability negotiation for supported `uap` versions and snapshot features.
- Keep old unversioned frames readable for one migration window, but write versioned frames only.

### 4. Universal-first is still partly legacy projection

Evidence:

- Provider output is still commonly produced as `NormalizedEvent` and converted to `UniversalEventKind` by `universal_event_from_normalized`.
- The conversion path infers turn ids, item ids, plan status, and tool shape from provider payload fragments.

Impact:

- Universal semantics are not fully owned by provider adapters.
- Provider payload shape changes can silently break universal projection.
- The generic projection layer knows too much about Codex/ACP payload details.

Solutions:

- Preferred: make provider adapters emit `AgentUniversalEvent` directly as their primary output.
- Keep `NormalizedEvent` only as a derived compatibility or debug projection until removed.
- Move provider-payload parsing into provider-specific normalizers with fixture tests against checked-in Codex and ACP traces.
- Add a lint-style test that prevents new runtime paths from sending provider events only through `NormalizedEvent` when a universal event is possible.

### 5. Codex turn scope filtering only checks thread id

Evidence:

- `CodexTurnScope` stores both thread and turn ids.
- `codex_message_belongs_to_scope` checks only thread id.

Impact:

- Stale or background turn-scoped events from the same thread can be accepted as part of the active turn.
- The current one-active-turn assumption masks this, but the code shape is brittle for reconnect, retries, background notifications, and future concurrency.

Solutions:

- Classify Codex notifications as `turn_scoped`, `thread_scoped`, or `global`.
- Require matching turn id for `turn_scoped` notifications once a turn id is known.
- Allow thread-only lifecycle, title, usage, and account events explicitly.
- Emit visible diagnostic `codex.scope_dropped` events in debug mode with safe native refs, instead of silently relying on logs.

### 6. Codex EOF without `turn/completed` is ambiguous

Evidence:

- If `server.next_message()` returns `None`, `run_codex_turn_on_server` exits `Ok(())`.
- Pending request cleanup currently runs on `turn/completed`, not on EOF.

Impact:

- A native app-server exit or transport failure during a turn can leave browser state stuck in running, waiting for approval, or waiting for input.
- Pending approvals/questions may remain present in the control plane with no live native waiter.

Solutions:

- Treat EOF during an active turn as `turn.detached` when native ownership is uncertain, otherwise `turn.failed`.
- Always drain pending server requests and emit resolved/orphaned obligation events on EOF and error paths.
- Add tests for EOF while running, EOF while waiting for approval, EOF while waiting for input, and EOF after interrupt.

### 7. Approval and question state is over-split

Evidence:

- Approval/question state exists in runner pending maps, control-plane memory, DB columns, and universal snapshots.
- Browser resolution can become `resolving` before runner/native acknowledgement, but stale or missing runner waiters still produce adapter-level errors.

Impact:

- Retries, reconnects, stale answers, and runner restarts are hard to reason about.
- The same user-visible card can be reconstructed from several sources with different truth levels.

Solutions:

- Preferred: introduce durable `agent_obligations` for approvals and user-input questions.
- Store session id, turn id, runner id, native request id, obligation kind, canonical options/fields, status, delivery generation, and resolution command id.
- Model lifecycle as `pending -> presented -> resolving -> delivered_to_runner -> accepted_by_native -> resolved`, plus `orphaned`, `expired`, and `detached`.
- Treat runner maps as runtime waiters only. They may accelerate native delivery, but they are not the canonical obligation state.

### 8. Capability model is useful but too shallow

Evidence:

- `CapabilitySet` exists, but important semantics are still coarse booleans.
- Provider-specific details exist, but universal capability fields do not yet describe native blocking behavior, policy patch support, replay guarantees, tool ownership, or plan implementation semantics.

Impact:

- Frontend code can still need provider-specific assumptions for nuanced behavior.
- Degraded provider commands may be visible only after a failed call.

Solutions:

- Expand capabilities for obligation semantics, native history, replay durability, tool execution ownership, plan approval/implementation, and provider-command safety.
- Require every degraded browser command to point to a provider capability detail.
- Add frontend tests proving UI affordances are gated by capabilities, not provider names.

### 9. Snapshot replay works, but truncation and live checkpoint semantics need protocol clarity

Evidence:

- `BrowserSessionSnapshot` returns snapshot, replay events, `latest_seq`, and `has_more`.
- Existing tests cover strict order and truncated replay behavior.
- The semantics are subtle: snapshot subscriptions may continue streaming live events even when historical replay is truncated.

Impact:

- Clients can accidentally advance cursors incorrectly if they treat `latest_seq` as always meaning fully replayed.
- Future non-browser connectors may implement the protocol incorrectly.

Solutions:

- Rename fields or add explicit cursors: `snapshot_seq`, `replay_from_seq`, `replay_through_seq`, and `replay_complete`.
- Document the exact reconnect algorithm in the protocol spec.
- Add protocol conformance tests at the JSON frame level, not only reducer-level Rust tests.

## Recommended Hardening Plan

### Phase 1: Runner supervisor and reconnect ownership

Files likely to change:

- `crates/agenter-runner/src/main.rs`
- New `crates/agenter-runner/src/supervisor.rs`
- Existing Codex/ACP runtime modules as needed for clean ownership boundaries

Work:

- Add `RunnerSupervisor` that owns provider runtimes, session bindings, pending waiters, active turn tasks, and outbound event queue.
- Make WebSocket connection sessions transient transport workers.
- Reconnect in a loop without reconstructing provider state.
- Ensure background discovery and operation updates use retryable operation delivery, while replayable agent events use WAL.

Verification:

```sh
cargo test -p agenter-runner supervisor
cargo test -p agenter-runner interrupt_cancels_blocked_approval_for_same_session
cargo test -p agenter-runner interrupt_does_not_count_completed_approval_cancel_replay_as_new_cancel
```

Exit criteria:

- Control-plane WebSocket disconnect does not drop active turn event receivers.
- Pending approval/question answer delivery still works after reconnect.

### Phase 2: Durable runner event receipts

Files likely to change:

- New migration under `migrations/`
- `crates/agenter-db/src/repositories.rs`
- `crates/agenter-control-plane/src/state.rs`
- `crates/agenter-control-plane/src/runner_ws.rs`

Work:

- Persist runner event receipts transactionally with universal event append.
- Deduplicate replay by durable `(runner_id, runner_event_seq)`.
- Derive ack cursor from DB on runner hello.
- Keep process-local dedupe only as a cache.

Verification:

```sh
cargo test -p agenter-db universal_event
cargo test -p agenter-control-plane runner_event
cargo test -p agenter-control-plane seeded_runner_ack_marks_old_replay_as_duplicate
```

Exit criteria:

- Control-plane restart cannot duplicate a replayed runner WAL event in DB-backed mode.

### Phase 3: Universal envelope versioning

Files likely to change:

- `crates/agenter-core/src/events.rs`
- `crates/agenter-protocol/src/browser.rs`
- `crates/agenter-protocol/src/runner.rs`
- `web/src/api/types.ts`
- `web/src/lib/sessionSnapshot.ts`

Work:

- Add `protocol_version: "uap/1"` to universal command/event envelopes.
- Accept legacy missing-version frames only where needed for migration compatibility.
- Add JSON round-trip tests for versioned browser snapshot and live universal event frames.

Verification:

```sh
cargo test -p agenter-core universal
cargo test -p agenter-protocol browser
cd web && npm run test -- sessionSnapshot universalEvents
```

Exit criteria:

- Browser/control-plane universal schema version is explicit on every new universal frame.

### Phase 4: Universal-first adapter output

Files likely to change:

- `crates/agenter-runner/src/agents/adapter.rs`
- `crates/agenter-runner/src/agents/codex.rs`
- `crates/agenter-runner/src/agents/acp.rs`
- `crates/agenter-control-plane/src/state.rs`

Work:

- Emit `AgentUniversalEvent` directly from Codex and ACP runtime paths.
- Keep `NormalizedEvent` as a derived/debug compatibility projection.
- Remove provider-specific payload scraping from generic universal projection.
- Add fixture-driven tests for Codex and ACP universal event sequences.

Verification:

```sh
cargo test -p agenter-runner codex_stage10_conformance_trace_preserves_expected_milestones
cargo test -p agenter-runner acp_stage10_provider_traces_share_prompt_plan_permission_shape
cargo test -p agenter-control-plane snapshot
```

Exit criteria:

- Runtime provider event flow is universal-first.
- Generic fallback projection is no longer required for primary Codex/ACP behavior.

Implementation status, 2026-05-05:

- Worker E added provider-owned universal mapping for primary Codex Stage 10 milestones: turn lifecycle, plan updates, command item/output/completion, diff updates, and approval requests.
- Worker E added provider-owned universal mapping for primary ACP-family Stage 10 updates: assistant message chunks/completion, plan updates, tool updates, permission requests, and provider errors.
- The ACP live runner channel now emits `AdapterEvent` into the runner WAL path instead of sending `NormalizedEvent` to `main.rs` for generic conversion.
- `NormalizedEvent` remains available as compatibility/debug projection, and the generic normalized-to-universal mapper remains as a fallback for events that do not yet have provider-owned universal semantics.
- Stage 10 fixture tests now assert provider-owned native summaries for primary milestones so Codex/ACP traces cannot satisfy the important checks solely through the generic fallback projection.
- Verification passed:
  - `cargo test -p agenter-runner codex_stage10_conformance_trace_preserves_expected_milestones`
  - `cargo test -p agenter-runner acp_stage10_provider_traces_share_prompt_plan_permission_shape`

### Phase 5: Codex state machine hardening

Files likely to change:

- `crates/agenter-runner/src/agents/codex.rs`
- `crates/agenter-runner/src/agents/codex_turn_state.rs`
- `crates/agenter-runner/src/agents/codex_protocol_coverage.rs`

Work:

- Classify Codex notifications by scope.
- Enforce turn id matching for turn-scoped events.
- Add EOF/error terminal transitions.
- Centralize pending request cleanup for all terminal exits.

Verification:

```sh
cargo test -p agenter-runner codex_turn
cargo test -p agenter-runner codex_protocol_coverage
cargo test -p agenter-runner codex_server_request
```

Exit criteria:

- No active Codex turn can exit without a terminal universal turn state.
- Pending approvals/questions cannot survive terminal Codex turn cleanup.

Implementation status, 2026-05-05:

- Worker C implemented Codex notification scope classification in the runner adapter.
- Turn-scoped Codex notifications now require the active turn id once known; same-thread thread/global notifications such as usage, title, and status still pass.
- EOF during an active Codex turn emits a detached terminal state and drains pending approval/question waiters.
- Read errors during an active Codex turn emit a failed terminal state and drain pending approval/question waiters.
- Focused tests cover stale same-thread turn notification drops, allowed thread-scoped notifications, EOF while running, EOF while waiting for approval, EOF while waiting for input, and error while waiting for approval.
- Verification passed:
  - `cargo test -p agenter-runner codex_turn`
  - `cargo test -p agenter-runner codex_protocol_coverage`
  - `cargo test -p agenter-runner codex_server_request`

### Phase 6: Durable obligation state machine

Files likely to change:

- New migration under `migrations/`
- `crates/agenter-core/src/approval.rs`
- `crates/agenter-control-plane/src/state.rs`
- `crates/agenter-control-plane/src/api/approvals.rs`
- `crates/agenter-runner/src/agents/approval_state.rs`
- `web/src/lib/sessionSnapshot.ts`

Work:

- Introduce durable obligations for approvals and user-input questions.
- Use one lifecycle for approval and question delivery/resolution.
- Keep native waiters runner-local and reconstructable from obligation state where possible.
- Add orphan/detached UI states.

Verification:

```sh
cargo test -p agenter-control-plane approval
cargo test -p agenter-control-plane question
cargo test -p agenter-runner interrupt_cancels_blocked_approval_for_same_session
cd web && npm run test -- sessionSnapshot universalEvents
```

Exit criteria:

- Browser reload, runner reconnect, stale answer, and duplicate answer behavior are deterministic and represented in universal snapshots.

Implementation status, 2026-05-05:

- Worker F added `agent_obligations` as an additive durable table for approvals and user-input questions, with a shared lifecycle from `pending` through delivery/resolution and terminal `orphaned`, `expired`, and `detached` states.
- `pending_approvals` remains in place for compatibility, including policy-rule and existing approval API paths; universal approval events now bridge into both `pending_approvals` and `agent_obligations`.
- Universal question events now bridge into `agent_obligations`, and the control-plane registry tracks pending question request state instead of a boolean only.
- Stopped, failed, or archived native session evidence marks unresolved questions as `orphaned` in the universal snapshot, matching existing approval orphan behavior. Transient runner disconnects remain non-orphaning.
- Browser snapshot materialization now preserves terminal question states such as `orphaned` and `detached`.
- Durable storage decision recorded in `docs/decisions/2026-05-05-durable-agent-obligations.md`.
- Verification passed:
  - `cargo test -p agenter-control-plane approval`
  - `cargo test -p agenter-control-plane question`
  - `cargo test -p agenter-runner interrupt_cancels_blocked_approval_for_same_session`
  - `cd web && npm run test -- sessionSnapshot universalEvents`

### Phase 7: Protocol conformance and live-provider smoke

Files likely to change:

- `docs/runbooks/universal-protocol-smoke.md`
- `docs/harness/VERIFICATION.md`
- `crates/agenter-runner/tests/fixtures/`
- Control-plane and frontend tests as needed

Work:

- Add JSON-frame conformance tests for snapshot/replay and live universal events.
- Add chaos tests for browser reconnect and runner reconnect during streaming, approval, question, interrupt, and Codex EOF.
- Record live Codex app-server smoke separately from automated fixture coverage.

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

Exit criteria:

- Automated tests prove the hardening behavior.
- Live-provider limitations are documented without overclaiming.

Implementation status, 2026-05-05:

- Worker G added a dedicated browser JSON-frame conformance integration test for versioned `session_snapshot` replay frames, truncated replay frames, and live `universal_event` frames.
- `docs/runbooks/universal-protocol-smoke.md` now separates automated JSON-frame/reconnect coverage from live-provider chaos drills.
- `docs/harness/VERIFICATION.md` now lists the focused Phase 7 checks, including protocol frame conformance, browser normalizer/reducer tests, runner receipt dedupe, interrupt behavior, and Codex EOF state-machine tests.
- Remaining live-provider smoke is intentionally manual because it requires locally installed and authenticated provider CLIs plus real process/socket interruption.
- Verification passed:
  - `cargo test -p agenter-protocol --test browser_json_frame_conformance`
  - `cargo test -p agenter-protocol browser`
  - `cargo test -p agenter-runner codex_turn`
  - `cargo test -p agenter-control-plane runner_event_receipts_survive_new_app_state_and_prevent_duplicate_append` (compiled; test ignored without `DATABASE_URL`)
  - `cd web && npm run test -- normalizers sessionSnapshot universalEvents`
  - `rustfmt --edition 2021 --check crates/agenter-protocol/tests/browser_json_frame_conformance.rs crates/agenter-db/src/repositories.rs crates/agenter-control-plane/src/state.rs`
  - `rg -n "TBD|TODO|fill in|PLACEHOLDER" docs`
  - `git diff --check`
- Full workspace gate and live-provider chaos were not run by Worker G because the tree has concurrent in-progress Phase 1/2/5/6 edits and live-provider smoke requires authenticated local provider CLIs plus manual process/socket interruption.

## Discussion Points

- Should the project keep `NormalizedEvent` at all after universal-first adapters land, or should it become a frontend-only compatibility artifact generated from universal snapshots?
- Should no-DB mode be allowed to advertise `after_seq_replay` and runner WAL reliability, or should those capabilities be false unless durable storage is active?
- Should `native.unknown` be visible by default for all unknown native messages, or should visibility depend on severity/category to avoid transcript noise?
- Should Codex thread-level notifications be allowed to update the active session even when no active turn is known, or should they be stored only as session metadata events?
- Should approvals and user-input questions share one durable `agent_obligations` table, or stay separate tables with a shared lifecycle enum?

## Verification For This Report

Run documentation-phase checks after editing this file:

```sh
find . -maxdepth 4 -type f | sort
rg -n "TBD|TODO|fill in|PLACEHOLDER" docs
git diff --check
```

This report is complete when the file exists under `docs/plans/`, contains concrete findings with solution options, and includes phased verification criteria for implementation follow-up.
