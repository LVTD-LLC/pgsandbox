# Postgres Experiment MCP

Agent-facing MCP for disposable Postgres experimentation databases.

This repo is for a small internal tool that gives coding agents safe, low-friction Postgres sandboxes. The first useful version should be boring: create isolated databases and users, return connection strings, run SQL, inspect schema, and clean up expired resources.

## Why This Exists

Agents often need a real database to validate migrations, reproduce backend bugs, test SQL assumptions, or build seeded demo states. Today that usually means touching a shared development database, hand-rolling a local container, or skipping the database verification entirely.

The goal is to make the safe path the easy path:

- create a fresh Postgres database for a task
- isolate it from production and other agents
- apply schema or seed data
- run SQL and inspect results
- delete it automatically after a TTL

## Initial Scope

V0 should support one persistent Postgres instance and MCP tools for database lifecycle operations:

- `create_database`
- `delete_database`
- `get_connection_string`
- `run_sql`
- `describe_schema`
- `list_databases`
- `cleanup_expired`

See [docs/mcp-tools.md](docs/mcp-tools.md) for the proposed tool contract.

## Non-Goals For V0

- no production database access
- no instant branching or copy-on-write clones
- no Supabase/Auth/Storage/Realtime-style app platform
- no internet-exposed admin surface
- no multi-tenant SaaS assumptions

If simple database creation proves useful, we can evaluate DBLab, stagDB, or Neon-style branching as a backend later.

## Local Shape

The expected local development stack is:

- Node.js/TypeScript MCP server
- Postgres admin connection with permissions to create databases and roles
- metadata table for ownership, TTL, and audit data
- optional local Docker Compose file for development

Start with [docker-compose.example.yml](docker-compose.example.yml) as the local Postgres baseline.

## Safety Rules

- All databases must have explicit TTLs.
- Generated role names and database names must use a predictable prefix.
- Agent-created users must not be superusers.
- Destructive tools must only operate on resources created by this MCP.
- Connection strings should be returned only to the caller and should not be logged in full.
- The service should run on a private network or behind OpenClaw-controlled access, not exposed publicly.

## Repo Status

Planning/scaffold only. No production implementation exists yet.
