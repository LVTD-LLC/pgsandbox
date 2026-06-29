# PGSandbox MCP Technical Steering

## Stack

- Runtime: Rust native binary.
- Language: Rust 2021 edition.
- MCP: `rmcp` stdio server.
- Database: `tokio-postgres` with `native-tls` for TLS profiles.
- Validation: Serde input/config structs plus explicit runtime checks.
- Tests: Cargo unit tests beside source modules.
- Packaging: GitHub release archives plus Homebrew-oriented packaging scripts.

## Commands

```bash
cargo check
cargo test
cargo build --release
```

Other useful commands:

```bash
npm run package:homebrew
pgsandbox-mcp local status
pgsandbox-mcp doctor
pgsandbox-mcp smoke-test
```

## Runtime Configuration

When `PGSANDBOX_ADMIN_DATABASE_URL` and `PGSANDBOX_CONFIG` are both absent,
PGSandbox initializes and starts the managed local cluster under
`~/.pgsandbox/postgres`, then loads a `local` profile from
`~/.pgsandbox/local-postgres.json`.

Explicit single-profile setup comes from environment variables:

- `PGSANDBOX_ADMIN_DATABASE_URL`
- `PGSANDBOX_HOME`
- `PGSANDBOX_DATABASE_PREFIX`
- `PGSANDBOX_DEFAULT_TTL_MINUTES`
- `PGSANDBOX_MAX_TTL_MINUTES`
- `PGSANDBOX_TELEMETRY`
- `PGSANDBOX_NO_TELEMETRY`
- `PGSANDBOX_DISABLE_TELEMETRY`
- `DO_NOT_TRACK`

Multi-profile setup comes from `PGSANDBOX_CONFIG`, which points at a JSON file
matching the shape documented in `README.md`. `PGSANDBOX_CONFIG` and
`PGSANDBOX_ADMIN_DATABASE_URL` are explicit opt-ins to external Postgres
profiles.

Do not introduce config sources that silently override these without documenting
the precedence in `README.md` and tests.

Telemetry is enabled by default. `PGSANDBOX_TELEMETRY=false` disables it, and
the no-telemetry flags above disable it regardless of `PGSANDBOX_TELEMETRY`.
JSON config files may also set `"telemetry": { "enabled": false }`.

## Core Modules

- `rust-src/main.rs`: binary entrypoint.
- `rust-src/cli.rs`: CLI dispatch, stdio startup, local runtime, setup, doctor,
  and smoke-test.
- `rust-src/mcp.rs`: MCP server and registered tool schemas.
- `rust-src/config.rs`: env/JSON config loading and profile validation.
- `rust-src/local.rs`: managed local Postgres cluster init/start/stop/status,
  port selection, and runtime config persistence.
- `rust-src/postgres.rs`: lifecycle, metadata, SQL execution, schema inspection, and
  cleanup.
- `rust-src/names.rs`: identifier generation and SQL quoting helpers.
- `rust-src/doctor.rs`: local diagnostics.
- `rust-src/setup.rs`: MCP client config target resolution and writers.
- `rust-src/telemetry.rs`: anonymous PostHog capture client and payload shaping.
- `rust-src/lib.rs`: library module exports and package version export.

## Database Rules

- The default local runtime must not run Docker commands, stop containers, bind
  `localhost:5432`, or mutate any existing developer database.
- Explicit profile admin URLs must point to a database where the configured user
  can create databases and roles.
- Sandbox SQL should run through the generated sandbox role, not the admin role.
- Cloned database restores should run through the generated sandbox role, not
  the admin role.
- Metadata lives in `pgsandbox_databases` on the admin connection database.
- Deletion and cleanup must find a live metadata row before dropping anything.
- `cleanup_expired` should remain bounded; it currently selects up to 50 expired
  rows per call.
- Readonly SQL must stay protected against transaction/session escape hatches.
- The managed local runtime depends on `initdb`, `pg_ctl`, and `postgres`, but
  it must not depend on Docker.
- `clone_database` may depend on `pg_dump` and `pg_restore`; ordinary sandbox
  create/query/delete must not require those dump/restore tools.

## Client Config Rules

`pgsandbox-mcp setup` writes client config for:

- Codex: TOML under `mcp_servers`.
- Cursor and Claude Desktop: JSON under `mcpServers`.
- VS Code: JSON under `servers` with `type: "stdio"`.

Upsert behavior must preserve unrelated existing config. Add or update tests
when changing any config shape.

## Telemetry Rules

- Telemetry must stay anonymous and usage-focused.
- Do not send Postgres URLs, connection strings, database identifiers, SQL text,
  owner or label values, full local paths, or raw error messages.
- Telemetry must never make a CLI command or MCP tool fail.

## Preferred Libraries

Use the existing dependencies before adding new ones:

- Use `tokio-postgres` for Postgres access.
- Use Serde-derived config/input structs plus explicit checks for external input.
- Use Rust standard library APIs for filesystem, paths, crypto, and OS-specific
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
