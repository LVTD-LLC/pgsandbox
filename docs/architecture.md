# Architecture Notes

## V0 Design

The first version is a local Rust MCP server in front of one or more reachable Postgres admin connections. It does not require Docker. Docker is only useful as a quick way to run Postgres locally if the developer does not already have it installed.
For remote Postgres profiles, the configured Postgres URL can require TLS with `sslmode=require`.

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

Generated names should be deterministic enough to audit but random enough to avoid collisions:

```text
pgsandbox_<slug>_<short_id>
```

The admin connection is used only for lifecycle operations. Tool calls that run user SQL connect using the generated sandbox role.
Sandbox role passwords are encrypted before being persisted in the metadata table; existing plaintext metadata rows remain readable for compatibility.

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

## Future Branching Backend

If isolated empty databases are useful but slow for seeded application states, evaluate a cloning backend:

- DBLab Engine
- stagDB
- Neon OSS
- filesystem snapshots on a dedicated Postgres host

The MCP contract should stay mostly the same. The backend can later learn `fork_database` or `create_from_snapshot`.
