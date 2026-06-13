# Open Questions

- Should the GitHub repo be renamed from `postgres-experiment-mcp` to `pgsandbox-mcp`?
- Where should secrets live for shared agents: OpenClaw env, Infisical, or another store?
- Do agents need `run_sql`, or should the MCP mostly return connection strings and let existing DB tools handle SQL?
- Is 240 minutes the right default TTL for task databases?
- Should cleanup be automatic only, explicit only, or both?
- Do we need per-agent quotas from day one?
- Which repos would actually benefit from a seeded database template first?
- Should `fork_database` wait for DBLab/stagDB integration, or should we add a simple `pg_dump`/`pg_restore` implementation first?
