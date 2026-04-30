# Fake Runner Browser Smoke

## Purpose

Prove the development-only fake runner can connect to the control plane, receive an `agent_send_input` command, emit normalized app events, and make those events available to browser WebSocket subscribers.

## Prerequisites

- Rust workspace builds locally.
- No Postgres database is required for this smoke path.
- The runner token is development-only. It defaults to `dev-runner-token` and can be overridden with `AGENTER_DEV_RUNNER_TOKEN`.

## Automated Smoke Test

Run:

```sh
cargo test -p agenter-control-plane http::tests::smoke_routes_runner_events_to_subscribed_browser
```

Expected output shape:

```text
test http::tests::smoke_routes_runner_events_to_subscribed_browser ... ok
```

The test starts the Axum app on an ephemeral local port, connects a runner WebSocket, sends `runner_hello`, receives the control-plane `agent_send_input` command, connects a browser WebSocket, subscribes to the fixed smoke session, emits a runner event, and asserts that the browser receives an `app_event`.

## Manual Smoke

Terminal 1:

```sh
AGENTER_DEV_RUNNER_TOKEN=dev-runner-token cargo run -p agenter-control-plane
```

Expected startup log includes a listener on `127.0.0.1:7777`. Health check:

```sh
curl http://127.0.0.1:7777/healthz
```

Expected:

```text
ok
```

Terminal 2:

```sh
AGENTER_RUNNER_MODE=fake \
AGENTER_CONTROL_PLANE_WS=ws://127.0.0.1:7777/api/runner/ws \
AGENTER_DEV_RUNNER_TOKEN=dev-runner-token \
cargo run -p agenter-runner
```

The control plane sends a deterministic `agent_send_input` command for session:

```text
11111111-1111-1111-1111-111111111111
```

If `websocat` is already installed, a browser subscription can be inspected with:

```sh
printf '%s\n' '{"type":"subscribe_session","request_id":"sub-1","session_id":"11111111-1111-1111-1111-111111111111"}' \
  | websocat ws://127.0.0.1:7777/api/browser/ws
```

Expected messages include an `ack` followed by cached or live `app_event` messages such as `user_message`, `agent_message_delta`, and `agent_message_completed`.

## Cleanup

Stop both `cargo run` processes with `Ctrl-C`.

## Troubleshooting

- If the runner disconnects immediately, confirm `AGENTER_DEV_RUNNER_TOKEN` matches on both processes.
- If port `7777` is busy, set `AGENTER_BIND_ADDR=127.0.0.1:7778` for the control plane and update `AGENTER_CONTROL_PLANE_WS` accordingly.
