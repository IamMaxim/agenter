# Codex runner observability implementation plan

Status: implemented

Date: 2026-05-03

## Goal

Add opt-in runner-local raw Codex wire logs and richer runner tracing so stuck
Codex chats and approval stalls can be diagnosed from runner-side evidence.

## Implementation checklist

- [x] Add `CodexWireLogger` to the runner Codex adapter.
- [x] Gate raw JSONL logging behind `AGENTER_CODEX_RAW_LOG=1`.
- [x] Support `AGENTER_CODEX_RAW_LOG_DIR` with a safe local default under
  `tmp/agenter-logs/codex-wire`.
- [x] Log outbound JSON-RPC requests, outbound JSON-RPC responses/errors,
  inbound stdout JSON-RPC frames, Codex stderr, interleaved queue/drain events,
  and scope-dropped frames.
- [x] Include session id, workspace, JSON-RPC id, method, provider thread/turn
  ids, runner runtime thread/turn ids, classification, reason, and raw payload
  where applicable.
- [x] Expand turn-loop tracing for approvals, questions, unsupported requests,
  unexpected responses, and active-scope drops.
- [x] Keep raw provider payloads runner-local; do not add control-plane,
  database, WebSocket, or browser event surfaces.
- [x] Record the privacy/placement decision in
  `docs/decisions/2026-05-03-runner-local-codex-wire-logs.md`.
- [x] Add operator usage notes in `docs/runbooks/codex-wire-logging.md`.

## Verification

- [x] Add failing Codex runner tests for wire classification, scope context, and
  JSONL record writing.
- [x] Run `cargo test -p agenter-runner codex_wire` and confirm the new tests
  pass after implementation.
- [x] Run `cargo test -p agenter-runner codex_` for existing Codex adapter
  coverage.
- [x] Run full Rust verification from `docs/harness/VERIFICATION.md`:
  `cargo fmt --all -- --check`, `cargo check --workspace`,
  `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace`.

## Known risks

- Raw JSONL files contain sensitive provider payloads when enabled.
- Logs are local process diagnostics, not durable product audit records.
- Manual reproduction against a live Codex session is still needed to capture a
  real stuck approval transcript.
