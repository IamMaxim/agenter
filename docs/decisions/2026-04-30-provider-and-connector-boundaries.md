# Provider and Connector Boundaries

Status: accepted
Date: 2026-04-30

## Context

Agenter needs extensible agent providers and extensible interaction interfaces. The first implementation includes local ACP providers, Telegram, and Mattermost, with likely future providers and connectors.

## Decision

Keep agent provider protocols separate from interaction connectors. Runners reduce native provider events into the shared `uap/2` universal event model. Connectors render and route universal events as projections. Rendering is separated from connector transport so browser, Telegram, and Mattermost can represent the same event differently without changing provider adapters.

## Consequences

Adding a provider should not require changes to connector business logic. Adding a connector should not require provider-specific protocol handling except for rendering provider metadata already preserved as safe native references or provider notifications on universal events. This increases upfront typing and module boundaries, but keeps the system scalable.

## Alternatives Considered

- Connector-specific provider handling: simpler initial bots, but can lead to duplicated provider logic.
- One broad shared utility layer: fewer files early, but weak ownership boundaries and harder testing.
