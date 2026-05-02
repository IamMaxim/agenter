# ACP Provider Matrix

| Provider | Command | Local version | Transport | Initialize status | Session support | Auth policy |
| --- | --- | --- | --- | --- | --- | --- |
| Qwen Code | `qwen --acp --approval-mode default` | `0.15.6` | stdio JSON-RPC JSONL | answered in `./` and `./tmp/workspace/` | `loadSession`, `list`, `resume` advertised | local prerequisite |
| Gemini CLI | `gemini --acp` | `0.40.1` | stdio JSON-RPC JSONL | answered outside sandbox in `./` and `./tmp/workspace/`; sandbox hits local auth bind `EPERM` | `loadSession` advertised; `list` and `resume` not advertised | local prerequisite |
| OpenCode | `opencode acp --cwd <workspace>` | `1.14.32` | stdio JSON-RPC JSONL; command also accepts server flags | answered | `loadSession`, `list`, `resume`, `fork` advertised | local prerequisite |

## Shared ACP Expectations

- `initialize` negotiates protocol version and agent capabilities.
- `session/new` creates a conversation for an absolute workspace path.
- `session/load` loads persisted history when `loadSession` is advertised.
- `session/list` must only be called when `sessionCapabilities.list` is advertised.
- `session/resume` should not be used until a provider-specific live spike proves the request and response shape; use `session/load` for persisted sessions when `loadSession` is true.
- `session/prompt` sends user input and completes with a stop reason.
- `session/update` streams message chunks, tool calls, plans, command availability, mode/config changes, and session info.
- `session/request_permission` asks the client to approve provider actions.
- `fs/*` and `terminal/*` methods are client services served by the runner.
