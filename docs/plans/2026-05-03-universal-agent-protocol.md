# Universal Agent Protocol Implementation Plan

Status: in progress
Date: 2026-05-03
Source spec: `docs/chatgpt/002_protocol.md`

Stage 0 ADRs:

- `docs/decisions/2026-05-03-universal-agent-protocol.md`
- `docs/decisions/2026-05-03-durable-agent-event-log.md`

## Goal

Implement Agenter's universal agent protocol as a versioned, event-sourced, capability-gated superset above Codex app-server and ACP-family harnesses, while preserving the control-plane / runner split and native-agent history source-of-truth rule.

## Target Decisions

- Codex uses native `codex app-server` as the primary adapter because it exposes richer threads, turns, items, plans, diffs, approvals, and lifecycle events than a generic ACP profile.
- Gemini, Qwen, and OpenCode use the generic ACP runtime as the primary adapter path.
- OpenCode HTTP/SSE and Qwen stream-json remain later enhanced/fallback profiles, not the first universal protocol milestone.
- The browser speaks universal protocol state, not provider-specific branches. Provider-specific details stay in `native` payloads and capability declarations.
- Native references store redacted summaries, stable IDs, hashes, or pointers by default. Full raw native provider payload persistence requires a future explicit policy ADR; raw Codex wire payloads remain runner-local opt-in diagnostics.
- One active turn per native harness session is the default. Parallel work uses multiple Agenter sessions unless a future adapter advertises safe turn concurrency.
- Approvals are protocol obligations. The runner owns live native request delivery; the control plane owns policy, durable state, idempotency, and frontend presentation.
- Transient control-plane / runner interruption should keep native work alive when possible. Durable event replay requires runner-side WAL plus control-plane ack before we claim lossless reconnect.

## Current Implementation Summary

Current Agenter already has useful foundations:

- `AppEvent` normalizes user, assistant, plan, tool, command, file-change, approval, question, usage-ish provider, and error events in `crates/agenter-core/src/events.rs`.
- Runner commands and responses use `RunnerCommandEnvelope { request_id, command }` in `crates/agenter-protocol/src/runner.rs`.
- Browser WebSocket subscribes to one session and receives universal snapshot/replay frames from `crates/agenter-protocol/src/browser.rs`.
- Control plane keeps in-memory per-session broadcast state and persists universal `agent_events` plus `session_snapshots`.
- Approval delivery is already ack-sensitive: browser decisions become resolved only after runner acknowledgement.
- The runner has a generic ACP runtime for Qwen, Gemini, and OpenCode, and a richer Codex app-server runtime.

Main gaps versus `002_protocol.md`:

- No universal envelope with `seq`, `turn_id`, `item_id`, `ts`, `source`, `native`, and typed `data`.
- No durable append-only `agent_events` log exposed as `after_seq` replay.
- No `SessionSnapshot` built from materialized session / turn / item / approval / plan / diff / artifact state.
- No first-class `Turn`, `Item`, `ContentBlock`, `NativeRef`, `IdMap`, or persistent recent-turn cache.
- Capabilities are flat booleans instead of nested protocol/content/tools/approvals/plan/modes/integration gates.
- Browser commands are REST endpoint calls without universal `command_id` and `idempotency_key`.
- Runner-originated events are not acked and the runner has no local WAL replay.
- Interrupt/cancel is currently accepted but not actually wired through Codex or ACP.
- Plans and approvals exist, but not as full state machines with canonical options, risk/subject data, plan approval, user-input distinction, orphaning, or restart-aware behavior.

## Execution Model

The controller agent maintains the target vision, dispatches exactly one implementation worker subagent per stage, reviews results, and only then schedules the next stage. Each stage should use this pattern:

- Worker subagent: implements the stage in the named files and runs the stage verification.
- Spec-review subagent: compares the patch against this plan and `docs/chatgpt/002_protocol.md`.
- Code-quality subagent: reviews local design, tests, compatibility, and migration safety.
- Controller: resolves conflicts, runs or requests final verification, updates this plan's status notes, and dispatches the next stage.

Worker subagents are not alone in the codebase. They must preserve unrelated local changes and avoid reverting work outside their assigned files.

## Stage 0: Contract ADRs And Compatibility Strategy

Owner: documentation worker subagent.

Purpose: lock the foundational protocol choices before schema and code changes begin.

Files:

- Create `docs/decisions/2026-05-03-universal-agent-protocol.md`.
- Create `docs/decisions/2026-05-03-durable-agent-event-log.md`.
- Modify `docs/specs/2026-04-30-remote-agent-control-plane.md`.
- Modify this plan with final accepted ADR links and any discovered constraints.

Steps:

- [x] Write ADR: universal protocol version `uap/1`, Codex-native primary, ACP baseline for Qwen/Gemini/OpenCode, native payload preservation, capability-gated frontend.
- [x] Write ADR: durable append-only `agent_events` supersedes lightweight `event_cache` for browser replay, while native harnesses remain the canonical history source when history can be reloaded.
- [x] Update the remote control-plane spec with the universal entities: session, turn, item, content block, approval, plan, diff, artifact, native ref, command envelope.
- [x] Record the compatibility rule: old `AppEvent` and `event_cache` were migration-only and are no longer Agenter-facing after the `uap/1` cutover.
- [x] Run documentation verification.

Discovered constraints:

- `agent_events` is a browser/reconnect/control-plane projection log, not an audit-grade canonical full transcript unless a future ADR expands scope.
- `NativeRef`, `native_json`, event metadata, and compatibility caches store redacted summaries or pointers by default, not raw full provider payloads. Raw payload persistence requires explicit policy; Codex wire logs stay runner-local by default.
- `seq` is a global `agent_events` database cursor (`bigint`, Rust `i64`/domain-safe `u64`), serialized as a string over JSON. Browser `after_seq` is that global cursor filtered by session subscription.
- The recent-turn cache is a persistent correlation/recovery aid, not the transcript source of truth.
- Lossless runner reconnect requires runner WAL plus control-plane ack after durable append; until both exist, reconnect claims must remain best-effort.
- The old stopped-on-runner-disconnect rule remains acceptable before Stage 4. After Stage 4 WAL/reconnect lands, transient control-plane WebSocket disconnect alone must not stop sessions when runner evidence proves native ownership survived.
- Pending approvals wait through transient runner/control-plane reconnects by default; timeout cancellation requires explicit configured policy.

Stage 0 verification evidence:

- `find . -maxdepth 4 -type f | sort` exited 0. Output included the repo inventory plus generated/dependency paths.
- The placeholder scan matched only `docs/harness/VERIFICATION.md` policy text and this plan's verification command, not unresolved placeholders in the Stage 0 docs.

Verification:

```sh
find . -maxdepth 4 -type f | sort
rg -n "TBD|TODO|fill in|PLACEHOLDER" docs
```

Exit criteria:

- ADRs explicitly answer the four open questions from `002_protocol.md`.
- Spec names the universal protocol as the browser/control-plane contract.
- No implementation code is touched in this stage.

## Stage 1: Universal Core Types And Protocol Envelopes

Owner: protocol worker subagent.

Purpose: add shared Rust types for the universal event and command model without switching runtime behavior yet.

Files:

- Modify `crates/agenter-core/src/events.rs`.
- Modify `crates/agenter-core/src/session.rs`.
- Modify `crates/agenter-core/src/approval.rs`.
- Modify `crates/agenter-core/src/ids.rs`.
- Modify `crates/agenter-protocol/src/browser.rs`.
- Modify `crates/agenter-protocol/src/runner.rs`.
- Add focused tests in the same modules.

Steps:

- [x] Add ID wrappers for `TurnId`, `ItemId`, `PlanId`, `DiffId`, `ArtifactId`, and `CommandId`.
- [x] Add `UniversalEventEnvelope` with `event_id`, global `seq`, `session_id`, optional `turn_id`, optional `item_id`, `ts`, `source`, optional safe `native`, and `event`.
- [x] Add `UniversalEventKind` using dot-compatible serialized names such as `session.created`, `turn.started`, `item.created`, `content.delta`, `approval.requested`, `plan.updated`, `diff.updated`, `artifact.created`, `usage.updated`, and `native.unknown`.
- [x] Add `NativeRef` with protocol, method/type/id, and redacted summary, hash, or pointer data by default.
- [x] Add `UniversalCommandEnvelope { command_id, idempotency_key, session_id, turn_id, command }`.
- [x] Add command variants for start/load/close session, start/cancel turn, send user input, resolve approval, set mode, set model, request diff, revert change, subscribe, and get snapshot.
- [x] Add nested `CapabilitySet` while keeping `AgentCapabilities` available for compatibility conversion.
- [x] Add first-class `TurnState`, `ItemState`, `ContentBlock`, `ApprovalRequest`, `ApprovalOption`, `PlanState`, `PlanEntry`, `DiffState`, `ArtifactState`, and `SessionSnapshot`.
- [x] Extend browser subscription to include `after_seq` and `include_snapshot` fields while keeping old `subscribe_session` decoding valid.
- [x] Extend runner event envelopes with optional runner-local sequence/ack placeholders, but keep current runner messages backward-compatible; control-plane-assigned universal `seq` is not known by runner-originated events.
- [x] Add serde round-trip tests for all new envelopes and representative events.

Stage 1 verification evidence:

- `cargo fmt --all -- --check` passed.
- `cargo test -p agenter-core` passed with 26 tests.
- `cargo test -p agenter-protocol` passed with 22 tests.
- `cargo check --workspace` passed after downstream compatibility literals were updated with default optional fields.
- Spec review approved with no findings.
- Quality review approved after removing raw/provider payload paths from universal approval options and approval resolve commands, keeping legacy replay capabilities false, validating non-negative `UniversalSeq`, renaming runner seq placeholders to runner-local fields, and allowing empty snapshots without `latest_seq`.

Verification:

```sh
cargo fmt --all -- --check
cargo test -p agenter-core
cargo test -p agenter-protocol
```

Exit criteria:

- New types compile and serialize predictably.
- No existing `AppEvent` consumer breaks.
- Browser and runner protocol tests cover backward-compatible old frames and new universal frames.

## Stage 2: Durable Event Log, Snapshots, And Reducer Storage

Owner: storage/reducer worker subagent.

Purpose: introduce durable universal event storage and materialized state before replacing live broadcast behavior.

Files:

- Add migration `migrations/0007_universal_agent_events.sql`.
- Modify `crates/agenter-db/src/models.rs`.
- Modify `crates/agenter-db/src/repositories.rs`.
- Modify `crates/agenter-control-plane/src/state.rs`.
- Add control-plane and DB tests.

Schema:

- Add `agent_events` with global `seq bigserial primary key`, `event_id uuid unique`, `workspace_id`, `session_id`, nullable `turn_id`, nullable `item_id`, `event_type`, `event_json`, nullable redacted/pointer `native_json`, `source`, nullable `command_id`, `created_at`.
- Add `agent_event_idempotency` with `idempotency_key`, `command_id`, nullable `session_id`, `status`, nullable `response_json`, timestamps.
- Add `session_snapshots` with `session_id primary key`, `latest_seq`, `snapshot_json`, `updated_at`.
- Add `recent_turn_caches` with `session_id`, `turn_id`, `cache_json`, `updated_at`.
- Extend or supersede `pending_approvals` with universal approval status, native request id, canonical options, risk, subject, and redacted native summary or pointer data.

Steps:

- [x] Write repository methods to append universal events transactionally and return assigned `seq`.
- [x] Write repository methods to list events after `seq` for a session, bounded by a caller-provided limit.
- [x] Write repository methods to load and store `SessionSnapshot`.
- [x] Write a reducer that applies one universal event to a `SessionSnapshot`.
- [x] Persist snapshots after append in the same logical operation.
- [x] Stop writing `event_cache` after the browser cutover; universal app-event compatibility ingress writes only `agent_events` and `session_snapshots`.
- [x] Add tests for monotonic seq, snapshot reconstruction, `after_seq` listing, pending approval materialization, and old event cache coexistence.

Stage 2 verification evidence:

- `cargo fmt --all -- --check` passed.
- `cargo test -p agenter-db` passed with 2 non-ignored tests and ignored disposable-Postgres integration tests. The ignored universal event log test covers monotonic seq, `after_seq`, snapshot storage, pending approval materialization/resolution, and projection reset without legacy cache tables.
- `cargo test -p agenter-control-plane snapshot` passed with 5 tests.
- `cargo test -p agenter-control-plane event` passed with 10 tests.
- `cargo check --workspace` passed.

Stage 2 notes:

- Runtime browser delivery uses universal snapshot/replay frames only.
- Legacy `AppEvent` dual-write is explicitly compatibility-only: provider payloads are not copied into universal `native_json`; only safe native IDs/summaries are projected where available.
- Universal snapshot writes now lock the parent `agent_sessions` row before assigning/reducing a session event, and snapshot storage rejects `latest_seq` regressions.
- Durable approval resolution updates `pending_approvals.universal_status`; legacy `ApprovalResolved` events project into universal approval state without raw provider payload persistence.
- Forced discovered-history refresh clears the session's universal projection (`agent_events`, `session_snapshots`, and recent turn cache) before rewriting imported history. This is not canonical native history deletion.
- `agent_events.event_id` is a control-plane UUID. Native stable event/request IDs belong in `NativeRef`; non-UUID envelope event IDs fail with a clear error.
- A narrow compile-only test update was required in `crates/agenter-control-plane/src/api.rs` to ignore the Stage 1 `BrowserServerMessage::SessionSnapshot` test frame while waiting for Stage 3 browser replay wiring.

Verification:

```sh
cargo fmt --all -- --check
cargo test -p agenter-db
cargo test -p agenter-control-plane snapshot
cargo test -p agenter-control-plane event
```

Exit criteria:

- Universal event append and snapshot reducer work without changing browser behavior yet.
- `event_cache` is dropped by migration `0008_drop_legacy_event_cache.sql`; new writes must not reference those tables.
- DB migration is append-only and does not rewrite existing migrations.

## Stage 3: Browser Snapshot Replay And Command Idempotency

Owner: control-plane/browser-protocol worker subagent.

Purpose: make frontend reload/reconnect boring: subscribe with cursor, receive snapshot, replay missed events, and route commands idempotently.

Files:

- Modify `crates/agenter-control-plane/src/browser_ws.rs`.
- Modify `crates/agenter-control-plane/src/api/sessions.rs`.
- Modify `crates/agenter-control-plane/src/api/approvals.rs`.
- Modify `crates/agenter-control-plane/src/api/slash.rs`.
- Modify `crates/agenter-control-plane/src/state.rs`.
- Modify `crates/agenter-protocol/src/browser.rs`.

Steps:

- [x] Implement `subscribe_session` with optional `after_seq` and `include_snapshot`.
- [x] Add server message `session_snapshot` that includes `snapshot`, `events`, and `latest_seq`.
- [x] Expose `seq` on every live universal event delivered to browser subscribers.
- [x] Implement durable `UniversalCommandEnvelope` handling for send message, approval resolve, question answer, cancel turn, set mode/model, and provider command execution.
- [x] Store command idempotency before dispatching to the runner.
- [x] Return the previous response for duplicate same-key commands.
- [x] Reject duplicate conflicting commands with a typed error.
- [x] Preserve the current REST routes by wrapping them internally in universal command envelopes when the frontend has not migrated yet.
- [x] Add tests for duplicate send message, duplicate approval resolve, conflicting duplicate approval resolve, snapshot replay after cache miss, and legacy REST compatibility.

Stage 3 verification evidence:

- `cargo fmt --all -- --check` passed.
- `cargo test -p agenter-control-plane subscribe -- --nocapture` passed with 6 tests.
- `cargo test -p agenter-control-plane approval -- --nocapture` passed with 8 tests.
- `cargo test -p agenter-control-plane idempot -- --nocapture` passed with 10 tests.
- `cargo test -p agenter-db` passed with 2 tests and 10 ignored DATABASE_URL-backed integration tests; ignored coverage now includes `finish_command_idempotency_missing_row_returns_row_not_found`.
- `cargo test -p agenter-protocol browser -- --nocapture` passed with 7 tests.
- `cargo check --workspace` passed.
- `git diff --check` passed.

Stage 3 notes:

- Browser WebSocket subscriptions remain legacy-compatible by default. Clients that send `after_seq` or `include_snapshot` receive `session_snapshot` replay and live `universal_event` frames with `seq`.
- `session_snapshot` includes `has_more`; bounded replay fetches one extra event and does not report the full snapshot cursor as the replay cursor when the missed-event window is truncated. DB replay failures also mark `has_more` and keep the replay cursor at the requested `after_seq` so clients cannot advance past unseen events.
- Universal WebSocket replay tracks `(seq, event_id)` values sent in `session_snapshot.events` and skips matching queued live `universal_event` frames. If replay is incomplete (`has_more`), the server sends `snapshot_replay_incomplete` with instructions to resubscribe/page from `snapshot.latest_seq`, then closes that universal subscription instead of forwarding live universal events that could advance the client past a gap.
- REST routes now create internal universal command envelopes before runner dispatch. Existing clients can omit command ids/keys; retry-capable clients can pass optional idempotency fields on message/settings routes, while approval and question routes derive stable keys from the routed obligation.
- Duplicate same-key commands replay the stored response when available; pending approval duplicates preserve the existing resolving-envelope behavior; conflicting duplicate commands return a typed `idempotency_conflict` response.
- Durable idempotency uses `agent_event_idempotency` when Postgres is configured. DB-backed begin failures reject before dispatch, and DB-backed finish failures surface as service errors instead of claiming durable success. No-DB development/tests still use the process-local fallback.
- Question answers begin idempotency before checking unresolved question state, so same-key retries after success replay the stored response instead of 404.
- Slash command requests now accept optional universal command/idempotency fields. Explicit `idempotency_key` values are retry keys and conflict on different semantic command bodies; omitted keys get fresh one-shot keys so legacy repeated slash commands still execute again. Slash user echoes publish only after idempotency begin succeeds, so duplicate/conflicting retries do not append duplicate transcript rows.
- Side-effectful local slash commands are wrapped in universal commands, including `/new`, `/title`, `/refresh`, `/model`, `/mode`, and `/reasoning`; `/help` remains a non-side-effect local command.
- Narrow updates outside the named Stage 3 files were required: universal command variants for question/provider/settings commands in `agenter-core`, repository APIs/models for `agent_event_idempotency` in `agenter-db`, and focused API tests in `crates/agenter-control-plane/src/api.rs`.

Verification:

```sh
cargo fmt --all -- --check
cargo test -p agenter-control-plane subscribe
cargo test -p agenter-control-plane approval
cargo test -p agenter-control-plane idempot
```

Exit criteria:

- A browser can reconnect with `after_seq` and receive a snapshot plus missed events.
- Current frontend still works through compatibility routes.
- Approval retry after browser/network interruption is idempotent.

## Stage 4: Runner Event Acks, WAL, And Reconnect Loop

Owner: runner reliability worker subagent.

Purpose: prevent runner/control-plane interruptions from losing native events or silently dropping blocked approval state.

Files:

- Modify `crates/agenter-protocol/src/runner.rs`.
- Modify `crates/agenter-runner/src/main.rs`.
- Add `crates/agenter-runner/src/wal.rs`.
- Modify `crates/agenter-control-plane/src/runner_ws.rs`.
- Modify `crates/agenter-control-plane/src/state.rs`.
- Add runner/control-plane tests.

Steps:

- [x] Add runner event sequence and ack frames to the runner protocol.
- [x] Add runner-local WAL records for outbound runner events with runner event sequence, request id, session id, and event payload.
- [x] Write WAL before sending runner-originated events to the control plane.
- [x] Ack runner events from the control plane only after the event is accepted into the control-plane projection.
- [x] On reconnect, runner sends hello plus last known ack/replay cursor and replays unacked WAL events.
- [ ] Keep adapter tasks alive across transient control-plane WebSocket disconnects when native process ownership allows it.
- [x] Mark sessions `stopped` or turns `detached` only when runner evidence says live ownership is gone.
- [x] Add bounded WAL cleanup after ack.
- [x] Add focused tests for runner protocol serde, WAL append/replay/ack cleanup, corrupt trailing WAL recovery, duplicate replay dedupe, seeded ack replay dedupe, strict acceptance failure, duplicate-check non-poisoning, and disconnect session preservation.

Stage 4 verification evidence:

- `cargo fmt --all -- --check` passed.
- `cargo test -p agenter-protocol runner -- --nocapture` passed with 18 tests.
- `cargo test -p agenter-runner wal -- --nocapture` passed with 2 WAL tests.
- `cargo test -p agenter-control-plane runner -- --nocapture` passed with 22 tests.
- `cargo check --workspace` passed.

Stage 4 notes:

- Runner event acknowledgements are runner-local and cumulative by `runner_event_seq`; they are distinct from control-plane universal `seq`.
- The runner writes a JSONL WAL under `.agenter/runner-<runner-id>-events.jsonl` by default, or `AGENTER_RUNNER_WAL` when configured. The WAL is written before send through temp-file write, file sync, atomic rename, and best-effort directory sync. Open tolerates a corrupt trailing record after a valid prefix. Records are replayed after hello and cleaned up after ack with bounded retention.
- Control-plane dedupe is in-memory for this stage and keyed by `(runner_id, runner_event_seq)`. Duplicate checking is read-only; a seq is marked accepted only after event validation and projection acceptance. In DB-backed deployments this still prevents browser-visible duplication during a process lifetime; durable cross-control-plane-restart dedupe should move into the event append/idempotency store in a later hardening pass.
- Runner-originated `AppEvent` acceptance is fallible. In DB-backed mode, ack is withheld if durable `agent_events` append cannot be guaranteed; in no-DB mode, in-memory append/broadcast is accepted. `SessionsDiscovered` is always processed and forced-refresh summaries are recorded, but it is acked only in no-DB mode for now because strict DB import/reset acceptance still needs a fallible repository path; DB-backed runners will replay unacked discovery WAL records and imports must remain idempotent/best-effort until that path is hardened.
- In no-DB mode, acked runner WAL records are not replayed after a control-plane process restart because accepted runner seq state is process-local. No-DB mode is therefore not lossless across control-plane restarts; lossless restart requires DB-backed universal event storage plus future durable runner seq dedupe.
- Control-plane WebSocket disconnect no longer marks runner-owned sessions stopped. Explicit shutdown/session status evidence from the runner is required for stopped state.
- Live adapter task survival across an already-established control-plane WebSocket disconnect remains incomplete: current runner processes replay unacked WAL records on the next connection/start, but the top-level connection loops still need to be split so provider runtimes and pending approval maps live outside transient socket lifetimes.
- Stage 4 WAL is compatibility-event persistence and may contain existing redacted-or-legacy payload fields from `RunnerEvent`/`AppEvent`. Deployments should treat the WAL as sensitive local diagnostic/state. Stage 4 does not claim redacted-by-default WAL payloads; Stage 5 should enforce redaction centrally when adapter outputs become universal events first.

Verification:

```sh
cargo fmt --all -- --check
cargo test -p agenter-protocol runner
cargo test -p agenter-runner wal
cargo test -p agenter-control-plane runner
```

Exit criteria:

- Runner-originated events are not considered delivered until acked after durable append.
- Replayed runner events do not duplicate browser-visible universal events.
- Pending native approvals remain blocked through transient reconnects when the runner process survives.

## Stage 5: Harness Adapter Trait And Reducer Split

Owner: adapter architecture worker subagent.

Purpose: introduce the shared adapter interface, provider/session registry, event projection seam, and native codec/reducer boundaries before the larger command-dispatch migration.

Files:

- Add `crates/agenter-runner/src/agents/adapter.rs`.
- Modify `crates/agenter-runner/src/agents/mod.rs`.
- Modify `crates/agenter-runner/src/agents/codex.rs`.
- Modify `crates/agenter-runner/src/agents/acp.rs`.
- Modify `crates/agenter-runner/src/main.rs`.

Steps:

- [x] Define `HarnessAdapter` with start/load/start_turn/send_user_input/resolve_approval/cancel_turn/set_mode/close/capabilities/event stream behavior.
- [x] Define adapter request/response structs using universal command and event types.
- [x] Create an adapter registry keyed by provider id and session id.
- [x] Move provider/session resolution and compatibility event projection in `main.rs` behind adapter runtime helpers.
- [ ] Move full command dispatch in `main.rs` behind concrete `HarnessAdapter` trait objects.
- [x] Split Codex code into transport, codec, and semantic reducer modules without changing observed behavior.
- [x] Split ACP code into transport/client services, codec, and semantic reducer modules without changing observed behavior.
- [x] Introduce adapter outputs with safe universal event shells and legacy `AppEvent` compatibility projections.
- [ ] Convert production runtime outputs to full universal events first, then legacy `AppEvent` only through compatibility projection.
- [x] Add tests proving Codex and ACP fixture events produce stable universal events and unchanged legacy projections during migration.

Stage 5 verification evidence:

- `cargo test -p agenter-runner adapter -- --nocapture` passed with 3 adapter seam tests.
- `cargo test -p agenter-runner codex -- --nocapture` passed with 57 runner tests plus 2 Codex spike tests.
- `cargo test -p agenter-runner acp -- --nocapture` passed with 11 runner tests.

Stage 5 notes:

- `HarnessAdapter` is introduced as the shared asynchronous boundary, but the concrete Codex and ACP runtimes are not yet implemented behind trait objects. Full command dispatch remains in the existing runner command loop.
- The multi-provider runner uses `AdapterRuntime` for provider/session resolution and binds/unbinds sessions as create/resume/shutdown commands flow through the current runner command loop.
- Codex and ACP reducer seams currently wrap the existing legacy normalizers and emit `AdapterEvent` objects with a safe universal `native.unknown` shell plus unchanged legacy `AppEvent` projection. Stage 6 owns the full turn/item/content/diff/artifact mapping.
- `AdapterEvent` no longer fabricates nil universal session ids for sessionless legacy events. Sessionless adapter events keep `session_id: None` and are not projected to runner WAL `AgentEvent` records.
- Runner WAL sending still persists compatibility `RunnerEvent::AgentEvent` records, but those records now carry an optional `universal_event` draft alongside the legacy `AppEvent`. The runner sends both where adapter semantics are available, and the control plane prefers the supplied universal draft for durable append/reducer/broadcast while preserving the legacy event cache projection.

Verification:

```sh
cargo fmt --all -- --check
cargo test -p agenter-runner codex
cargo test -p agenter-runner acp
cargo check --workspace
```

Exit criteria:

- `main.rs` uses adapter helpers for provider/session resolution and compatibility event projection.
- Codex and ACP normalization can be tested as pure reducer logic from native fixtures.
- Existing provider behavior remains visible to the current frontend.
- Full trait-object command dispatch and full universal-first runtime output remain explicit follow-up work.

## Stage 6: Turn, Item, Content, Diff, And Artifact Mapping

Owner: semantic mapping worker subagent.

Purpose: make the universal timeline real across Codex and ACP.

Files:

- Modify Codex reducer modules from Stage 5.
- Modify ACP reducer modules from Stage 5.
- Modify `crates/agenter-core/src/events.rs`.
- Modify `crates/agenter-control-plane/src/state.rs`.
- Add golden fixtures under `crates/agenter-runner/tests/fixtures/` or an equivalent repo-local fixture path.

Steps:

- [x] Add universal turn lifecycle emission: `turn.started`, `turn.status_changed`, `turn.completed`, `turn.failed`, `turn.cancelled`, `turn.interrupted`, `turn.detached`.
- [x] Map one ACP `session/prompt` call to one universal turn and attach all updates/permissions during that prompt to the active turn.
- [x] Map Codex native turn ids into universal turn ids and store native id refs.
- [x] Map assistant text/reasoning/tool/command/file/MCP/web-search/context-compaction/image events into item/content events.
- [x] Emit `content.delta` and `content.completed` for streaming assistant and terminal output.
- [x] Emit `diff.updated` for Codex turn diffs and file-change diffs; keep `artifact.created` limited to actual safe artifact/file/image refs when available.
- [x] Preserve unknown native events as `native.unknown` with safe native refs, redacted summaries, hashes, or pointers unless a future policy ADR explicitly allows raw payload persistence.
- [x] Add golden trace tests for Codex plan+approval+command+diff, ACP message+tool+permission+plan, and unknown native events.

Stage 6 verification evidence:

- `cargo fmt --all -- --check` passed.
- `cargo test -p agenter-runner codex -- --nocapture` passed with 59 runner tests plus 2 Codex spike tests.
- `cargo test -p agenter-runner acp -- --nocapture` passed with 13 runner tests.
- `cargo test -p agenter-control-plane reducer -- --nocapture` passed with 4 reducer tests.
- `cargo check --workspace` passed.
- `git diff --check` passed.

Stage 6 notes:

- `UniversalEventKind` now includes explicit turn lifecycle variants and `content.completed`.
- The adapter semantic projection maps legacy Codex/ACP normalizer output into universal turn/item/content/plan/diff/approval events while preserving the legacy `AppEvent` projection for the Stage 4 WAL/frontend path.
- The control-plane compatibility reducer also materializes message content in universal snapshots from current legacy WAL events, so snapshots are no longer limited to `native.unknown` rows for assistant text.
- Command completion uses a separate status block, preserving the original command invocation/tool-call block and streamed stdout/stderr blocks in the universal snapshot.
- ACP has a small stateful reducer object that creates a deterministic prompt turn scoped by provider, session, and native prompt id; it also exposes explicit prompt completion to emit `turn.completed` and clear active turn state. Production command dispatch still needs the Stage 5 follow-up before this state can be the sole live runtime path.
- `artifact.created` is supported in the universal core and reducer, but this pass does not fabricate artifacts for ordinary structured plan updates. Current Codex/ACP live normalizers do not expose safe plan-file/image/file-ref artifacts without provider payload inspection, so richer artifact extraction remains a Stage 8/adapter hardening follow-up.
- Web-search and image events remain `native.unknown` unless the current normalizers expose safe structured fields; this preserves debuggability without copying broad raw provider payloads.

Verification:

```sh
cargo fmt --all -- --check
cargo test -p agenter-runner codex
cargo test -p agenter-runner acp
cargo test -p agenter-control-plane reducer
```

Exit criteria:

- Universal snapshot contains turns/items/content, not only transcript rows.
- Unknown provider behavior remains visible and debuggable.
- Existing Codex-specific visibility from prior coverage work is preserved.

## Stage 7: Approval, User Input, Policy, And Cancel State Machines

Owner: approval/policy worker subagent.

Purpose: promote approvals and questions into durable universal state machines and wire real cancellation.

Files:

- Modify `crates/agenter-core/src/approval.rs`.
- Modify `crates/agenter-core/src/session.rs`.
- Modify `crates/agenter-control-plane/src/api/approvals.rs`.
- Modify `crates/agenter-control-plane/src/state.rs`.
- Add `crates/agenter-control-plane/src/policy.rs`.
- Modify `crates/agenter-runner/src/agents/approval_state.rs`.
- Modify Codex and ACP adapter reducers.

Steps:

- [x] Add canonical approval statuses: pending, presented, resolving, approved, denied, cancelled, expired, orphaned.
- [x] Add canonical options: approve once, approve always, deny, deny with feedback, cancel turn.
- [x] Add approval risk, subject, native request id, native blocking flag, turn id, item id, and policy metadata.
- [x] Map existing `accept`, `accept_for_session`, `decline`, and `cancel` decisions to canonical options.
- [x] Persist every approval request and status transition.
- [x] Keep question/user-input flows separate from danger approvals.
- [x] Implement policy engine decisions `allow`, `ask`, `deny`, and `rewrite` for command/file/network/tool requests where available.
- [ ] Wire `CancelTurn` to Codex interrupt/cancel behavior and ACP `session/cancel`.
- [x] On cancel, respond to blocked native approvals/questions with a native cancelled/declined outcome where required by the harness protocol.
- [x] Mark approvals orphaned when runner/harness ownership is lost.
- [x] Add tests for approval duplicate same decision, conflicting duplicate, runner reconnect during approval, cancel while awaiting approval, question answer idempotency, and orphaning on harness death.

Stage 7 verification evidence:

- `cargo fmt --all -- --check` passed.
- `cargo test -p agenter-control-plane approval -- --nocapture` passed with 13 tests.
- `cargo test -p agenter-runner approval -- --nocapture` passed with 13 runner approval tests plus 1 Codex app-server spike approval test.
- `cargo test -p agenter-runner codex -- --nocapture` passed with 59 runner tests plus 2 Codex app-server spike tests.
- `cargo test -p agenter-runner acp -- --nocapture` passed with 14 runner tests.
- `cargo test -p agenter-control-plane policy -- --nocapture` passed with 3 tests, including the typed policy engine tests.
- `cargo check --workspace` passed.
- `git diff --check` passed.
- Tight review repair added coverage for authz-before-mutation, idempotency-conflict-before-mutation, provider-specific payload hash conflicts, resolved approval plus pending idempotency replay repair, completed-cancel replay not counting as a fresh cancellation, and canonical approval native request id materialization.

Stage 7 notes:

- Universal approval requests now carry canonical options, canonical lifecycle status, native request id, native blocking flag, risk/subject, and policy metadata in the redacted universal projection. Legacy approval events are enriched at the control-plane boundary so current provider/browser flows keep working.
- Listing pending approvals marks them `presented` and persists that transition. Beginning resolution persists `resolving`; runner-acknowledged resolution persists the final approved/denied/cancelled projection through the existing Stage 3 idempotent route.
- Question/user-input requests remain `QuestionRequested` / `QuestionAnswered` and do not become danger approvals.
- The policy surface is intentionally conservative: command/file/tool/provider approval requests currently evaluate to typed `ask` decisions with coarse risk classification; `allow`, `deny`, and `rewrite` are modeled for future rules but not auto-applied to native requests yet.
- `/interrupt` / `CancelTurn` no longer returns inert success when there is no live cancellation hook. The runner answers matching blocked provider approvals with native cancel where present; otherwise it returns a clear `provider_cancel_not_supported` error. Full Codex turn interrupt and ACP `session/cancel` still require live runtime handles outside the current provider-specific command loops.
- ACP provider advertisements no longer claim generic interrupt support. `CapabilitySet.approvals.cancel_turn` is therefore only advertised when a provider has an actual interrupt capability; individual blocked approval cards may still offer a cancel option because that maps to answering the native blocked request with a cancelled/declined outcome.
- Approvals are marked `orphaned` when explicit runner/harness evidence moves the session to stopped, failed, or archived. Transient runner WebSocket disconnect still does not orphan approvals.

Verification:

```sh
cargo fmt --all -- --check
cargo test -p agenter-control-plane approval
cargo test -p agenter-runner approval
cargo test -p agenter-runner codex
cargo test -p agenter-runner acp
```

Exit criteria:

- Approval lifecycle survives browser reload, browser retry, and runner reconnect.
- Native blocked requests are answered exactly once.
- Cancel is wired to native provider behavior.

## Stage 8: Plan State Machine And Plan Approval

Owner: plan workflow worker subagent.

Purpose: model plans as state, not just markdown transcript content.

Files:

- Modify `crates/agenter-core/src/events.rs`.
- Modify Codex and ACP reducer modules.
- Modify `crates/agenter-control-plane/src/state.rs`.
- Modify `docs/decisions/2026-05-02-plan-mode-implement-handoff.md` if the handoff contract changes.
- Add reducer tests.

Steps:

- [x] Add `PlanState` statuses: none, discovering, draft, awaiting approval, revision requested, approved, implementing, completed, cancelled.
- [x] Map Codex `turn/plan/updated` and plan item deltas into both plan item content and active `PlanState`.
- [x] Map ACP plan updates as replace-all entries unless marked partial.
- [x] Map Gemini plan mode transitions into plan approval and implementation-start events where ACP update payloads expose typed plan status.
- [x] Map OpenCode `todowrite` tool output into synthetic plan state when no richer native plan exists.
- [x] Keep the accepted Codex handoff behavior: implementation starts in default mode after plan approval, without reusing plan-mode settings accidentally.
- [x] Add tests for append-vs-replace behavior, plan approval, revision request, implementation started, completion, and cancellation.

Verification:

```sh
cargo fmt --all -- --check
cargo test -p agenter-core plan
cargo test -p agenter-runner plan
cargo test -p agenter-control-plane plan
```

Exit criteria:

- Snapshot exposes active plan state with structured entries.
- Plan approval is distinct from file/tool approval.
- Current Codex plan UX does not regress while universal state becomes available.

Stage 8 evidence:

- `PlanState` now carries content, source, artifact refs, and a partial marker so the snapshot reducer can model native structured, markdown-file, todo-tool, and synthetic plan updates, including failed plan and failed/cancelled entry states.
- Full plan updates replace current content and entries; partial updates append content and upsert entries by `entry_id` so status deltas do not duplicate existing rows.
- Plan updates materialize a plan item in the universal snapshot while preserving the active `PlanState`; empty full status-only updates clear the materialized item to avoid stale plan text.
- Codex plan handoff behavior was not changed; existing Codex plan-mode turn-start tests still pass.
- Gemini plan approval/implementation/revision/completion/cancellation/failure are mapped only when ACP plan update payloads expose typed status fields such as `awaiting_approval`, `revision_requested`, `approved`, `implementing`, `completed`, `cancelled`, or `failed`.
- OpenCode `todowrite` output maps to a `todo_tool` plan only when the tool payload exposes structured todo entries.

Stage 8 limitations:

- Current generic ACP runtime does not yet have provider-specific Gemini plan-file artifact extraction; plan-file/image/file-ref artifacts remain limited to safe shapes already present in normalized update payloads.
- OpenCode `todowrite` mapping is stateless at the adapter boundary. It creates a todo-tool plan event only from structured `todos`/`items`/`tasks` arrays; unstructured text output remains a normal tool projection. Stage 9/frontend or later adapter hardening can decide how to prefer native structured plans over todo-tool plans when both are present.

## Stage 9: Frontend Universal Snapshot Client

Owner: frontend worker subagent.

Purpose: migrate the Svelte app from history+event-cache projection to snapshot+after-seq universal state.

Files:

- Modify `web/src/api/types.ts`.
- Modify `web/src/api/events.ts`.
- Modify `web/src/api/sessions.ts`.
- Modify `web/src/lib/normalizers.ts`.
- Add `web/src/lib/universalEvents.ts`.
- Add `web/src/lib/sessionSnapshot.ts`.
- Modify `web/src/lib/chatEvents.ts`.
- Modify `web/src/routes/ChatRoute.svelte`.
- Modify `web/src/components/PlanCard.svelte`.
- Modify `web/src/components/InlineEventRow.svelte`.
- Modify `web/src/components/SessionTreeSidebar.svelte` as needed for capabilities/status.

Steps:

- [x] Add TypeScript types for universal events, commands, capabilities, session snapshot, turns, items, content blocks, approvals, plans, diffs, artifacts.
- [x] Change WebSocket subscription to send `after_seq` and `include_snapshot`.
- [x] Track `latestSeq` in route state and use it for reconnect.
- [x] Apply snapshot first, then replay events after the snapshot seq.
- [x] Remove legacy history loading fallback after backend `uap/1` readiness.
- [x] Add universal reducer that materializes timeline rows from snapshot state.
- [x] Render plans from `PlanState.entries` while preserving markdown content and Codex handoff actions.
- [x] Render approval options from canonical options rather than hard-coded Codex decisions.
- [x] Render question/user-input requests separately from approvals.
- [x] Render diff/artifact refs as inline rows first; right-rail organization can remain a later UI polish task.
- [x] Feature-gate mode/model/reasoning/approval/diff controls from capabilities.
- [x] Add tests for snapshot apply, after-seq replay, duplicate seq dedupe, approval option rendering, plan state rendering, capability gating, and legacy fallback.

Stage 9 evidence:

- Added frontend universal protocol types, WebSocket cursor subscription options, browser message normalization for `session_snapshot` and `universal_event`, and snapshot/replay reducers in `web/src/lib/sessionSnapshot.ts` and `web/src/lib/universalEvents.ts`.
- `ChatRoute.svelte` subscribes with `include_snapshot`, preserves `latestSeq` across reconnect for the same route, and applies only universal snapshot/replay messages without duplicating replay/live boundary events.
- Plans now render structured `PlanState.entries` while preserving markdown content and existing Codex plan handoff actions.
- Approval buttons are derived from canonical universal options. Question cards now materialize from first-class universal question state instead of dual-delivered legacy events.
- Diff and artifact state materialize as inline rows; richer right-rail organization remains deferred.
- Capability data gates mode/model/reasoning/approval controls only when the snapshot advertises a real capability signal; controls remain available for legacy/no-capability sessions.
- Focused tests cover snapshot materialization, after-seq replay, duplicate seq/event-id dedupe, incomplete replay cursor safety, legacy fallback, capability detection, and WebSocket subscription options.

Stage 9 verification evidence:

- `npm run check` passed with 0 Svelte diagnostics.
- `npm run lint` passed.
- `npm run test` passed with 67 tests across 14 files.
- `npm run build` passed.
- `cargo check --workspace` passed.

Stage 9 repair evidence:

- Live universal events at or behind the current `latestSeq` are now ignored even when the `(seq, event_id)` tuple is new, preventing stale replay/live duplication.
- Malformed universal events without a valid non-negative `seq` or non-empty `event_id` now fail normalization and go through the WebSocket parse-error path instead of becoming applyable seq `0`.
- Universal approval rows preserve canonical `option_id` when posted from the frontend; the legacy approval REST endpoint accepts the extra `option_id`/`feedback` fields and keeps existing `decision` callers compatible.
- Universal browser subscriptions dual-deliver legacy question requested/answered app events until `uap/1` grows first-class question event variants.
- Terminal approval states (`approved`, `denied`, `cancelled`, `expired`, `orphaned`) materialize as resolved rows without clickable decision options.
- Snapshot/replay/live materialization records first-seen row ordering from universal event `seq`/`ts`, so reconnect rendering preserves deterministic timeline order for interleaved assistant, approval, diff, plan, and artifact rows.
- Repair tests added coverage for same-seq/different-id stale events, older live events, malformed universal frames, canonical approval option posting, terminal approval rows, dual-delivered live questions, and interleaved replay chronology.
- Repair verification: `npm run check`, `npm run lint`, `npm run test` (73 tests across 14 files), `npm run build`, `cargo check --workspace`, and `git diff --check` all passed.

Verification:

```sh
cd web
npm run check
npm run lint
npm run test
npm run build
```

Exit criteria:

- Frontend can reload during a running turn and reconstruct from snapshot+replay.
- No duplicate transcript rows when events are replayed.
- Existing browser workflows still work for Codex and ACP sessions.

## Stage 10: Conformance, Chaos, And Live Protocol Validation

Owner: verification worker subagent.

Purpose: prove the universal protocol is not just types, but a reliable cross-harness behavior contract.

Files:

- Add or modify `docs/runbooks/universal-protocol-smoke.md`.
- Add native trace fixtures under the runner test fixture path.
- Add integration tests in `crates/agenter-control-plane` and `crates/agenter-runner`.
- Update `docs/harness/VERIFICATION.md`.
- Update `docs/acp/provider-matrix.md` and `docs/acp/progress.md`.

Steps:

- [x] Add sanitized golden fixture slices for Codex shell approval, file approval, plan update, command output, diff, and completion. Live Codex capture for this exact Stage 10 story remains pending.
- [x] Add sanitized ACP fixture slices for Qwen prompt/permission/plan, Gemini plan/question-like permission, and OpenCode tool/todowrite-like plan where live setup was not available in this worker environment.
- [x] Add cross-harness conformance test story: inspect repo, make a plan, ask before edits, implement, run tests. The story is captured in `docs/runbooks/universal-protocol-smoke.md` and exercised by the Stage 10 fixture tests.
- [x] Add or identify focused chaos/conformance tests for snapshot+after-seq replay order, duplicate runner replay/idempotency, runner reconnect seed dedupe, frontend incomplete replay state, approval retry/idempotency, and cancel while awaiting approval. Harness crash during tool remains manual/future live-provider coverage.
- [x] Update verification policy with universal protocol smoke checks and known environment prerequisites.
- [x] Run the full Rust and frontend gates. The final gate passed after rustfmt cleanup and boxing the large runner protocol enum variants.
- [ ] Run manual local browser smoke with fake runner and at least one real provider available on the machine.

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
```

Exit criteria:

- Universal protocol milestones appear in the same order across Codex and ACP harness stories.
- Chaos cases have explicit expected states: replayed, resolving, detached, cancelled, failed, or orphaned.
- Verification docs tell future agents exactly how to reproduce the smoke tests.

Stage 10 evidence:

- Added `docs/runbooks/universal-protocol-smoke.md` with setup, fake-runner/browser path, DB-backed path, provider trace path, expected event order, snapshot/replay checks, approval/question/cancel checks, chaos cases, cleanup, and troubleshooting.
- Added `crates/agenter-runner/tests/fixtures/codex_stage10_trace.json` and `crates/agenter-runner/tests/fixtures/acp_stage10_trace.json`. These are sanitized golden slices derived from current reducer vocabulary, not fresh raw provider logs.
- Added `codex_stage10_conformance_trace_preserves_expected_milestones`, `acp_stage10_provider_traces_share_prompt_plan_permission_shape`, and `subscribe_snapshot_replays_after_seq_in_strict_order`.
- Updated `docs/harness/VERIFICATION.md`, `docs/acp/provider-matrix.md`, and `docs/acp/progress.md` with universal smoke prerequisites and live-provider status.

Stage 10 verification evidence so far:

- Red phase: `cargo test -p agenter-runner codex_stage10_conformance_trace_preserves_expected_milestones` and `cargo test -p agenter-runner acp_stage10_provider_traces_share_prompt_plan_permission_shape` failed because the Stage 10 fixture files were missing.
- `cargo test -p agenter-control-plane subscribe_snapshot_replays_after_seq_in_strict_order` passed.
- After adding fixtures, `cargo test -p agenter-runner codex_stage10_conformance_trace_preserves_expected_milestones` passed.
- After adding fixtures, `cargo test -p agenter-runner acp_stage10_provider_traces_share_prompt_plan_permission_shape` passed.
- `cargo test -p agenter-control-plane runner_event` passed, including duplicate runner replay/idempotency and fake-runner browser smoke coverage.
- `cargo test -p agenter-control-plane seeded_runner_ack_marks_old_replay_as_duplicate` passed.
- `cargo test -p agenter-runner interrupt_` passed.
- `cargo check --workspace` passed.
- `cargo test --workspace` passed: 232 tests, 10 DATABASE_URL-backed agenter-db tests ignored, 0 failed.
- Frontend gate passed: `npm run check`, `npm run lint`, `npm run test`, and `npm run build`.
- `git diff --check` passed.
- `cargo fmt --all -- --check` passed after applying `cargo fmt --all`.
- `cargo clippy --workspace -- -D warnings` passed after boxing `RunnerClientMessage::Event` and `RunnerEvent::AgentEvent`, preserving the serde wire shape.

Stage 10 remaining risks:

- Live Codex shell/file approval, plan, diff, and cancel trace capture was not run by this worker.
- Live Qwen, Gemini, and OpenCode Stage 10 conformance traces were not run by this worker. Qwen needs a working configured model path; Gemini may require outside-sandbox auth helper access; OpenCode may require outside-sandbox local state database access.
- Harness crash during tool remains a manual chaos item in the smoke runbook rather than a new automated harness test.
- Runner reconnect preserves WAL replay/ack state, but live adapter ownership is still scoped to the runner process and current provider runtime loops. A transient control-plane WebSocket reconnect by the same runner process should not orphan control-plane approvals, but full proof that native waiters/provider runtimes survive every reconnect path remains a Stage 10 risk until those maps/runtimes are hoisted into a reconnect-stable supervisor and covered by an automated chaos test.

## Scheduling And Validation Rules

- Stage 0 must complete before any schema or code work.
- Stages 1 and 2 are sequential because storage depends on shared types.
- Stage 3 depends on Stage 2 and should land before frontend changes.
- Stage 4 can start after Stage 1 but must be integrated with Stage 2 append semantics before it is considered complete.
- Stage 5 can start after Stage 1 and may run in parallel with Stage 3 if file ownership is kept separate.
- Stages 6, 7, and 8 depend on Stage 5 and should run sequentially because they touch the same reducers.
- Stage 9 depends on Stages 2, 3, 6, 7, and 8.
- Stage 10 runs last and owns final cross-harness validation.

Controller validation before scheduling the next stage:

- [ ] Check `git status --short` and identify unrelated user changes.
- [ ] Read the worker's changed files.
- [ ] Dispatch a spec-review subagent for the completed stage.
- [ ] Dispatch a code-quality subagent for the completed stage.
- [ ] Ensure stage verification commands ran or record why they could not run.
- [ ] Update this plan with completion evidence and remaining risks.

## Final Acceptance Criteria

- Browser/control-plane protocol supports `uap/1` snapshot + `after_seq` replay.
- Control plane stores universal events durably and can rebuild session snapshots.
- Runner acks/replays events through a local WAL.
- Codex native adapter and ACP adapter both emit universal turns/items/content/plans/approvals while preserving safe native references. Raw full provider payload persistence remains out of scope without a future explicit policy ADR.
- Approval decisions and user-input answers are idempotent and native blocked requests are answered exactly once.
- Cancel/interrupt changes real native state only for supported provider hooks or blocked native approval cancellation. Unsupported live-turn cancel returns a typed `provider_cancel_not_supported` error and must not be advertised as a generic capability.
- Frontend renders from universal capabilities and snapshot state, with legacy fallback only for migration safety.
- Full Rust and frontend verification gates pass, or any environment limitation is recorded in this plan with the exact failed command and next action.
