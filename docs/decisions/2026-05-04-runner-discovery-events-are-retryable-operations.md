# Runner Discovery Events Are Retryable Operations

Status: accepted

Date: 2026-05-04

## Context

Runner WAL replay is for canonical runner-originated universal agent events that must survive a control-plane WebSocket reconnect before durable control-plane acceptance.

Session discovery refreshes can carry large imported native histories. Treating those refresh payloads as WAL-replayable events caused the runner to retain and repeatedly serialize a large `SessionsDiscovered` record while processing normal small acknowledgements.

## Decision

`SessionsDiscovered`, `OperationUpdated`, health, and runner error events are retryable operation/control messages, not runner WAL replay records.

Only universal `AgentEvent` records are replayable through the runner WAL. Session discovery/import durability is owned by the control plane: it records refresh status, import summaries, and failures, and the operator/browser retries an explicit refresh when needed.

Runner WAL acknowledgements are persisted as a small cursor. The WAL record log remains append-only during normal event emission and acknowledgement, with occasional compaction and startup repair for legacy non-replayable records.

## Consequences

- A failed session refresh may need an explicit retry instead of automatic runner replay.
- Large discovered-history payloads no longer stay in the runner WAL or make unrelated event acknowledgements CPU-heavy.
- Reconnect replay remains lossless for canonical universal agent events.
- Old WAL files containing discovery payloads are repaired on runner startup by dropping non-replayable records while preserving agent events.
