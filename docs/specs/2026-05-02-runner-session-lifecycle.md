# Runner Session Lifecycle Spec

Status: proposed
Date: 2026-05-02

## Goal

Make runner session handling truthful under parallel use, restarts, and multiple local providers. One local runner should expose Codex plus available ACP providers, and each Agenter session should own its own native agent process so independent sessions can run concurrently.

## Status Model

Session status is durable UI state owned by the control plane. Runner/provider events may update it, and the control plane persists those updates before broadcasting them.

- `running`: a turn is actively executing.
- `waiting_for_approval`: the provider is blocked on an approval.
- `waiting_for_input`: the provider is blocked on extra user input.
- `idle`: the session is not executing and is available to continue.
- `stopped`: the session is persisted but no live native process is attached, normally after runner restart or disconnect.
- Existing terminal/degraded states remain: `failed`, `interrupted`, `degraded`, `archived`.

New turn completion should transition to `idle`. Older `completed` statuses remain readable for compatibility.

## Runner Model

The default runner mode auto-detects providers and advertises all local providers it can run:

- Codex, when `codex` is available.
- ACP providers from the generic ACP profiles: Qwen, Gemini, OpenCode.

Provider-specific CLI/env modes remain available for reproducible local testing.

Codex uses a per-session app-server process instead of one shared app-server. ACP already uses per-session clients and should keep an explicit per-session state map. Same-session prompts are serialized by the session runtime; different sessions may run at the same time.

## Reconnect Behavior

When a runner disconnects, the control plane marks active sessions on that runner as `stopped` and broadcasts status changes. Archived and failed sessions are not overwritten.

When the runner reconnects, discovered or explicitly resumed sessions can move back to `idle`, `running`, or a waiting state based on live runner/provider evidence.

## Non-Goals

- Browser-mediated provider authentication.
- Audit-grade native process supervision.
- Multi-agent orchestration across one chat.
