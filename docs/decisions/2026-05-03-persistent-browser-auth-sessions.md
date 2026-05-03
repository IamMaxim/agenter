# Persistent Browser Auth Sessions

Status: accepted
Date: 2026-05-03

## Context

Browser login already uses an opaque `agenter_session` HttpOnly cookie, but the control plane kept the token-to-user mapping only in process memory. When the control plane restarted, clients still had a cookie but `/api/auth/me` could no longer resolve it, so the browser returned to the login screen.

## Decision

Persist browser auth sessions in Postgres when `DATABASE_URL` is configured. Store only a SHA-256 hash of the cookie token, bind it to the user, and expire it after 30 days. Logout revokes the persisted row and expires the browser cookie. In-memory development mode remains process-local when the control plane runs without Postgres.

## Consequences

Control-plane restarts no longer log out browser clients in normal Postgres-backed deployments. Stolen cookie exposure is bounded by the 30-day expiry and can be cut short by logout. The control plane now has a small server-side browser session table, but the browser API and WebSocket auth contract remain cookie-based and unchanged.

## Alternatives Considered

- Keep sessions in memory: simplest, but preserves the restart logout bug.
- Store raw cookie tokens: easier to query, but unnecessarily exposes active bearer tokens if the database is leaked.
- Use stateless signed cookies: avoids a session table, but makes revocation and future session management harder.
