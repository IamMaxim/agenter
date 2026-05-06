# Codex App-Server Live Smoke

Last refreshed: 2026-05-06

## Purpose

This runbook validates the Codex app-server adapter against a live Codex
app-server process, the Agenter runner/control-plane/browser path, and the
`uap/2` raw-payload browser contract.

The current Stage 10 checkpoint is documentation-only. The Codex implementation
in `crates/agenter-runner/src/agents/codex/` is a set of transport, codec,
session, turn, reducer, obligation, provider-command, and reconnect helper
modules. Those modules have focused automated coverage, but the live Agenter
runner mode that uses them is not wired into `crates/agenter-runner/src/main.rs`
yet. Until that integration exists, the live browser smoke below is a repeatable
future procedure, not a completed validation.

## Prerequisites

- Rust workspace builds locally.
- Node dependencies are installed under `web/`.
- Docker Compose and `just` are available for the DB-backed local stack.
- The local `codex` CLI is installed and authenticated.
- If `codex` is installed through a shell-only manager such as nvm and the
  runner cannot resolve it, set `AGENTER_CODEX_BIN` to the absolute binary path
  from `which codex`.
- `tmp/codex` is the Codex checkout used for protocol drift checks.
- Optional raw WebSocket inspection uses `websocat`.

## Record Native Codex Version

Run these commands at the start of every smoke and paste the output into the
plan or smoke notes:

```sh
git status --short
git -C tmp/codex rev-parse HEAD
git -C tmp/codex status --short
command -v codex
codex --version
codex app-server --help
```

Current Stage 10 checkpoint output:

```text
git -C tmp/codex rev-parse HEAD
e4310be51f617f5e60382038fa9cbf53a2429ca4

command -v codex
/Users/maxim/.nvm/versions/node/v20.19.2/bin/codex

codex --version
WARNING: proceeding, even though we could not update PATH: Operation not permitted (os error 1)
codex-cli 0.128.0

codex app-server --help
WARNING: proceeding, even though we could not update PATH: Operation not permitted (os error 1)
[experimental] Run the app server or related tooling
```

The PATH warning above is an environment warning from the local CLI startup; it
does not by itself prove app-server failure.

To regenerate protocol inventory artifacts when the Codex checkout changes:

```sh
mkdir -p /private/tmp/agenter-codex-schema /private/tmp/agenter-codex-ts
codex app-server generate-json-schema --experimental --out /private/tmp/agenter-codex-schema
codex app-server generate-ts --experimental --out /private/tmp/agenter-codex-ts
find /private/tmp/agenter-codex-schema /private/tmp/agenter-codex-ts -maxdepth 2 -type f | sort
```

Then run the drift guard:

```sh
rg -n "client_request_definitions|server_request_definitions|server_notification_definitions|pub enum ThreadItem" tmp/codex/codex-rs/app-server-protocol/src/protocol
cargo test -p agenter-runner codex_protocol_coverage
```

## Current Automated Coverage

These checks verify the mapping/helper modules without a live browser Codex
session:

```sh
cargo test -p agenter-runner codex_protocol_coverage
cargo test -p agenter-runner codex_codec
cargo test -p agenter-runner codex_transport
cargo test -p agenter-runner codex_session_lifecycle
cargo test -p agenter-runner codex_history_import
cargo test -p agenter-runner codex_id_map
cargo test -p agenter-runner codex_turn_commands
cargo test -p agenter-runner codex_reducer
cargo test -p agenter-runner codex_approval_requests
cargo test -p agenter-runner codex_question_requests
cargo test -p agenter-runner codex_provider_commands
cargo test -p agenter-runner codex_reconnect
cd web
npm run test -- sessionSnapshot universalEvents normalizers approvals questions
```

Expected output shape is exit status `0` with matching `... ok` lines. These
tests currently prove:

- protocol drift is detected against `tmp/codex`;
- decoded, undecoded, and malformed app-server frames preserve raw JSON;
- session lifecycle and history import helpers preserve native thread, turn,
  item, metadata, and raw payloads;
- turn start, steer, and interrupt helpers preserve request/response raw
  payloads;
- reducer helpers map known notifications/items and expose unknown native
  notifications as visible native rows;
- approval and question helpers preserve native request IDs and raw payloads;
- provider command metadata exists, but command execution is not wired for
  Codex yet;
- browser materialization can carry raw payloads into generic rows.

They do not prove live Codex app-server interaction through Agenter. Confirm the
missing live wiring with:

```sh
rg -n "AGENTER_RUNNER_MODE.*codex|starting .*codex|CodexTransport|CodexSessionClient|CodexTurnClient" crates/agenter-runner/src/main.rs crates/agenter-runner/src/agents
```

At this checkpoint, `CodexTransport`, `CodexSessionClient`, and
`CodexTurnClient` appear in helper modules, while `main.rs` has no Codex runner
mode.

## Prepare Disposable Workspace

Use a disposable repo so approval, command, and file-change checks are
repeatable:

```sh
rm -rf /private/tmp/agenter-codex-smoke
mkdir -p /private/tmp/agenter-codex-smoke
cd /private/tmp/agenter-codex-smoke
git init
printf 'Codex app-server smoke workspace.\n' > README.md
git add README.md
git commit -m 'seed codex smoke workspace'
```

If Git identity is not configured, the commit may fail. That is an environment
setup issue; either configure local Git identity or continue with an uncommitted
workspace and record that history-import checks may have less metadata.

Do not start `just codex-runner /private/tmp/agenter-codex-smoke` until this
directory exists. The runner now reports a dedicated missing-workspace error
before attempting to spawn Codex.

## Start Agenter

Start the standard local stack:

```sh
just db-up
just control-plane
just web
```

Start the Codex runner in a separate terminal:

```sh
AGENTER_LOG_PAYLOADS=1 just codex-runner /private/tmp/agenter-codex-smoke
```

If the runner logs `failed to start Codex app-server command`, retry with the
absolute Codex binary path:

```sh
AGENTER_CODEX_BIN=/Users/maxim/.nvm/versions/node/v20.19.2/bin/codex \
AGENTER_LOG_PAYLOADS=1 \
just codex-runner /private/tmp/agenter-codex-smoke
```

Expected control-plane health:

```sh
curl http://127.0.0.1:7777/healthz
```

Expected output:

```text
ok
```

Open the Vite URL printed by `just web`, normally:

```text
http://127.0.0.1:5173/
```

Log in with the local bootstrap credentials from
`docs/runbooks/local-manual-testing.md`, then create a Codex session for
`/private/tmp/agenter-codex-smoke`.

## Minimal Live Prompt

Use this first prompt to exercise plan, question, command, file change, diff,
reasoning, and raw-payload rendering in one small turn:

```text
This is an Agenter Codex app-server smoke test in a disposable repository.

Please do all of the following:
1. Make a short visible plan before taking action.
2. Ask me one concise question before editing files.
3. Run the harmless command `printf 'codex-smoke-command\n'`.
4. Create or update `codex-smoke-output.txt` with one line: `codex smoke file change`.
5. Show the resulting diff.
6. Briefly explain what you did.

Do not touch files outside this repository.
```

Expected browser evidence:

- a user-message row appears without waiting for provider echo;
- plan state renders as a plan row, not provider-noise text;
- a question card appears and remains actionable after browser reload;
- a command/tool row shows `printf 'codex-smoke-command\n'` and output;
- a file-change or diff row references `codex-smoke-output.txt`;
- an assistant response includes reasoning summary or visible reasoning row if
  the current Codex settings emit one;
- every decoded Codex-derived row that has a native payload exposes a raw JSON
  dropdown.

If Codex does not ask the requested question, record the exact prompt and
observed event order. Do not mark question handling live-smoked from that turn.

## Approval Checks

Set the session or turn to a policy that asks before command/file changes. The
exact UI may change, so record the active mode, approval policy, sandbox policy,
and model before sending the prompt.

Use this prompt if the minimal smoke did not trigger an approval:

```text
Please run `printf 'codex-approval-smoke\n'` and write `approval smoke\n` to
`approval-smoke.txt`. Ask for approval before running the command or editing the
file if approval is required by the current policy.
```

Expected browser evidence:

- command approval maps to `approval.requested` kind `Command`;
- file-change approval maps to `approval.requested` kind `FileChange`;
- permission approval, if Codex emits one, maps to kind `Permission` with native
  profile/scope visible;
- approval card raw payload dropdown contains the original app-server request;
- browser answer transitions through resolving and then resolved, or reports a
  typed visible error;
- duplicate answer with the same idempotency key does not produce a second
  native answer.

## Raw Payload Checks

Decoded payload check:

1. Expand raw JSON on a known row such as plan, command, file change,
   approval, question, usage, or model notification.
2. Confirm the dropdown includes app-server fields such as `method`, `params`,
   `threadId`, `turnId`, `item`, or native request ID.
3. Capture the row label, event type, and one non-secret JSON key path.

Unknown/native payload check:

1. Trigger or inject one unrecognized app-server notification after the live
   runner has a debug injection hook. If no hook exists, this check is
   unavailable for live smoke.
2. Confirm the browser shows a compact native/unknown row rather than silently
   dropping the frame.
3. Expand raw JSON and confirm the full unknown native payload is reachable.

Current automated substitute:

```sh
cargo test -p agenter-runner codex_reducer_undecoded_notifications_remain_visible_with_full_raw_payload
cd web && npm run test -- sessionSnapshot universalEvents normalizers
```

These tests prove the helper and browser materialization paths. They do not
prove that a live unknown Codex frame traversed runner, control plane, and
browser.

## Interrupt Check

Send a long-running prompt:

```text
Run a slow harmless loop that prints five lines with a one second delay between
lines, then summarize the output.
```

While the turn is active, press the browser stop/send control or invoke the
future Codex cancel command. Expected evidence:

- runner sends native `turn/interrupt`;
- browser turn state becomes interrupted, cancelled, failed, or a typed
  unsupported-cancel error;
- no final state claims success if the native turn continued running;
- raw payload is visible for the interrupt request/response or the error row.

## Reload And Browser Reconnect Checks

During a pending approval or question:

1. Reload the browser tab.
2. Reopen the same session.
3. Answer the pending card.

Expected evidence:

- the browser subscribes with `include_snapshot` and an `after_seq`;
- pending approval/question is restored from state;
- answering after reload is accepted once;
- rows at the snapshot/live boundary are not duplicated.

Optional raw WebSocket check:

```sh
printf '%s\n' '{"type":"subscribe_session","request_id":"codex-smoke-sub","session_id":"<session-id>","after_seq":"0","include_snapshot":true}' \
  | websocat ws://127.0.0.1:7777/api/browser/ws
```

Expected message types:

```text
ack
session_snapshot
universal_event
```

The snapshot must use `uap/2` fields: `snapshot_seq`, `replay_from_seq`,
`replay_through_seq`, and `replay_complete`.

## Runner Reconnect Check

During streaming output or a pending approval/question:

1. Stop the Codex runner process without stopping the control plane.
2. Restart the Codex runner with the same `AGENTER_RUNNER_ID`,
   `AGENTER_WORKSPACE`, and WAL path.
3. Reopen the session in the browser.

Expected evidence:

- runner WAL replay is acked once;
- imported Codex history reconciles without duplicate final items;
- pending obligations either remain actionable or are marked detached/orphaned
  with a visible reason;
- the turn ends in a truthful state, not silent success.

## App-Server Crash Check

During an active Codex turn, terminate only the Codex app-server child process
if the runner exposes its PID in logs. If no PID is available, terminate the
Codex runner process and record that this checks runner loss rather than native
app-server child loss.

Expected evidence:

- session emits degraded or stopped state;
- error row includes app-server exit/stderr context when available;
- pending approvals/questions are cleared, failed, detached, or orphaned;
- browser raw payload/debug details are available for the visible error.

## History Import Check

After the file-change turn completes:

1. Refresh/import Codex sessions from the browser or future provider command.
2. Reopen the same native Codex thread.
3. Reload the browser.

Expected evidence:

- Codex native thread ID is stored as external session ID;
- human-readable title or preview is used when Codex provides one;
- imported turn/item IDs are stable across repeated imports;
- plan, command, file-change, diff, question/approval resolution, reasoning,
  and assistant rows do not duplicate;
- raw native payloads remain reachable after snapshot materialization.

Current automated substitute:

```sh
cargo test -p agenter-runner codex_history_import
cargo test -p agenter-runner codex_reconnect
cd web && npm run test -- sessionSnapshot
```

## Completion Record Template

Copy this template into the active plan after each live attempt:

```text
Codex app-server smoke date:
Agenter commit:
Codex checkout revision:
codex --version:
Workspace:
Model/mode/approval/sandbox:
Prompt:
Live runner wiring present: yes/no
Plan observed: yes/no
Question observed: yes/no
Command observed: yes/no
File change/diff observed: yes/no
Reasoning observed: yes/no
Approval observed: command/file/permission/none
Interrupt observed: yes/no
Browser reload restored pending state: yes/no
Runner reconnect result:
App-server crash result:
History import result:
Decoded raw payload visible in browser: yes/no, row:
Unknown/native raw payload visible in browser: yes/no, row:
Automated commands run:
Unavailable checks and why:
Follow-up:
```

## Cleanup

Stop local services:

```sh
just db-down
```

Remove only the disposable smoke workspace when it is no longer needed:

```sh
rm -rf /private/tmp/agenter-codex-smoke
```
