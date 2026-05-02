# ACP Runner Support Spec

Status: proposed
Date: 2026-05-02

## Goal

Add generic ACP support to the runner for Qwen Code, Gemini CLI, and OpenCode while preserving Agenter's control-plane / runner boundary. The runner remains the only component that speaks native provider protocols and maps them into normalized app events.

## Architecture

The runner gets a shared ACP runtime that owns stdio JSON-RPC framing, request correlation, initialization, session lifecycle, prompt turns, cancellation, provider fallback events, and provider-side requests to the client.

Provider-specific behavior lives in small profiles:

- `qwen`: `qwen --acp --approval-mode default`
- `gemini`: `gemini --acp`
- `opencode`: `opencode acp --cwd <workspace>`

One runner process advertises all configured ACP providers that are locally available for the workspace. Provider authentication is not managed by Agenter; missing auth or setup failures are reported as degraded provider errors with actionable local commands.

## Session Model

ACP sessions use provider-native `sessionId` values as Agenter `external_session_id`.

- `CreateSession` calls `session/new` and returns `SessionCreated`.
- `ResumeSession` prefers ACP resume when advertised, then falls back to `session/load` when supported.
- `RefreshSessions` calls `session/list` when advertised and emits discovered sessions with metadata.
- `AgentSendInput` calls `session/prompt` against the stored native session id.

Missing provider session discovery or history support degrades gracefully. Unknown ACP notifications are emitted as `ProviderEvent` rather than dropped.

## ACP Client Services

The runner implements ACP client methods for the configured workspace:

- `fs/read_text_file`: reads only absolute paths contained by the workspace.
- `fs/write_text_file`: writes only contained paths and emits file-change app events.
- `terminal/create`: runs a command in the workspace and emits command start/output/completed events.
- `terminal/output`, `terminal/wait_for_exit`, `terminal/kill`, and `terminal/release`: track command state through runner-managed terminal handles.
- `session/request_permission`: creates an Agenter approval and returns the selected provider option.

All provider payloads remain attached to events for diagnostics.

## Non-Goals

- Browser-mediated provider authentication.
- Full ACP MCP server hosting.
- Provider-native slash command execution beyond normalized/fallback visibility.
- Audit-grade transcript storage.

