# Durable Agent Event Log

Status: accepted
Date: 2026-05-03

## Context

Agenter already keeps a lightweight `event_cache` for UI and connector recovery, while native agent systems remain the preferred source of truth for conversation history when their sessions can be listed, loaded, or read. The universal protocol requires browser reload and reconnect behavior that can rebuild visible state from a snapshot plus missed events. It also needs clear boundaries for runner reconnects, approval recovery, and recent native-turn correlation.

The first product scope still excludes audit-grade full transcript storage. A durable browser projection log must not silently redefine the control plane as the canonical full transcript database.

## Decision

Add an append-only `agent_events` log as the durable `uap/1` browser replay and control-plane projection log. Each universal event receives a monotonic global database `seq` from the `agent_events` sequence and stores the normalized event payload, source, optional command correlation, optional turn and item IDs, and safe native reference data.

`seq` is global across `agent_events`, not per session. In Rust it is represented as a 64-bit integer type (`i64` for SQLx/Postgres `bigint`, or `u64` where the domain type guarantees non-negative values). On JSON wires it is serialized as a string to avoid JavaScript integer precision loss. Browser `after_seq` uses this global cursor, filtered by the subscribed session.

`event_cache` remains readable and writable as a compatibility cache during migration. New code may emit both `agent_events` and `event_cache` until the browser no longer depends on the legacy event model.

Native harnesses remain canonical history where their history can be reloaded. The control-plane `agent_events` log is authoritative for Agenter's browser/reconnect projection and pending control-plane state, not an audit-grade full transcript unless a future ADR explicitly expands that scope.

`native_json` and native references in `agent_events` store redacted summaries, native IDs, method/type names, hashes, or pointers by default, not full raw provider payloads. Raw Codex JSON-RPC/wire payloads remain runner-local opt-in diagnostics under `docs/decisions/2026-05-03-runner-local-codex-wire-logs.md` and must not be stored in the control-plane database, browser WebSocket stream, control-plane API, or app event cache by default. Persisting full raw native payloads requires a later explicit policy ADR.

`SessionSnapshot` and related session, turn, item, approval, plan, diff, and artifact views are materialized views derived from `agent_events`. Snapshots may be rebuilt from the log, and clients should treat snapshots plus events after a cursor as the replay contract.

The recent-turn cache is a persistent correlation and recovery aid, not the transcript source of truth. It maps native request IDs, active item IDs, pending approval IDs, content buffers, outstanding RPCs, and latest universal sequence for active or recently active turns.

Lossless runner reconnect cannot be claimed until the runner writes outbound events to a local WAL and the control plane acknowledges only after durable append to `agent_events`. Until runner WAL plus control-plane ack exists, runner reconnect behavior may be useful but not lossless.

This decision supersedes the stopped-on-disconnect portion of `docs/decisions/2026-05-02-runner-session-process-lifecycle.md` after Stage 4 lands. Before runner WAL/reconnect is implemented, active sessions may still be marked `stopped` on runner disconnect to avoid stale running state. After Stage 4, a transient control-plane WebSocket disconnect does not by itself stop sessions when runner evidence proves native process ownership survived. If runner or native process ownership is gone, the control plane marks sessions `stopped`, turns `detached` or failed, and approvals orphaned according to the available evidence.

Browser reconnect uses snapshot plus `after_seq` replay. The browser subscribes with its last seen sequence and may request a snapshot; the control plane responds with the current `SessionSnapshot`, missed events after `after_seq`, and the latest sequence before live streaming resumes.

## Open Question Resolutions

1. Codex-native app-server remains the primary Codex source; its reloadable native history is preferred where available, with `agent_events` serving Agenter replay.
2. Harnesses execute their own tools for the first milestone; `agent_events` stores the resulting universal facts and native payloads needed for policy, UI, and debugging.
3. One active turn per native session keeps recent-turn correlation tractable. Parallelism uses separate Agenter sessions until native adapters prove concurrency support.
4. Pending approvals wait through transient runner/control-plane outages. If the runner survives, it retains the native request obligation; if the runner or native process dies, the approval becomes orphaned and the turn is failed, cancelled, or detached according to evidence.

## Consequences

Frontend reload and reconnect can become deterministic without making `event_cache` canonical.

The database gains an append-only projection log and materialized snapshots. This adds storage and reducer work, but gives a clear cursor-based replay contract.

Runner reliability has an explicit bar: outbound WAL before send and durable append ack before cleanup. Without that bar, documentation and UI must not describe reconnect as lossless. Once that bar is met, transient control-plane WebSocket disconnects no longer imply stopped sessions unless runner evidence says native ownership was lost.

The source-of-truth rule stays intact. Provider-backed history is still preferred for full history where possible; Agenter's durable log covers browser projection, reconnect, approvals, and control-plane state.

## Alternatives Considered

- Continue using only `event_cache`: lower migration cost, but insufficient for universal sequence replay and snapshot reconstruction.
- Make the control plane the canonical full transcript store now: simpler replay model, but conflicts with the v1 non-goal and increases privacy, audit, and storage obligations.
- Keep recent-turn state only in memory: simpler implementation, but loses approval and native correlation across restarts and reconnects.
- Claim runner reconnect is reliable without WAL and durable ack: misleading, because events can be lost between native receipt and control-plane persistence.
