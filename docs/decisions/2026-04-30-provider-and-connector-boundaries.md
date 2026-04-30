# Provider and Connector Boundaries

Status: accepted
Date: 2026-04-30

## Context

Agenter needs extensible agent providers and extensible interaction interfaces. The first implementation includes Codex, Qwen, browser, Telegram, and Mattermost, with likely future providers and connectors.

## Decision

Keep agent provider protocols separate from interaction connectors. Runners normalize native provider events into a common app event model. Connectors render and route app events as projections. Rendering is separated from connector transport so browser, Telegram, and Mattermost can represent the same event differently without changing provider adapters.

## Consequences

Adding a provider should not require changes to connector business logic. Adding a connector should not require provider-specific protocol handling except for rendering provider metadata already preserved in normalized events. This increases upfront typing and module boundaries, but keeps the system scalable.

## Alternatives Considered

- Connector-specific provider handling: simpler initial bots, but leads to duplicated Codex/Qwen logic.
- One broad shared utility layer: fewer files early, but weak ownership boundaries and harder testing.
