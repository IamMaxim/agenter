# Runner-local Codex wire logs

Status: accepted

Date: 2026-05-03

## Context

Codex chats can get stuck around native JSON-RPC requests such as approvals. The
runner already emitted metadata-level tracing and preserved provider payloads on
some normalized app events, but it did not keep a direction-aware transcript of
the runner-to-Codex stdio exchange. This made it hard to distinguish provider
silence, dropped scope-filtered messages, unanswered server requests, and failed
approval delivery.

Raw Codex payloads may include prompts, model output, file paths, diffs,
approval bodies, account metadata, and provider error details. They should not be
forwarded to browser clients or stored in the control plane event cache by
default.

## Decision

Codex wire logs are runner-local and opt-in.

- `AGENTER_CODEX_RAW_LOG=1` enables JSONL wire logging for the runner Codex
  adapter.
- `AGENTER_CODEX_RAW_LOG_DIR` optionally selects the output directory; otherwise
  logs go under `tmp/agenter-logs/codex-wire`.
- Normal tracing remains metadata-oriented by default.
- Raw JSON-RPC payloads are written only to the runner-local JSONL file and are
  not exposed through the control-plane API, browser WebSocket, database, or app
  event cache in this iteration.

## Consequences

- Operators can inspect exact Codex stdio traffic when debugging stuck sessions.
- The control plane remains a normalized event projection rather than raw
  provider transcript storage.
- Operators must treat raw wire logs as sensitive local diagnostics and delete or
  rotate them according to their environment.

## Alternatives Considered

- Always write raw payloads locally: easier debugging, but too risky as a
  default because prompts, diffs, and account details can appear in provider
  frames.
- Forward raw wire frames as app events: useful in the browser, but it pollutes
  transcript history and changes the privacy boundary.
- Enrich tracing only: safer, but insufficient when the exact JSON-RPC request
  and response bodies are needed to debug provider stalls.
