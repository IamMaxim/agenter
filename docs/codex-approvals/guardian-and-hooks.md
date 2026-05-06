# Guardian (auto-reviewer), strict auto-review, and hooks

## When approvals route to Guardian instead of interactive UI

`routes_approval_to_guardian` (`core/src/guardian/review.rs`):

Returns true when:

- Turn `approval_policy` is **`OnRequest` or `Granular(_)`**, **and**
- Config `approvals_reviewer == ApprovalsReviewer::AutoReview`.

Effects in `ToolOrchestrator`:

- **`guardian_review_id = Some(new_guardian_review_id())`** is attached to `ApprovalCtx`.
- **`ShellRuntime::start_approval_async` / unified_exec / apply_patch`** call `review_approval_request(...)` instead of registering a user `ExecApprovalRequest` / patch approval (still emit analytics-style events).

When **`strict_auto_review`** is enabled for the turn (`Session::strict_auto_review_enabled_for_turn`):

- **Even `Skip`** requirements still run **`request_approval`** with hooks **off**, forcing a guardian-shaped review (`orchestrator.rs` branch for `Skip`).

## Guardian review lifecycle

`review_approval_request` delegates to **`run_guardian_review`** (`review.rs`), which:

1. Emits `EventMsg::GuardianAssessment` with `InProgress` (includes stable `review_id`).
2. Runs **`run_guardian_review_session`** — an isolated reviewer session pinned to **`approval_policy = never`** without inheriting exec-policy rules (`review.rs` module doc around `run_guardian_review_session`).
3. Ends with **`GuardianAssessment` terminal event** (`Approved` / `Denied` / `TimedOut` / `Aborted`).
4. On deny, stores rationale in **`SessionServices.guardian_rejections[review_id]`** for later **`guardian_rejection_message`** when the tool rejects.
5. Emits **`GuardianWarning`** with human-readable summaries for both allows and denies.
6. **Circuit breaker**: repeated denies can **`abort_turn`** with `Interrupted` (`record_guardian_denial`).

Return mapping to **`ReviewDecision`** for tools:

- Allow → **`Approved`**
- Deny → **`Denied`**
- Timeout → **`TimedOut`** (distinct from denial in orchestrator rejection path)

Note: guardian review does **not** produce `ApprovedForSession` / execpolicy amendments on the wire — it collapses to allow/deny/timeout style outcomes as seen by tools.

### Parallel / threaded reviews

`spawn_approval_request_review` runs **`review_approval_request_with_cancel`** on a dedicated **current-thread** Tokio runtime in a **`std::thread`** (fire-and-forget `oneshot` result). Used where nested async waiting would deadlock or block (see call sites).

## Permission-request hooks (precedence over Guardian / human)

Hooks run only when **`evaluate_permission_request_hooks`** is true in `request_approval` (`orchestrator.rs`):

- **False** during **strict_auto-review** skips and some guardian-aligned paths as wired today.
- When the tool exposes `permission_request_payload` (`PermissionRequestPayload` includes `HookToolName` + JSON inputs; shell tools use **`bash`** with command/description).

Hook outcomes (`codex_hooks::PermissionRequestDecision`):

- **Allow** → `ReviewDecision::Approved` immediately (telemetry `Config`).
- **Deny { message }** → `ReviewDecision::Denied` wrapped as **`ToolError::Rejected`** (telemetry `Config`).

If hooks return neither, approval falls through to guardian or **`start_approval_async`**.
