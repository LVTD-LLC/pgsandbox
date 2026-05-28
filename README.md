# PGSandbox MCP

Agent-facing MCP server for disposable Postgres sandboxes.

PGSandbox lets local coding agents create isolated Postgres databases on demand without requiring Docker, a hosted control plane, or a browser. It works against any reachable Postgres admin connection: a local install, a container-local Postgres, a VPS, or a private development database host.

## Why This Exists

Agents often need a real database to validate migrations, reproduce backend bugs, test SQL assumptions, or build seeded demo states. Today that usually means touching a shared development database, hand-rolling a container, or skipping database verification entirely.

The goal is to make the safe path the easy path:

- create a fresh Postgres database for a task
- isolate it from production and other agents
- apply schema or seed data
- run SQL and inspect results
- delete it automatically after a TTL

## Install

This package is designed for local MCP clients via npm:

```bash
npx pgsandbox-mcp
```

For development from this repo:

```bash
npm install
npm run build
npm start
```

## Configuration

The fastest setup is one admin connection string:

```bash
export PGSANDBOX_ADMIN_DATABASE_URL="postgres://postgres:postgres@localhost:5432/postgres"
npx pgsandbox-mcp
```

For multiple Postgres versions or hosts, use profiles:

```json
{
  "defaultProfile": "local-pg17",
  "profiles": [
    {
      "name": "local-pg17",
      "adminUrl": "postgres://postgres:postgres@localhost:5432/postgres",
      "databasePrefix": "pgsandbox",
      "defaultTtlMinutes": 240,
      "maxTtlMinutes": 1440
    },
    {
      "name": "local-pg16",
      "adminUrl": "postgres://postgres:postgres@localhost:5433/postgres"
    }
  ]
}
```

Then run:

```bash
export PGSANDBOX_CONFIG="./pgsandbox.config.json"
npx pgsandbox-mcp
```

PGSandbox does not install or manage Postgres versions itself. It can target different versions through different profiles as long as those Postgres servers are already running.

## MCP Tools

V0 supports:

- `create_database`
- `delete_database`
- `get_connection_string`
- `run_sql`
- `describe_schema`
- `list_databases`
- `cleanup_expired`

See [docs/mcp-tools.md](docs/mcp-tools.md) for details.

## Local Shape

The service uses:

- Node.js/TypeScript MCP server
- Postgres admin connection with permissions to create databases and roles
- metadata table for ownership, TTL, credentials, and cleanup state
- optional Docker Compose only for local demo Postgres

Start with [docker-compose.example.yml](docker-compose.example.yml) only if you do not already have local Postgres running.

## Safety Rules

- All databases have explicit TTLs.
- Generated role names and database names use a predictable prefix.
- Agent-created users are not superusers.
- Destructive tools only operate on resources created by this MCP.
- Connection strings are returned only to the caller and are not logged in full.
- The service should run locally or on a private network, not as a public internet-exposed admin surface.

## Status

Early v0. Treat this as a local/internal utility until the MCP surface and cleanup semantics have more mileage.
