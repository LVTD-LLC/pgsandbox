# PGSandbox MCP

PGSandbox MCP is a local-first Rust CLI and MCP stdio server that lets coding
agents create, inspect, use, clone, and delete disposable PostgreSQL databases.
It is for engineers who want agents to validate migrations, SQL, seed data, and
backend bug reproductions against a real isolated database without touching a
shared developer database.

## Key Features

- Creates one tracked PostgreSQL database and one scoped login role per task.
- Starts and reuses a managed local Postgres cluster under `~/.pgsandbox/` by
  default, without Docker and without binding `localhost:5432`.
- Supports explicit external Postgres admin profiles when you intentionally opt
  in through environment variables or a JSON config file.
- Enforces positive TTLs, max TTL caps, metadata-backed deletion, and optional
  per-owner active sandbox quotas.
- Runs user SQL through sandbox role credentials, not the admin connection.
- Returns bounded, typed SQL result sets so agents do not dump unbounded rows.
- Describes schemas, computes schema digests, diffs schemas, creates named
  schema snapshots, and returns `EXPLAIN (FORMAT JSON)` plans.
- Runs repo migration and seed commands with sandbox credentials injected
  through environment variables instead of rewriting project settings.
- Creates reusable local template artifacts from PGSandbox-owned sandboxes.
- Writes MCP client config for Codex, Cursor, VS Code, and Claude Desktop.
- Masks admin URLs and sandbox credentials in diagnostics and safe summaries.

## Table Of Contents

- [Tech Stack](#tech-stack)
- [Prerequisites](#prerequisites)
- [Getting Started](#getting-started)
- [Development From This Repo](#development-from-this-repo)
- [Architecture](#architecture)
- [Environment Variables](#environment-variables)
- [Available Scripts](#available-scripts)
- [Testing](#testing)
- [Deployment](#deployment)
- [Troubleshooting](#troubleshooting)
- [Contributing](#contributing)
- [License](#license)

## Tech Stack

- **Language**: Rust 2021 edition
- **Runtime**: Native CLI binary named `pgsandbox-mcp`
- **MCP Framework**: `rmcp` stdio server
- **Async Runtime**: Tokio
- **Database Client**: `tokio-postgres`
- **TLS**: `native-tls` and `postgres-native-tls` for Postgres URLs that use
  TLS options such as `sslmode=require`
- **Configuration**: Serde, JSON, TOML, and environment variables
- **Serialization**: `serde`, `serde_json`, `serde_yaml_ng`, and `schemars`
- **Security Helpers**: `aes-gcm` for encrypted sandbox role passwords, `sha2`
  for digests/checksums, `uuid` for ids
- **Diagnostics and HTTP**: `reqwest` for telemetry delivery, `url` for
  connection-string parsing
- **Website**: Astro 6, TypeScript, Node.js, and npm under `site/`
- **Testing**: Cargo unit tests plus opt-in live Postgres integration tests
- **Packaging**: GitHub release archives, Homebrew tap packaging, and a hosted
  install script
- **Deployment**: Native binary distribution for the MCP server; CapRover
  deployment for the separate static Astro site

## Prerequisites

For normal end-user installation:

- macOS or Linux on `x86_64` or `aarch64`
- Local PostgreSQL server binaries for the managed local runtime. `setup`
  checks for them, installs PostgreSQL through Homebrew when available, and
  starts the managed local cluster.
- Optional PostgreSQL dump tools for clone/template workflows:
  `pg_dump` and `pg_restore`
- One MCP client: Codex, Cursor, VS Code, Claude Desktop, or another client
  that can launch stdio MCP servers
- Homebrew, `curl`, or `wget` if installing a released binary

For repository development:

- Git
- Rust toolchain compatible with the repo. CI uses Rust `1.91.1`.
- Node.js 22 or newer. CI uses Node 22; `mise.toml` pins Node `24.15.0` for
  local toolchain management.
- npm
- PostgreSQL server binaries available on `PATH`, in a common package-manager
  location, or through `PGSANDBOX_POSTGRES_BIN_DIR`

PGSandbox checks `PATH`, common Homebrew locations such as
`/opt/homebrew/opt/postgresql/bin` and `/opt/homebrew/opt/postgresql@18/bin`,
Postgres.app locations, and explicit bin dir environment variables. Homebrew
kegs do not need to be linked globally if PGSandbox can discover the `opt` path.

Docker is not required. `docker-compose.example.yml` is only a demo helper for
users who intentionally want an external local Postgres profile.

## Getting Started

These steps are for a normal user who wants to install the released
`pgsandbox-mcp` binary and use it from an MCP client. You do not need to clone
this repository, install Rust, or run Cargo for the standard setup.

### 1. Install PGSandbox MCP

Homebrew is the recommended install path:

```bash
brew install LVTD-LLC/tap/pgsandbox-mcp
```

If you do not use Homebrew, install the latest GitHub release binary with the
hosted installer:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | sh
```

If the installer uses `~/.local/bin`, make sure that directory is on your
`PATH` before continuing.

Verify the installed binary:

```bash
pgsandbox-mcp --version
```

### 2. Run Setup

Pick the client you use:

```bash
pgsandbox-mcp setup --client codex
pgsandbox-mcp setup --client cursor
pgsandbox-mcp setup --client vscode
pgsandbox-mcp setup --client claude-desktop
```

`setup` does the normal local setup work for you:

- checks for the PostgreSQL server binaries the managed runtime needs
- installs PostgreSQL with Homebrew when those binaries are missing and
  Homebrew is available
- initializes and starts the managed local Postgres cluster under
  `~/.pgsandbox/`
- writes the MCP client config

For Cursor or VS Code project-local config:

```bash
pgsandbox-mcp setup --client cursor --scope project
pgsandbox-mcp setup --client vscode --scope project
```

By default, setup does not write `PGSANDBOX_ADMIN_DATABASE_URL`. That is
intentional: the MCP server will use the managed local Postgres cluster under
`~/.pgsandbox/`.

Only pass `--admin-url` when you intentionally want to use an explicit external
local/private Postgres admin profile:

```bash
pgsandbox-mcp setup --client codex --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
```

### 3. Restart Your MCP Client

Restart Codex, Cursor, VS Code, or Claude Desktop after setup. MCP clients cache
server metadata and usually do not notice a newly configured server until
restart.

In Codex, run:

```text
/mcp
```

Verify that the `pgsandbox` server appears.

### 4. Optional Verification

Run diagnostics:

```bash
pgsandbox-mcp doctor
```

Run the disposable end-to-end check:

```bash
pgsandbox-mcp smoke-test
```

The smoke test creates a sandbox, runs SQL, validates serialization behavior,
and deletes the sandbox before exiting.

You can also inspect the managed local runtime directly:

```bash
pgsandbox-mcp local status
pgsandbox-mcp local start
```

The runtime starts at port `65432` and scans upward for a free port. It should
not collide with Docker or another developer Postgres already using `5432`.

### 5. Use A Sandbox

Ask your agent to create a disposable Postgres sandbox for the task. A typical
agent workflow is:

1. Create a sandbox with `create_database`.
2. Run SQL with `run_sql` or run a repo command with `run_repo_command`.
3. Inspect schema with `describe_schema` or `schema_digest`.
4. Delete the sandbox with `delete_database`, or let TTL cleanup remove it
   later.

For direct CLI troubleshooting, this command starts the MCP server over stdio:

```bash
pgsandbox-mcp
```

You normally do not run it yourself; your MCP client launches it.

## Development From This Repo

Use this section when contributing to PGSandbox MCP, testing unreleased changes,
or pointing an MCP client at a local development build. Normal users should use
the packaged setup in [Getting Started](#getting-started).

### 1. Clone The Repository

```bash
git clone https://github.com/LVTD-LLC/pgsandbox-mcp.git
cd pgsandbox-mcp
```

### 2. Install Toolchains

If you use `mise`, the repo already declares tool versions:

```bash
mise install
```

Without `mise`, install Rust and Node manually:

```bash
rustup toolchain install stable
node --version
npm --version
```

The site requires Node `>=22.12.0`.

### 3. Install JavaScript Dependencies

The root package is mostly a command runner for Cargo and packaging scripts.
The website has its own package under `site/`.

```bash
npm ci
npm --prefix site ci --include=dev
```

### 4. Build And Check The CLI

```bash
cargo build
cargo run -- doctor
cargo run -- smoke-test
```

The development binary is created at:

```text
target/debug/pgsandbox-mcp
```

For an optimized release build:

```bash
cargo build --release
```

### 5. Configure An MCP Client For Local Development

For development, point the MCP client at the binary in this checkout so it does
not accidentally launch a separately installed release:

```bash
cargo build
cargo run -- setup --client codex --command "$(pwd)/target/debug/pgsandbox-mcp"
```

Other supported clients:

```bash
cargo run -- setup --client cursor --scope project --command "$(pwd)/target/debug/pgsandbox-mcp"
cargo run -- setup --client vscode --scope project --command "$(pwd)/target/debug/pgsandbox-mcp"
cargo run -- setup --client claude-desktop --command "$(pwd)/target/debug/pgsandbox-mcp"
cargo run -- setup --client all --command "$(pwd)/target/debug/pgsandbox-mcp"
```

Use `--dry-run` to inspect the config without writing it:

```bash
cargo run -- setup --client codex --dry-run --command "$(pwd)/target/debug/pgsandbox-mcp"
```

Restart the MCP client after setup. MCP clients cache tool metadata.

### 6. Verify MCP Server Startup Manually

The binary starts the stdio server by default:

```bash
cargo run
```

Equivalent explicit form:

```bash
cargo run -- stdio
```

You usually do not run this command directly because the MCP client owns the
stdio transport. Use it only when checking startup failures.

### 7. Run The Documentation Site

The Astro website lives in `site/`.

```bash
npm --prefix site run dev
```

Build the site:

```bash
npm run site:build
```

No build-time environment variables are required for the site.

## Architecture

### Directory Structure

```text
.
|-- rust-src/
|   |-- main.rs              Binary entrypoint
|   |-- cli.rs               CLI dispatch, setup, doctor, local runtime commands, smoke test
|   |-- mcp.rs               MCP server, public tool registration, response envelopes
|   |-- config.rs            Env and JSON config loading, profile validation
|   |-- local.rs             Managed local Postgres init/start/stop/status
|   |-- postgres.rs          Sandbox lifecycle, SQL, schema, repo workflow, templates
|   |-- names.rs             Identifier generation and SQL quoting helpers
|   |-- doctor.rs            Diagnostics and connection-string masking
|   |-- setup.rs             MCP client config writers
|   |-- telemetry.rs         Anonymous usage telemetry
|   `-- lib.rs               Library exports and package version
|-- docs/
|   |-- architecture.md      Resource model and backend notes
|   |-- mcp-tools.md         MCP tool contracts
|   |-- install.md           Install and setup guide
|   |-- homebrew.md          Homebrew packaging notes
|   |-- agent-workflows.md   Copyable agent workflow examples
|   `-- open-questions.md    Product and architecture questions
|-- tests/
|   |-- dogfood_reliability.rs
|   `-- run_sql_serialization.rs
|-- scripts/
|   |-- install.sh
|   |-- package-homebrew.sh
|   |-- package-release.sh
|   `-- update-homebrew-formula.sh
|-- site/                    Astro documentation and marketing site
|-- .github/workflows/       CI, site deploy, and Homebrew tap update workflows
|-- Cargo.toml               Rust package metadata
|-- package.json             Root npm command wrappers
|-- docker-compose.example.yml
|-- .env.example
`-- README.md
```

### Runtime Shape

PGSandbox is one native binary with multiple command modes:

```text
User or MCP client
        |
        v
pgsandbox-mcp CLI
        |
        +-- stdio MCP server
        +-- setup config writer
        +-- doctor diagnostics
        +-- local Postgres runtime manager
        `-- smoke-test verifier
```

The MCP server talks to one selected Postgres profile at a time unless the
caller explicitly requests all-version listing or cleanup. The default profile
is a managed local Postgres cluster. External profiles are opt-in.

```text
Agent / MCP client
        |
        v
PGSandbox MCP stdio server
        |
        v
Managed local cluster or explicit Postgres admin profile
        |
        v
Tracked sandbox databases and scoped sandbox roles
```

### Entry Points

- `rust-src/main.rs` starts Tokio and calls `pgsandbox_mcp::cli::run`.
- `rust-src/cli.rs` defaults to `stdio` when no command is provided.
- `rust-src/mcp.rs` exposes the public MCP tools.
- `rust-src/postgres.rs` owns lifecycle behavior and database interactions.

CLI commands:

```text
pgsandbox-mcp
pgsandbox-mcp stdio
pgsandbox-mcp setup [options]
pgsandbox-mcp doctor [options]
pgsandbox-mcp local init [options]
pgsandbox-mcp local start [options]
pgsandbox-mcp local stop [options]
pgsandbox-mcp local status [options]
pgsandbox-mcp smoke-test [options]
pgsandbox-mcp --version
pgsandbox-mcp --help
```

### Managed Local Runtime

When neither `PGSANDBOX_ADMIN_DATABASE_URL` nor `PGSANDBOX_CONFIG` is set,
PGSandbox initializes and starts a local Postgres cluster:

- root: `~/.pgsandbox` by default
- default profile: `local`
- data directory: `~/.pgsandbox/postgres/data`
- private runtime config: `~/.pgsandbox/local-postgres.json`
- default port search start: `65432`
- Unix socket directory on Unix: short PGSandbox-owned path under
  `/tmp/pgsandbox-sockets/`
- admin user: `pgsandbox_admin`

Versioned local profiles use separate state:

```text
Postgres 18 profile: local-pg18
Config:              ~/.pgsandbox/local-postgres-18.json
Data:                ~/.pgsandbox/postgres/versions/18/data
Log:                 ~/.pgsandbox/postgres/versions/18/postgres.log
```

Start a specific installed version:

```bash
pgsandbox-mcp local start --postgres-version 18
```

In MCP tools, agents should usually omit `profile` and pass only
`postgresVersion`:

```json
{ "postgresVersion": "18" }
```

Supplying both `profile` and `postgresVersion` is reserved for exact profile
targeting. A mismatch returns a structured `version_mismatch` error.

### Profiles

A profile describes an admin connection PGSandbox may use for lifecycle and
metadata operations. Profiles can be:

- managed local profiles created by PGSandbox
- explicit local Postgres URLs
- explicit private remote Postgres URLs, only with external-host opt-in
- versioned profiles carrying `postgresVersion` metadata

Admin connections are used for:

- metadata table setup
- role creation
- database creation
- database deletion
- cleanup
- audit events

User SQL and repo commands use sandbox role credentials generated for the
specific database.

### Resource Model

Each sandbox gets:

- a UUID `databaseId`
- one database
- one login role
- one generated role password
- one owner string, if supplied
- one purpose/name hint, if supplied
- JSON labels, if supplied
- a `createdAt` timestamp
- an `expiresAt` timestamp
- a `deletedAt` timestamp after deletion

Generated database and role names use the configured prefix, a slugified name
hint, and a short random id while staying within Postgres's 63 byte identifier
limit:

```text
pgsandbox_<hint>_<short_id>
pgsandbox_<hint>_<short_id>_role
```

All generated SQL identifiers and literals go through helpers in
`rust-src/names.rs`.

### Metadata And Audit Tables

PGSandbox stores lifecycle metadata in the admin database for the selected
profile.

`pgsandbox_databases`:

| Column | Purpose |
|--------|---------|
| `database_id` | Stable sandbox id returned to agents |
| `profile_name` | Profile that owns the sandbox |
| `database_name` | Generated Postgres database name |
| `role_name` | Generated Postgres login role |
| `role_password` | Encrypted sandbox role password |
| `owner` | Optional agent/session/user owner |
| `purpose` | Optional name hint or task purpose |
| `labels` | JSON metadata for repo, branch, task, suite, etc. |
| `created_at` | Creation timestamp |
| `expires_at` | TTL deadline |
| `deleted_at` | Deletion marker |

`pgsandbox_events`:

| Column | Purpose |
|--------|---------|
| `event_id` | Event UUID |
| `profile_name` | Profile for the event |
| `database_id` | Sandbox id |
| `database_name` | Sandbox database name |
| `role_name` | Sandbox role name when applicable |
| `event_type` | Lifecycle event such as `create_database` or `cleanup_expired` |
| `details` | Small JSON details |
| `created_at` | Event timestamp |

Destructive operations must find a live metadata row before dropping a
database or role. PGSandbox does not drop arbitrary databases by name.

### MCP Response Envelope

Every public MCP tool returns JSON text using a compact envelope:

```json
{
  "ok": true,
  "summary": "Tool completed successfully.",
  "warnings": [],
  "errors": [],
  "detailHandles": [],
  "result": {}
}
```

Workflow tools can also include `changedObjects`. Tool failures use the same
shape with `ok: false`, stable error `code`, a broad `category`, a human
message, and a hint. Expected categories include:

- `validation`
- `database_not_found`
- `version_mismatch`
- `restore_incompatible`
- `sql_analysis`
- `sql_syntax`
- `constraint_violation`
- `readonly_violation`
- `template_not_found`
- `timeout`

Postgres errors include SQLSTATE when available.

### Public MCP Tools

| Tool | Purpose |
|------|---------|
| `list_profiles` | List configured profiles and discovered local Postgres versions. |
| `doctor` | Return MCP-safe diagnostics and profile health. |
| `create_database` | Create one isolated sandbox database and role. |
| `clone_database` | Clone an existing source database into a new sandbox with `pg_dump`/`pg_restore`. |
| `delete_database` | Delete a metadata-owned sandbox database and role. |
| `get_connection_string` | Return a redacted connection string by default, or raw credentials when explicitly requested. |
| `run_sql` | Run SQL against a sandbox with bounded result rows. |
| `describe_schema` | Return relation, column, constraint, index, view, materialized view, foreign table, and extension metadata. |
| `schema_digest` | Return a compact checksummed schema summary. |
| `schema_diff` | Compare a previous digest with the current schema. |
| `explain_query` | Return `EXPLAIN (FORMAT JSON)` for one safe plannable statement. |
| `create_schema_snapshot` | Store a named local schema checkpoint. |
| `list_schema_snapshots` | List named schema checkpoints for a sandbox. |
| `diff_schema_snapshot` | Compare a stored snapshot with the current schema. |
| `delete_schema_snapshot` | Delete a local schema snapshot artifact. |
| `prepare_for_repo` | Write secret-free repo workflow metadata to `.pgsandbox/project.json`. |
| `run_repo_command` | Run an explicit repo command with sandbox DB env vars. |
| `validate_schema_change` | Capture before/after schema digests around a repo command. |
| `seed_database` | Run an explicit seed command against a sandbox. |
| `create_template_from_sandbox` | Export a sandbox to a reusable local template artifact. |
| `create_sandbox_from_template` | Restore a template into a new tracked sandbox. |
| `list_templates` | List local template artifacts for a profile. |
| `delete_template` | Delete a template dump and metadata. |
| `list_databases` | List active metadata-owned sandboxes. |
| `cleanup_expired` | Delete expired metadata-owned sandboxes, or dry-run the selection. |

See [docs/mcp-tools.md](docs/mcp-tools.md) for full tool inputs and outputs.

### SQL Execution

`run_sql` resolves the selected sandbox, obtains its sandbox role connection
string, and connects as that role. It does not execute user SQL through the
admin connection.

Result behavior:

- default `rowLimit`: `100`
- `rowLimit: 0`: valid zero-row preview
- hard row limit cap: `1000`
- negative `rowLimit` values return `invalid_row_limit`
- returns `returnedRowCount`
- returns `affectedRowCount` for DML/DDL command tags when available
- reports `totalRowCountKnown`
- reports `truncated`
- returns ordered `resultSets` for multi-statement SQL with 1-based
  `statementIndex` values. Top-level `rows` and row metadata summarize the last
  row-returning statement, or the last statement when no statement returned
  rows. The row limit applies independently to each row-returning result set.
- preserves `int8` and `numeric` values as JSON strings
- serializes `json`/`jsonb` as nested JSON
- serializes common Postgres arrays as JSON arrays
- returns unsupported non-null types as an object with the original type and a
  cast-to-text hint
- keeps SQL `NULL` as JSON `null`

With `readonly: true`, PGSandbox runs SQL in a read-only transaction, rejects
transaction-control escape hatches, and rolls the transaction back after
execution. Mutating statements are returned as structured `readonly_violation`
errors; harmless settings that Postgres permits inside the transaction, such as
`SET search_path`, may still run.

### Repo Workflow Tools

Repo workflow tools are intentionally conservative:

- They require an explicit `repoPath`.
- They execute argv arrays directly without an implicit shell.
- They reject shell wrappers and indirect launchers such as `bash -lc`, `sh -c`,
  `env`, `sudo`, and `nsenter`.
- They inject sandbox credentials into the child process environment.
- They return bounded stdout/stderr with truncation flags.
- They do not permanently rewrite application configuration.

Injected database environment variables include:

```text
DATABASE_URL
PGSANDBOX_DATABASE_URL
PGHOST
PGPORT
PGDATABASE
PGUSER
PGPASSWORD
```

Good command examples:

```json
["npm", "run", "migrate"]
```

```json
["psql", "-v", "ON_ERROR_STOP=1", "-f", "migrations/schema.sql"]
```

```json
["./scripts/seed.sh"]
```

`prepare_for_repo` writes `.pgsandbox/project.json` without secrets. It can
store `migrationCommand`, `seedCommand`, `databaseUrlEnv`, `postgresVersion`,
and `preparedAt`. It can infer a Postgres major version from Compose files or a
devcontainer image such as `postgres:16`, `postgis/postgis:16-3.4`, or
`timescale/timescaledb:pg16`.

### Schema Snapshots And Templates

Schema snapshots are JSON metadata artifacts under:

```text
~/.pgsandbox/schema-snapshots/<profile>/<database-id>/<snapshot-name>.json
```

They store object counts, fingerprints, profile, sandbox id, owner/purpose,
labels, Postgres version, digest version, notes, and creation time. They are
manual checkpoints, not automatically refreshed truth.

Templates are dump plus metadata artifacts under:

```text
~/.pgsandbox/templates/<profile>/<template-name>.dump
~/.pgsandbox/templates/<profile>/<template-name>.json
```

Templates can only be created from PGSandbox-owned sandboxes and restored into
new PGSandbox-owned sandboxes. They are useful for local seeded-state loops,
regression fixtures, and repeatable agent QA. They are not copy-on-write forks
or a production-data import workflow.

### Clone Workflow

`clone_database` uses a portable dump/restore path:

1. Preflight source and target Postgres major versions.
2. Create an empty tracked target sandbox.
3. Run `pg_dump` against the source database.
4. Run `pg_restore` into the target sandbox using the sandbox role.
5. Delete the target sandbox if restore fails.

Newer-to-older clone paths fail before target creation with
`restore_incompatible`. Cloning and template tools require `pg_dump` and
`pg_restore`; ordinary create/query/delete flows do not.

### Telemetry

Telemetry is enabled by default and sends anonymous, personless usage events to
PostHog. It records command/tool names, version, OS/architecture, success,
elapsed time, and small booleans/counts.

Telemetry must not include:

- Postgres URLs
- connection strings
- database names or ids
- SQL text
- owner values
- label keys or values
- full local paths
- raw error messages

Telemetry never blocks CLI or MCP tool results. See
[Environment Variables](#environment-variables) for opt-out settings.

### Safety Boundaries

PGSandbox is designed as local/private infrastructure:

- It installs PostgreSQL packages only during explicit `setup` runs when
  Homebrew is available.
- It does not require Docker.
- It does not stop Docker containers.
- It does not bind `localhost:5432` by default.
- It does not silently configure external admin URLs.
- It does not expose a public network admin surface.
- It does not delete databases missing from `pgsandbox_databases`.
- It does not log full connection strings in diagnostics.
- It does not return raw sandbox credentials unless a caller explicitly asks
  for them.

Hosted database platform work is a future product direction, but it needs a
deliberate auth, tenancy, quota, billing, and security design before a public
network admin surface is added.

### Website

The `site/` directory is an Astro site. It contains docs pages, changelog
rendering, blog content, sitemap/robots routes, and a CapRover deployment
workflow. It is separate from the MCP runtime and does not run the MCP server.

## Environment Variables

### Configuration Precedence

PGSandbox loads runtime configuration in this order:

1. If `PGSANDBOX_CONFIG` is set, load the JSON config file it points to.
2. Else if `PGSANDBOX_ADMIN_DATABASE_URL` is set, create a single explicit
   profile from environment variables.
3. Else start or reuse the managed local profile under `PGSANDBOX_HOME` or
   `~/.pgsandbox`.

Telemetry opt-out environment variables are applied after config loading.

### Core Runtime Variables

| Variable | Required | Description | Default |
|----------|----------|-------------|---------|
| `PGSANDBOX_CONFIG` | No | Path to JSON multi-profile config. Takes precedence over single-profile env setup. | unset |
| `PGSANDBOX_ADMIN_DATABASE_URL` | No | Explicit Postgres admin URL for single-profile mode. Use only when intentionally bypassing managed local. | unset |
| `PGSANDBOX_DEFAULT_PROFILE` | No | Name for the single env profile. With managed local it must remain `local`. | `default` with admin URL, `local` without |
| `PGSANDBOX_HOME` | No | Local state root for managed Postgres, templates, and snapshots. | `~/.pgsandbox` |
| `PGSANDBOX_DATABASE_PREFIX` | No | Prefix for generated database and role names. | `pgsandbox` |
| `PGSANDBOX_DEFAULT_TTL_MINUTES` | No | Default sandbox TTL for the env profile. Must be positive. | `240` |
| `PGSANDBOX_MAX_TTL_MINUTES` | No | Max allowed TTL for the env profile. Must be positive and >= default TTL. | `1440` |
| `PGSANDBOX_MAX_ACTIVE_DATABASES_PER_OWNER` | No | Optional per-owner active sandbox quota for the env profile. | unlimited |
| `PGSANDBOX_POSTGRES_VERSION` | No | Default managed local Postgres major version to select. | discovered/default |

### Local Postgres Binary Discovery

| Variable | Description |
|----------|-------------|
| `PGSANDBOX_POSTGRES_BIN_DIR` | Directory containing `initdb`, `pg_ctl`, and `postgres`. |
| `PGSANDBOX_POSTGRES_<MAJOR>_BIN_DIR` | Version-specific binary directory, such as `PGSANDBOX_POSTGRES_18_BIN_DIR`. |

Discovery order favors explicit version-specific settings, then other
configured bin dirs, common package-manager locations, local `PATH` entries,
and finally direct `PATH` command resolution.

### External Admin URL Policy

By default, explicit profile admin URLs must be local:

- `localhost`
- `127.0.0.1`
- `::1`
- URLs without a host, such as Unix socket style libpq URLs

Remote/private hosts require opt-in.

| Variable | Description | Example |
|----------|-------------|---------|
| `PGSANDBOX_ALLOW_EXTERNAL_ADMIN_URL` | Allows a non-local admin URL in single-profile env mode. | `true` |
| `PGSANDBOX_ALLOWED_ADMIN_HOSTS` | Comma-separated allowlist for admin URL hosts. | `db.internal.example,postgres.internal` |

### Telemetry Variables

| Variable | Effect |
|----------|--------|
| `PGSANDBOX_TELEMETRY=false` | Disable telemetry. |
| `PGSANDBOX_NO_TELEMETRY=1` | Disable telemetry. |
| `PGSANDBOX_DISABLE_TELEMETRY=1` | Disable telemetry. |
| `DO_NOT_TRACK=1` | Disable telemetry. |

JSON config can also disable telemetry:

```json
{
  "defaultProfile": "external-pg17",
  "profiles": [
    {
      "name": "external-pg17",
      "adminUrl": "postgres://postgres:postgres@localhost:6543/postgres"
    }
  ],
  "telemetry": {
    "enabled": false
  }
}
```

### Install Script Variables

These variables are consumed by `scripts/install.sh`:

| Variable | Description | Default |
|----------|-------------|---------|
| `PGSANDBOX_REPO` | GitHub repo to download releases from. | `LVTD-LLC/pgsandbox-mcp` |
| `PGSANDBOX_GITHUB_BASE_URL` | GitHub web base URL. | `https://github.com` |
| `PGSANDBOX_GITHUB_API_URL` | GitHub API base URL. | `https://api.github.com` |
| `PGSANDBOX_INSTALL_DIR` | Directory for the installed binary. | `~/.local/bin` or `/usr/local/bin` |
| `PGSANDBOX_VERSION` | Release version to install. | latest release |
| `PGSANDBOX_TARGET` | Release target triple. | detected OS/arch/libc |
| `PGSANDBOX_SKIP_CHECKSUM` | Skip checksum verification when set to `1`. | `0` |

Pin the current manifest version explicitly:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh \
  | PGSANDBOX_VERSION=0.4.3 sh
```

### Site Variables

The Astro site has no required build-time environment variables. Production
deployment uses GitHub Actions secrets:

| Secret | Purpose |
|--------|---------|
| `CAPROVER_PGSANDBOX_SITE_URL` | CapRover instance URL |
| `CAPROVER_PGSANDBOX_SITE_APP` | CapRover app name |
| `CAPROVER_PGSANDBOX_SITE_TOKEN` | CapRover app token |

### Example `.env`

The root `.env.example` documents a simple local/default setup:

```bash
# By default, PGSandbox uses a managed local cluster under ~/.pgsandbox.
# Set this only when intentionally using an external Postgres admin profile.
# PGSANDBOX_ADMIN_DATABASE_URL=postgres://postgres:postgres@localhost:6543/postgres
# PGSANDBOX_HOME=/path/to/pgsandbox-home
PGSANDBOX_DATABASE_PREFIX=pgsandbox
PGSANDBOX_DEFAULT_TTL_MINUTES=240
PGSANDBOX_MAX_TTL_MINUTES=1440

# Optional alternative to single-profile env configuration.
# PGSANDBOX_CONFIG=./pgsandbox.config.json

# Optional telemetry opt-out.
# PGSANDBOX_TELEMETRY=false
```

### Multi-Profile JSON Config

Use `PGSANDBOX_CONFIG` for multiple external profiles or version-specific
profiles:

```json
{
  "defaultProfile": "external-pg17",
  "profiles": [
    {
      "name": "external-pg17",
      "adminUrl": "postgres://postgres:postgres@localhost:6543/postgres",
      "postgresVersion": "17",
      "databasePrefix": "pgsandbox",
      "defaultTtlMinutes": 240,
      "maxTtlMinutes": 1440,
      "maxActiveDatabasesPerOwner": 3
    },
    {
      "name": "external-pg16",
      "adminUrl": "postgres://postgres:postgres@localhost:6544/postgres",
      "postgresVersion": "16"
    }
  ],
  "telemetry": {
    "enabled": false
  }
}
```

Use it:

```bash
export PGSANDBOX_CONFIG="$PWD/pgsandbox.config.json"
pgsandbox-mcp doctor
```

For a private remote host, opt in explicitly:

```json
{
  "defaultProfile": "private-dev",
  "profiles": [
    {
      "name": "private-dev",
      "adminUrl": "postgres://postgres:postgres@db.internal.example/postgres?sslmode=require",
      "postgresVersion": "17",
      "allowedAdminHosts": ["db.internal.example"]
    }
  ]
}
```

## Available Scripts

Run root scripts from the repository root.

| Command | Description |
|---------|-------------|
| `npm run check` | Run `cargo check`. |
| `npm test` | Run `cargo test`. |
| `npm run build` | Run `cargo build --release`. |
| `npm run typecheck` | Alias for `cargo check`. |
| `npm run start` | Run `cargo run --`. |
| `npm run package:homebrew` | Build release binary and create `dist/pgsandbox-mcp-<version>.tar.gz` for the Homebrew formula flow. |
| `npm run package:release` | Build target-specific release archive and checksum file in `dist/`. |
| `npm run site:build` | Run the Astro site build through the root package. |
| `npm run site:check-changelog` | Verify site changelog fallback content. |
| `npm run site:check-blog-content` | Validate blog content conventions. |
| `npm run site:check-blog-tables` | Validate generated/rendered blog tables after site build. |

Direct Cargo commands:

| Command | Description |
|---------|-------------|
| `cargo fmt -- --check` | Check Rust formatting. |
| `cargo clippy --all-targets -- -D warnings` | Run Clippy with warnings denied. |
| `cargo check` | Typecheck the Rust package. |
| `cargo test` | Run unit tests and skipped-by-default integration tests. |
| `cargo build` | Build debug binary. |
| `cargo build --release` | Build optimized release binary. |
| `cargo run -- --help` | Print CLI help. |
| `cargo run -- --version` | Print package version. |

CLI commands after installation:

| Command | Description |
|---------|-------------|
| `pgsandbox-mcp` | Start stdio MCP server. |
| `pgsandbox-mcp stdio` | Start stdio MCP server explicitly. |
| `pgsandbox-mcp setup --client codex` | Prepare managed local Postgres and write user-scoped Codex MCP config. |
| `pgsandbox-mcp setup --client cursor --scope project` | Prepare managed local Postgres and write project `.cursor/mcp.json`. |
| `pgsandbox-mcp setup --client vscode --scope project` | Prepare managed local Postgres and write project `.vscode/mcp.json`. |
| `pgsandbox-mcp setup --client claude-desktop` | Prepare managed local Postgres and write Claude Desktop user config. |
| `pgsandbox-mcp setup --client all` | Prepare managed local Postgres and write supported user-scoped configs. |
| `pgsandbox-mcp setup --client codex --dry-run` | Print intended config without writing or preparing local Postgres. |
| `pgsandbox-mcp doctor` | Check config and Postgres connectivity. |
| `pgsandbox-mcp doctor --postgres-version 18` | Check a requested managed local major version. |
| `pgsandbox-mcp local init` | Initialize managed local Postgres without starting it. |
| `pgsandbox-mcp local start` | Initialize if needed and start managed local Postgres. |
| `pgsandbox-mcp local status` | Show managed local status. |
| `pgsandbox-mcp local stop` | Stop managed local Postgres. |
| `pgsandbox-mcp local start --postgres-version 18` | Start versioned local profile `local-pg18`. |
| `pgsandbox-mcp smoke-test` | Create, query, and delete a sandbox. |
| `pgsandbox-mcp smoke-test --postgres-version 18` | Smoke test a specific local major version. |

Site commands:

| Command | Description |
|---------|-------------|
| `npm --prefix site run dev` | Start Astro dev server. |
| `npm --prefix site run check` | Run `astro check`. |
| `npm --prefix site run build` | Run `astro check && astro build`. |
| `npm --prefix site run preview` | Preview built Astro output. |

## Testing

### Standard Test Suite

Run the expected local checks:

```bash
npm run check
npm test
npm run build
```

For full CI parity:

```bash
cargo fmt -- --check
cargo clippy --all-targets -- -D warnings
npm run check
npm test
npm run build
npm run site:check-changelog
npm run site:check-blog-content
npm --prefix site ci --include=dev
npm run site:build
npm run site:check-blog-tables
```

### Test Layout

Rust unit tests live beside the code they cover:

```text
rust-src/config.rs     Config loading, env handling, profile validation
rust-src/local.rs      Managed local runtime paths, ports, binary discovery
rust-src/mcp.rs        Envelope normalization and tool error shaping
rust-src/names.rs      Identifier generation and SQL quoting
rust-src/postgres.rs   SQL execution, schema digest/diff, workflow behavior
rust-src/setup.rs      MCP client config writing
```

Integration-style tests live under `tests/`:

```text
tests/dogfood_reliability.rs
tests/run_sql_serialization.rs
```

These tests are compiled by `cargo test`, but live database scenarios are
skipped unless the relevant environment variable is set.

### Live Postgres Tests

Run the dogfood reliability suite against real disposable sandboxes:

```bash
PGSANDBOX_DOGFOOD_E2E=1 cargo test --test dogfood_reliability -- --nocapture
```

Run the PG18 schema snapshot regression when Postgres 18 binaries are
installed:

```bash
PGSANDBOX_DOGFOOD_PG18_E2E=1 \
  cargo test --test dogfood_reliability \
  pg18_schema_snapshot_minimal_schema_returns_without_timeout_when_enabled \
  -- --nocapture
```

Run the SQL serialization E2E test:

```bash
PGSANDBOX_RUN_SQL_SERIALIZATION_E2E=1 \
  cargo test --test run_sql_serialization -- --nocapture
```

Run the readonly transaction contract E2E test:

```bash
PGSANDBOX_RUN_SQL_READONLY_E2E=1 \
  cargo test --test run_sql_serialization \
  run_sql_readonly_contract_matches_postgres_transaction_when_enabled \
  -- --nocapture
```

Each live test creates a sandbox and attempts cleanup at the end. If cleanup
fails, the test prints the failure so you can remove the sandbox with the MCP
tool or with `pgsandbox-mcp smoke-test`/manual diagnostics. Live tests use the
normal PGSandbox config path: the managed local runtime when no explicit config
is set, or the configured `PGSANDBOX_ADMIN_DATABASE_URL`/`PGSANDBOX_CONFIG`
profile when present.

### Manual Runtime Checks

Useful local runtime checks:

```bash
pgsandbox-mcp local status
pgsandbox-mcp doctor
pgsandbox-mcp smoke-test
```

For a versioned local runtime:

```bash
pgsandbox-mcp local status --postgres-version 18
pgsandbox-mcp doctor --postgres-version 18
pgsandbox-mcp smoke-test --postgres-version 18
```

### Packaging Checks

```bash
npm run package:homebrew
npm run package:release
```

Generated release archives and checksum files are build artifacts. Do not edit
them by hand.

## Deployment

PGSandbox has two separate deployable artifacts:

1. The native `pgsandbox-mcp` CLI/MCP binary.
2. The static Astro website under `site/`.

The MCP server is intended to run locally or on a private trusted machine as a
stdio process launched by an MCP client. It is not a public web service in this
repository.

### Install Released Binary With Homebrew

This is the same recommended packaged path shown in
[Getting Started](#getting-started).

Recommended user flow:

```bash
brew install LVTD-LLC/tap/pgsandbox-mcp
pgsandbox-mcp setup --client codex
pgsandbox-mcp doctor
pgsandbox-mcp smoke-test
```

The formula lives in
[LVTD-LLC/homebrew-tap](https://github.com/LVTD-LLC/homebrew-tap), which
Homebrew addresses as `LVTD-LLC/tap`.

Restart the MCP client after setup. In Codex, run `/mcp` after restart to
verify that the `pgsandbox` server is available.

### Install Released Binary With The Script

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | sh
pgsandbox-mcp setup --client codex
pgsandbox-mcp doctor
```

The script downloads a platform-specific release archive, verifies checksums
when the release includes `pgsandbox-mcp-<version>-checksums.txt`, and installs
to `~/.local/bin` by default.

Install the current manifest version explicitly:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh \
  | PGSANDBOX_VERSION=0.4.3 sh
```

Install to a custom directory:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh \
  | PGSANDBOX_INSTALL_DIR=/usr/local/bin sh
pgsandbox-mcp setup --client codex --command /usr/local/bin/pgsandbox-mcp
```

### Install From Source For Development

Use source installs when contributing, testing a local checkout, or validating
an unreleased tag. Normal users should prefer Homebrew or the GitHub release
installer.

From a checkout:

```bash
cargo install --path . --force
pgsandbox-mcp setup --client codex
pgsandbox-mcp doctor
```

From GitHub:

```bash
cargo install --git https://github.com/LVTD-LLC/pgsandbox-mcp --tag v0.4.3 --force
pgsandbox-mcp setup --client codex
pgsandbox-mcp doctor
```

### Update An Existing Installation

Homebrew:

```bash
brew update
brew upgrade LVTD-LLC/tap/pgsandbox-mcp
pgsandbox-mcp --version
pgsandbox-mcp setup --client codex
pgsandbox-mcp doctor
```

Install script:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | sh
pgsandbox-mcp --version
pgsandbox-mcp setup --client codex
pgsandbox-mcp doctor
```

Source install for development:

```bash
cargo install --path . --force
pgsandbox-mcp setup --client codex
pgsandbox-mcp doctor
```

Rerun `setup` when the binary path, explicit admin URL, selected client, or
scope changes. Restart the MCP client after updating.

### MCP Client Rollout

`pgsandbox-mcp setup` writes config while preserving unrelated servers.

Codex user config:

```bash
pgsandbox-mcp setup --client codex
```

Cursor project config:

```bash
pgsandbox-mcp setup --client cursor --scope project
```

VS Code project config:

```bash
pgsandbox-mcp setup --client vscode --scope project
```

Claude Desktop user config:

```bash
pgsandbox-mcp setup --client claude-desktop
```

A generated Codex config entry looks like:

```toml
[mcp_servers.pgsandbox]
command = "pgsandbox-mcp"
args = ["stdio"]
```

If you intentionally configure an external Postgres admin URL, `setup` writes
it into the client config environment so desktop clients do not depend on shell
startup files:

```bash
pgsandbox-mcp setup \
  --client codex \
  --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
```

Do not use `--admin-url` for the managed local default.

### Release Packaging For Maintainers

1. Update versions in `Cargo.toml` and `package.json`.
2. Run the full test suite.
3. Build release archives.
4. Publish a GitHub release with the generated artifacts.
5. Let the Homebrew tap workflow open a PR.
6. Merge the tap PR before telling Homebrew users to upgrade.

Commands:

```bash
cargo test
npm run package:homebrew
npm run package:release
```

`npm run package:homebrew` creates:

```text
dist/pgsandbox-mcp-<version>.tar.gz
```

`npm run package:release` creates:

```text
dist/pgsandbox-mcp-<version>-<target>.tar.gz
dist/pgsandbox-mcp-<version>-checksums.txt
```

When a GitHub release is published,
`.github/workflows/update-homebrew-tap.yml` downloads or builds the Homebrew
archive, computes SHA-256, checks out `LVTD-LLC/homebrew-tap`, updates
`Formula/pgsandbox-mcp.rb`, and opens or updates a PR.

The workflow requires the `HOMEBREW_TAP_PAT` repository secret with write access
to the tap repository.

### Static Site Deployment

The site deploy workflow runs on pushes to `main` that affect site files,
`CHANGELOG.md`, the changelog checker, or the deploy workflow itself. It can
also be run manually with `workflow_dispatch`.

Workflow:

1. Install site dependencies with Node 22.
2. Run the changelog fallback check.
3. Build the Astro site.
4. Package `site/` without `node_modules`.
5. Deploy the archive to CapRover.

Local site build:

```bash
npm --prefix site ci --include=dev
npm --prefix site run build
```

Production deployment requires these GitHub secrets:

```text
CAPROVER_PGSANDBOX_SITE_URL
CAPROVER_PGSANDBOX_SITE_APP
CAPROVER_PGSANDBOX_SITE_TOKEN
```

### Docker Demo Postgres

`docker-compose.example.yml` starts a normal Postgres service on `5432` for
users who want to test explicit external profile mode:

```bash
docker compose -f docker-compose.example.yml up -d
export PGSANDBOX_ADMIN_DATABASE_URL="postgres://postgres:postgres@localhost:5432/postgres"
pgsandbox-mcp doctor
pgsandbox-mcp smoke-test
```

This is optional. Runtime code must not require Docker.

## Troubleshooting

### `could not find local Postgres binaries`

Rerun setup first. It checks the local runtime, installs PostgreSQL through
Homebrew when available, starts the managed local cluster, and writes MCP config:

```bash
pgsandbox-mcp setup --client codex
```

If setup cannot install PostgreSQL automatically, install the server binaries
with your system package manager and point PGSandbox at their bin directory:

```bash
export PGSANDBOX_POSTGRES_BIN_DIR="/path/to/postgres/bin"
pgsandbox-mcp setup --client codex
pgsandbox-mcp doctor
```

For a requested major version on Homebrew:

```bash
brew install postgresql@18
export PGSANDBOX_POSTGRES_18_BIN_DIR="/opt/homebrew/opt/postgresql@18/bin"
pgsandbox-mcp setup --client codex --postgres-version 18
```

### Requested Postgres Version Is Missing

List discovered local versions:

```bash
pgsandbox-mcp doctor
```

From MCP, call `list_profiles` with:

```json
{ "includeDiscoveredLocal": true }
```

Then install the missing version, set `PGSANDBOX_POSTGRES_<MAJOR>_BIN_DIR`, or
choose an available version.

### Existing Postgres On Port 5432

This is expected and should not be a problem. The managed local runtime starts
at `65432` and scans upward.

```bash
pgsandbox-mcp local start
pgsandbox-mcp local status
```

If you intentionally want to use the service on `5432`, set an explicit admin
URL or JSON profile.

### Stale MCP Config Uses An Old Admin URL

If `doctor` reports that it is using an admin URL from an MCP config but you
want managed local, rerun setup without `--admin-url`:

```bash
pgsandbox-mcp setup --client codex
```

Restart the MCP client and rerun:

```bash
pgsandbox-mcp doctor
```

### `pgsandbox-mcp --version` Shows A Node.js Stack Trace

An old npm/global install may be earlier in `PATH`.

```bash
which -a pgsandbox-mcp
/opt/homebrew/bin/pgsandbox-mcp --version
```

Remove the stale install or point the MCP client at the native binary:

```bash
npm uninstall -g pgsandbox-mcp
hash -r 2>/dev/null || rehash
pgsandbox-mcp setup --client codex --command /opt/homebrew/bin/pgsandbox-mcp
```

### `clone_database` Or Template Restore Fails

Install `pg_dump` and `pg_restore` from PostgreSQL client tools. Ordinary
create/query/delete flows do not require them.

```bash
pg_dump --version
pg_restore --version
```

Also check source and target Postgres major versions. Newer-to-older clone
paths are rejected before creating the target sandbox.

### `run_repo_command` Rejects A Command

Repo commands are executed directly and cannot invoke shells or launchers.
Change this:

```json
["bash", "-lc", "npm run migrate && npm run seed"]
```

To direct commands or an executable repo script:

```json
["npm", "run", "migrate"]
```

```json
["./scripts/seed.sh"]
```

### Repo Workflow Has No Migration Command

Pass a command directly:

```json
{
  "repoPath": "/absolute/path/to/repo",
  "databaseId": "sandbox-id",
  "command": ["npm", "run", "migrate"]
}
```

Or store a secret-free default:

```json
{
  "repoPath": "/absolute/path/to/repo",
  "migrationCommand": ["npm", "run", "migrate"]
}
```

`prepare_for_repo` writes `.pgsandbox/project.json`.

### SQL Results Are Truncated

`run_sql` defaults to 100 rows, accepts `rowLimit: 0` for a zero-row preview,
rejects negative `rowLimit` values with `invalid_row_limit`, and caps
`rowLimit` at 1000.

```json
{
  "databaseId": "sandbox-id",
  "sql": "select * from large_table order by id",
  "readonly": true,
  "rowLimit": 1000
}
```

Use SQL filters, aggregates, or pagination for larger inspection tasks.

### Readonly SQL Fails

With `readonly: true`, PGSandbox runs SQL in a read-only transaction and rolls
it back after execution. Mutating statements such as `INSERT` or
`CREATE TEMP TABLE` fail with `readonly_violation`; harmless settings that
Postgres permits inside the transaction, such as `SET search_path`, may still
run. If mutation is intentional, omit `readonly` or set it to `false`.

### Sandbox Was Not Found

Unscoped `databaseId` lookup searches configured profiles and running managed
local profiles. If the sandbox cannot be resolved:

1. Call `list_databases` with `includeAllVersions: true`.
2. Retry the operation with the returned `profile` or `postgresVersion`.
3. Check whether the sandbox expired or was deleted.

### Expired Sandboxes Remain

Cleanup is explicit unless you run it from a scheduler or call the MCP tool.

Profile-scoped dry run:

```json
{ "dryRun": true }
```

All running versions:

```json
{ "includeAllVersions": true, "dryRun": true }
```

Then run without `dryRun` to delete selected expired sandboxes.

### Local State Permissions

If PGSandbox cannot write under `~/.pgsandbox`, fix ownership or use another
state root:

```bash
export PGSANDBOX_HOME="$HOME/.local/state/pgsandbox"
pgsandbox-mcp local start
```

### Old Unix Socket Path After Upgrade

TCP connections continue to work. If a local Unix-socket consumer needs the new
short socket path under `/tmp/pgsandbox-sockets/`, restart the local runtime:

```bash
pgsandbox-mcp local stop
pgsandbox-mcp local start
```

### External Admin URL Is Refused

Non-local admin URLs require explicit opt-in:

```bash
export PGSANDBOX_ALLOW_EXTERNAL_ADMIN_URL=true
```

Or use a host allowlist:

```bash
export PGSANDBOX_ALLOWED_ADMIN_HOSTS="db.internal.example"
```

For JSON config, use `allowExternalAdminUrl` or `allowedAdminHosts`.

### Site Build Fails

Install site dependencies and run Astro checks directly:

```bash
npm --prefix site ci --include=dev
npm --prefix site run check
npm --prefix site run build
```

If blog table checks fail, build first and then run:

```bash
npm run site:check-blog-tables
```

## Contributing

Before editing, check the worktree:

```bash
git status --short
```

Keep changes scoped to the relevant module:

- Config loading changes belong in `rust-src/config.rs` with tests there.
- Identifier generation and quoting changes belong in `rust-src/names.rs`.
- SQL execution, cleanup, TTL, schema, template, and response-shape changes
  belong in `rust-src/postgres.rs`.
- MCP tool surface changes belong in `rust-src/mcp.rs` and should be reflected
  in [docs/mcp-tools.md](docs/mcp-tools.md).
- MCP client config writing changes belong in `rust-src/setup.rs`.
- Managed local runtime changes belong in `rust-src/local.rs`.
- User-facing setup, config, command, and packaging changes should update this
  README and any relevant files in `docs/`.

Run at least:

```bash
npm run check
npm test
npm run build
```

For behavior that needs live Postgres, add the smallest practical integration
path and document whether it uses the managed local runtime or an explicit
`PGSANDBOX_ADMIN_DATABASE_URL`.

## License

MIT. See [LICENSE](LICENSE).
