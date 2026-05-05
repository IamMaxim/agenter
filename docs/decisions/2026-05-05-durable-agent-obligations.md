# Durable Agent Obligations

Status: accepted

Date: 2026-05-05

## Context

Approval and user-input question state was split across runner-local waiter maps,
control-plane registries, `pending_approvals`, universal events, and browser
snapshots. That made reload, reconnect, stale answer, and runner-loss behavior
hard to reason about, especially for questions, which had no durable lifecycle
state comparable to approvals.

## Decision

Introduce `agent_obligations` as the durable shared lifecycle table for native
approval and user-input question obligations.

The table stores a common lifecycle:

- `pending`
- `presented`
- `resolving`
- `delivered_to_runner`
- `accepted_by_native`
- `resolved`
- `orphaned`
- `expired`
- `detached`

Approvals continue to project into `pending_approvals` for compatibility with
existing APIs and policy-rule code. `agent_obligations` is additive in this
phase and is populated from universal approval/question events. Runner-local
maps remain runtime waiters only; they are not canonical durable state.

Browser snapshots represent terminal approval and question states explicitly.
Stopped, failed, or archived native session evidence marks unresolved approvals
and questions as `orphaned`; transient runner WebSocket disconnects do not.

## Consequences

- Existing `pending_approvals` callers keep working while the project migrates
  toward one obligation state machine.
- Questions now have deterministic terminal reload state instead of remaining
  silently pending after provider ownership is lost.
- Future work can move delivery generation and native acknowledgement transitions
  from process memory into `agent_obligations` without another storage boundary
  change.
