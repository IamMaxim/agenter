# Universal Protocol Smoke

## Purpose

Prove that `uap/1` snapshot/replay, provider normalization, approval/question/cancel state, and runner replay behavior work across the fake runner, DB-backed browser path, and locally available native providers.

The repeatable automated checks use repo-local sanitized fixtures. Live provider captures are still required before claiming real Codex/Qwen/Gemini/OpenCode parity for a local machine.

## Prerequisites

- Rust workspace builds locally.
- Node dependencies are installed under `web/` for frontend checks.
- Docker Compose is available for DB-backed smoke.
- Optional live provider paths require authenticated local CLIs:
  - `codex app-server --listen stdio://`
  - `qwen --acp --approval-mode default`
  - `gemini --acp`
  - `opencode acp --cwd <workspace>`
- Optional WebSocket inspection uses `websocat`.

## Setup

Start from a clean enough working tree and note unrelated edits:

```sh
git status --short
```

Run the focused conformance checks first:

```sh
cargo test -p agenter-runner codex_stage10_conformance_trace_preserves_expected_milestones
cargo test -p agenter-runner acp_stage10_provider_traces_share_prompt_plan_permission_shape
cargo test -p agenter-control-plane subscribe_snapshot_replays_after_seq_in_strict_order
cargo test -p agenter-control-plane runner_event_ack_state_dedupes_replayed_sequences
cargo test -p agenter-control-plane seeded_runner_ack_marks_old_replay_as_duplicate
cargo test -p agenter-runner interrupt_cancels_blocked_approval_for_same_session
cargo test -p agenter-runner interrupt_does_not_count_completed_approval_cancel_replay_as_new_cancel
```

Expected output shape is one or more `... ok` lines and exit status `0`.

## Fake Runner And Browser Path

Run the automated fake-runner browser smoke:

```sh
cargo test -p agenter-control-plane http::tests::smoke_routes_runner_events_to_subscribed_browser
```

For manual inspection:

```sh
AGENTER_DEV_RUNNER_TOKEN=dev-runner-token cargo run -p agenter-control-plane
```

In another terminal:

```sh
AGENTER_RUNNER_MODE=fake \
AGENTER_CONTROL_PLANE_WS=ws://127.0.0.1:7777/api/runner/ws \
AGENTER_DEV_RUNNER_TOKEN=dev-runner-token \
cargo run -p agenter-runner
```

Subscribe with snapshot replay:

```sh
printf '%s\n' '{"type":"subscribe_session","request_id":"sub-uap","session_id":"11111111-1111-1111-1111-111111111111","after_seq":"0","include_snapshot":true}' \
  | websocat ws://127.0.0.1:7777/api/browser/ws
```

Expected messages:

```text
ack
session_snapshot
universal_event or app_event for subsequent live events
```

## DB-Backed Path

Start Postgres and services:

```sh
just db-up
just control-plane
just fake-runner
```

Open the browser UI with:

```sh
just web
```

Create or open a fake-runner session, send a prompt, then reload the browser while the session is visible. The WebSocket subscription should request `include_snapshot` and `after_seq`; reconnect must apply the snapshot first and then replay later universal events without duplicating rows.

Database spot checks:

```sh
psql "$DATABASE_URL" -c "select seq, event_type, session_id from agent_events order by seq desc limit 10;"
psql "$DATABASE_URL" -c "select latest_seq, session_id from session_snapshots order by updated_at desc limit 10;"
```

Expected shape:

- `agent_events.seq` increases monotonically.
- `session_snapshots.latest_seq` matches the latest durable event applied to that session.
- Re-subscribing with an older `after_seq` returns events in ascending `seq` order.

## Provider Trace Path

Automated fixture checks:

- Codex fixture: `crates/agenter-runner/tests/fixtures/codex_stage10_trace.json`.
- ACP fixture: `crates/agenter-runner/tests/fixtures/acp_stage10_trace.json`.

These fixture slices are sanitized golden vocabulary derived from the current reducers. They cover:

- Codex turn start, plan update, shell approval, file approval, command output, diff update, and turn completion.
- Qwen-like prompt text, plan update, and permission request.
- Gemini-like plan/question permission shape.
- OpenCode-like `todowrite`/tool, plan update, and permission request.

Live capture is still pending for the exact Stage 10 stories. To capture fresh provider traces, run the provider spike runbooks:

```sh
just codex-spike /tmp/agenter-codex-smoke
cargo run -p agenter-runner --bin qwen_acp_spike -- /tmp/agenter-qwen-smoke
```

For Gemini and OpenCode, use the `docs/acp/spikes/` notes and the corresponding runner modes:

```sh
just gemini-runner /tmp/agenter-gemini-smoke
just opencode-runner /tmp/agenter-opencode-smoke
```

Prompts should follow the same conformance story:

```text
Inspect this disposable repo, make a short plan, ask before edits, create one harmless file, run one harmless command, then report the result.
```

## Expected Event Order

The universal order must be stable for all providers that support the feature:

```text
session.created or session.loaded
session.ready
turn.started
item.created or content.delta for user input echo
plan.updated when planning is visible
approval.requested or user_input.requested before risky tool/file work
approval.updated/resolving when the browser answers
approval.resolved after runner/native acknowledgement
item.created for tool/command/file work
content.delta for streamed assistant or terminal output
content.completed or item.completed
diff.updated when a provider reports a diff
turn.completed, turn.failed, turn.cancelled, turn.interrupted, or turn.detached
```

Unknown native events should become `native.unknown` or visible provider rows with safe `native` references, not disappear silently.

## Snapshot And Replay Checks

Automated checks:

```sh
cargo test -p agenter-control-plane subscribe_snapshot_replays_after_seq_in_strict_order
cargo test -p agenter-control-plane subscribe_snapshot_replays_universal_events_after_legacy_cache_miss
cargo test -p agenter-control-plane subscribe_snapshot_marks_universal_replay_has_more_when_bounded
cd web && npm run test -- sessionSnapshot.test.ts
```

Manual checks:

- Subscribe with `after_seq: "0"` and `include_snapshot: true`; verify the replay starts from the first available event.
- Subscribe with `after_seq` equal to a known event; verify only higher `seq` events are replayed.
- Force a bounded/incomplete replay; verify the server reports `snapshot_replay_incomplete` and closes the universal subscription instead of forwarding later live events.
- Reload the browser; verify timeline rows are not duplicated at the replay/live boundary.

## Approval, Question, And Cancel Checks

Automated checks:

```sh
cargo test -p agenter-control-plane approval
cargo test -p agenter-runner approval
cargo test -p agenter-runner interrupt_cancels_blocked_approval_for_same_session
```

Manual checks:

- Shell/file approval: answer once, then retry the same browser request with the same idempotency key. Expected state: no second native answer; duplicate returns the stored result or in-flight resolving state.
- Conflicting duplicate approval: retry with the same key but a different semantic decision. Expected state: `idempotency_conflict` or `approval_conflicting_decision`.
- Question/user input: answer a question and retry with the same key. Expected state: stored response replay, not a second native response.
- Cancel while approval is pending: expected state is `cancelled` when a blocked native request is answered with cancel; otherwise a typed `provider_cancel_not_supported` error and no false `turn.cancelled`.
- Cancel while a provider turn is running without a blocked native approval: expected state is a typed `provider_cancel_not_supported` error unless that provider explicitly advertises and implements a live interrupt hook. ACP currently must not advertise generic interrupt support.
- Harness death while approval is pending: expected state is `approval.orphaned` only when runner/harness evidence says ownership is lost, not on transient WebSocket disconnect.

## Chaos Checks

Run these against fake runner first, then one live provider when locally available:

- Browser reload during approval: pending/resolving approval must reappear from control-plane state.
- Browser WebSocket disconnect during replay: reconnect with the last known `after_seq`; no duplicate rows.
- Runner WebSocket reconnect after unacked event: runner WAL replay must be acked once and deduped by runner sequence.
- Duplicate runner event replay: no second browser-visible universal event for the same accepted runner sequence.
- Runner reconnect during native permission: pending native waiter should accept the eventual answer exactly once when the runner process and provider runtime survive. This remains a Stage 10 risk until reconnect-stable runtime ownership is covered by an automated chaos test.
- Cancel while updates are still arriving: final state must be one of `turn.cancelled`, `turn.interrupted`, `turn.failed`, or a typed unsupported-cancel error; do not report inert success.
- Harness crash during tool: pending approvals become `orphaned` and the turn becomes `failed` or `detached` based on runner evidence.

## Cleanup

Stop services with `Ctrl-C`, then:

```sh
just db-down
rm -rf /tmp/agenter-codex-smoke /tmp/agenter-qwen-smoke /tmp/agenter-gemini-smoke /tmp/agenter-opencode-smoke
rm -f /tmp/agenter-*-smoke/agenter-*-approval-probe.txt
```

If provider spike processes remain:

```sh
pkill -f "codex app-server --listen stdio://"
pkill -f "qwen --acp"
pkill -f "gemini --acp"
pkill -f "opencode acp"
```

## Troubleshooting

- `listen EPERM: operation not permitted 0.0.0.0` during Gemini auth is a local sandbox limitation; rerun outside the restrictive sandbox.
- Qwen `-32603 Internal error` with `Connection error.` after ACP framing usually points at provider/model connectivity, not Agenter ACP wiring.
- OpenCode may need outside-sandbox access to its local state database.
- Codex errors mentioning `~/.codex/sessions`, `~/.codex/shell_snapshots`, or `Operation not permitted` usually require running the spike from a normal terminal.
- If `snapshot_replay_incomplete` appears, resubscribe from the snapshot cursor named in the error instead of advancing to later live events.
- If Cargo or Vite fail with lockfile/temp-file `EPERM`, rerun in a writable environment before treating the result as a product failure.
