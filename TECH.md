# PGSandbox MCP Technical Steering

## Stack

- Runtime: Node.js, package engine `>=20`; CI uses Node 22.
- Language: TypeScript with `strict` enabled and ESM via `type: "module"`.
- Module settings: `NodeNext` module and resolution.
- MCP: `@modelcontextprotocol/sdk`.
- Database: `pg` and `pg-cursor`.
- Validation: `zod`.
- Tests: Vitest.
- Packaging: npm package plus bundled release archive for Homebrew.

## Commands

```bash
npm ci
npm run check
npm test
npm run build
```

Other useful commands:

```bash
npm run typecheck
npm run package:homebrew
pgsandbox-mcp doctor --admin-url postgres://postgres:postgres@localhost:5432/postgres
pgsandbox-mcp smoke-test --admin-url postgres://postgres:postgres@localhost:5432/postgres
```

## Runtime Configuration

Single-profile setup comes from environment variables:

- `PGSANDBOX_ADMIN_DATABASE_URL`
- `PGSANDBOX_DATABASE_PREFIX`
- `PGSANDBOX_DEFAULT_TTL_MINUTES`
- `PGSANDBOX_MAX_TTL_MINUTES`

Multi-profile setup comes from `PGSANDBOX_CONFIG`, which points at a JSON file
matching the shape documented in `README.md`.

Do not introduce config sources that silently override these without documenting
the precedence in `README.md` and tests.

## Core Modules

- `src/index.ts`: CLI dispatch, stdio startup, setup, doctor, and smoke-test.
- `src/server.ts`: MCP server and registered tool schemas.
- `src/config.ts`: env/JSON config loading and profile validation.
- `src/postgres.ts`: lifecycle, metadata, SQL execution, schema inspection, and
  cleanup.
- `src/names.ts`: identifier generation and SQL quoting helpers.
- `src/doctor.ts`: local diagnostics.
- `src/setup/client-config.ts`: MCP client config target resolution and writers.
- `src/version.ts`: package version export.

## Database Rules

- The admin URL must point to a database where the configured user can create
  databases and roles.
- Sandbox SQL should run through the generated sandbox role, not the admin role.
- Metadata lives in `pgsandbox_databases` on the admin connection database.
- Deletion and cleanup must find a live metadata row before dropping anything.
- `cleanup_expired` should remain bounded; it currently selects up to 50 expired
  rows per call.
- Readonly SQL must stay protected against transaction/session escape hatches.

## Client Config Rules

`pgsandbox-mcp setup` writes client config for:

- Codex: TOML under `mcp_servers`.
- Cursor and Claude Desktop: JSON under `mcpServers`.
- VS Code: JSON under `servers` with `type: "stdio"`.

Upsert behavior must preserve unrelated existing config. Add or update tests
when changing any config shape.

## Preferred Libraries

Use the existing dependencies before adding new ones:

- Use `pg`/`pg-cursor` for Postgres access.
- Use `zod` for external input and config validation.
- Use Node standard library APIs for filesystem, paths, crypto, and OS-specific
  locations.

Add dependencies only when they materially simplify maintained behavior.

## Documentation Contract

Update these docs when behavior changes:

- `README.md` for user-facing install, setup, config, and tool summaries.
- `docs/mcp-tools.md` for tool names, inputs, and outputs.
- `docs/architecture.md` for resource model or backend changes.
- `docs/install.md` for setup flow changes.
- `docs/homebrew.md` for release artifact changes.
- `docs/open-questions.md` when resolving or adding product decisions.
