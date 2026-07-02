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

To use a specific installed local Postgres major version, pass
`--postgres-version` or set `PGSANDBOX_POSTGRES_VERSION`:

```bash
pgsandbox-mcp local start --postgres-version 18
pgsandbox-mcp setup --client codex --postgres-version 18
```

Versioned local clusters use separate profiles and state. For example,
Postgres 18 uses profile `local-pg18`, private config
`~/.pgsandbox/local-postgres-18.json`, and data under
`~/.pgsandbox/postgres/versions/18/`. PGSandbox still does not install
Postgres; it selects an installed toolchain from `PATH`, common package-manager
locations such as Homebrew `postgresql@18`, `PGSANDBOX_POSTGRES_BIN_DIR`, or
`PGSANDBOX_POSTGRES_18_BIN_DIR`.

Before requesting a version, call MCP `list_profiles` with
`includeDiscoveredLocal: true` or run `pgsandbox-mcp doctor`. The response shows
the binary version, exposed tool count, discovered local Postgres majors,
profile version/port details when available, and a restart reminder for MCP
clients that cache tool metadata. On macOS, install
the major versions you need with Homebrew, for example:

```bash
brew install postgresql@16 postgresql@17 postgresql@18
```

The Homebrew kegs can remain unlinked; PGSandbox discovers the `opt` bin
directories. If a requested major is missing, install it, set
`PGSANDBOX_POSTGRES_<MAJOR>_BIN_DIR`, or choose one of the versions listed by
`list_profiles`.

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
  ]
}
```

Then run:

```bash
export PGSANDBOX_CONFIG="./pgsandbox.config.json"
pgsandbox-mcp
```

MCP tools can select profiles by `profile` or by `postgresVersion` when profile
metadata is present. On the managed local default, requesting `postgresVersion`
starts the corresponding `local-pg<major>` cluster on demand when matching
binaries are installed.

For agent workflows, the canonical versioned-create shape is to omit `profile`
and pass only `postgresVersion`, for example `{ "postgresVersion": "18" }`.
Supplying both is reserved for intentionally targeting an exact versioned
profile; mismatches return a structured `version_mismatch` error. Major-only
version strings such as `"16"`, `"17"`, and `"18"` are canonical, and patch
strings are normalized to the major.

Sandbox `databaseId` lookup works across configured profiles and running
managed-local profiles when the call provides only `databaseId`. If a database
id cannot be resolved, the error tells the caller to retry with `profile` or
`postgresVersion`, or to inspect active sandboxes with `list_databases` and
`includeAllVersions: true`.

If an MCP client still has a stale explicit `PGSANDBOX_ADMIN_DATABASE_URL`,
database tools return structured errors with a safe code, category, message,
and remediation hint. Run `pgsandbox-mcp doctor` to see which config source is
active. To switch a client back to managed local, rerun setup without
`--admin-url` and restart the MCP client:

```bash
pgsandbox-mcp setup --client codex
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
- `list_profiles`
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
- `run_repo_command`
- `run_migrations`
- `validate_schema_change`
- `validate_migration`
- `seed_database`
- `create_template_from_sandbox`
- `create_sandbox_from_template`
- `list_templates`
- `delete_template`
- `list_databases`
- `cleanup_expired`
- `doctor`

See [docs/mcp-tools.md](docs/mcp-tools.md) for tool contracts and
[docs/agent-workflows.md](docs/agent-workflows.md) for copyable agent-first
workflow examples.

For tool discovery, the schema inspection family is `describe_schema`,
`schema_digest`, `schema_diff`, `create_schema_snapshot`,
`list_schema_snapshots`, `diff_schema_snapshot`, and `delete_schema_snapshot`.
Use those for migration review, before/after schema diff workflows, stored
baselines, and drift detection. The reusable seeded-state family is
`create_template_from_sandbox`, `create_sandbox_from_template`,
`list_templates`, and `delete_template`.

## Agent Workflows

For repo-backed database work, an agent can:

1. Create or select a sandbox with `create_database`.
2. Use `run_sql` for direct SQL changes, or `run_repo_command` for an explicit
   repo command such as `["npm", "run", "migrate"]`.
3. Use `validate_schema_change` when the agent needs a before/after schema diff
   around a direct repo command. The older `run_migrations` and
   `validate_migration` names remain as backward-compatible aliases for
   configured migration workflows and are no longer Django-only.
4. Optionally call `prepare_for_repo` with explicit `migrationCommand` or
   `seedCommand` argv arrays. This writes a secret-free
   `.pgsandbox/project.json`. If the repo has a `postgres:<major>` or compatible
   Compose/devcontainer image, PGSandbox records that version for later workflow
   calls.
5. Optionally call `seed_database` with an explicit seed command.
6. Save reusable state with `create_schema_snapshot` or
   `create_template_from_sandbox`.

Workflow tools use compact envelopes with `ok`, `summary`, structured
`errors`, bounded output, and optional `changedObjects`. Command and tool
errors include stable categories such as `sql_analysis`, `sql_syntax`,
`database_not_found`, `version_mismatch`, `restore_incompatible`,
`constraint_violation`, `readonly_violation`, and `template_not_found` so
agents can branch without parsing prose. Postgres errors include SQLSTATE when
available. Commands are executed without an implicit shell and receive sandbox
credentials through environment variables, not permanent settings rewrites.
Schema inspection includes relation-kind counts, constraints, column defaults
and generated expressions, and view definition hashes. `run_sql` returns common
Postgres arrays such as `text[]`, integer arrays, `uuid[]`, `jsonb[]`, and
`timestamptz[]` as JSON arrays; numeric values remain strings for precision.
It also reports `returnedRowCount`, `affectedRowCount`, `totalRowCountKnown`,
and `truncated` so agents do not infer row-count semantics from `rowCount`.
Tools that intentionally return credentials also include
`connectionStringRedacted` for safe summaries and task trackers. Use `doctor`
from MCP when a client cannot shell out to `pgsandbox-mcp doctor`.

By default, `list_databases` and `cleanup_expired` are scoped to the selected
profile. Pass `includeAllVersions: true` or `postgresVersion: "*"` for an
explicit cross-version listing or cleanup across configured profiles and running
managed-local versions. Those all-version operations continue past individual
profile connection failures and return profile-level `failures` entries. Clone
requests preflight source and target Postgres majors before creating the target
sandbox; newer-to-older clone paths fail with `restore_incompatible` and include
`sourceVersion` and `targetVersion` instead of creating a sandbox and then
failing during restore.

## Local Shape

The service uses:

- Rust native binary
- `rmcp` stdio MCP server
- PG Sandbox-managed local Postgres cluster under `~/.pgsandbox/postgres` by default
- versioned local clusters under `~/.pgsandbox/postgres/versions/<major>` when requested
- local `initdb`, `pg_ctl`, and `postgres` binaries on `PATH` or in a configured bin dir for the managed local runtime
- optional explicit Postgres admin profiles with permission to create databases and roles
- metadata and audit tables for ownership, TTL, encrypted sandbox credentials,
  cleanup state, and lifecycle events
- optional `pg_dump` and `pg_restore` on `PATH` for `clone_database` and template tools

The local runtime stores its selected port, socket directory, data directory,
log file, selected Postgres version, binary directory, and private admin URL in
`~/.pgsandbox/local-postgres.json` or `~/.pgsandbox/local-postgres-<major>.json`.
CLI output masks the password.
On Unix, the socket directory is a short PGSandbox-owned path under
`/tmp/pgsandbox-sockets/` so deeply nested `PGSANDBOX_HOME` values do not exceed
Postgres Unix socket path limits.
If you upgrade from a version that stored sockets under `~/.pgsandbox/` while a
managed local cluster is already running, TCP connections continue to work. To
move the running Unix socket to the short path immediately, run
`pgsandbox-mcp local stop` and then `pgsandbox-mcp local start`.
PGSandbox first uses PostgreSQL server binaries on `PATH`, then checks common
Homebrew and Postgres.app install locations such as
`/opt/homebrew/opt/postgresql@18/bin`. If an older MCP client config still
contains `PGSANDBOX_ADMIN_DATABASE_URL`, rerun `pgsandbox-mcp setup --client
<client>` without `--admin-url` to return that client to the managed local
default.

Managed-local clusters are intentionally long-lived once started so repeated
agent tasks can create sandboxes quickly. They only host PGSandbox-owned
metadata and sandbox databases. To stop clusters without touching unrelated
Postgres services, use:

```bash
pgsandbox-mcp local status
pgsandbox-mcp local stop
pgsandbox-mcp local stop --postgres-version 18
```

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

The opt-in dogfooding regression suite exercises MCP reliability paths against
real disposable sandboxes when local Postgres is available:

```bash
PGSANDBOX_DOGFOOD_E2E=1 cargo test --test dogfood_reliability -- --nocapture
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
