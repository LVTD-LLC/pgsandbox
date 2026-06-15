# Architecture Notes

## V0 Design

The first version is a local Rust MCP server in front of one or more reachable Postgres admin connections. It does not require Docker. Docker is only useful as a quick way to run Postgres locally if the developer does not already have it installed.
For remote Postgres profiles, the configured Postgres URL can require TLS with `sslmode=require`.

This local-first shape is the current deployment boundary, not the permanent
product ceiling. A future hosted PGSandbox database platform should preserve
the same agent workflow while adding managed compute, auth, tenancy, quotas,
billing, and faster clone/fork backends.

```text
Agent / MCP client
        |
        v
PGSandbox MCP
        |
        v
Configured Postgres profile
        |
        v
Task-specific databases and roles
```

The MCP server owns all database lifecycle metadata in an internal table:

- database id
- profile name
- database name
- role name
- encrypted role password
- owner agent/session
- purpose
- labels
- created timestamp
- expiry timestamp
- deleted timestamp

## Resource Model

Each experiment gets:

- one database
- one login role
- credentials scoped to that database
- a TTL
- optional labels for task, repo, branch, or agent

Cloned experiments follow the same resource model. The source database is read
through a supplied source connection string or a future source profile, and the
destination is still a PGSandbox-created database and role recorded in
metadata.

Generated names should be deterministic enough to audit but random enough to avoid collisions:

```text
pgsandbox_<slug>_<short_id>
```

The admin connection is used only for lifecycle operations. Tool calls that run user SQL connect using the generated sandbox role.
Sandbox role passwords are encrypted before being persisted in the metadata table; existing plaintext metadata rows remain readable for compatibility.

## Telemetry

The local server sends anonymous PostHog events for CLI command completion,
MCP server startup, and MCP tool completion. The events are usage-level only:
tool or command names, version, OS/architecture, success, elapsed time, and
small booleans or counts such as `dryRun`, `readonly`, and label count.

Telemetry must not include Postgres URLs, connection strings, database names or
ids, SQL text, owner values, label keys or values, full local paths, or raw
error messages. Users can disable telemetry with environment variables or with
`"telemetry": { "enabled": false }` in `PGSANDBOX_CONFIG`.

## Profiles

Profiles are the mechanism for supporting multiple Postgres versions or hosts without PGSandbox installing Postgres itself.

Example:

```json
{
  "defaultProfile": "local-pg17",
  "profiles": [
    {
      "name": "local-pg17",
      "adminUrl": "postgres://postgres:postgres@localhost:5432/postgres"
    },
    {
      "name": "local-pg16",
      "adminUrl": "postgres://postgres:postgres@localhost:5433/postgres"
    }
  ]
}
```

## Cleanup

Cleanup can run in two ways:

- explicit MCP tool: `cleanup_expired`
- scheduled process: cron or long-running interval inside the service

Cleanup should only delete databases listed in the metadata table and matching the configured prefix.

## Cloning And Future Branching Backends

The first cloning backend should favor portability and clarity:

- create an empty sandbox database and scoped role
- run `pg_dump` against the source database with ownership and privileges omitted
- stream the dump into `pg_restore` connected as the sandbox role
- delete the destination sandbox if the clone fails

This requires local PostgreSQL client tools for cloning only. Normal empty
sandbox creation should continue to work without `pg_dump` or `pg_restore`.

If `pg_dump`/`pg_restore` becomes too slow for large seeded states, evaluate a
branching backend:

- DBLab Engine
- stagDB
- Neon OSS
- filesystem snapshots on a dedicated Postgres host

The MCP contract should stay mostly the same. The backend can later learn `fork_database` or `create_from_snapshot`.
