# Composer Usage Bar Plan

Status: implemented
Date: 2026-05-01

## Goal

Expose session controls and live usage directly under the chat composer input.

## Implemented Behavior

- `SessionInfo` carries an optional `usage` snapshot for first paint.
- The control plane folds Codex `token_usage` provider events into context usage.
- The control plane folds Codex `rate_limits` provider events into 5h and weekly windows.
- `primary` rate limit maps to the 5h window and `secondary` maps to the weekly window.
- The browser composer bottom bar renders mode, model, thinking level, context usage, 5h remaining, and weekly remaining.
- Missing metrics render as `--`.

## Files Changed

- Backend API and state: `crates/agenter-core/src/session.rs`, `crates/agenter-control-plane/src/state.rs`, `crates/agenter-db/src/repositories.rs`
- Frontend contract and UI: `web/src/api/types.ts`, `web/src/lib/normalizers.ts`, `web/src/routes/ChatRoute.svelte`, `web/src/styles.css`
- Persistence: existing migration `migrations/0004_session_usage_snapshot.sql`

## Verification

- Rust: run `cargo fmt --all -- --check`, `cargo check --workspace`, and focused tests for control-plane usage parsing.
- Frontend: run `cd web && npm run check && npm run test`.
- Manual: open a Codex session, confirm the composer bar order and hover titles.

## Exit Criteria

- Session API includes usage fields when available.
- Missing usage does not display misleading zero values.
- Model, mode, and reasoning controls remain interactive in the composer.
