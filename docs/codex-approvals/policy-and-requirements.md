# Policy and approval requirements

## `AskForApproval`

Defined in `codex-rs/protocol/src/protocol.rs` as `AskForApproval` (serialized kebab-case; `UnlessTrusted` is `"untrusted"` in JSON).

Semantic summary from code comments and `default_exec_approval_requirement`:

| Variant | Typical effect on “ask before exec” |
| --- | --- |
| `UnlessTrusted` | Ask for approvals broadly (legacy “untrusted” naming in serde). |
| `OnFailure` | Do **not** ask upfront; sandboxed run may fail, then escalation may prompt (see orchestrator `wants_no_sandbox_approval`). |
| `OnRequest` | Default; interacts with filesystem sandbox **kind**: see below. |
| `Granular(GranularApprovalConfig)` | Per-category toggles; can **forbid** sandbox approval prompts entirely. |
| `Never` | No user prompts for the default path; failures are not escalated for approval in the same way. |

`GranularApprovalConfig` fields (`protocol.rs`): `sandbox_approval`, `rules`, `skill_approval`, `request_permissions`, `mcp_elicitations`.

- `allows_sandbox_approval()` exposes `sandbox_approval`.
- If `NeedsApproval` would apply but granular disallows sandbox approval, the requirement becomes **`Forbidden`** with reason `"approval policy disallowed sandbox approval prompt"` (`core/src/tools/sandboxing.rs`, `default_exec_approval_requirement`).

## `ExecApprovalRequirement`

`core/src/tools/sandboxing.rs` defines what the **`ToolOrchestrator`** does before the first sandbox attempt:

- **`Skip { bypass_sandbox, proposed_execpolicy_amendment, .. }`** — no user/guardian approval step for policy purposes; may still hit **strict auto-review** (see guardian doc).
- **`NeedsApproval { reason, proposed_execpolicy_amendment, .. }`** — run the approval pipeline (`request_approval` → hook / guardian / user).
- **`Forbidden { reason }`** — refuse immediately (`ToolError::Rejected`).

The default mapper `default_exec_approval_requirement(policy, file_system_sandbox_policy)`:

- Computes `needs_approval` false for `Never` and `OnFailure`.
- For `OnRequest` / `Granular(_)`, requires approval when **`FileSystemSandboxKind::Restricted`**.
- For `UnlessTrusted`, `needs_approval` is always true (subject to granular forbidden case above).

Tools may override via `Approvable::exec_approval_requirement` — e.g. **`ApplyPatchRuntime`** always supplies patch-specific requirement rather than inferring purely from global policy (`core/src/tools/runtimes/apply_patch.rs`).

## First-attempt sandbox bypass

`sandbox_override_for_first_attempt(SandboxPermissions, &ExecApprovalRequirement)` returns `BypassSandboxFirstAttempt` when:

- The model/request asked for **`require_escalated_permissions`**, **or**
- The requirement is `Skip { bypass_sandbox: true, .. }` (e.g. exec-policy allow implying full trust).

## Session-scoped approval cache

`with_cached_approval` (`sandboxing.rs`) consults `SessionServices.tool_approvals`:

- Uses serialized JSON keys (`ApprovalStore`).
- If **all** keys map to `ReviewDecision::ApprovedForSession`, returns that without prompting.
- After a successful `ApprovedForSession` response, stores that decision **per key** so subsets of paths/commands can reuse session approval.

Shell / unified_exec use a composite **`ApprovalKey`** (canonicalized command, cwd, sandbox permissions, optional additional permissions). Apply-patch uses **`AbsolutePathBuf`** per file path for multi-key session semantics.

## Config surfaces

User-facing knobs include:

- `approval_policy` → `AskForApproval`
- `approvals_reviewer` → routes some prompts to **Guardian** when `AutoReview` (see [guardian-and-hooks.md](./guardian-and-hooks.md))
- Permissions / sandbox profile → drives `Restricted` vs unrestricted FS and networking behavior downstream of approvals

Additional model-facing guidance lives under `core/src/context/prompts/permissions/` (Markdown injected into prompts).
