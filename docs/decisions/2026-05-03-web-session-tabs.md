# Web Session Tabs (Frontend Scope)

Status: accepted

## Context

The web UI is currently a single-session chat view. Stage work already established the
mockup-aligned sidebar and full chat/event stream pipeline, so users currently
need to navigate away to switch sessions.

## Decision

- Introduce frontend tab state in `web/App.svelte` for active sessions under `#/sessions/{id}`.
- A session can be opened once as a tab; activating an existing session reuses that tab.
- The tab list is persisted in `localStorage` and restored on reload (`agenter.chat.tabs.v1` via `web/src/lib/sessionTabs.ts`).
- `ChatRoute` instances are mounted per open tab and stay available while the user switches tabs.
- `ChatRoute` publishes session metadata (`sessionMeta`) so the tab bar label and ordering data stay synchronized with live session updates.

## Consequences

- URL changes still determine active route; hash navigation is preserved for deep links.
- Closing a tab updates the URL to an adjacent tab when possible, or to home when no tabs remain.
- Multiple sessions may have long-lived websocket/session state simultaneously; this is accepted for now and can be refined when a dedicated tab lifecycle manager is introduced.

## Alternatives Considered

- Single shared chat component with route-only session switching:
  rejected because it would discard per-session state unless extra caching layers were built.
- Reopening historical sessions in fresh components on every tab activation:
  rejected because it loses current draft and event state that users expect to preserve.
