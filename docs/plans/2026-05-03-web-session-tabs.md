# Web Session Tabs Plan

Status: implemented
Date: 2026-05-03

## Goal

Add full frontend tab support in `web` so multiple active sessions can be open and
switched locally, while keeping hash routing as the canonical deep-link source.

## Summary

- Track open session tabs in `App.svelte`.
- Persist and restore open tabs via `web/src/lib/sessionTabs.ts`.
- Add a new tab bar component and connect tab activation/closure to hash navigation.
- Render one `ChatRoute` per open tab and keep each mounted for state continuity.
- Emit `sessionMeta` from `ChatRoute` and use it to keep tab titles up-to-date.

## Changes

- `web/src/lib/sessionTabs.ts`
  - Added tab persistence parsing/serialization helpers.
  - Added a small schema with versioned payload and dedupe/truncation behavior.
- `web/src/App.svelte`
  - Added tab state management: open/restore, activate, close, metadata sync, local persistence.
  - Added `SessionTabsBar` and route-linked tab rendering.
  - Replaced route-only chat rendering with tab stack rendering.
- `web/src/components/SessionTabsBar.svelte` (new)
  - Added tab row controls and close action.
- `web/src/routes/ChatRoute.svelte`
  - Added `sessionMeta` dispatch event for title/status updates.
- `web/src/lib/sessionTabs.test.ts` (new)
  - Added unit tests for parse/serialize/persistence edge cases.
- `web/src/styles.css`
  - Added tab bar, tab-stack, and multi-tab layout adjustments.

## Verification

Run:

- `cd web && npm run check`
- `cd web && npm run lint`
- `cd web && npm run test`
- `cd web && npm run build`

Manual check:

- Open one session from sidebar.
- Open another session in a second tab and return via tab switching.
- Rename/session title updates in-chat should update the tab label.
- Close active and inactive tabs and confirm route follows expected fallback behavior.
