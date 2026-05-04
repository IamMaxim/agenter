# Codex App-Server Protocol Spike

Use this runbook to capture the installed `codex app-server` JSON-RPC behavior before implementing the runner adapter. Keep raw provider payloads in this runbook or attached spike logs; promote only observed shapes into typed adapter code.

## Prerequisites

- `codex` is installed and authenticated for the account/model used by the spike.
- The spike workspace is a disposable directory or clean working tree.
- `python3` is available for the temporary JSONL client.
- Network access is available if the chosen model requires it.
- The runner/control-plane are not involved. This talks directly to the provider process over stdio.

## Locate Codex

```sh
command -v codex
codex --version
codex app-server --help
```

Optional schema capture for comparison with the live transcript:

```sh
rm -rf /tmp/agenter-codex-schema
codex app-server generate-json-schema --out /tmp/agenter-codex-schema
find /tmp/agenter-codex-schema -maxdepth 2 -type f | sort
```

## Refresh Protocol Coverage Matrix

The checked-in matrix lives in `crates/agenter-runner/src/agents/codex_protocol_coverage.rs` and is compared against the checked-in method fixture at `crates/agenter-runner/tests/fixtures/codex_app_server_protocol_methods.rs`. Tests must pass in a clean Agenter checkout without `tmp/codex`.

When `tmp/codex` changes:

1. Inspect `tmp/codex/codex-rs/app-server-protocol/src/protocol/common.rs`.
2. Refresh the fixture by copying or extracting the `client_request_definitions!`, `server_request_definitions!`, and `server_notification_definitions!` invocation method lines for the pinned snapshot.
3. Update each changed matrix entry with `direction`, `method`, `support`, `agenter_surface`, and `notes`.
4. Treat new server requests and notifications as protocol drift until classified; do not rely on a fallback handler as the coverage record.
5. Run:

```sh
cargo test -p agenter-runner codex_protocol_coverage
cargo test -p agenter-runner codex_
```

Server request and notification coverage is exhaustive. Full client request coverage is not enforced yet; the Stage 1 guard verifies only that the selected classified client request methods still exist in `client_request_definitions!`.

Interpretation: `supported` means Agenter has a first-class or deliberate current projection, `degraded` means visible but incomplete behavior, `unsupported` means intentionally blocked, `ignored` means deliberately hidden as non-user-facing noise, `not_applicable` means local TUI or runner-host-only behavior, and `deferred` means planned or possible later work without current remote surface.

## Start App-Server

Raw server command:

```sh
cd /path/to/spike-workspace
codex app-server --listen stdio://
```

Rust spike binary command:

```sh
cargo run -p agenter-runner --bin codex_app_server_spike -- /path/to/spike-workspace
```

Payload-logging diagnostic command:

```sh
just codex-spike /path/to/spike-workspace
```

Expected success markers are a JSON-RPC response for `initialize`, a JSON-RPC response or notification containing a Codex thread id for `thread/start`, a sent `turn/start` request, one or more item events such as `item/agentMessage/delta`, then `turn/completed` with a null error. If the spike times out before `turn/start`, inspect the provider stderr lines above the timeout first.

Agenter runner adapter command:

```sh
AGENTER_RUNNER_MODE=codex \
AGENTER_WORKSPACE=/path/to/workspace \
AGENTER_CONTROL_PLANE_WS=ws://127.0.0.1:7777/api/runner/ws \
AGENTER_DEV_RUNNER_TOKEN=dev-runner-token \
  cargo run -p agenter-runner
```

The adapter advertises a single configured workspace and the `codex` provider, lazily starts one persistent `codex app-server --listen stdio://` process for that runner workspace, starts a native thread when the browser creates an Agenter session, starts turns with read-only sandbox policy, normalizes known message, command, file, tool, plan, compaction, status, error, question, and approval request events, and routes approval/question answers back to the JSON-RPC server request id. Live Codex 0.125 agent text currently arrives as `item/agentMessage/delta` and `item/completed` with `params.item.type == "agentMessage"`; echoed `userMessage` and `reasoning` item lifecycle events are intentionally ignored by the adapter, while reasoning deltas and other lower-priority provider notifications are surfaced as generic provider events instead of being silently dropped. The control plane stores the native Codex thread id as `external_session_id` in its session registry and sends that id on later browser messages. For a thread created or resumed by the current runner process, the runner may use the live thread id directly for `turn/start`; for persisted or provider-refreshed sessions from an older process, the runner must call `thread/resume` before `turn/start` so a fresh app-server process loads the persisted Codex thread. If Codex rejects `thread/resume` with `no rollout found for thread id ...`, treat it as a pre-first-turn thread and send `turn/start` directly; do not apply that fallback to real `thread not found` errors.

Runner WebSocket transport must keep individual frames smaller than the control plane's frame limit while preserving full payload data. Large runner messages are split into chunk frames and reassembled before the normal runner JSON protocol is decoded, so raw Codex provider payloads and `thread/read` history should remain complete unless the configurable reassembled-message limit is exceeded.

Codex 0.125 validates `thread/start.params.sessionStartSource` as a provider-owned enum. Use `"startup"` for normal Agenter-created sessions. Do not send arbitrary client labels such as `"agenter"`; live Codex rejects those with `unknown variant ..., expected startup or clear`.

Use either an extra CLI argument or `AGENTER_SPIKE_PROMPT` to override the default approval-probing prompt:

```sh
AGENTER_SPIKE_PROMPT='Reply briefly and request approval for one harmless command.' \
  cargo run -p agenter-runner --bin codex_app_server_spike -- /path/to/spike-workspace
```

The Rust spike starts `codex app-server --listen stdio://` in the supplied workspace, sends JSON-RPC requests over stdin, reads JSONL from stdout, logs request/notification method names, declines the first observed approval request, and exits successfully when the active turn emits `turn/completed` with a null error. If `codex` is missing or the account is not authenticated, the binary should fail with a local setup error without affecting compilation.

Observed local failure on 2026-04-30: under the Codex-controlled sandbox, `codex app-server` reached `thread/start` but returned a JSON-RPC error saying it could not access `~/.codex/sessions`; related stderr may also mention `~/.codex/shell_snapshots/...` and `Operation not permitted`. When this appears, rerun `just codex-spike /path/to/workspace` from a normal terminal to distinguish an Agenter adapter issue from a Codex runtime permission issue.

For an executable spike, use the JSONL client below. It starts the server, sends initialize, creates a thread, sends one turn, logs all responses/notifications/requests, and auto-denies the first approval request so the turn can continue.

```sh
WORKSPACE=/path/to/spike-workspace python3 -u - <<'PY'
import json
import os
import subprocess
import sys
import threading
import time

workspace = os.environ["WORKSPACE"]
proc = subprocess.Popen(
    ["codex", "app-server", "--listen", "stdio://"],
    cwd=workspace,
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    bufsize=1,
)

next_id = 1
responses = {}
thread_id = None
first_approval_seen = threading.Event()

def send(message):
    proc.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
    proc.stdin.flush()
    print(">>>", json.dumps(message), file=sys.stderr)

def request(method, params):
    global next_id
    msg_id = next_id
    next_id += 1
    send({"id": msg_id, "method": method, "params": params})
    return msg_id

def respond(msg_id, result):
    send({"id": msg_id, "result": result})

def reader():
    global thread_id
    for line in proc.stdout:
        line = line.strip()
        if not line:
            continue
        print("<<<", line, file=sys.stderr)
        msg = json.loads(line)
        if "id" in msg and ("result" in msg or "error" in msg):
            responses[msg["id"]] = msg
            result = msg.get("result") or {}
            thread = result.get("thread") or {}
            if thread.get("id"):
                thread_id = thread["id"]
            elif result.get("threadId"):
                thread_id = result["threadId"]
        method = msg.get("method")
        if method in {
            "item/commandExecution/requestApproval",
            "item/fileChange/requestApproval",
        }:
            first_approval_seen.set()
            respond(msg["id"], {"decision": "decline"})
        elif method == "item/permissions/requestApproval":
            first_approval_seen.set()
            respond(msg["id"], {"permissions": {"fileSystem": None, "network": None}, "scope": "turn"})

threading.Thread(target=reader, daemon=True).start()
threading.Thread(
    target=lambda: [print("ERR", l.rstrip(), file=sys.stderr) for l in proc.stderr],
    daemon=True,
).start()

init_id = request("initialize", {
    "clientInfo": {
        "name": "agenter-codex-spike",
        "title": "Agenter Codex Spike",
        "version": "0.1.0",
    },
    "capabilities": {"experimentalApi": True},
})
time.sleep(2)

start_id = request("thread/start", {
    "cwd": workspace,
    "approvalPolicy": "on-request",
    "approvalsReviewer": "user",
    "sandbox": "read-only",
    "sessionStartSource": "startup",
})
time.sleep(3)

if not thread_id:
    result = responses.get(start_id, {}).get("result", {})
    thread = result.get("thread") or {}
    thread_id = thread.get("id") or result.get("threadId")
if not thread_id:
    raise SystemExit("No thread id observed; inspect transcript above.")

request("turn/start", {
    "threadId": thread_id,
    "cwd": workspace,
    "approvalPolicy": "on-request",
    "approvalsReviewer": "user",
    "sandboxPolicy": {"type": "readOnly", "networkAccess": False},
    "input": [{
        "type": "text",
        "text": "Protocol spike: reply briefly, then try to run `pwd` and try to create `agenter-codex-approval-probe.txt`. Ask for approval when required.",
    }],
})

deadline = time.time() + 180
while time.time() < deadline and not first_approval_seen.is_set():
    time.sleep(0.5)

print("THREAD_ID", thread_id, file=sys.stderr)
print("APPROVAL_SEEN", first_approval_seen.is_set(), file=sys.stderr)
proc.terminate()
try:
    proc.wait(timeout=5)
except subprocess.TimeoutExpired:
    proc.kill()
PY
```

## Resume a Thread

Use the `THREAD_ID` printed by the first run:

```sh
WORKSPACE=/path/to/spike-workspace THREAD_ID=codex-thread-id python3 -u - <<'PY'
import json
import os
import subprocess
import sys
import threading
import time

workspace = os.environ["WORKSPACE"]
thread_id = os.environ["THREAD_ID"]
proc = subprocess.Popen(
    ["codex", "app-server", "--listen", "stdio://"],
    cwd=workspace,
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    bufsize=1,
)

def send(message):
    proc.stdin.write(json.dumps(message, separators=(",", ":")) + "\n")
    proc.stdin.flush()
    print(">>>", json.dumps(message), file=sys.stderr)

threading.Thread(target=lambda: [print("<<<", l.rstrip(), file=sys.stderr) for l in proc.stdout], daemon=True).start()
threading.Thread(target=lambda: [print("ERR", l.rstrip(), file=sys.stderr) for l in proc.stderr], daemon=True).start()

send({"id": 1, "method": "initialize", "params": {
    "clientInfo": {"name": "agenter-codex-spike", "version": "0.1.0"},
    "capabilities": {"experimentalApi": True},
}})
time.sleep(2)
send({"id": 2, "method": "thread/resume", "params": {
    "threadId": thread_id,
    "cwd": workspace,
    "approvalPolicy": "on-request",
    "approvalsReviewer": "user",
    "excludeTurns": False,
}})
time.sleep(8)
proc.terminate()
PY
```

## Messages to Capture

Capture complete JSON lines for these methods and response IDs:

- Client request: `initialize`
- Client request: `thread/start`
- Client request: `thread/resume`
- Client request: `thread/read` if used to inspect history
- Client request: `turn/start`
- Server notifications: `thread/started`, `thread/status/changed`, `thread/compacted`, `turn/started`, `turn/completed`, `turn/plan/updated`, `item/started`, `item/completed`, `item/agentMessage/delta`, `item/plan/delta`, `item/commandExecution/outputDelta`, `item/fileChange/outputDelta`, `item/fileChange/patchUpdated`, warning/model/reasoning/MCP notifications, and any previously unknown method observed during a turn
- Server requests: `item/commandExecution/requestApproval`, `item/fileChange/requestApproval`, `item/permissions/requestApproval`, `item/tool/requestUserInput`, `mcpServer/elicitation/request`, and any unsupported request that carries an `id`
- Approval responses sent by the client for every server request ID

## Schema-Derived Shape Notes

These names were seen from the locally generated app-server schema on 2026-04-30 and refreshed on 2026-05-01 with `codex app-server generate-ts --experimental` and `codex app-server generate-json-schema --experimental`. Treat live transcripts as the final authority for adapter work.

```json
{"id":1,"method":"initialize","params":{"clientInfo":{"name":"agenter-codex-spike","version":"0.1.0"},"capabilities":{"experimentalApi":true}}}
{"id":2,"method":"thread/start","params":{"cwd":"/path/to/workspace","approvalPolicy":"on-request","approvalsReviewer":"user","sandbox":"read-only"}}
{"id":3,"method":"thread/resume","params":{"threadId":"codex-thread-id","cwd":"/path/to/workspace","approvalPolicy":"on-request","approvalsReviewer":"user","excludeTurns":false}}
{"id":4,"method":"turn/start","params":{"threadId":"codex-thread-id","cwd":"/path/to/workspace","input":[{"type":"text","text":"Hello"}]}}
{"id":"server-request-id","method":"item/commandExecution/requestApproval","params":{"threadId":"...","turnId":"..."}}
{"id":"server-request-id","result":{"decision":"decline"}}
{"id":"server-request-id","method":"item/fileChange/requestApproval","params":{"threadId":"...","turnId":"..."}}
{"id":"server-request-id","result":{"decision":"decline"}}
```

Approval decisions observed in the generated schema include `accept`, `acceptForSession`, `decline`, and `cancel` for command and file-change approval requests. Command approvals also include provider-specific policy amendment decisions.

Protocol coverage policy:

- First-class normalized events: agent message deltas/completion, turn completion, implementation plan updates, command start/output/completion, file-change items, approval requests, user-input questions, and MCP elicitation questions.
- Visible provider fallback events: compaction (`thread/compacted` and `contextCompaction` items), thread/turn status, warnings, model reroutes/verifications, reasoning deltas, MCP progress, hooks, auto-approval reviews, token usage, fuzzy search, realtime notifications, and other supported-but-not-yet-promoted server notifications.
- Unsupported server requests with JSON-RPC `id` are surfaced as provider events and answered with JSON-RPC method-not-found errors so a turn does not hang invisibly.

Browser slash command mapping:

- `/compact` sends `thread/compact/start` with the active `threadId`.
- `/review` sends `review/start`; default target is uncommitted changes, with base branch, commit, custom, and detached variants represented in Agenter slash-command arguments.
- `/steer` sends `turn/steer` and requires an active `turnId`.
- `/fork` sends `thread/fork`; the control plane registers the returned native thread id as a new Agenter session when present.
- `/archive` and `/unarchive` send the matching thread archive requests.
- `/rollback` sends `thread/rollback` and requires explicit browser confirmation because it rewrites provider history.
- `/shell` sends `thread/shellCommand` and requires explicit browser confirmation because Codex describes this method as unsandboxed full-access shell execution.

## Observed Rust Spike Output

Manual provider run status:

- Command: `cargo run -p agenter-runner --bin codex_app_server_spike -- /path/to/spike-workspace`
- Status: live execution requires an installed and authenticated local `codex` CLI. Outside-sandbox smoke on 2026-04-30 succeeded with `just codex-spike /tmp/agenter-codex-debug 'Reply with OK only. Do not use tools.'`.
- Expected log shape:

```text
starting codex app-server spike
json-rpc request direction="send" method="initialize" id=1
json-rpc request direction="send" method="thread/start" id=2
json-rpc method direction="recv" provider="codex" method="..."
json-rpc request direction="send" method="turn/start" id=3
json-rpc method direction="recv" provider="codex" method="item/agentMessage/delta"
json-rpc method direction="recv" provider="codex" method="item/completed"
json-rpc method direction="recv" provider="codex" method="turn/completed"
codex app-server spike finished approval_seen=false thread_id=...
```

## Cleanup

```sh
rm -f /path/to/spike-workspace/agenter-codex-approval-probe.txt
rm -rf /tmp/agenter-codex-schema
```

Kill any remaining spike server if the client exits early:

```sh
pkill -f "codex app-server --listen stdio://"
```
