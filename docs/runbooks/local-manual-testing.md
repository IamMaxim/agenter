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

The real provider runners require authenticated local `codex` or `qwen` CLIs and currently start fresh native provider sessions for each browser prompt until native session ID persistence is implemented.

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
