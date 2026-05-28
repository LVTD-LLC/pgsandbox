# Architecture Notes

## V0 Design

The first version should be a thin MCP server in front of one Postgres instance.

```text
Agent / MCP client
        |
        v
Postgres Experiment MCP
        |
        v
Admin Postgres connection
        |
        v
Task-specific databases and roles
```

The MCP server should own all database lifecycle metadata in an internal table, for example:

- database name
- role name
- owner agent/session
- purpose
- created timestamp
- expiry timestamp
- status
- last cleanup attempt

## Resource Model

Each experiment gets:

- one database
- one login role
- credentials scoped to that database
- a TTL
- optional labels for task, repo, branch, or agent

Generated names should be deterministic enough to audit but random enough to avoid collisions:

```text
agent_exp_<agent>_<slug>_<short_id>
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
