# Local Manual Testing

## Purpose

Run Agenter locally with a Postgres database, the Rust control plane, a runner, and the Svelte browser UI.

## Prerequisites

- Docker with Compose support.
- Rust toolchain compatible with the workspace.
- Node dependencies installed under `web/`.
- `just` installed.

The default development credentials created by the runbook are:

```text
email: admin@example.com
password: agenter-dev-password
```

Override them with `AGENTER_BOOTSTRAP_ADMIN_EMAIL` and `AGENTER_BOOTSTRAP_ADMIN_PASSWORD`.

## Database

Start Postgres:

```sh
just db-up
```

The default database URL is:

```text
postgres://agenter:agenter@127.0.0.1:5432/agenter
```

The control plane runs SQLx migrations automatically on startup when `DATABASE_URL` is set.

To run the ignored SQLx integration tests against the Compose database:

```sh
just db-test
```

To remove the local database volume and start fresh:

```sh
just db-reset
```

## Control Plane

Start the control plane:

```sh
just control-plane
```

Expected output includes a listener on `127.0.0.1:7777`. Health check:

```sh
curl http://127.0.0.1:7777/healthz
```

Expected:

```text
ok
```

For local HTTP testing the just recipe sets `AGENTER_COOKIE_SECURE=0`, because secure cookies are not sent over plain `http://127.0.0.1`.

By default `just control-plane` writes terminal logs and also appends structured logs to:

```text
tmp/agenter-logs/agenter-control-plane.log
```

Useful logging environment variables:

```text
RUST_LOG=agenter=debug,tower_http=debug,sqlx=warn
AGENTER_LOG_FORMAT=pretty
AGENTER_LOG_DIR=tmp/agenter-logs
AGENTER_LOG_PAYLOADS=0
```

Set `AGENTER_LOG_FORMAT=json` when logs will be consumed by Promtail or another log shipper. Keep `AGENTER_LOG_PAYLOADS=0` unless you are doing local-only protocol debugging; when enabled, provider payload previews can include prompt text.

## Runner

Start the deterministic fake runner:

```sh
just fake-runner
```

Start a real provider runner for a workspace:

```sh
just codex-runner /path/to/workspace
just qwen-runner /path/to/workspace
```

The real provider runners require authenticated local `codex` or `qwen` CLIs. Codex runner mode keeps one persistent app-server process per runner workspace and stores the native thread id on Agenter session creation; Qwen resume behavior is still provider-spike dependent.

Codex runner mode now creates the native Codex thread when the browser creates an Agenter session. A successful create-session flow should log `CreateSession`, `thread/start`, and a `SessionCreated` runner response with the Codex thread id stored as `external_session_id`; browser messages should then include that stored external id.

If Codex sessions accept messages but never stream a response, run the direct provider diagnostic:

```sh
just codex-spike /path/to/workspace
```

The spike logs raw JSON-RPC previews with `AGENTER_LOG_PAYLOADS=1`. A failure mentioning `~/.codex/sessions`, `~/.codex/shell_snapshots`, or `Operation not permitted` points at local Codex runtime permissions rather than the browser or control-plane pipeline.
For live Codex 0.125, a successful no-tool diagnostic should show `item/agentMessage/delta`, `item/completed`, and `turn/completed` with a null error for the active thread. When this tool sandbox blocks access to `~/.codex`, rerun the same `just codex-spike` command from a normal terminal or approve the outside-sandbox command.

Runner logs are written to:

```text
tmp/agenter-logs/agenter-runner.log
```

If the control plane logs a runner WebSocket receive error like `Space limit exceeded: Message too long`, confirm both services are using the current runner WebSocket transport. Large runner messages are split into chunk frames and reassembled before normal JSON decoding, preserving full provider payloads and discovered-history data. `AGENTER_RUNNER_WS_CHUNK_BYTES` controls the raw chunk target, and `AGENTER_RUNNER_WS_MAX_MESSAGE_BYTES` controls the maximum reassembled message size.

Tail both Rust service logs:

```sh
just logs-tail
```

## Browser UI

Start Vite:

```sh
just web
```

Open the printed Vite URL, normally:

```text
http://127.0.0.1:5173/
```

Log in with the bootstrap credentials, create a session for an advertised runner workspace, and send a prompt.

The Vite dev server proxies `/api` and `/healthz` to the Rust control plane on `127.0.0.1:7777`, including the browser WebSocket.

To add browser console diagnostics for API and WebSocket lifecycle:

```sh
just web-debug
```

This sets `VITE_AGENTER_DEBUG=1`. The default `just web` stays quiet.

## Optional Loki and Grafana

Start the optional local logging stack:

```sh
just logs-up
```

Use JSON-formatted Rust logs while Promtail is running:

```sh
just control-plane-json
just runner-json fake .
```

Grafana is available at:

```text
http://127.0.0.1:3000/
```

Default local credentials:

```text
user: admin
password: agenter
```

Add a Loki data source pointed at:

```text
http://loki:3100
```

Query examples:

```logql
{job="agenter"}
{job="agenter", level="INFO"}
{job="agenter", target=~".*runner.*"}
```

Stop the logging stack:

```sh
just logs-down
```

## Useful Commands

List all recipes:

```sh
just
```

Run backend and frontend verification:

```sh
just verify
```

Stop Postgres:

```sh
just db-down
```

## Troubleshooting

- If login succeeds but the browser still appears unauthenticated, confirm the control plane was started through `just control-plane` or set `AGENTER_COOKIE_SECURE=0` manually for local HTTP.
- If no workspaces appear, confirm a runner is connected and its token matches `AGENTER_DEV_RUNNER_TOKEN`.
- If port `7777` is busy, set `AGENTER_BIND_ADDR` for the control plane and set `AGENTER_CONTROL_PLANE_WS` to the matching WebSocket URL for the runner.
- If the web UI cannot reach the backend, use the Vite dev server URL and keep the control plane on `127.0.0.1:7777`, or update `web/vite.config.ts` to match your custom `AGENTER_BIND_ADDR`.
- If logs are missing from Grafana, confirm the Rust service was started with `AGENTER_LOG_FORMAT=json` and `AGENTER_LOG_DIR=tmp/agenter-logs`, then check `just logs-tail`.
