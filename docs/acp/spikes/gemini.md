# Gemini ACP Spike

## Local Setup

- Command: `gemini`
- Version: `0.40.1`
- ACP launch: `gemini --acp`

## Initialize Probe

Official Gemini CLI docs describe ACP mode as JSON-RPC over stdio and list `initialize`, `authenticate`, `newSession`, `loadSession`, `prompt`, and `cancel` as core methods.

Local probe status:

- Outside-sandbox initialize succeeds in both `/Users/maxim/work/agenter` and `/Users/maxim/work/agenter/tmp/workspace`.
- Sandboxed initialize fails while Gemini tries to start a local auth helper: `Error authenticating: Error: listen EPERM: operation not permitted 0.0.0.0`.
- Outside-sandbox stderr may include `Ripgrep is not available. Falling back to GrepTool.`

Observed initialize response summary:

- `agentInfo.name`: `gemini-cli`
- `agentInfo.version`: `0.40.1`
- `agentCapabilities.loadSession`: `true`
- `agentCapabilities.sessionCapabilities`: absent
- `promptCapabilities.image`, `audio`, and `embeddedContext`: true
- `mcpCapabilities.http` and `sse`: true
- `authMethods`: Google OAuth, Gemini API key, Vertex AI, AI API Gateway

## Session And Prompt Probe

Workspace: `/Users/maxim/work/agenter/tmp/workspace`

`session/new` response summary:

- `sessionId`: UUID string, for example `307a3dd5-45e5-47ba-ad51-413748d4c0cf`
- `modes.currentModeId`: `default`
- `modes.availableModes`: `default`, `autoEdit`, `yolo`, `plan`
- `models.currentModelId`: `auto-gemini-3`
- `models.availableModels`: includes `auto-gemini-3`, `auto-gemini-2.5`, `gemini-3.1-pro-preview`, `gemini-3-flash-preview`, `gemini-2.5-pro`, `gemini-2.5-flash`, and `gemini-2.5-flash-lite`

Harmless prompt request:

```json
{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"<session-id>","prompt":[{"type":"text","text":"Reply with OK only. Do not use tools."}]}}
```

Observed prompt/update shape:

- First `session/update`: `update.sessionUpdate` is `available_commands_update` with `availableCommands`.
- Stream chunk: `update.sessionUpdate` is `agent_message_chunk`, `content.type` is `text`, and `content.text` was `OK`.
- Prompt response: `result.stopReason` is `end_turn`.
- Prompt response metadata: `result._meta.quota.token_count` and `result._meta.quota.model_usage` were present.

## Implementation Notes

- Treat Gemini authentication as local setup.
- Gemini does not advertise `sessionCapabilities.list`; refresh/import should return an empty discovered-session list with a capability message instead of calling `session/list`.
- The runner should surface a clear degraded provider setup error when `initialize` does not complete, including recent stderr and the sandbox/auth/trust hint.
- Keep Gemini enabled through the shared ACP provider profile, with a longer initialize/session response timeout than faster providers.
