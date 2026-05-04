# Persistent Approval Policy Rules

## Goal

Reduce repeated approval prompts by letting users persist broader approval rules
for a workspace/provider, similar to Codex TUI command-prefix approval behavior.

## Architecture

The control plane owns durable rules in Postgres. Rules are scoped by user,
workspace, provider, and approval kind. Runner adapters still own native
approval delivery; persisted rules only decide whether the control plane should
answer a native approval automatically.

## Data Model

`approval_policy_rules` stores the rule id, owner user, workspace, provider,
approval kind, matcher JSON, approval decision JSON, source approval id,
creator/disabled metadata, and timestamps.

Initial matcher shapes:

- `command_prefix`: command begins with the stored argv prefix.
- `workspace_file_change`: provider asks for file-change approval in the same
  workspace/provider scope.
- `native_method`: provider-specific permission/tool method matches exactly.

## Behavior

Approval events are enriched with persistent rule-preview options when agenter
can derive a stable matcher. Selecting one stores the rule and resolves the
current approval. Later matching approvals are automatically resolved through
the same runner command/ack path as manual approval decisions.

The browser lists active rules for the current workspace/provider and can
disable a rule. Disabled rules no longer match but remain in storage.

## Verification

Use the standard Rust and frontend gates from `docs/harness/VERIFICATION.md`.
Focused checks should include policy matcher tests, approval route tests,
repository compile coverage, frontend API tests, and Svelte type checks.
