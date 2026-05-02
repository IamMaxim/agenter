# Workbench Redesign Plan

Status: implemented
Date: 2026-05-02

## Goal

Align the browser workbench with the checked-in mockups: sidebar/navigation from `tmp/mockup-1/Agenter Prototype.html` and chat/tool-call rows from `tmp/mockup-1/Tool Calls Mockup.html`.

## Implemented Behavior

- The sidebar uses the prototype structure: brand/action header, compact search, collapsible workspace groups, status/count summaries, flat active session rows, relative update times, and footer links/status.
- The transcript uses the tool-call mockup structure for inline event rows: chevron, compact tool icon, status glyph, muted summary text, and rail-indented expandable details.
- Subagent rows use the same inline event row language as tool rows.
- The chat header and composer inherit the redesigned terminal-adjacent token layer.
- No backend, protocol, or database contract changes were required.

## Files Changed

- `web/src/components/SessionTreeSidebar.svelte`
- `web/src/components/InlineEventRow.svelte`
- `web/src/components/SubagentEventRow.svelte`
- `web/src/routes/ChatRoute.svelte`
- `web/src/styles.css`

## Verification

- Frontend verification recommended: `cd web && npm run check && npm run lint && npm run test && npm run build`.
- Manual browser verification recommended against a live session: compare sidebar against `Agenter Prototype.html`, compare tool rows against `Tool Calls Mockup.html`, switch sessions to confirm stale socket callbacks do not affect the active chat, and confirm composer usage hover text still renders.

## Notes

- The existing runner/workspace tree API shape was sufficient.
- Session status, provider, and timestamps are derived from existing `SessionInfo`.
- Tool-row details continue to render from existing normalized `ChatItem` data.

## 2026-05-02 Visual Repair Iteration

- Fixed the sidebar grid contract so brand, search, session tree, and footer each occupy their intended row; the search box no longer stretches into a large overlay.
- Re-tuned sidebar overflow, footer fit, transcript width, inline event rows, subagent rows, and composer focus/spacings toward `tmp/mockup-1`.
- Verification still pending per interactive workflow; recommended frontend commands remain `cd web && npm run check && npm run lint && npm run test && npm run build`.

## 2026-05-02 Mockup Polish Iteration

- Added local inline SVG icons for sidebar controls, tool rows, subagent rows, approval file rows, and composer chips without adding a package dependency.
- Replaced composer native selects with compact chip menus while preserving the accepted bottom-bar order: mode, model, thinking level, context usage, 5h usage, and weekly usage.
- Restyled approval and question cards into neutral log-style panels with compact metadata, file rows, and smaller action buttons.
- Consolidated the newest mockup-facing CSS in a final override layer so this visual pass does not require backend, protocol, or storage changes.
- Verification: `cd web && npm run check`, `npm run lint`, `npm run test`, and `npm run build` passed on 2026-05-02.

## 2026-05-02 Row Polish Follow-Up

- Removed stale text pseudo-icons from tool rows now that rows render local SVG icons.
- Replaced the disclosure chevron path with a cleaner compact icon and kept the same rotation contract.
- Tightened the running glyph into a true circular spinner and limited spinners to active command/tool rows, so generic status/provider rows do not spin.
- Collapsed resolved approvals into a compact transcript row that keeps the decision visible without leaving the full approval panel open.
- Verification: `cd web && npm run check`, `npm run lint`, `npm run test`, and `npm run build` passed on 2026-05-02.
