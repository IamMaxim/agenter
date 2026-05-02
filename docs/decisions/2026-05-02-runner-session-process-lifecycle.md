# Runner Session Process Lifecycle

Status: accepted
Date: 2026-05-02

## Context

The runner previously mixed two lifecycle assumptions. Codex reused one app-server process across all sessions, while ACP sessions owned provider clients per session. The control plane also registered sessions as `running` and broadcast status events without persisting those status transitions, so the session list could show stale running sessions after restarts.

## Decision

Use per-session native agent processes and make session status a persisted control-plane field. The default runner advertises all locally available providers, including Codex and available ACP providers. Turn completion transitions sessions to `idle`; runner disconnect marks active sessions as `stopped`.

## Consequences

- Different sessions can execute in parallel without sharing one provider process lock.
- Session list and chat status use the same persisted source of truth.
- Runner restart does not leave stale `running` sessions in the UI.
- Codex process count grows with active sessions, so shutdown and interrupt handling must clean up session-local runtimes.

## Alternatives Considered

- Keep one process per provider: simpler, but serializes unrelated sessions and makes status/process ownership ambiguous.
- Keep `completed` as idle: avoids a new status but conflates turn result with live process availability.
- Add a separate runtime-only status overlay: less invasive, but leaves durable session list state misleading after restart.
