# Approval resolution events

Status: accepted

Date: 2026-05-04

## Context

Universal approval state previously used `approval.requested` for both request
creation and terminal resolution projections. When a provider emitted only an
answer event, the snapshot reducer could create a resolved approval row with no
request timestamp or request details. Browser reloads then rendered those
answer-only rows at the transcript tail.

## Decision

`approval.requested` creates or enriches the approval entity. `approval.resolved`
is a lifecycle update for an existing approval and carries the approval id,
final status, resolution timestamp, optional resolving user, and redacted native
reference metadata.

Snapshot reducers apply `approval.resolved` only when the approval already
exists in snapshot state. Orphan resolution events remain in the universal event
log for audit and debugging, but they do not create transcript approval cards.

## Consequences

- Approval transcript rows are anchored by request events, not answer events.
- Request metadata such as title, details, options, subject, risk, and
  `requested_at` survives terminal resolution.
- Existing malformed projections are not repaired automatically. Users can use
  force session reload to rebuild adapter-returned histories from native source
  data.
