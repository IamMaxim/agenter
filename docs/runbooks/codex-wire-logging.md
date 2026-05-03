# Codex wire logging

Use this runbook when a Codex-backed Agenter session gets stuck, especially
around approvals, questions, unexpected JSON-RPC responses, or logs such as
`ignored codex message outside active turn scope`.

## Enable raw wire logs

Raw Codex wire logs are disabled by default because they may contain prompts,
model output, diffs, file paths, approval bodies, and provider account details.

```sh
AGENTER_CODEX_RAW_LOG=1 \
AGENTER_LOG_FORMAT=json \
AGENTER_RUNNER_MODE=codex \
AGENTER_WORKSPACE=/path/to/workspace \
AGENTER_CONTROL_PLANE_WS=ws://127.0.0.1:7777/api/runner/ws \
AGENTER_DEV_RUNNER_TOKEN=dev-runner-token \
  cargo run -p agenter-runner
```

By default, files are written under:

```text
tmp/agenter-logs/codex-wire/
```

Override the directory when collecting logs outside the repo:

```sh
AGENTER_CODEX_RAW_LOG=1 \
AGENTER_CODEX_RAW_LOG_DIR=/tmp/agenter-codex-wire \
  cargo run -p agenter-runner
```

## Record shape

Each line is one JSON object. Important fields:

- `direction`: `send`, `recv`, `stderr`, or `internal`.
- `classification`: for example `client_request_sent`,
  `client_response_sent`, `server_request_received`,
  `server_response_received`, `server_notification_received`,
  `provider_stderr`, `interleaved_queued`, `interleaved_drained`, or
  `scope_dropped`.
- `session_id`: Agenter session id when the runner knows the active turn.
- `method`: JSON-RPC method when present.
- `jsonrpc_id`: request/response id when present.
- `provider_thread_id` / `provider_turn_id`: ids extracted from the payload.
- `runtime_thread_id` / `runtime_turn_id`: ids currently tracked by the runner
  runtime.
- `payload`: exact JSON-RPC object for send/receive records.
- `stderr`: exact Codex stderr line for provider stderr records.

## Useful filters

Show all approval-related wire traffic:

```sh
jq 'select((.method // "") | test("Approval|requestApproval|permissions"))' \
  tmp/agenter-logs/codex-wire/*.jsonl
```

Show requests Codex sent to Agenter:

```sh
jq 'select(.classification == "server_request_received") | {ts, session_id, method, jsonrpc_id, provider_thread_id, provider_turn_id}' \
  tmp/agenter-logs/codex-wire/*.jsonl
```

Show Agenter responses back to Codex:

```sh
jq 'select(.classification == "client_response_sent") | {ts, session_id, reason, jsonrpc_id, payload}' \
  tmp/agenter-logs/codex-wire/*.jsonl
```

Show scope-dropped frames with expected and actual routing context:

```sh
jq 'select(.classification == "scope_dropped") | {ts, method, reason, expected_thread_id, actual_thread_id, expected_turn_id, actual_turn_id, jsonrpc_id}' \
  tmp/agenter-logs/codex-wire/*.jsonl
```

## Interpreting stuck approval cases

- If a `server_request_received` approval exists but no matching
  `client_response_sent` with the same `jsonrpc_id` appears, inspect the
  control-plane approval state and browser approval request.
- If `client_response_sent` exists but Codex does not continue, inspect provider
  stderr and later `server_response_received` or `server_notification_received`
  records.
- If `scope_dropped` appears for the active session, compare
  `expected_thread_id` and `actual_thread_id`; cross-thread messages are ignored
  by design, while same-thread turn-id differences should not be dropped.
- If `interleaved_queued` appears during startup or request/response waits, the
  runner preserved provider traffic while awaiting a specific JSON-RPC response.

## Cleanup

Raw wire logs are local diagnostics. Remove them after collecting evidence:

```sh
rm -f tmp/agenter-logs/codex-wire/*.jsonl
```
