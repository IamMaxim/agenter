# ACP Runner Support Implementation Plan

Status: active
Date: 2026-05-02

## Phase 1: Documentation And Spikes

- [x] Create `docs/acp/` notebook files for progress, provider matrix, and provider spike notes.
- [x] Add ACP spec and ADR for the generic runtime/provider profile architecture.
- [x] Record live initialize evidence for Qwen, Gemini, and OpenCode.
- [x] Verification: documentation file inventory and placeholder scan.

## Phase 2: Shared ACP Runtime

- [x] Add runner tests for provider profile advertisement, initialize capability mapping, fallback events, permission mapping, and workspace-contained file service.
- [x] Introduce `agents::acp` with JSON-RPC framing, provider profiles, session runtime, normalizers, and runner-backed client services.
- [x] Drop the old production Qwen adapter after promoting its useful framing/permission behavior into the generic runtime.
- [x] Verification: `cargo test -p agenter-runner acp`.

## Phase 3: Runner And Control-Plane Integration

- [x] Add `gemini` and `opencode` provider ids.
- [x] Add multi-provider ACP runner mode while preserving existing fake and Codex modes.
- [x] Make non-Codex session creation call the runner and persist native external session ids.
- [x] Route create/resume/refresh/send/approval/interrupt/shutdown through the generic ACP runtime.
- [x] Verification: focused control-plane tests for non-Codex `CreateSession`, plus runner ACP tests.

## Phase 4: Final Verification

- [x] Run Rust baseline: `cargo fmt --all -- --check`, `cargo check --workspace`, `cargo clippy --workspace -- -D warnings`, `cargo test --workspace`.
- [x] Run live initialize probes for all providers where the local environment permits.
- [x] Record any blocked live smoke evidence in `docs/acp/progress.md`.

## Phase 5: Gemini Trust/Auth Follow-Up

- [x] Update ACP notebook evidence for Gemini `0.40.1`, Qwen trusted-workspace initialize, and the Gemini sandbox `listen EPERM 0.0.0.0` limitation.
- [x] Preserve initialize-derived ACP capability details separately for `loadSession`, `sessionCapabilities.list`, `sessionCapabilities.resume`, and `sessionCapabilities.fork`.
- [x] Skip refresh/import `session/list` calls when list support is not advertised.
- [x] Give Gemini a longer ACP response timeout and include provider id plus stderr excerpts in timeout/setup errors.
- [x] Probe `session/new` and harmless `session/prompt` for Qwen and Gemini in `tmp/workspace`; record update/completion shapes in `docs/acp/spikes/`.
- [x] Re-run Rust baseline after follow-up hardening.
