# Approval Delivery Idempotency

Status: accepted

Date: 2026-05-03

## Context

Browser reloads during Vite hot reload can interrupt the visible approval flow while
Codex or an ACP provider is still blocked on a native approval request. The control
plane already replays pending and resolving approvals, but the runner removed a
native pending approval from its in-memory map as soon as it received an
`AnswerApproval` command. If the browser or runner connection dropped before the
control plane observed final acknowledgement, a retry could find the control plane
back in `Pending` while the runner had already forgotten the provider request.

## Decision

For this iteration, approval reliability covers browser reloads, aborted browser
fetches, browser WebSocket drops, and runner WebSocket reconnects while the
control-plane process remains alive.

Runner approval delivery is idempotent:

- a pending approval stays registered in the runner until native provider delivery
  is acknowledged;
- duplicate same-decision answers join the in-flight native delivery and receive
  the same outcome;
- duplicate conflicting answers are rejected as conflicts;
- completed native delivery can be replayed to late duplicate same-decision
  commands.

The control plane remains the owner of browser-visible `Pending`, `Resolving`,
and `Resolved` state. Control-plane restarts are still out of scope because they
require persisted pending approval rows and resumable runner command operations.

## Consequences

- The runner may retain completed approval entries for the lifetime of the native
  session process.
- Transient frontend interruption no longer turns a provider approval into
  `approval_not_found` merely because the first browser response was interrupted.
- Full restart durability remains a separate storage/protocol design.
