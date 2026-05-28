# Open Questions

- Should v0 be local-only, CapRover-hosted, or both?
- Should this use a TypeScript MCP server or Python/FastAPI plus MCP adapter?
- Where should secrets live for shared agents: OpenClaw env, Infisical, or another store?
- Do agents need `run_sql`, or should the MCP only return connection strings and let existing DB tools handle SQL?
- What is the default TTL for task databases?
- Should cleanup be automatic only, explicit only, or both?
- Do we need per-agent quotas from day one?
- Which repos would actually benefit from a seeded database template first?
