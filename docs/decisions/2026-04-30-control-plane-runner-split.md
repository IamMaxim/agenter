# Control Plane and Runner Split

Status: accepted
Date: 2026-04-30

## Context

Agenter must support workspaces on a home PC, VPS, or other machine. Native agent harnesses need local workspace access and may expose stdio-first or otherwise local protocols. Public clients should not connect directly to native provider harness transports.

## Decision

Use a Rust control-plane service plus one or more Rust runner daemons. The control plane owns auth, registry metadata, connector routing, public APIs, pending approvals, and event fan-out. Runners live near workspaces and harness processes. Runners connect outbound to the control plane over a secure bidirectional channel and normalize provider events.

## Consequences

This supports machines behind NAT and keeps workspace access local to the runner. It adds a runner protocol and process supervision work, but prevents control-plane code from depending on local filesystem access or raw provider transports.

## Alternatives Considered

- Single backend process with direct filesystem and harness access: simpler for local development, but does not handle remote workspaces cleanly.
- Expose provider network protocols directly: unsafe for public deployment and incompatible with stdio-first stable transports.
- Messenger bots talking to harnesses directly: fastest for one connector, but would duplicate auth, routing, approval, and session logic.
