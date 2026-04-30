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

Once the Svelte frontend exists, expected baseline verification is:

```sh
npm run check
npm run lint
npm run test
npm run build
```

Adjust commands to the package manager chosen by the first frontend scaffold.

## Integration Phase

Protocol and connector work should include focused integration checks:

- Codex app-server spike can initialize, create or resume a session, send a turn, receive events, and route one approval request.
- Qwen ACP spike can initialize, create or resume a session when supported, send a prompt, receive session updates, and route one permission request.
- Runner reconnect test proves pending session state is recovered or marked degraded.
- Browser WebSocket test proves session subscription and event delivery.
- Telegram test proves login linking, session selection, message routing, and approval decision.
- Mattermost test proves login linking, thread binding, message routing, and approval decision.

## Completion Rule

When verification cannot run, record:

- command attempted;
- failure output summary;
- whether it is a product failure, missing dependency, or environment limitation;
- next step to make the verification runnable.
