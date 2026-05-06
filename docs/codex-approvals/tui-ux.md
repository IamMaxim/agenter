# TUI presentation and interaction

## Event → modal pipeline

Primary handler: `tui/src/chatwidget.rs`:

- Incoming events include `EventMsg::ExecApprovalRequest`, `ApplyPatchApprovalRequest`, `RequestPermissions`, MCP elicitations (`on_*` methods).
- **`defer_or_handle`** queues “interrupt-class” UX while assistant **streaming** is active:

  ```text
  if stream_controller.is_some() || interrupts non-empty → enqueue InterruptManager
  else → handle immediately
  ```

  Rationale (`chatwidget.rs`): preserve FIFO ordering for related exec begin/end deltas.

  Flushing happens on stream completion (`handle_stream_finished` → `flush_interrupt_queue`).

- When handled, exec approvals call **`handle_exec_approval_now`**, which converts `ExecApprovalRequestEvent` → internal **`ApprovalRequest::Exec`** and forwards to **`BottomPane::push_approval_request`**.
- **`effective_available_decisions()`** mirrors server defaults so the overlay options match canonical protocol behavior (`chatwidget.rs` uses the event helper).

Parallel path: **`chatwidget/interrupts.rs`** queues `QueuedInterrupt::*` typed variants; flushing replays through the same `handle_*_now` methods.

### Guardian UX

Guardian emits separate **`GuardianAssessment`** messages; **`on_guardian_assessment`** summarizes into status/history (“automatic approval review …”) without necessarily opening the approve/deny overlay.

## Approval modal (`ApprovalOverlay`)

File: `tui/src/bottom_pane/approval_overlay.rs`.

- Presents **`ApprovalRequest`** variants: **`Exec`**, **`Permissions`**, **`ApplyPatch`**, **`McpElicitation`**.
- Implemented as **`ListSelectionView`** over **`ApprovalOption`** entries with contextual hints (`ApprovalKeymap` + list keybindings).
- **Ctrl-C / Esc semantics**: module-level contract — MCP elicitation maps Esc to **`Cancel`**, avoiding silent “continue without answering” mistakes (see file header docs / tests).

Decisions propagate through **`AppEventSender`** (`tui/src/app_event_sender.rs`):

- Exec → `AppCommand::exec_approval(id, turn_id_none, decision)` wrapped in `AppEvent::SubmitThreadOp { thread_id, op }`.
- Patch → `AppCommand::patch_approval`.
- Permissions → `request_permissions_response`.
- Elicitation → `resolve_elicitation(...)`.

Cancellation keys are configurable (`RuntimeKeymap`); tests demonstrate list cancel emitting **`ReviewDecision::Abort`** for exec.

## Composer typing delay (“don’t steal focus immediately”)

`BottomPane` (`bottom_pane/mod.rs`):

- Constant **`APPROVAL_PROMPT_TYPING_IDLE_DELAY`** = **1 second** after last composer activity (`last_composer_activity_at`).
- If an approval arrives while composer was recently active, or other approvals are queued, requests go to **`delayed_approval_requests`** (FIFO).
- `maybe_show_delayed_approval_requests_at` promotes the earliest request after idle expiry; merges multiple backlog entries into **`ApprovalOverlay::enqueue_request`** preserving FIFO rendering.

## Overlay stacking (`try_consume_approval_request`)

If another **`BottomPaneView`** is active (`view_stack`), the top view may **`try_consume_approval_request`** to merge or postpone; **`None`** from that hook causes **early return without showing** (`bottom_pane/mod.rs` — dependent on active view semantics).
