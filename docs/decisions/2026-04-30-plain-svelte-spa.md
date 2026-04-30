# Plain Svelte SPA for Browser UI

Status: accepted
Date: 2026-04-30

## Context

Agenter's browser UI is the full-fidelity interaction surface for login, workspace selection, session lists, chat, streaming events, and approvals. The Rust control plane owns authentication, authorization, REST APIs, and WebSocket APIs. The UI does not need server-side rendering or server-owned route handlers for the initial browser MVP.

## Decision

Use a plain Svelte TypeScript SPA built with Vite. The app uses client-side routes and talks to the Rust control plane through `/api/*` REST endpoints plus `/api/browser/ws` for realtime session events. The compiled frontend can be served as static assets by the control plane, a reverse proxy, or another static file host.

## Consequences

This keeps frontend deployment simple and avoids adding a second server-side runtime beside the Rust control plane. It fits self-hosted LAN and VPS deployments where the API and static UI can share an origin. Browser route handling stays client-side, so the static host must fall back to `index.html` for deep links if non-hash routes are introduced later.

## Alternatives Considered

- SvelteKit: useful for server-side rendering, form actions, and server route handlers, but duplicates responsibilities already owned by the Rust control plane for this MVP.
- Server-rendered Rust templates: fewer frontend dependencies, but weaker fit for a streaming chat workbench and richer event rendering.
- Plain JavaScript without Svelte: lower tooling cost, but would make stateful chat and approval views harder to maintain.
