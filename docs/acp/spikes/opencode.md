# OpenCode ACP Spike

## Local Setup

- Command: `opencode`
- Version: `1.14.32`
- ACP launch: `opencode acp --cwd <workspace>`

## Initialize Probe

Request:

```json
{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":1,"clientInfo":{"name":"agenter-acp-probe","version":"0.1.0"},"clientCapabilities":{"fs":{"readTextFile":false,"writeTextFile":false},"terminal":false}}}
```

Observed response summary:

- `agentInfo.name`: `OpenCode`
- `agentInfo.version`: `1.14.32`
- `agentCapabilities.loadSession`: `true`
- `agentCapabilities.sessionCapabilities.list`: present
- `agentCapabilities.sessionCapabilities.resume`: present
- `agentCapabilities.sessionCapabilities.fork`: present
- `promptCapabilities.image` and `embeddedContext`: true
- `mcpCapabilities.sse` and `http`: true
- `authMethods`: `opencode-login`

## Implementation Notes

- `opencode acp --help` exposes host/port flags, but the initialize probe confirms stdio JSON-RPC works for Agenter's first integration.
- Running inside the sandbox can fail on OpenCode's local state database; live spikes may need outside-sandbox approval.

