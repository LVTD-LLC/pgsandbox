# PGSandbox MCP Brand

## Product

PGSandbox MCP is a local MCP server that gives coding agents safe, tracked, disposable Postgres databases for tasks that need real database proof.

## Primary Persona

Technical founders, backend engineers, staff engineers, platform leads, and AI-agent operators who let coding agents modify backend code and want those agents to validate database work without touching shared development state.

## Positioning

- Local-first and private by default.
- Bring your own Postgres; PGSandbox does not install or host Postgres.
- Narrow MCP surface for database lifecycle, not a general SQL IDE or admin shell.
- One task gets one database, one scoped role, TTL metadata, bounded query results, and cleanup.
- The core promise is safer verification: agents can prove migrations, SQL, seeded states, and bug reproductions against real Postgres.

## Anti-Positioning

- Not a hosted database platform in the current local-first product.
- Not a replacement for Neon, Supabase, RDS, or other managed Postgres providers.
- Not a production database operations tool.
- Not a secret manager.
- Not a long-lived application database.
- Not Docker-required setup.

## Differentiators

- Gives agents a disposable database lifecycle through MCP instead of making them improvise with admin credentials and shell commands.
- Separates lifecycle operations from application SQL: admin connection creates/tracks sandboxes, sandbox role runs task SQL.
- Cleanup is scoped to PGSandbox-created resources and backed by metadata.
- Supports existing local, container-local, VPS, or private Postgres hosts.
- Includes setup, doctor, and smoke-test flows aimed at agent-assisted installation.
- Current blog content is managed in Rowset and rendered by Astro.

## Competitor / Alternative Set

Use these as research seeds, not as a claim that every product is a direct substitute:

- Neon database branching
- Supabase database branching
- PlanetScale Postgres branching
- Xata database branching
- Postgres.ai / DBLab
- Docker Compose local Postgres
- Testcontainers
- Generic Postgres MCP servers
- pgEdge Postgres MCP Server
- Anthropic/modelcontextprotocol postgres server
- Crystal DBA Postgres MCP Pro

## Voice

Write like a technical founder explaining infrastructure to another technical operator:

- Clear, concrete, and low-hype.
- Practical before conceptual.
- Honest about boundaries and tradeoffs.
- Uses specific implementation nouns: Postgres, MCP client, admin URL, scoped role, TTL, cleanup, pg_dump, pg_restore.
- Prefers short paragraphs and direct explanations.
- Avoids abstract platform language unless discussing future direction.
- Treats safety as an operational property, not a slogan.

## Forbidden Words and Moves

- Do not call it "magic", "revolutionary", "effortless", "seamless", "game-changing", or "AI-powered" as filler.
- Do not imply PGSandbox installs, hosts, or manages Postgres.
- Do not imply it can mutate production databases safely.
- Do not invent customer stories, usage metrics, benchmarks, or incident claims.
- Do not publish unmasked database URLs.
- Do not overstate the hosted roadmap as current product.

## Information Gain Sources

Potential original angles available from the product/repo:

- The concrete MCP tool contract: create, clone, delete, get connection string, run SQL, describe schema, list, cleanup.
- The setup prompt and safety rules for agent-assisted installation.
- The local-first resource model: one database, one scoped login role, TTL, encrypted sandbox role password in metadata.
- The clone workflow: create sandbox, pg_dump source, pg_restore into scoped sandbox role, cleanup on restore failure.
- The distinction between database branching, database cloning, disposable sandboxes, and generic Postgres MCP query servers.
- The safety checklist for giving agents database access.

## Default Author

PGSandbox Team

## Existing Content

- Homepage: "Disposable Postgres for coding agents."
- Published blog post: "Postgres MCP Server Safety Checklist for Coding Agents"
- Published blog post: "Database Branching vs Disposable Postgres Sandboxes"
- Docs: install/setup, MCP tool contract, architecture, Homebrew packaging.
