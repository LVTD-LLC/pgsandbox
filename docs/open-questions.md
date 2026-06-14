# Open Questions

- Should the GitHub repo be renamed from `postgres-experiment-mcp` to `pgsandbox-mcp`?
- Where should secrets live for shared agents: OpenClaw env, Infisical, or another store?
- Do agents need `run_sql`, or should the MCP mostly return connection strings and let existing DB tools handle SQL?
- Is 240 minutes the right default TTL for task databases?
- Should cleanup be automatic only, explicit only, or both?
- Do we need per-agent quotas from day one?
- Which repos would actually benefit from a seeded database template first?
- How should users provide source database credentials for cloning without
  putting production URLs in prompts or git-tracked files?
- What sanitization or table-exclusion affordances are needed before cloning
  production data becomes a common workflow?
- When should `pg_dump`/`pg_restore` cloning give way to DBLab, stagDB,
  Neon-style branching, filesystem snapshots, or a hosted PGSandbox backend?
- What auth, tenancy, quota, billing, and audit model would a hosted PGSandbox
  database platform need before public launch?
