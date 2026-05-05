# Universal-Only UAP/2

Status: active
Date: 2026-05-05

## Purpose

Agenter's shared runtime contract is `uap/2`. Provider runtimes, the runner WAL,
the control plane, and browser clients exchange universal commands and universal
events. Provider-specific protocols are adapter inputs only; they are not shared
architecture.

## Runtime Contract

- Provider adapters emit `AgentUniversalEvent` / `UniversalEventKind` directly.
- Browser WebSocket frames are limited to `session_snapshot`,
  `universal_event`, `ack`, and `error`.
- Session snapshots are materialized from universal events and carry explicit
  replay cursor fields.
- Approval, question, turn, item, plan, tool, diff, artifact, usage, and
  provider notification state is represented with universal structs.
- Native details are safe references in `NativeRef`; raw provider payloads do
  not become browser-facing state.

## Provider IDs

Provider IDs are open strings. The first live provider family is ACP with
profiles for `qwen`, `gemini`, and `opencode`. The protocol must not branch on
provider names for semantic behavior.

## Removed Compatibility Surface

This development branch has no `NormalizedEvent` runtime bridge and no Codex
compatibility window. Codex can return later as another adapter that emits
universal events directly.

## Acceptance Signals

- ACP reducers emit direct `UniversalEventKind` values with stable active
  `turn_id` attachment.
- Fake runner smoke emits a deterministic universal event story.
- Control-plane ingestion accepts runner universal events as the canonical path.
- Browser tests reject stale snapshot/frame shapes and materialize only `uap/2`
  frames.
- UI copy and presentation are driven by universal fields and capabilities, not
  Codex-specific variants.
