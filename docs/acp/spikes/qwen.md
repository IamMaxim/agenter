# Qwen ACP Spike

## Local Setup

- Command: `qwen`
- Version: `0.15.6`
- ACP launch: `qwen --acp --approval-mode default`

## Initialize Probe

Status: confirmed in both `/Users/maxim/work/agenter` and `/Users/maxim/work/agenter/tmp/workspace`.

Request:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientInfo":{"name":"agenter-acp-probe","version":"0.1.0"},"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}}}
```

Observed response summary:

- `agentInfo.name`: `qwen-code`
- `agentInfo.version`: `0.15.6`
- `agentCapabilities.loadSession`: `true`
- `agentCapabilities.sessionCapabilities.list`: present
- `agentCapabilities.sessionCapabilities.resume`: present
- `promptCapabilities.image`, `audio`, and `embeddedContext`: true
- `mcpCapabilities.sse` and `http`: true
- `authMethods`: OpenAI API key and Qwen OAuth terminal flows

## Session And Prompt Probe

Workspace: `/Users/maxim/work/agenter/tmp/workspace`

`session/new` response summary:

- `sessionId`: UUID string, for example `0cff1550-08f3-4ecf-80de-4fe8a6a1c955`
- `models.currentModelId`: `qwen/qwen3-coder:free(openai)` in the local probe
- `models.availableModels`: provider-configured OpenRouter/Qwen model list with `_meta.contextLimit`
- `modes.currentModeId`: `default`
- `modes.availableModes`: `plan`, `default`, `auto-edit`, `yolo`
- `configOptions`: includes selectable `mode` and `model`

Harmless prompt request:

```json
{"jsonrpc":"2.0","id":3,"method":"session/prompt","params":{"sessionId":"<session-id>","prompt":[{"type":"text","text":"Reply with OK only. Do not use tools."}]}}
```

Observed prompt/update shape:

- First `session/update`: `update.sessionUpdate` is `available_commands_update` with `availableCommands` and `_meta.availableSkills`.
- Prompt response: JSON-RPC error `code: -32603`, `message: Internal error`, `data.details: Connection error.`
- Provider stderr included `Error handling request` for `session/prompt`; this points to local provider/model connectivity, not an ACP framing failure.

## Implementation Notes

- The previous `crates/agenter-runner/src/agents/qwen_acp.rs` production adapter was removed after its useful JSONL framing and permission mapping were promoted into the shared ACP runtime.
- Avoid restoring its per-turn subprocess lifecycle; Qwen should stay on the shared ACP runtime.
- Replace inert fs/terminal responses with runner-backed services.
- Keep `sessionCapabilities.list` and `sessionCapabilities.resume` distinct from `loadSession`; Qwen advertises all three in initialize.
