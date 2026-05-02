# Approval registry snapshots for reconnect replay

Status: accepted

Date: 2026-05-02

## Context

Pending Codex approvals can outlive individual `approval_requested` rows in the control plane’s bounded in-memory ring buffer (`SESSION_EVENT_CACHE_LIMIT`). Without another source of truth, a browser reload could lose the approval card while the runner still waited on JSON-RPC approval.

## Decision

1. **Registry holds the request envelope** while an approval is `Pending` or `Resolving`: `ApprovalStatus` stores `Box<BrowserEventEnvelope>` for the originating `approval_requested` event (cloned at publish time). `Resolved` continues to hold the resolved event envelope only.

2. **Replay merge** applies after loading session history from Postgres or memory and when building the WebSocket subscribe snapshot: append any pending/resolving request envelopes whose `approval_id` is not already present in the stream (requested or resolved).

3. **`GET /api/approvals?session_id=…`** returns the same pending/resolving request envelopes for an owned session (auth + `can_access_session`).

## Consequences

- Memory per pending approval grows by one boxed envelope until resolution.
- Resolved approvals do not retain the original request envelope in the registry.
