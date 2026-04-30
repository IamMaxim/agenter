# Agenter

Agenter is a planned self-hostable remote wrapper for coding agents. It will let authenticated users interact with persistent agent sessions from a browser, Telegram, or Mattermost while the actual agent harnesses run near the target workspaces.

The project is currently in harness/documentation initialization. Application code should come after the technical spec and implementation plan are finalized.

## Local Manual Testing

Use the checked-in `justfile` for the common local commands:

```sh
just db-up
just control-plane
just fake-runner
just web
```

See `docs/runbooks/local-manual-testing.md` for the full terminal sequence, default credentials, and troubleshooting notes.

For verbose diagnostics, the runbook also covers `RUST_LOG`, local log files under `tmp/agenter-logs`, browser debug logging, and the optional Loki/Grafana Compose profile.

## Documentation Map

- `docs/chatgpt/001_initial.md` - initial product discussion and technical direction.
- `docs/harness/PROJECT_CONTEXT.md` - distilled current architecture and assumptions.
- `docs/harness/WORKFLOW.md` - how future agent sessions should work.
- `docs/harness/VERIFICATION.md` - verification policy by project phase.
- `docs/harness/MEMORY.md` - project-local memory protocol.
- `docs/specs/` - approved technical specs.
- `docs/plans/` - implementation plans.
- `docs/decisions/` - architecture decision records.
- `docs/runbooks/` - operational and spike procedures.

Start with `AGENTS.md` when working in this repository.
