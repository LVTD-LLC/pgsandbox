# PGSandbox MCP Agent Guide

## Scope

This file is the repo-level operating manual for coding agents working on
`pgsandbox-mcp`. Apply it to the whole repository unless a more specific
steering file says otherwise.

## Project Summary

PGSandbox MCP is a local-first MCP server that lets coding agents create,
inspect, use, and delete disposable Postgres databases. The goal is to make a
real isolated database the easy default for migrations, SQL validation, seeded
demo states, and backend bug reproduction.

The package is a Rust native CLI and MCP stdio server. It does not install or
manage Postgres itself. It targets one or more existing Postgres admin
connections configured by environment variables or a JSON config file.

## First Files To Read

- `README.md` for user-facing install and workflow.
- `docs/architecture.md` for the resource model and future backend ideas.
- `docs/mcp-tools.md` for MCP tool contracts.
- `docs/open-questions.md` before expanding scope.
- `PRODUCT.md`, `TECH.md`, `STRUCTURE.md`, `VISION.md`, and `DESIGN.md`
  for steering context.

## Reliable Commands

Run these from the repo root:

```bash
npm run check
npm test
npm run build
```

These npm scripts delegate to Cargo. The direct equivalents are:

```bash
cargo check
cargo test
cargo build --release
```

Useful manual checks when Postgres is available:

```bash
pgsandbox-mcp doctor --admin-url postgres://postgres:postgres@localhost:5432/postgres
pgsandbox-mcp smoke-test --admin-url postgres://postgres:postgres@localhost:5432/postgres
```

Release packaging check:

```bash
npm run package:homebrew
```

## Implementation Rules

- Keep the MCP tool surface narrow and explicit in `rust-src/mcp.rs`; put
  database lifecycle behavior in `rust-src/postgres.rs`.
- Preserve the distinction between admin connections and sandbox role
  connections. Admin connections are for lifecycle and metadata only.
- Destructive operations must only target databases recorded in the
  `pgsandbox_databases` metadata table for the selected profile.
- Keep TTL enforcement and `maxTtlMinutes` caps intact when changing create or
  cleanup behavior.
- Do not log full connection strings. Mask or omit passwords in user-facing
  output unless the tool intentionally returns sandbox credentials to the caller.
- Use `quote_ident` and `quote_literal` from `rust-src/names.rs` for generated SQL
  identifiers/literals. Do not interpolate identifiers by hand.
- Preserve `run_sql` row limiting and truncation behavior. New query paths
  should have tests that prove large result sets do not dump unbounded rows.
- Prefer structured parsing and validation through Rust types, Serde, JSON,
  TOML, or Postgres client APIs over regex/string manipulation when practical.
- Do not make Docker a hard requirement. `docker-compose.example.yml` is only a
  local demo helper.
- Treat the current server as local/private infrastructure. Hosted database
  platform work is allowed as an explicit product direction, but it needs a
  deliberate design for auth, tenancy, quotas, billing, and security before
  adding a public network admin surface.

## Testing Expectations

- Changes to config loading belong with tests in `rust-src/config.rs`.
- Changes to identifier generation or quoting belong with tests in
  `rust-src/names.rs`.
- Changes to SQL execution, cleanup, TTL, or response shape belong with tests in
  `rust-src/postgres.rs`.
- Changes to MCP client config writing belong with tests in
  `rust-src/setup.rs`.
- For behavior that needs a live Postgres server, add the smallest practical
  integration path and document the required `PGSANDBOX_ADMIN_DATABASE_URL`.

## Workflow

- Check `git status --short` before editing and do not overwrite unrelated user
  changes.
- Keep changes scoped to the requested behavior and update docs when command,
  config, or tool contracts change.
- Prefer feature branches with the `rasul/` prefix when creating branches.
- Use Conventional Commit style when asked to commit.
- Before handing off implementation work, run at least `npm run check`,
  `npm test`, and `npm run build` unless the change is documentation-only.

## Publishing And Distribution

- Release archives and generated outputs are build artifacts; do not hand-edit
  them.
- `npm run package:homebrew` builds the release archive used by the tap formula.
- Client setup remains explicit through `pgsandbox-mcp setup`; installers should
  not silently write user MCP configs.
