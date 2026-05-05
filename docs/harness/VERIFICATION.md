# Verification Policy

Verification should scale with the current phase. Rust workspace verification is active once the workspace skeleton exists.

## Documentation Phase

Run:

```sh
find . -maxdepth 4 -type f | sort
```

Review:

- no placeholder sections such as `TBD` or unchecked TODOs;
- docs agree on architecture boundaries;
- active plans mention verification and exit criteria;
- any unresolved ambiguity is listed as an open question.

## Rust Phase

Run the Rust baseline verification after any Rust workspace, crate, or source change:

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
```

If SQLx offline checking is used, include the repository's SQLx prepare/check command in this file.

## Frontend Phase

The Svelte frontend uses npm and lives in `web/`. Run frontend verification from that directory after any frontend source, package, or build configuration change:

```sh
cd web
npm run check
npm run lint
npm run test
npm run build
```

`npm run check` runs `svelte-check`, `npm run lint` runs ESLint, `npm run test` runs Vitest, and `npm run build` runs the Vite production build.

Composer usage bar manual checklist:

- Open a session with `token_usage` and `rate_limits` provider events.
- Confirm the composer bottom bar order is mode, dot, model, thinking level, dot, context, spacer, 5h, dot, week.
- Hover context usage and confirm the token count title appears when token totals are known.
- Hover 5h and weekly metrics and confirm reset countdown plus local reset datetime appear.
- Simulate missing or partial usage and confirm unknown metrics render as `--` without implying `0%`.
- Change mode, model, and reasoning from the composer bar and confirm the saved values remain reflected after reload.

## Integration Phase

Protocol and connector work should include focused integration checks:

- ACP smoke can initialize, create or resume a session when supported, send a prompt, receive session updates, and route one permission request.
- Runner reconnect test proves pending session state is recovered or marked degraded.
- Browser WebSocket test proves session subscription and event delivery.
- Telegram test proves login linking, session selection, message routing, and approval decision.
- Mattermost test proves login linking, thread binding, message routing, and approval decision.

## Universal Protocol Smoke Phase

Run `docs/runbooks/universal-protocol-smoke.md` after changes to universal events, snapshots, replay, runner WAL/ack behavior, provider reducers, approval/question/cancel state, or frontend snapshot consumption.

Environment prerequisites:

- Rust toolchain and Node dependencies are installed.
- Docker Compose is available for the DB-backed path.
- `DATABASE_URL` points at the local Postgres database for DB spot checks.
- `websocat` is optional but useful for raw browser WebSocket inspection.
- Live provider checks require locally installed and authenticated `qwen`, `gemini`, and/or `opencode` CLIs. Provider authentication remains a local prerequisite; do not treat auth/setup failure as universal protocol failure without a direct provider spike.

Focused automated smoke:

```sh
cargo test -p agenter-protocol --test browser_json_frame_conformance
cargo test -p agenter-protocol browser
cargo test -p agenter-protocol runner
cargo test -p agenter-runner acp
cargo test -p agenter-runner fake
cargo test -p agenter-control-plane universal
cargo test -p agenter-control-plane approval
cargo test -p agenter-control-plane subscribe_snapshot
cargo test -p agenter-control-plane runner_event
cd web
npm run test -- normalizers sessionSnapshot universalEvents events sessions
```

Full universal protocol gate, when feasible:

```sh
cargo fmt --all -- --check
cargo check --workspace
cargo clippy --workspace -- -D warnings
cargo test --workspace
cd web
npm run check
npm run lint
npm run test
npm run build
git diff --check
```

Manual provider smoke should record:

- provider command and version;
- workspace path;
- prompt used;
- whether plan, approval/question, command/tool, diff/artifact, browser reconnect, runner reconnect, interrupt, harness death, and terminal state were observed;
- expected final state: replayed, resolving, detached, cancelled, failed, or orphaned;
- exact setup limitation if the provider could not run.


## Completion Rule

When verification cannot run, record:

- command attempted;
- failure output summary;
- whether it is a product failure, missing dependency, or environment limitation;
- next step to make the verification runnable.
