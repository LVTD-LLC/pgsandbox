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

Useful manual checks with the managed local runtime:

```bash
pgsandbox-mcp local status
pgsandbox-mcp doctor
pgsandbox-mcp smoke-test
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
  integration path and document whether it uses the managed local runtime or an
  explicit `PGSANDBOX_ADMIN_DATABASE_URL`.

## Rowset Task Boards

- When creating or updating Rowset task-board datasets from dogfood or E2E test
  runs, use the Rowset project `PGSandbox MCP`
  (`d008690f-e78f-4e7c-bcd2-5c1bed537d0a`).
- Put those datasets in the `Dogfooding Task Boards` section
  (`13aae3f4-de24-4fcf-b13e-a218cbd70914`). This section is for structured issue
  inventories from E2E runs, MCP smoke tests, and agent usability reviews.
- Keep Rowset rows secret-free. Do not include full admin URLs, sandbox
  connection strings, passwords, production URLs, SQL containing secrets, or raw
  local paths beyond repo-relative references.

## ReviewGate Setup Guidance

- When documenting or helping users set up ReviewGate, configure the actual
  review job to run only after a PR comment containing exactly
  `@reviewgate review` is created. Do not configure the ReviewGate review to run
  automatically on every PR commit through `pull_request` or `synchronize`.
- The GitHub Actions trigger should use `issue_comment` and guard both the PR
  context and trigger phrase before running the ReviewGate action:

```yaml
on:
  issue_comment:
    types: [created]

jobs:
  reviewgate:
    if: >
      github.event.issue.pull_request &&
      contains(github.event.comment.body, '@reviewgate review')
    runs-on: ubuntu-latest
    steps:
      - uses: actions/checkout@v7
      # Use the ReviewGate action's documented invocation here.
```

- No PGSandbox MCP Rust code changes are needed for this trigger policy unless
  the project later ships a generator, installer, or template that writes
  ReviewGate workflow files.

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
