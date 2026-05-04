# Persistent Approval Policy Rules Implementation Plan

Status: implemented, verification passed
Date: 2026-05-04
Spec: `docs/specs/2026-05-04-persistent-approval-policy-rules.md`
Decision: `docs/decisions/2026-05-04-persistent-approval-policy-rules.md`

## Goal

Port Codex TUI-style broad approval behavior to agenter by storing durable,
workspace/provider-scoped approval rules in the control plane.

## Steps

- [x] Add the policy-rule ADR and spec.
- [x] Add `approval_policy_rules` migration and DB repository functions.
- [x] Extend canonical approval options with persistent rule-preview metadata.
- [x] Derive conservative command, file-change, and native-method rule previews.
- [x] Persist selected rule-preview options from approval decisions.
- [x] Auto-resolve matching future approvals through the existing runner ack path.
- [x] Add browser API and UI to list and revoke rules.
- [x] Run full Rust and frontend verification.

## Verification

- `cargo test -p agenter-control-plane policy -- --nocapture`
- `cargo check --workspace`
- `cargo fmt --all -- --check`
- `cargo test --workspace`
- `cd web && npm run check`
- `cd web && npm run lint`
- `cd web && npm run test`
- `cd web && npm run build`

Verification completed on 2026-05-04:

- `cargo test -p agenter-control-plane policy -- --nocapture`
- `cargo test -p agenter-control-plane approval -- --nocapture`
- `cargo check --workspace`
- `cargo fmt --all -- --check`
- `cargo clippy --workspace -- -D warnings`
- `cargo test --workspace`
- `cd web && npm run check`
- `cd web && npm run lint`
- `cd web && npm run test`
- `cd web && npm run build`
- `git diff --check`
