# PGSandbox MCP Vision

## Durable Direction

PGSandbox should become the default local database lifecycle primitive for
coding agents. An agent should be able to ask for a real Postgres database,
prove a change, and throw the state away without a human preparing special
infrastructure first.

The local product should not be positioned as "a hosted database, but local."
It is the first trust-boundary shape of the same broader idea: agent-native
Postgres environments that can later include a hosted PGSandbox database
platform.

## What Should Not Drift

- Local-first and private-by-default.
- Client-neutral MCP surface.
- Existing Postgres first; no mandatory Docker or hosted service.
- Auditable lifecycle metadata for every created database.
- Scoped sandbox roles rather than handing user SQL the admin credentials.
- Explicit TTLs and cleanup paths.
- Cloned databases remain disposable PGSandbox-owned resources; production or
  staging sources are read from, not mutated.

## Product Taste

Favor boring, inspectable infrastructure over clever orchestration. The system
should feel like a small reliable tool an engineer can understand in one
session, not a platform that takes ownership of their database environment.

Agents should get enough structured affordances to do the right thing without
needing a long prompt: create, connect, query, describe, list, delete, cleanup.

## Future Shape

The v0 resource model is one empty database plus one login role. That is enough
to validate migrations and SQL safely.

Future work can add faster seeded states through snapshot or branching
backends, such as DBLab, stagDB, Neon-style branching, filesystem snapshots, or
a simple `pg_dump`/`pg_restore` path. Those additions should feel like backend
upgrades to the same concept, not a rewrite of the user workflow.

Database cloning is the next major capability because realistic data makes
agent validation materially more useful. The first implementation can be
boring and portable; later hosted or snapshot-backed variants can make the same
MCP workflow faster and cheaper at larger scale.

## Non-Goals

- Running a public Postgres admin surface from the local MCP server.
- Becoming a general secret manager.
- Becoming a general SQL IDE.
- Replacing application-specific migration or seed tooling.
- Optimizing for production database operations.

## Success Criteria

- Agents routinely create disposable databases instead of asking users for
  shared dev database access.
- Engineers can trust that cleanup only targets PGSandbox-owned resources.
- Setup and diagnostics are clear enough that database connectivity problems
  are fixed without reading source code.
- New backends and clients can be added without changing the core mental model.
