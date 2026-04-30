# Qwen ACP Protocol Spike

Use this runbook to capture the installed `qwen --acp` behavior before implementing the runner adapter. ACP capabilities vary by agent/version, so history and resume support must be detected from `initialize` and confirmed by live calls.

## Prerequisites

- `qwen` is installed and authenticated for the model/provider used by the spike.
- The spike workspace is a disposable directory or clean working tree.
- `python3` is available for the temporary JSONL client.
- Network access is available if the chosen model requires it.
- The runner/control-plane are not involved. This talks directly to Qwen ACP over stdio JSONL.

## Locate Qwen

```sh
command -v qwen
qwen --version
qwen --help
qwen --acp --help
```

Optional local SDK shape check when Qwen was installed through npm:

```sh
npm root -g
rg -n '"session/(new|prompt|load|resume|list|request_permission|update)"|AGENT_METHODS|CLIENT_METHODS' "$(npm root -g)/@qwen-code/qwen-code"
```

## Start ACP

Raw agent command:

```sh
cd /path/to/spike-workspace
qwen --acp --approval-mode default
```

Rust spike binary command:

```sh
cargo run -p agenter-runner --bin qwen_acp_spike -- /path/to/spike-workspace
```

Agenter runner adapter command:

```sh
AGENTER_RUNNER_MODE=qwen \
AGENTER_WORKSPACE=/path/to/workspace \
AGENTER_CONTROL_PLANE_WS=ws://127.0.0.1:7777/api/runner/ws \
AGENTER_DEV_RUNNER_TOKEN=dev-runner-token \
  cargo run -p agenter-runner
```

The adapter advertises a single configured workspace and the `qwen` provider, starts `qwen --acp --approval-mode default` per browser turn, initializes ACP, creates a session, sends one prompt, normalizes known `session/update` message/error notifications and `session/request_permission` requests, routes approval answers to ACP option selections, and answers basic ACP fs/terminal client requests with inert responses. The adapter can use an external session id if the runner command supplies one, but the current control plane does not yet persist native Qwen session ids between browser prompts.

Use either an extra CLI argument or `AGENTER_SPIKE_PROMPT` to override the default permission-probing prompt:

```sh
AGENTER_SPIKE_PROMPT='Reply briefly and request permission for one harmless command.' \
  cargo run -p agenter-runner --bin qwen_acp_spike -- /path/to/spike-workspace
```

The Rust spike starts `qwen --acp --approval-mode default` in the supplied workspace, sends ACP JSON-RPC requests over stdin, reads JSONL from stdout, logs response/notification/request method names, answers the first permission request with a reject option when available, handles basic ACP file-system and terminal client requests with inert responses, then closes stdin and kills the child if it does not exit promptly. If `qwen` is missing or the account is not authenticated, the binary should fail with a local setup error without affecting compilation.

For an executable spike, use the JSONL client below. It starts Qwen in ACP mode, sends `initialize`, creates a session, sends one prompt, logs all responses/notifications/requests, and answers the first permission request with a reject option when the agent offers one.

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
    ["qwen", "--acp", "--approval-mode", "default"],
    cwd=workspace,
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=subprocess.PIPE,
    text=True,
    bufsize=1,
)

next_id = 1
responses = {}
session_id = None
first_permission_seen = threading.Event()

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

def answer_permission(msg):
    params = msg.get("params") or {}
    options = params.get("options") or []
    selected = None
    for option in options:
        if option.get("kind") in {"reject_once", "reject_always"}:
            selected = option.get("optionId")
            break
    if selected is None and options:
        selected = options[-1].get("optionId")
    if selected:
        respond(msg["id"], {"outcome": {"outcome": "selected", "optionId": selected}})
    else:
        respond(msg["id"], {"outcome": {"outcome": "cancelled"}})

def reader():
    global session_id
    for line in proc.stdout:
        line = line.strip()
        if not line:
            continue
        print("<<<", line, file=sys.stderr)
        msg = json.loads(line)
        if "id" in msg and ("result" in msg or "error" in msg):
            responses[msg["id"]] = msg
            result = msg.get("result") or {}
            if result.get("sessionId"):
                session_id = result["sessionId"]
        method = msg.get("method")
        if method == "session/request_permission":
            first_permission_seen.set()
            answer_permission(msg)
        elif method == "fs/read_text_file":
            respond(msg["id"], {"content": ""})
        elif method == "fs/write_text_file":
            respond(msg["id"], {})
        elif method == "terminal/create":
            respond(msg["id"], {"terminalId": "agenter-spike-terminal-denied"})
        elif method == "terminal/output":
            respond(msg["id"], {"output": "", "truncated": False, "exitStatus": {"exitCode": 1}})
        elif method == "terminal/wait_for_exit":
            respond(msg["id"], {"exitCode": 1})
        elif method in {"terminal/release", "terminal/kill"}:
            respond(msg["id"], {})

threading.Thread(target=reader, daemon=True).start()
threading.Thread(
    target=lambda: [print("ERR", l.rstrip(), file=sys.stderr) for l in proc.stderr],
    daemon=True,
).start()

init_id = request("initialize", {
    "protocolVersion": 1,
    "clientInfo": {
        "name": "agenter-qwen-acp-spike",
        "title": "Agenter Qwen ACP Spike",
        "version": "0.1.0",
    },
    "clientCapabilities": {
        "fs": {"readTextFile": True, "writeTextFile": True},
        "terminal": True,
    },
})
time.sleep(3)

new_id = request("session/new", {"cwd": workspace, "mcpServers": []})
time.sleep(5)

if not session_id:
    result = responses.get(new_id, {}).get("result", {})
    session_id = result.get("sessionId")
if not session_id:
    raise SystemExit("No session id observed; inspect transcript above.")

request("session/prompt", {
    "sessionId": session_id,
    "prompt": [{
        "type": "text",
        "text": "Protocol spike: reply briefly, then try to run `pwd` and try to create `agenter-qwen-approval-probe.txt`. Ask for permission when required.",
    }],
})

deadline = time.time() + 180
while time.time() < deadline and not first_permission_seen.is_set():
    time.sleep(0.5)

print("SESSION_ID", session_id, file=sys.stderr)
print("PERMISSION_SEEN", first_permission_seen.is_set(), file=sys.stderr)
proc.terminate()
try:
    proc.wait(timeout=5)
except subprocess.TimeoutExpired:
    proc.kill()
PY
```

## Resume or Load a Session

Use the `SESSION_ID` printed by the first run. Check `initialize.result.agentCapabilities` first:

- If `agentCapabilities.sessionCapabilities.resume` is present, probe `session/resume`.
- If `agentCapabilities.loadSession` is true, probe `session/load`.
- If neither is supported, record that Qwen sessions are not resumable in this installed version.

```sh
WORKSPACE=/path/to/spike-workspace SESSION_ID=qwen-session-id python3 -u - <<'PY'
import json
import os
import subprocess
import sys
import threading
import time

workspace = os.environ["WORKSPACE"]
session_id = os.environ["SESSION_ID"]
proc = subprocess.Popen(
    ["qwen", "--acp", "--approval-mode", "default"],
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
    "protocolVersion": 1,
    "clientInfo": {"name": "agenter-qwen-acp-spike", "version": "0.1.0"},
    "clientCapabilities": {"fs": {"readTextFile": True, "writeTextFile": True}, "terminal": True},
}})
time.sleep(3)
send({"id": 2, "method": "session/resume", "params": {
    "sessionId": session_id,
    "cwd": workspace,
    "mcpServers": [],
}})
time.sleep(5)
send({"id": 3, "method": "session/load", "params": {
    "sessionId": session_id,
    "cwd": workspace,
    "mcpServers": [],
}})
time.sleep(8)
proc.terminate()
PY
```

## Messages to Capture

Capture complete JSON lines for these methods and response IDs:

- Client request: `initialize`
- Client request: `session/new`
- Client request: `session/list` when `agentCapabilities.sessionCapabilities.list` is present
- Client request: `session/resume` when `agentCapabilities.sessionCapabilities.resume` is present
- Client request: `session/load` when `agentCapabilities.loadSession` is true
- Client request: `session/prompt`
- Agent notification: `session/update`
- Agent request: `session/request_permission`
- Agent requests for client capabilities: `fs/read_text_file`, `fs/write_text_file`, `terminal/create`, `terminal/output`, `terminal/wait_for_exit`, `terminal/release`, `terminal/kill`
- Permission responses sent by the client for every `session/request_permission` request ID

## SDK-Derived Shape Notes

These names were seen in the locally installed Qwen ACP SDK bundle on 2026-04-30. Treat live transcripts as the final authority for adapter work.

```json
{"id":1,"method":"initialize","params":{"protocolVersion":1,"clientInfo":{"name":"agenter-qwen-acp-spike","version":"0.1.0"},"clientCapabilities":{"fs":{"readTextFile":true,"writeTextFile":true},"terminal":true}}}
{"id":2,"method":"session/new","params":{"cwd":"/path/to/workspace","mcpServers":[]}}
{"id":3,"method":"session/prompt","params":{"sessionId":"qwen-session-id","prompt":[{"type":"text","text":"Hello"}]}}
{"id":4,"method":"session/resume","params":{"sessionId":"qwen-session-id","cwd":"/path/to/workspace","mcpServers":[]}}
{"id":5,"method":"session/load","params":{"sessionId":"qwen-session-id","cwd":"/path/to/workspace","mcpServers":[]}}
{"method":"session/update","params":{"sessionId":"qwen-session-id","update":{"sessionUpdate":"agent_message_chunk","content":{"type":"text","text":"..."}}}}
{"id":"agent-request-id","method":"session/request_permission","params":{"sessionId":"qwen-session-id","toolCall":{"toolCallId":"..."},"options":[{"optionId":"...","kind":"allow_once","name":"Allow once"}]}}
{"id":"agent-request-id","result":{"outcome":{"outcome":"selected","optionId":"..."}}}
```

Permission option kinds in the bundled SDK are `allow_once`, `allow_always`, `reject_once`, and `reject_always`. A cancelled answer uses:

```json
{"id":"agent-request-id","result":{"outcome":{"outcome":"cancelled"}}}
```

## Observed Rust Spike Output

Manual provider run status:

- Command: `cargo run -p agenter-runner --bin qwen_acp_spike -- /path/to/spike-workspace`
- Status: not run during Task 0.3 verification; live execution requires an installed and authenticated local `qwen` CLI.
- Expected log shape:

```text
starting qwen ACP spike
json-rpc request direction="send" method="initialize" id=1
json-rpc request direction="send" method="session/new" id=2
json-rpc method direction="recv" provider="qwen" method="..."
json-rpc request direction="send" method="session/prompt" id=3
json-rpc method direction="recv" provider="qwen" method="session/request_permission"
json-rpc response direction="send" id=...
qwen ACP spike finished permission_seen=true session_id=...
```

## Cleanup

```sh
rm -f /path/to/spike-workspace/agenter-qwen-approval-probe.txt
```

Kill any remaining spike server if the client exits early:

```sh
pkill -f "qwen --acp"
```
