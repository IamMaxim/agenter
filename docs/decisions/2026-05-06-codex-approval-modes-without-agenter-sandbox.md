# Codex Approval Modes Without Agenter Sandbox

Status: accepted

Date: 2026-05-06

## Context

Codex TUI pairs approval policy with native permission or sandbox presets.
Agenter needs similar approval behavior for remote browser control, persistent
rules, reload/reconnect recovery, and VM-isolated runners that intentionally
allow all operations. At the same time, Agenter should not invent a second
sandbox layer around native agents.

## Decision

Agenter approval mode is a first-class session setting, not a sandbox setting.
The supported modes are:

- `ask`
- `read_only_ask`
- `trusted_workspace`
- `allow_all_session`
- `allow_all_workspace`

Codex-native launch fields such as `approvalPolicy`, `sandboxPolicy`, and
`permissions` are provider pass-through configuration derived from approval
mode unless raw provider extras override them. They are not an Agenter sandbox
contract.

`allow_all_session` auto-approves every approval for the current session only.
`allow_all_workspace` creates or reuses a durable workspace/provider rule whose
matcher is `{ "type": "allow_all", "applies_to": "all_approval_kinds" }`.
Per user request, that rule applies to every approval kind by default.

Changing explicitly away from `allow_all_workspace` disables the active durable
workspace/provider allow-all rule before the safer mode is persisted. Sparse
model/mode/reasoning settings updates do not change approval-mode rules unless
`approval_mode` is present in the request.

Browser UI must label dangerous modes as "Allow all operations" and
"danger-full-access" and require deliberate confirmation before enabling them.

## Consequences

- Agenter can support dangerous VM-runner workflows without hiding the risk.
- Approval replay and rule revocation remain owned by the control plane.
- Native Codex sandbox or permission values may still appear in raw payloads
  and runner launch parameters, but Agenter does not promise to enforce them.
- Connector surfaces should project existing approval options, but dangerous
  approval-mode changes should remain browser-confirmed until a dedicated
  multi-step connector confirmation exists.
