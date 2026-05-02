# Runner Session Lifecycle Implementation Plan

Status: active
Date: 2026-05-02

## Phase 1: Status Semantics

- [x] Add `idle` and `stopped` session statuses to Rust, SQL, and TypeScript contracts.
- [x] Persist `SessionStatusChanged` events into the session registry/database before broadcasting.
- [x] Transition provider turn completion to `idle`.

## Phase 2: Runner Disconnect/Reconnect Truth

- [x] Mark active sessions on a disconnected runner as `stopped`.
- [x] Keep archived and failed sessions unchanged.
- [x] Ensure discovered sessions import as `idle` instead of `running`.

## Phase 3: Provider And Process Lifecycle

- [x] Make the default runner advertise Codex plus available ACP providers.
- [x] Keep explicit single-provider modes for local reproducibility.
- [x] Move Codex to per-session runtimes so parallel sessions do not share one app-server lock.
- [x] Serialize same-session overlapping turns deterministically.
- [x] Implement shutdown cleanup for session-local runtimes where supported.

## Phase 4: Verification

- [x] Add focused Rust tests for status persistence, disconnect stop marking, and provider-mode selection.
- [x] Add focused web tests for `idle` and `stopped` labels.
- [x] Run Rust and web verification from `docs/harness/VERIFICATION.md`.
