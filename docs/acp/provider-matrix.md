# ACP Provider Matrix

| Provider | Command | Local version | Transport | Initialize status | Session support | Auth policy |
| --- | --- | --- | --- | --- | --- | --- |
| Qwen Code | `qwen --acp --approval-mode default` | `0.15.6` | stdio JSON-RPC JSONL | answered in `./` and `./tmp/workspace/` | `loadSession`, `list`, `resume` advertised | local prerequisite |
| Gemini CLI | `gemini --acp` | `0.40.1` | stdio JSON-RPC JSONL | answered outside sandbox in `./` and `./tmp/workspace/`; sandbox hits local auth bind `EPERM` | `loadSession` advertised; `list` and `resume` not advertised | local prerequisite |
| OpenCode | `opencode acp --cwd <workspace>` | `1.14.32` | stdio JSON-RPC JSONL; command also accepts server flags | answered | `loadSession`, `list`, `resume`, `fork` advertised | local prerequisite |

## Universal Protocol Stage 10 Fixture Coverage

The Stage 10 repo-local fixture `crates/agenter-runner/tests/fixtures/acp_stage10_trace.json` uses sanitized golden slices derived from the current ACP reducer vocabulary. It is not a fresh live-provider capture.

| Provider profile | Fixture story | Automated check | Live Stage 10 capture |
| --- | --- | --- | --- |
| Qwen Code | prompt text, plan update, permission request | `cargo test -p agenter-runner acp_stage10_provider_traces_share_prompt_plan_permission_shape` | pending; last live prompt reached ACP framing but failed later with provider/model connectivity |
| Gemini CLI | plan-mode-like plan update, clarifying question/permission shape | `cargo test -p agenter-runner acp_stage10_provider_traces_share_prompt_plan_permission_shape` | pending; live checks may need outside-sandbox auth helper access |
| OpenCode | `todowrite`-like tool update, plan update, bash permission request | `cargo test -p agenter-runner acp_stage10_provider_traces_share_prompt_plan_permission_shape` | pending; live checks may need outside-sandbox local state database access |

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
