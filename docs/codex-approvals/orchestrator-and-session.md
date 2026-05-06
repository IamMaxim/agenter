# Orchestrator and session bridging

## `ToolOrchestrator::run` (single pipeline for all tools)

File: `core/src/tools/orchestrator.rs`.

Sequence:

1. **Decide reviewer mode** — `use_guardian = routes_approval_to_guardian(turn_ctx) || strict_auto_review`.
2. **Classify exec approval requirement** — `tool.exec_approval_requirement(req)` or `default_exec_approval_requirement(approval_policy, fs_policy)`.
3. **Approval branch**
   - **`Skip`**: normally no prompt; **unless** `strict_auto_review`, in which case a guardian-shaped approval still runs (`new_guardian_review_id()`), with permission hooks **disabled**.
   - **`Forbidden`**: immediate `ToolError::Rejected`.
   - **`NeedsApproval`**: `request_approval(...)` with optional `guardian_review_id`; permission hooks run when `!strict_auto_review`.

`request_approval` order (`orchestrator.rs`):

1. Optional **`run_permission_request_hooks`** when the tool exposes `permission_request_payload` — can return Allow → `Approved`, or Deny → rejected with hook message (telemetry source `Config`).
2. Else **`tool.start_approval_async`** — either Guardian sub-session or `Session::request_command_approval` / patch path / cache (telemetry source `AutomatedReviewer` vs `User`).

Rejected outcomes map to **`ToolError::Rejected`** with either guardian rationale (`guardian_rejection_message`) or `"rejected by user"`. **`ReviewDecision::TimedOut`** maps to `guardian_timeout_message()`.

## Network approval integration

Before each attempt, `begin_network_approval` may activate managed-network gating (`NetworkApprovalMode::Immediate` vs `Deferred`). Deferred mode can attach a **`network_denial_cancellation_token`** to the sandbox attempt so in-flight work respects proxy policy updates.

If the first sandboxed attempt returns `CodexErr::Sandbox(SandboxErr::Denied { network_policy_decision, .. })` and the tool `escalate_on_failure()`:

- Computes optional **`network_approval_context`** for managed network.
- **`wants_no_sandbox_approval(approval_policy)`** gates whether retry-without-sandbox is even offered.
- Special case: `OnRequest` may allow a **network-only** follow-up approval when FS policy still implies `NeedsApproval` (see orchestrator comments around `allow_on_request_network_prompt`).
- Retry approval uses a distinct permission-hook run id: `"{call_id}:retry"`.

Successful retry runs with **`SandboxType::None`** (elevated).

## Session: pending approvals

### Command execution — `Session::request_command_approval`

File: `core/src/session/mod.rs`.

- Builds `effective_approval_id = approval_id.unwrap_or_else(|| call_id)` — sub-command / execve intercept paths set `approval_id` separately from the outer `call_id`.
- Registers `oneshot::Sender<ReviewDecision>` in **active turn** turn-state via `insert_pending_approval(effective_approval_id, tx)`.
- Constructs `ExecApprovalRequestEvent` including parsed command segments, optional `network_approval_context`, `proposed_execpolicy_amendment`, `additional_permissions`, and **`available_decisions`** (either caller-supplied or derived via `ExecApprovalRequestEvent::default_available_decisions`).
- `send_event(turn_context, EventMsg::ExecApprovalRequest(...))`.
- **`rx_approve.await.unwrap_or(ReviewDecision::Abort)`** — if the pending entry was cleared (e.g. interruption), resolves to **abort** rather than dangling.

### Patch approval — `Session::request_patch_approval`

- Registers pending approval under **`call_id`** (same string used as patch item id).
- Emits `EventMsg::ApplyPatchApprovalRequest`.
- Returns the **`Receiver`**; callers `.await` it (defaults apply on drop — see runtime).

### Applying user responses

Handlers `exec_approval` / `patch_approval` (`core/src/session/handlers.rs`) call `sess.notify_approval`.

`Session::notify_approval` removes the pending sender and **`send(decision)`** on the stored `oneshot`.

Special cases:

- **`ReviewDecision::Abort`** interrupts the session task (`interrupt_task`) rather than only notifying approval.
- **`ApprovedExecpolicyAmendment`** triggers `persist_execpolicy_amendment` and records a conversational fragment on success before notifying.

## Effective client decisions (`ExecApprovalRequestEvent`)

Implemented in `protocol/src/approvals.rs`:

- **`effective_available_decisions()`** uses explicit `available_decisions` when present; otherwise derives defaults:

  - Network prompt: Approved, ApprovedForSession, optional `NetworkPolicyAmendment` if an allow-rule is proposed in the amendments list, Abort.
  - Additional-permissions-only: Approved, Abort.
  - Default exec: Approved, optional `ApprovedExecpolicyAmendment` if amendment present, Abort.

Legacy note: **`Denied`** is a valid enum variant globally but excluded from these defaults; tooling can expose it explicitly when building custom `available_decisions`.
