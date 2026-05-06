# Codex TUI approvals — documentation index

This folder documents how **command, patch, permission, and related approvals** work in the Codex codebase checked out at `tmp/codex/` (upstream layout: `codex-rs/` Rust crates, including the terminal UI).

The approval system spans four layers:

1. **Policy** — when a tool invocation must ask (`AskForApproval`, filesystem sandbox kind, granular flags).
2. **Orchestration** — `ToolOrchestrator`: permission hooks → (guardian **or** user) decision → sandboxed run → optional escalation/retry approval.
3. **Session bridging** — `Session::request_command_approval` / `request_patch_approval`: register a pending `oneshot`, emit `EventMsg::*` to the client, await `ReviewDecision`.
4. **TUI** — `ApprovalOverlay`: list/select modal, keyboard shortcuts, delayed display while the user types, routing back via `AppCommand::ExecApproval` / `PatchApproval` / etc.

Read in order:

| Doc | Topics |
| --- | --- |
| [policy-and-requirements.md](./policy-and-requirements.md) | `AskForApproval`, `ExecApprovalRequirement`, `GranularApprovalConfig`, `with_cached_approval` |
| [orchestrator-and-session.md](./orchestrator-and-session.md) | `ToolOrchestrator::run`, `request_command_approval`, effective approval IDs, sandbox retry |
| [guardian-and-hooks.md](./guardian-and-hooks.md) | Auto-reviewer routing, guardian review session, permission-request hooks |
| [tui-ux.md](./tui-ux.md) | `ApprovalOverlay`, composer typing delay, interrupt queue ordering |
| [other-surfaces.md](./other-surfaces.md) | Patch flow, `request_permissions`, MCP, protocol fields |

Primary source directories in `tmp/codex/codex-rs/`:

- `core/src/tools/orchestrator.rs`, `core/src/tools/sandboxing.rs`
- `core/src/session/mod.rs` (approval helpers)
- `core/src/guardian/` (automated reviewer)
- `protocol/src/protocol.rs`, `protocol/src/approvals.rs` (`ReviewDecision`, `ExecApprovalRequestEvent`)
- `tui/src/bottom_pane/approval_overlay.rs`, `tui/src/bottom_pane/mod.rs`, `tui/src/chatwidget.rs`

This is reverse-engineered documentation only; behavior may change upstream.
