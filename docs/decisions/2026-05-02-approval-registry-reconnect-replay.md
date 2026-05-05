# Approval registry snapshots for reconnect replay

Status: accepted

Date: 2026-05-02

## Context

Pending provider approvals can outlive individual `approval_requested` rows in the control plane’s bounded in-memory ring buffer (`SESSION_EVENT_CACHE_LIMIT`). Without another source of truth, a browser reload could lose the approval card while the runner still waits on a native approval.

## Decision

1. **Registry holds the request envelope** while an approval is `Pending` or `Resolving`: `ApprovalStatus` stores `Box<BrowserEventEnvelope>` for the originating `approval_requested` event (cloned at publish time). `Resolved` continues to hold the resolved event envelope only.

2. **Replay merge** applies after loading session history from Postgres or memory and when building the WebSocket subscribe snapshot: append any pending/resolving request envelopes whose `approval_id` is not already present in the stream (requested or resolved).

3. **`GET /api/approvals?session_id=…`** returns the same pending/resolving request envelopes for an owned session (auth + `can_access_session`).

4. **Resolution means provider-adapter acknowledgement, not WebSocket delivery.** Browser approval decisions move the registry to `Resolving` and start an in-memory runner command operation. The approval becomes `Resolved` only after the runner confirms the native approval response was written successfully. If the runner rejects, disconnects, times out, or loses the provider request, the control plane returns the approval to `Pending` and emits a visible error/status event.

5. **Replay exposes in-flight state.** Replayed unresolved approval requests may include `resolution_state: "pending" | "resolving"` and, while resolving, `resolving_decision`. Browser clients render resolving approvals as disabled/in-flight rather than resolved.

## Consequences

- Memory per pending approval grows by one boxed envelope until resolution.
- Resolved approvals do not retain the original request envelope in the registry.
- In-flight operation state is process-local for this iteration; control-plane restarts still lose unresolved command operations.
