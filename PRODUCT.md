# PGSandbox MCP Product Context

## Purpose

PGSandbox MCP exists so coding agents can use real Postgres safely without
touching shared development databases, production-like data, or ad hoc Docker
setups for every task.

The core product bet is simple: when an agent needs a database, creating an
isolated disposable one should be faster and safer than skipping verification.

## Target Users

- Local coding agents running through MCP clients such as Codex, Cursor,
  VS Code, and Claude Desktop.
- Engineers who want agents to validate migrations, SQL, seeds, and backend
  reproduction steps against a real Postgres database.
- Internal teams experimenting with agent workflows that need temporary
  database state before they are ready to adopt a hosted control plane.

## Primary Workflows

1. Configure a reachable Postgres admin URL.
2. Register the MCP server with a local client using `pgsandbox-mcp setup`.
3. Ask an agent to create a disposable database for a task.
4. Apply schema, seed data, run SQL, inspect the schema, and gather results.
5. Delete the database explicitly or let TTL cleanup remove it.

The next primary workflow is cloning an existing database into a disposable
sandbox. The source may be production, staging, or another development
database, but the destination should still be a scoped PGSandbox-owned database
with TTL and metadata tracking.

## What Good Looks Like

- Agents choose a fresh sandbox instead of a shared database by default.
- Created databases have auditable names, scoped roles, metadata, and TTLs.
- Destructive tools cannot delete databases that PGSandbox did not create.
- Setup works from npm, npx, and Homebrew without requiring Docker.
- Results are bounded, structured, and easy for an agent to reason about.
- Failures tell the user which Postgres or MCP client configuration is wrong.

## In Scope

- MCP tools for database lifecycle, connection retrieval, SQL execution, schema
  description, listing, and cleanup.
- Profiles for multiple Postgres hosts or versions.
- Client config writers for Codex, Cursor, VS Code, and Claude Desktop.
- Local/private development environments and trusted internal networks.
- Database cloning into tracked sandboxes, starting with a practical
  `pg_dump`/`pg_restore` path and leaving room for faster snapshot backends.
- Release artifacts for npm and Homebrew-style installation.

## Out Of Scope

- Installing or managing Postgres versions.
- Production database access.
- Long-lived application data.
- A hosted public control plane in the current local-first product. A hosted
  PGSandbox database platform is a likely future product line, but should be
  designed deliberately around auth, tenancy, quotas, billing, data isolation,
  and security review.
- Direct mutation of production databases. Cloning reads from a source and
  writes into a disposable sandbox.
- Long-lived application data in local sandboxes.
- Cross-user quota, billing, auth, or tenancy in the local-only runtime.

## Product Constraints

- Safety is more important than convenience around destructive actions.
- Keep the first-run path short: one admin URL plus `setup`, `doctor`, and
  `smoke-test` should be enough for a local developer.
- Do not overfit to one agent client. MCP and the CLI should stay client-neutral
  wherever possible.
- Hosted databases, advanced backends like DBLab, stagDB, Neon-style branching,
  or `pg_dump`/`pg_restore` should preserve the current MCP mental model.

## Outcomes That Matter

- Fewer agent tasks skip database verification.
- Fewer agent tasks mutate shared or production-like databases.
- More agent tasks can validate against realistic cloned data without touching
  the source database.
- Engineers can inspect and clean up agent-created resources confidently.
- Adding a new supported MCP client or Postgres profile is mechanical.
