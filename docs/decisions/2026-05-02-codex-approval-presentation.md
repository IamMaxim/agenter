# Codex approval presentation and item cache

Status: accepted

Date: 2026-05-02

## Context

Codex app-server sends **sparse** JSON-RPC bodies for `item/fileChange/requestApproval` (and sometimes minimal command approval bodies). Usable copy for the user lives on earlier notifications, especially `item/started` for `type: fileChange` with a `changes` collection, and may be refined by `item/fileChange/patchUpdated`. Agenter previously stringified the approval params when no `path`/`command` field was present, which produced opaque JSON in the browser and did not match Codex TUI behavior.

## Decision

1. **Runner-side correlation.** The Codex adapter maintains a **per-turn** `CodexApprovalItemCache` that ingests `item/started` (fileChange) and `item/fileChange/patchUpdated`, keyed by Codex **item id** (`itemId` / `params.item.id`), matching Codex TUI’s use of `item_id` for file-change approvals.

2. **`ApprovalRequestEvent.presentation`.** [`agenter-core`](../../crates/agenter-core/src/approval.rs) carries an optional `presentation` JSON value alongside stable `title`/`details`/kind. Codex-derived shapes emitted today:

   - `variant: codex_file_change` — `paths`, `files[]` with `path`, `change_kind`, `unified_diff`.
   - `variant: codex_command` — `command`, optional `cwd`, `available_decisions` (Codex-native strings when supplied).

   `provider_payload` remains the raw JSON-RPC message for debugging.

3. **Browser behavior.** [`web`](../../web/) maps `presentation` onto chat approval items, renders structured file diffs and command lines, exposes **Accept / Accept for session / Decline / Cancel** (subset when Codex sends `availableDecisions`), and posts decisions using existing `ApprovalDecision` wire format.

## Consequences

- Other agents may attach their own neutral `presentation` variants without coupling the core enum to Codex crates.
- Exec-policy / network-amendment decisions remain future work (`ApprovalDecision::ProviderSpecific`).
