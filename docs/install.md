# Install And Setup

PGSandbox is distributed as a native Rust binary. It needs a reachable Postgres admin connection that can create databases and roles.

The core create/query/delete workflow does not require PostgreSQL client tools.
The `clone_database` MCP tool additionally requires `pg_dump` and `pg_restore`
on `PATH` because it streams a source database dump into a new sandbox.

## Agent-Assisted Setup

Copy this prompt into your coding agent if you want it to install and configure
PGSandbox MCP for you:

```text
Install and configure PGSandbox MCP on this machine.

PGSandbox MCP is a local stdio MCP server for disposable Postgres databases. It
needs an existing Postgres admin connection that can create databases and roles.
It does not install Postgres and does not require Docker.

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
4. Find a usable Postgres admin URL with no user interaction. Try, in order:
   - existing PGSANDBOX_ADMIN_DATABASE_URL or PGSANDBOX_CONFIG values
   - existing local MCP configs that already contain a pgsandbox admin URL
   - running Docker, OrbStack, Colima, or Podman Postgres containers and exposed
     ports; derive local URLs from container port mappings and POSTGRES_USER,
     POSTGRES_PASSWORD, and POSTGRES_DB metadata when available
   - passwordless local libpq candidates with psql -w over Unix sockets and
     localhost for the current user and postgres user against postgres/template1
   - local .pg_service.conf, .pgpass, and project env files, without printing
     file contents. Use explicit PGSANDBOX_ADMIN_DATABASE_URL values as
     candidates. For all other file-sourced candidates, including
     .pg_service.conf, .pgpass, DATABASE_URL, POSTGRES_URL, and POSTGRES_*,
     proceed without asking only when the parsed or resolved host is clearly
     local: localhost, 127.0.0.1, ::1, a Unix socket, or a container port you
     just discovered. Do not validate or configure non-local file-sourced
     database credentials silently.
   Validate candidates with `pgsandbox-mcp doctor --admin-url "$CANDIDATE_URL"`.
   When possible, also verify the role can create databases and roles, or is a
   superuser. If one valid explicit PGSANDBOX_* candidate or clearly local
   candidate is found, export it as PGSANDBOX_ADMIN_DATABASE_URL for this setup
   and continue without asking me. Tell me which source you used and the URL
   with the password masked, for example postgres://user:***@host/db. Ask me
   for a URL only after all discovery paths fail or the only remaining
   candidates are non-local file-sourced database credentials, and briefly say
   what you checked.
5. Configure the MCP client:
   pgsandbox-mcp setup --client <client> --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
   Use --scope project for Cursor or VS Code only if I ask for project-local
   config. Otherwise use the default user scope.
6. Verify configuration and Postgres connectivity:
   pgsandbox-mcp doctor --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
   If this fails, explain whether the CLI, MCP config, or Postgres connection
   failed.
7. Run the disposable end-to-end check:
   pgsandbox-mcp smoke-test --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
   This should create, query, and delete a sandbox database.
8. Tell me exactly which MCP client config was updated and that I need to restart
   the MCP client. After restart, help me verify that the pgsandbox server is
   available.

Constraints:
- Do not install, start, or modify Postgres unless I explicitly ask.
- Default to discovery and execution. Do not ask for confirmation before using a
  discovered explicit PGSANDBOX_* URL or local Postgres admin URL that passes
  validation. Ask before using any non-local database credential sourced from
  .pg_service.conf, .pgpass, DATABASE_URL, POSTGRES_URL, or POSTGRES_* values.
- Do not inline the full admin URL in commands, docs, git-tracked files, shell
  startup files, or summaries. Use "$PGSANDBOX_ADMIN_DATABASE_URL" in commands.
  The MCP setup command may write the admin URL only to the selected local MCP
  client config.
- Do not leave a smoke-test database behind. If cleanup fails, report the
  database id or name so I can delete it.
```

## Homebrew

```bash
brew install LVTD-LLC/tap/pgsandbox-mcp
pgsandbox-mcp setup --client codex --admin-url postgres://postgres:postgres@localhost:5432/postgres
```

This uses the [LVTD-LLC/homebrew-tap](https://github.com/LVTD-LLC/homebrew-tap) repository, which Homebrew addresses as `LVTD-LLC/tap`.

## GitHub Install Script

For users who do not use Homebrew:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | sh
pgsandbox-mcp setup --client codex --admin-url postgres://postgres:postgres@localhost:5432/postgres
```

The installer fetches the latest GitHub release for the current OS and CPU,
installs `pgsandbox-mcp` to `~/.local/bin`, and verifies checksums when the
release includes `pgsandbox-mcp-<version>-checksums.txt`.

Pin a version or install somewhere else with environment variables:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | PGSANDBOX_VERSION=0.1.1 sh
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | PGSANDBOX_INSTALL_DIR=/usr/local/bin sh
```

## From Source

```bash
cargo install --path .
pgsandbox-mcp setup --client codex --admin-url postgres://postgres:postgres@localhost:5432/postgres
```

From GitHub without cloning first:

```bash
cargo install --git https://github.com/LVTD-LLC/pgsandbox-mcp --tag v0.1.1
pgsandbox-mcp setup --client codex --admin-url postgres://postgres:postgres@localhost:5432/postgres
```

## Update

The installed CLI binary is the MCP server process that clients launch. Updating
the CLI and restarting the MCP client updates the server. Rerun `setup` when the
binary path, admin URL, selected client, or scope changes.

Homebrew can only upgrade after a newer GitHub release exists and the
`LVTD-LLC/homebrew-tap` formula has been updated. If `brew upgrade
LVTD-LLC/tap/pgsandbox-mcp` says the current version is already installed, the
tap does not have a newer version yet.

With Homebrew:

```bash
brew update
brew upgrade LVTD-LLC/tap/pgsandbox-mcp
pgsandbox-mcp --version
pgsandbox-mcp setup --client codex --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
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
pgsandbox-mcp setup --client codex --command /opt/homebrew/bin/pgsandbox-mcp --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
```

With the GitHub install script:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | sh
pgsandbox-mcp --version
pgsandbox-mcp setup --client codex --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
pgsandbox-mcp doctor
```

For a custom install directory, reinstall there and keep the MCP config pointed
at the same binary:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | PGSANDBOX_INSTALL_DIR=/usr/local/bin sh
pgsandbox-mcp setup --client codex --command /usr/local/bin/pgsandbox-mcp --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
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
pgsandbox-mcp setup --client codex --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
pgsandbox-mcp setup --client cursor --scope project --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
pgsandbox-mcp setup --client vscode --scope project --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
pgsandbox-mcp setup --client claude-desktop --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
pgsandbox-mcp setup --client all --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
```

## Verify

```bash
pgsandbox-mcp doctor
pgsandbox-mcp smoke-test
```

Then restart your MCP client and ask it to create a disposable Postgres sandbox.
