# ACP Progress

## 2026-05-02

Plan approved:

- Spike first before hardening shared runner/protocol code.
- Use `docs/acp/` as the research notebook.
- Implement all three targets in v1: Qwen Code, Gemini CLI, and OpenCode.
- Prefer a shared ACP runtime with provider profiles.
- Use runner-backed ACP file-system and terminal services.
- Treat provider authentication as a local prerequisite.
- Run one multi-provider runner for the configured workspace.

Evidence collected:

- Qwen Code `0.15.6` answered `initialize` over stdio JSON-RPC and advertised `loadSession`, `sessionCapabilities.list`, and `sessionCapabilities.resume`.
- OpenCode `1.14.32` answered `initialize` over stdio JSON-RPC and advertised `loadSession`, `sessionCapabilities.list`, `sessionCapabilities.resume`, and `sessionCapabilities.fork`.
- Qwen Code `0.15.6` answered `initialize` in both `/Users/maxim/work/agenter` and `/Users/maxim/work/agenter/tmp/workspace`.
- Gemini CLI `0.40.1` answered `initialize` outside the sandbox in both `/Users/maxim/work/agenter` and `/Users/maxim/work/agenter/tmp/workspace`; it advertises `loadSession`, prompt image/audio/embedded-context support, and MCP `http`/`sse`, but does not advertise `sessionCapabilities.list` or `sessionCapabilities.resume`.
- Gemini sandboxed initialize still fails because the CLI tries to bind a local auth helper on `0.0.0.0` and gets `listen EPERM: operation not permitted 0.0.0.0`. Treat this as a local environment limitation, not an ACP protocol failure.

Implementation evidence:

- `cargo test -p agenter-runner acp` passed for ACP provider profiles, capability mapping, provider fallback events, permission mapping, and workspace path containment.
- `cargo test -p agenter-control-plane create_acp_session_waits_for_runner_and_stores_external_id` passed for non-Codex runner-backed session creation.
- `cargo check --workspace`, `cargo clippy --workspace -- -D warnings`, and `cargo test --workspace` passed after the shared ACP runtime landed.
- Fresh Qwen and OpenCode initialize probes answered with the same capability shapes recorded in `provider-matrix.md`; OpenCode still requires outside-sandbox access to its local state database for live probes.
- Follow-up ACP hardening added separate initialize-derived handling for `loadSession`, `sessionCapabilities.list`, `sessionCapabilities.resume`, and `sessionCapabilities.fork`.
- Refresh/import now skips `session/list` when a provider does not advertise list support; Gemini returns an empty discovered-session list instead of surfacing a provider error.
- Gemini profile startup/response timeout is 60 seconds; Qwen and OpenCode keep the shorter default. Timeout/setup errors include provider id, recent stderr, and Gemini auth/trust/sandbox guidance.
- Qwen `session/new` in `tmp/workspace` returns a native UUID session id plus model, mode, and config option metadata. A harmless prompt emitted `available_commands_update`, then failed with ACP error `-32603 Internal error` and stderr details `Connection error.`
- Gemini `session/new` in `tmp/workspace` returns a native UUID session id plus model and mode metadata. A harmless prompt emitted `available_commands_update`, streamed `agent_message_chunk` with text `OK`, and completed with `stopReason: end_turn` plus quota metadata.

Open questions to close during implementation:

- Exact provider shapes for `session/list`, `session/load`, `session/resume`, and prompt completion across all three harnesses.
- How much terminal streaming fidelity is needed for v1 beyond command start/output/completed app events.
- Whether provider-native slash commands should be surfaced through ACP `available_commands_update` in this pass or kept as fallback `ProviderEvent`s.
- Qwen browser smoke still needs a working configured model path; the current live prompt failed at provider/model connectivity, not ACP framing.
- OpenCode prompt/session smoke still needs follow-up after its outside-sandbox state database requirement is handled.
