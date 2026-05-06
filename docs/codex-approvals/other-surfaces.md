# Patch, permissions, MCP, and protocol notes

## `apply_patch` approvals

Runtime: `core/src/tools/runtimes/apply_patch.rs`.

Highlights:

- **Guardian**: when routed, constructs `GuardianApprovalRequest::ApplyPatch { cwd, files, patch, id }`.
- **`permissions_preapproved` + no retry_reason** short-circuit to **`Approved`** without UI.
- **Retry paths** (`retry_reason Some`) bypass `with_cached_approval` and issue **`request_patch_approval`** synchronously awaited.
- **Session cache**: `approval_keys()` are **`file_paths` vectors** — `ApprovedForSession` requires prior per-path session allowance for all touched files.
- **Sandbox retry policy**: overrides `wants_no_sandbox_approval` broadly true except `Never`/`Granular`-without-sandbox toggle so patch failures may ask for escalation more readily.

## `request_permissions` tool / events

Implemented on `Session` (`session/mod.rs`):

- **`AskForApproval::Never`** grants **empty default** permissions instantly (automatic “allow” scaffold).
- Other policies enqueue user-facing **`RequestPermissions`** prompts (TUI `ApprovalRequest::Permissions`).
- Responses can record additional permissions onto **turn** or **session** state (`PermissionGrantScope::Turn | Session`). Turn scope optionally enables **`strict_auto_review`** persistence.

Orchestration ties into tool sandboxes (`AdditionalPermissionProfile`, unified exec deferral semantics).

## MCP tool approvals and elicitation

`core/src/mcp_tool_call.rs` contains MCP-specific policy:

- Honors `AppToolApproval` (auto vs prompt vs policy).
- Computes **session** and **persistent** approval keys; remembered approvals short-circuit to accept.
- When guardian routing applies, MCP calls use guardian review shaped as **`GuardianApprovalRequest`-like actions** surfaced in **`GuardianAssessmentAction::McpToolCall`**.
- MCP **elicitation** uses separate protocol structs; TUI renders form URL modes via `McpServerElicitationFormRequest` helpers when applicable, falling back to `ApprovalRequest::McpElicitation`.

## Execve / sub-shell intercept

`ExecApprovalRequestEvent` carries optional **`approval_id`** distinct from **`call_id`** for intercepted child execution paths (`effective_approval_id()` in approvals module). Pending map keys match this effective id.

## App-server parity (non-TUI clients)

See `codex-rs/app-server/README.md` § **Approvals** — mirrors the sequence (item lifecycle + JSON-RPC prompts + `{ "decision": ... }`). The README documents experimental **`additional_permissions`** payloads and **`available_decisions`** as the authoritative UI hint.

## Telemetry

`with_cached_approval` increments **`codex.approval.requested`** counters with `{ tool, approved }` tags based on **`decision.to_opaque_string()`**.

Orchestrator uses OTEL **`SessionTelemetry.tool_decision`** with `ToolDecisionSource` (`User`, `AutomatedReviewer`, `Config`).
