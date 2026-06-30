# PGSandbox MCP

Safe disposable Postgres databases for coding agents.

PGSandbox is a local MCP server that gives agents a narrow, tracked way to create, use, and clean up real Postgres databases. Agents could improvise this with `psql`, `createdb`, and shell scripts. PGSandbox exists so they do not have to improvise with admin credentials every time.

By default, it manages its own local Postgres cluster under `~/.pgsandbox/` and
chooses a high local port such as `127.0.0.1:65432`, so it does not collide with
Docker or another service already bound to `localhost:5432`. You can still opt
into an external local, container, VPS, or private development Postgres profile
with `PGSANDBOX_ADMIN_DATABASE_URL` or `PGSANDBOX_CONFIG`.
Postgres URL `sslmode` settings are honored for explicit profiles, so remote
profiles can require TLS with `sslmode=require`.

## Why This Exists

Agents need real databases to validate migrations, reproduce backend bugs, test generated SQL, and build seeded demo states. Without a guardrail, the usual options are risky:

- hand an agent shared development credentials
- let it invent database create/drop commands in a shell
- keep stale test databases around after interrupted sessions
- skip database verification because setup is annoying

PGSandbox makes the safe path shorter:

- create one database and one scoped login role per task
- clone an existing Postgres source into a tracked sandbox when realistic data matters
- record every sandbox in metadata before it can be cleaned up
- run SQL through the sandbox role, not the admin connection
- cap TTLs and delete expired resources
- drop only databases PGSandbox created for the selected profile
- return bounded query results instead of dumping unbounded rows

The value is not that agents cannot use Postgres by themselves. The value is that database lifecycle becomes explicit, auditable, and disposable by default.

## Install

If you want your coding agent to install and configure PGSandbox for you, copy
this prompt into the agent:

```text
Install and configure PGSandbox MCP on this machine.

PGSandbox MCP is a local stdio MCP server for disposable Postgres databases. It
uses a PG Sandbox-managed local Postgres cluster by default. It requires local
Postgres server binaries such as `initdb`, `pg_ctl`, and `postgres` on `PATH`,
but it does not use Docker or touch any existing Postgres service on port 5432.

Do the following:
1. Detect my OS, shell, available package managers, and MCP client. Supported
   clients are codex, cursor, vscode, claude-desktop, and all. If this session
   is clearly running inside one supported MCP client, configure that client
   without asking. If several clients are installed, prefer the active client and
   ask only if you cannot infer where config should be written.
2. Install pgsandbox-mcp. Prefer:
   brew install LVTD-LLC/tap/pgsandbox-mcp
   If Homebrew is unavailable, use:
   curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | sh
   If the install script uses ~/.local/bin, make sure pgsandbox-mcp is available
   in the current shell PATH before continuing.
3. Run:
   pgsandbox-mcp --version
   If another pgsandbox-mcp appears earlier in PATH and is missing, broken, or a
   different version, use the absolute path to the healthy installed binary in
   the setup command with --command.
4. Verify the managed local runtime:
   pgsandbox-mcp local start
   pgsandbox-mcp doctor
   If `initdb`, `pg_ctl`, or `postgres` is missing, explain that local
   PostgreSQL server binaries must be installed. Do not start Docker, stop
   Docker containers, or bind `localhost:5432`.
5. Configure the MCP client without an admin URL unless I explicitly gave one:
   pgsandbox-mcp setup --client <client>
   Use --scope project for Cursor or VS Code only if I ask for project-local
   config. Otherwise use the default user scope.
6. Verify configuration and Postgres connectivity:
   pgsandbox-mcp doctor
   If this fails, explain whether the CLI, local Postgres runtime, MCP config,
   or explicit external Postgres connection failed.
7. Run the disposable end-to-end check:
   pgsandbox-mcp smoke-test
   This should create, query, and delete a sandbox database.
8. Tell me exactly which MCP client config was updated and that I need to restart
   the MCP client. After restart, help me verify that the pgsandbox server is
   available.

Constraints:
- Do not run Docker commands, stop Docker containers, bind `localhost:5432`, or
  mutate an existing developer database.
- Use the managed local cluster by default. Use `PGSANDBOX_ADMIN_DATABASE_URL`,
  `PGSANDBOX_CONFIG`, or `--admin-url` only when I explicitly ask for an
  external profile.
- Do not inline the full admin URL in commands, docs, git-tracked files, shell
  startup files, or summaries. Local runtime output should mask the password and
  point to `~/.pgsandbox/local-postgres.json` for the full private URL.
- Do not leave a smoke-test database behind. If cleanup fails, report the
  database id or name so I can delete it.
```

The intended local install is a native binary through Homebrew:

```bash
brew install LVTD-LLC/tap/pgsandbox-mcp
pgsandbox-mcp setup --client codex
pgsandbox-mcp doctor
```

The Homebrew formula lives in [LVTD-LLC/homebrew-tap](https://github.com/LVTD-LLC/homebrew-tap). Homebrew exposes that repo as the `LVTD-LLC/tap` tap.

Restart the MCP client after setup. In Codex, run `/mcp` to verify the `pgsandbox` server is available.

If you do not use Homebrew, install the latest GitHub release binary with the
hosted installer:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | sh
pgsandbox-mcp setup --client codex
pgsandbox-mcp doctor
```

The installer downloads a platform-specific release archive, verifies the
checksum when the release includes `pgsandbox-mcp-<version>-checksums.txt`, and
installs to `~/.local/bin` by default. Use `PGSANDBOX_VERSION=0.1.3` to pin a
release or `PGSANDBOX_INSTALL_DIR=/usr/local/bin` to choose a different install
directory.

For development from this repo:

```bash
cargo build
cargo run -- setup --client codex
cargo run -- smoke-test
```

Rust users can also install directly from GitHub:

```bash
cargo install --git https://github.com/LVTD-LLC/pgsandbox-mcp --tag v0.1.3
```

## Update

The CLI binary is also the MCP server process. To update both, update the
installed `pgsandbox-mcp` binary, refresh the MCP client entry if the command,
explicit admin URL, or target client changed, then restart the MCP client.

Homebrew can only upgrade after a newer GitHub release exists and the
`LVTD-LLC/homebrew-tap` formula has been updated. If `brew upgrade
LVTD-LLC/tap/pgsandbox-mcp` says the current version is already installed, the
tap does not have a newer version yet.

With Homebrew:

```bash
brew update
brew upgrade LVTD-LLC/tap/pgsandbox-mcp
pgsandbox-mcp --version
pgsandbox-mcp setup --client codex
pgsandbox-mcp doctor
```

If `pgsandbox-mcp --version` prints a Node.js stack trace or references
`dist/index.js`, another install is shadowing the Homebrew binary. Check the
resolution order:

```bash
which -a pgsandbox-mcp
/opt/homebrew/bin/pgsandbox-mcp --version
```

Remove the stale npm/global install or point the MCP client at the native
binary explicitly:

```bash
npm uninstall -g pgsandbox-mcp
hash -r 2>/dev/null || rehash
pgsandbox-mcp setup --client codex --command /opt/homebrew/bin/pgsandbox-mcp
```

With the GitHub install script:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | sh
pgsandbox-mcp --version
pgsandbox-mcp setup --client codex
pgsandbox-mcp doctor
```

If you installed to a custom path, keep the MCP client pointed at that binary:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | PGSANDBOX_INSTALL_DIR=/usr/local/bin sh
pgsandbox-mcp setup --client codex --command /usr/local/bin/pgsandbox-mcp
```

If you installed from source, rebuild and reinstall:

```bash
cargo install --path . --force
# or, from GitHub:
cargo install --git https://github.com/LVTD-LLC/pgsandbox-mcp --tag v<VERSION> --force
```

Replace `v<VERSION>` with the release tag you want to install.

Rerunning `setup` updates the local MCP client config in place and preserves
unrelated MCP servers. Restart the MCP client after updating; in Codex, run
`/mcp` after restart to verify the `pgsandbox` server is available.

For maintainers publishing a new version: bump the package version, publish a
GitHub release with the generated archives, wait for the `Update Homebrew tap`
workflow to open a PR in `LVTD-LLC/homebrew-tap`, and merge that tap PR before
telling Homebrew users to run `brew upgrade`.

## MCP Client Setup

The setup command writes the right MCP config shape for each supported client:

```bash
pgsandbox-mcp setup --client codex
pgsandbox-mcp setup --client cursor --scope project
pgsandbox-mcp setup --client vscode --scope project
pgsandbox-mcp setup --client claude-desktop
pgsandbox-mcp setup --client all
```

Supported targets:

- Codex: `~/.codex/config.toml` or project `.codex/config.toml`
- Cursor: `~/.cursor/mcp.json` or project `.cursor/mcp.json`
- VS Code: user `mcp.json` or project `.vscode/mcp.json`
- Claude Desktop: `claude_desktop_config.json`

Use `--dry-run` to print the config without writing files. Passing `--admin-url`
is an explicit opt-in to an external Postgres admin connection and writes that
URL into the MCP client config so desktop clients do not depend on shell startup
files.

## Configuration

With no database environment variables, PGSandbox initializes and starts a local
cluster under `~/.pgsandbox/postgres`, writes its private runtime config to
`~/.pgsandbox/local-postgres.json`, and uses the `local` profile. It starts at
port `65432` and picks the next free high port when needed, so an existing
Docker or developer Postgres on `5432` is left alone.

```bash
pgsandbox-mcp local start
pgsandbox-mcp
```

Set `PGSANDBOX_HOME` only when you want that managed local state somewhere other
than `~/.pgsandbox`.

For an explicit external Postgres admin connection, set a single URL:

```bash
export PGSANDBOX_ADMIN_DATABASE_URL="postgres://postgres:postgres@localhost:6543/postgres"
pgsandbox-mcp
```

For multiple external Postgres versions or hosts, use profiles:

```json
{
  "defaultProfile": "external-pg17",
  "profiles": [
    {
      "name": "external-pg17",
      "adminUrl": "postgres://postgres:postgres@localhost:6543/postgres",
      "databasePrefix": "pgsandbox",
      "defaultTtlMinutes": 240,
      "maxTtlMinutes": 1440,
      "maxActiveDatabasesPerOwner": 3
    },
    {
      "name": "external-pg16",
      "adminUrl": "postgres://postgres:postgres@localhost:6544/postgres"
    }
  ]
}
```

Then run:

```bash
export PGSANDBOX_CONFIG="./pgsandbox.config.json"
pgsandbox-mcp
```

Profiles default to local admin URLs only: `localhost`, `127.0.0.1`, `::1`, or
a URL without a host. To use a private remote Postgres host, opt in explicitly
with either `"allowExternalAdminUrl": true` or an `"allowedAdminHosts"` list.
The same policy is available for single-profile env setup:

```bash
export PGSANDBOX_ALLOW_EXTERNAL_ADMIN_URL=true
export PGSANDBOX_ALLOWED_ADMIN_HOSTS="db.internal.example"
export PGSANDBOX_MAX_ACTIVE_DATABASES_PER_OWNER=3
```

## Telemetry

PGSandbox sends anonymous usage telemetry to PostHog so the project can see
which CLI commands and MCP tools are used, whether they succeed, and how long
they take. Telemetry is enabled by default and never blocks the CLI or MCP tool
result.

Telemetry uses a random local installation id and sends personless PostHog
events. It does not send Postgres URLs, connection strings, database names or
ids, SQL text, owner values, label keys or values, full file paths, or error
messages.

Disable telemetry with any of:

```bash
export PGSANDBOX_TELEMETRY=false
export PGSANDBOX_NO_TELEMETRY=1
export PGSANDBOX_DISABLE_TELEMETRY=1
export DO_NOT_TRACK=1
```

When using `PGSANDBOX_CONFIG`, telemetry can also be disabled in JSON:

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

## MCP Tools

V0 supports:

- `create_database`
- `clone_database`
- `delete_database`
- `get_connection_string`
- `run_sql`
- `describe_schema`
- `schema_digest`
- `schema_diff`
- `explain_query`
- `create_schema_snapshot`
- `list_schema_snapshots`
- `diff_schema_snapshot`
- `delete_schema_snapshot`
- `prepare_for_repo`
- `run_migrations`
- `validate_migration`
- `seed_database`
- `create_template_from_sandbox`
- `create_sandbox_from_template`
- `list_templates`
- `delete_template`
- `list_databases`
- `cleanup_expired`

See [docs/mcp-tools.md](docs/mcp-tools.md) for details.

## Agent Workflows

For a Django repo, an agent can:

1. Create or select a sandbox with `create_database`.
2. Call `prepare_for_repo` with the repo path. This writes a secret-free
   `.pgsandbox/project.json` when Django is detected.
3. Call `validate_migration` to run `python manage.py migrate --noinput`
   against the sandbox and receive a before/after schema diff.
4. Optionally call `seed_database` with an explicit seed command.
5. Save reusable state with `create_schema_snapshot` or
   `create_template_from_sandbox`.

Workflow tools use compact envelopes with `ok`, `summary`, structured
`errors`, bounded output, and optional `changedObjects`. Commands are executed
without a shell and receive sandbox credentials through environment variables,
not permanent settings rewrites.

## Local Shape

The service uses:

- Rust native binary
- `rmcp` stdio MCP server
- PG Sandbox-managed local Postgres cluster under `~/.pgsandbox/postgres` by default
- local `initdb`, `pg_ctl`, and `postgres` binaries on `PATH` for the managed local runtime
- optional explicit Postgres admin profiles with permission to create databases and roles
- metadata and audit tables for ownership, TTL, encrypted sandbox credentials,
  cleanup state, and lifecycle events
- optional `pg_dump` and `pg_restore` on `PATH` for `clone_database` and template tools

The local runtime stores its selected port, socket directory, data directory,
log file, and private admin URL in `~/.pgsandbox/local-postgres.json`. CLI output
masks the password.
PGSandbox first uses PostgreSQL server binaries on `PATH`, then checks common
Homebrew and Postgres.app install locations such as
`/opt/homebrew/opt/postgresql@18/bin`. If an older MCP client config still
contains `PGSANDBOX_ADMIN_DATABASE_URL`, rerun `pgsandbox-mcp setup --client
<client>` without `--admin-url` to return that client to the managed local
default.

The MCP server runs over stdio:

```bash
pgsandbox-mcp
# or explicitly
pgsandbox-mcp stdio
```

## Development

```bash
cargo check
cargo test
cargo build --release
npm run site:build
```

Release packaging check:

```bash
npm run package:homebrew
npm run package:release
```

Upload the generated release archives and checksum file before publishing the
GitHub release. When the release is published,
`.github/workflows/update-homebrew-tap.yml` opens a PR against
[LVTD-LLC/homebrew-tap](https://github.com/LVTD-LLC/homebrew-tap) with the
immutable release URL and SHA256 for `Formula/pgsandbox-mcp.rb`.

The workflow requires a repository secret named `HOMEBREW_TAP_PAT`. Use a
fine-grained token with `Contents: Read and write` and `Pull requests: Read and
write` access to `LVTD-LLC/homebrew-tap`, or an equivalent classic PAT.

## Safety Rules

- All databases have explicit TTLs.
- Generated role names and database names use a predictable prefix.
- Agent-created users are not superusers.
- Destructive tools only operate on resources created by this MCP.
- Admin connections are used for lifecycle and metadata only.
- Lifecycle events are recorded in the admin database audit table.
- User SQL runs through generated sandbox credentials.
- Sandbox role passwords are encrypted before being stored in metadata.
- Non-local admin URLs require explicit profile opt-in or an allowed host list.
- Profiles can cap active sandbox count per owner with
  `maxActiveDatabasesPerOwner`.
- Connection strings are returned only to the caller and are not logged in full.
- The service should run locally or on a private network, not as a public internet-exposed admin surface.

## Status

Early v0. Treat this as local/private infrastructure until the MCP surface and cleanup semantics have more mileage.
