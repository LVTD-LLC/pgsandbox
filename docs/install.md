# Install And Setup

PGSandbox is distributed as a native Rust binary. By default, it manages a local
Postgres cluster under `~/.pgsandbox/` and chooses a high local port such as
`127.0.0.1:65432`, leaving Docker or another developer database on `5432`
untouched.

The managed local runtime requires `initdb`, `pg_ctl`, and `postgres`. The
`setup` command checks for those binaries, installs PostgreSQL with Homebrew
when they are missing and Homebrew is available, initializes and starts the
managed local cluster, then writes MCP client config. PGSandbox also checks
`PATH`, common Homebrew and Postgres.app install locations such as
`/opt/homebrew/opt/postgresql/bin` and versioned locations from
`postgresql@18` through `postgresql@13`, and explicit bin directory
environment variables. The `clone_database` MCP tool additionally requires
`pg_dump` and `pg_restore` because it streams a source database dump into a new
sandbox.

## Agent-Assisted Setup

Copy this prompt into your coding agent if you want it to install and configure
PGSandbox MCP for you:

```text
Install and configure PGSandbox MCP on this machine.

PGSandbox MCP is a local stdio MCP server for disposable Postgres databases. It
uses a PGSandbox-managed local Postgres cluster by default. The setup command
checks for local Postgres server binaries such as `initdb`, `pg_ctl`, and
`postgres`, installs PostgreSQL through Homebrew when possible, and does not use
Docker or touch any existing Postgres service on port 5432.

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
4. Configure the MCP client without an admin URL unless I explicitly gave one:
   pgsandbox-mcp setup --client <client>
   Let setup check/start the managed local runtime. If PostgreSQL server
   binaries are missing and Homebrew is available, setup should install them. If
   setup cannot install them automatically, explain the exact package-manager or
   `PGSANDBOX_POSTGRES_BIN_DIR` action needed. Do not start Docker, stop Docker
   containers, or bind `localhost:5432`.
   Use --scope project for Cursor or VS Code only if I ask for project-local
   config. Otherwise use the default user scope.
5. Verify configuration and Postgres connectivity:
   pgsandbox-mcp doctor
   If this fails, explain whether the CLI, local Postgres runtime, MCP config,
   or explicit external Postgres connection failed.
6. Run the disposable end-to-end check:
   pgsandbox-mcp smoke-test
   This should create, query, and delete a sandbox database.
7. Tell me exactly which MCP client config was updated and that I need to restart
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

## Homebrew

```bash
brew install LVTD-LLC/tap/pgsandbox-mcp
pgsandbox-mcp setup --client codex
```

This uses the [LVTD-LLC/homebrew-tap](https://github.com/LVTD-LLC/homebrew-tap) repository, which Homebrew addresses as `LVTD-LLC/tap`.
If PostgreSQL server binaries are missing, `setup` installs the Homebrew
`postgresql` package before starting the managed local runtime.

## GitHub Install Script

For users who do not use Homebrew:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | sh
pgsandbox-mcp setup --client codex
```

The installer fetches the latest GitHub release for the current OS and CPU,
installs `pgsandbox-mcp` to `~/.local/bin`, and verifies checksums when the
release includes `pgsandbox-mcp-<version>-checksums.txt`. The install script
installs the PGSandbox binary; `pgsandbox-mcp setup` owns the local Postgres
runtime check/start flow after that.

Pin a version or install somewhere else with environment variables:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | PGSANDBOX_VERSION=0.1.3 sh
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | PGSANDBOX_INSTALL_DIR=/usr/local/bin sh
```

## Install From Source For Development

Normal users should prefer Homebrew or the GitHub release installer above.
Source installs are for contribution work, testing a local checkout, or
validating unreleased changes.

```bash
cargo install --path .
pgsandbox-mcp setup --client codex
```

From GitHub without cloning first:

```bash
cargo install --git https://github.com/LVTD-LLC/pgsandbox-mcp --tag v0.1.3
pgsandbox-mcp setup --client codex
```

## Update

The installed CLI binary is the MCP server process that clients launch. Updating
the CLI and restarting the MCP client updates the server. Rerun `setup` when the
binary path, explicit admin URL, selected client, or scope changes.

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

For a custom install directory, reinstall there and keep the MCP config pointed
at the same binary:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | PGSANDBOX_INSTALL_DIR=/usr/local/bin sh
pgsandbox-mcp setup --client codex --command /usr/local/bin/pgsandbox-mcp
```

From source:

```bash
cargo install --path . --force
# or, from GitHub:
cargo install --git https://github.com/LVTD-LLC/pgsandbox-mcp --tag v<VERSION> --force
```

Replace `v<VERSION>` with the release tag you want to install.

Rerunning `setup` updates the existing local MCP config entry and preserves
unrelated MCP servers. Restart the MCP client after updating; in Codex, run
`/mcp` after restart to verify the `pgsandbox` server is available.

For maintainers publishing a new version: bump the package version, publish a
GitHub release with the generated archives, wait for the `Update Homebrew tap`
workflow to open a PR in `LVTD-LLC/homebrew-tap`, and merge that tap PR before
telling Homebrew users to run `brew upgrade`.

## Supported Clients

```bash
pgsandbox-mcp setup --client codex
pgsandbox-mcp setup --client cursor --scope project
pgsandbox-mcp setup --client vscode --scope project
pgsandbox-mcp setup --client claude-desktop
pgsandbox-mcp setup --client all
```

## Verify

```bash
pgsandbox-mcp doctor
pgsandbox-mcp smoke-test
```

Then restart your MCP client and ask it to create a disposable Postgres sandbox.

## No-Docker Quickstart

PGSandbox does not need Docker and does not bind `localhost:5432` by default.
This check is safe to run while Docker or another developer Postgres is already
using port `5432`:

```bash
pgsandbox-mcp local start
pgsandbox-mcp doctor
pgsandbox-mcp smoke-test
```

The local runtime should report its selected high port, usually `65432` or the
next free port. If `smoke-test` passes, PGSandbox created, queried, and deleted
a disposable database without touching the service on `5432`.

## MCP Config Examples

Use the current native binary command in client configs. The `setup` command
writes these shapes automatically, but `--dry-run` is useful when reviewing
what will change.

```bash
pgsandbox-mcp setup --client codex --dry-run
pgsandbox-mcp setup --client claude-desktop --dry-run
pgsandbox-mcp setup --client cursor --scope project --dry-run
pgsandbox-mcp setup --client vscode --scope project --dry-run
```

Codex and user-scoped setup are usually the shortest path:

```bash
pgsandbox-mcp setup --client codex
```

Cursor and VS Code can use project scope when the repo should carry the MCP
entry:

```bash
pgsandbox-mcp setup --client cursor --scope project
pgsandbox-mcp setup --client vscode --scope project
```

Only pass `--admin-url` when intentionally opting into an external Postgres
profile. The default setup uses the managed local cluster.

To make the MCP client default to a specific installed local Postgres major
version, pass `--postgres-version` during setup:

```bash
pgsandbox-mcp setup --client codex --postgres-version 18
```

You can also start or inspect a versioned local cluster directly:

```bash
pgsandbox-mcp local start --postgres-version 18
pgsandbox-mcp local status --postgres-version 18
```

PGSandbox keeps each requested major version in separate local state, such as
`~/.pgsandbox/postgres/versions/18/`, and uses profile names like `local-pg18`.
It discovers binaries from `PATH`, common package-manager locations,
`PGSANDBOX_POSTGRES_BIN_DIR`, or `PGSANDBOX_POSTGRES_18_BIN_DIR`. Common-path
version discovery includes installed Postgres 18, 17, 16, 15, 14, and 13
binaries.

## Repo Workflow Recipe

After installation and MCP restart, an agent can validate a repo migration
workflow without using the developer's real database:

1. Call `create_database` with a short `nameHint`, owner, labels, and positive
   TTL.
2. Optionally call `prepare_for_repo` with the repo path, sandbox id, and an
   explicit `migrationCommand` argv array to store reusable workflow metadata.
3. Call `validate_schema_change` with the same repo path, sandbox id, and either an
   explicit command or the configured `migrationCommand`.
4. Optionally call `seed_database` with an explicit seed command argv array.
5. Save a checkpoint with `create_schema_snapshot` or create a reusable local
   template with `create_template_from_sandbox`.

`prepare_for_repo` writes `.pgsandbox/project.json` without secrets. Migration
and seed tools run commands without a shell, inject the sandbox URL through
environment variables, and return bounded stdout/stderr. If the repo has a
Compose or devcontainer Postgres image such as `postgres:16`, `prepare_for_repo`
records that version so later workflow calls can use the matching local profile.

PGSandbox does not ship framework adapters. Agents should choose the appropriate
repo command for the task and pass it as a short argv array.

## Troubleshooting

- Missing `initdb`, `pg_ctl`, or `postgres`: rerun
  `pgsandbox-mcp setup --client <client>` first. Setup checks `PATH`, common
  Homebrew locations, Postgres.app, and explicit bin dir environment variables;
  when Homebrew is available it installs PostgreSQL for you before starting the
  managed local runtime. If setup cannot install automatically, install local
  PostgreSQL server binaries with your system package manager or set
  `PGSANDBOX_POSTGRES_BIN_DIR`.
- Missing a requested Postgres version: install that version locally or set
  `PGSANDBOX_POSTGRES_<major>_BIN_DIR`, for example
  `PGSANDBOX_POSTGRES_13_BIN_DIR=/opt/homebrew/opt/postgresql@13/bin`.
- Missing `pg_dump` or `pg_restore`: install PostgreSQL client tools before
  using `clone_database` or template tools.
- Occupied local ports: run `pgsandbox-mcp local start`; the managed runtime
  scans upward from `65432` and does not take over `5432`.
- Old managed-local socket path after upgrade: TCP connections continue to work,
  but Unix-socket consumers should run `pgsandbox-mcp local stop` and then
  `pgsandbox-mcp local start` to move the live socket under
  `/tmp/pgsandbox-sockets/`.
- Stale MCP admin URL: rerun `pgsandbox-mcp setup --client <client>` without
  `--admin-url`, restart the MCP client, and rerun `pgsandbox-mcp doctor`.
- Permissions under `~/.pgsandbox`: check ownership of the directory or set
  `PGSANDBOX_HOME` to a writable local path.
- Stale sandboxes: call `list_databases`, then `delete_database` for specific
  sandboxes or `cleanup_expired` for expired ones.
- External DB safety refusals: `PGSANDBOX_ADMIN_DATABASE_URL`,
  `PGSANDBOX_CONFIG`, and `--admin-url` are explicit opt-ins. Verify the host
  and profile are intended for local/private development.

## npm/npx Status

Public npm/npx publishing is intentionally deferred. The supported install paths
today are the native binary through Homebrew, the GitHub install script, and
source install. Do not rely on `npx pgsandbox-mcp` unless a later release
decision explicitly defines the package name, auth/release process, and binary
packaging.
