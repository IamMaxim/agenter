# Generic ACP Runner Runtime

Status: accepted
Date: 2026-05-02

## Context

Agenter originally had a Qwen-specific ACP adapter that spawned `qwen --acp` per turn and returned inert responses to file-system and terminal client requests. The new ACP target includes Qwen Code, Gemini CLI, and OpenCode. All three expose ACP entrypoints, with Qwen and OpenCode locally confirmed to answer `initialize` over stdio JSON-RPC.

## Decision

Implement one generic ACP runtime in the runner and represent Qwen, Gemini, and OpenCode as provider profiles. The runner serves ACP client file-system and terminal methods from the configured workspace, maps ACP permissions to Agenter approvals, and emits unknown ACP activity as provider fallback events.

One runner process can advertise multiple ACP providers for the same workspace.

## Consequences

- Provider adapters share session lifecycle, request correlation, and client-service behavior.
- Adding future ACP providers should be profile-driven when they follow the same transport.
- The runner has a stronger responsibility to enforce workspace path containment for ACP file and terminal operations.
- Provider authentication remains a local setup concern; Agenter must report setup failures clearly instead of hiding them.

## Alternatives Considered

- Harden Qwen first and generalize later: faster for one provider, but likely duplicates Gemini/OpenCode work.
- Keep inert file/terminal responses: safer but not useful for coding-agent behavior.
- Run one runner per ACP provider: simpler loops, but worse local ergonomics and unnecessary process sprawl.

