# Install And Setup

PGSandbox is distributed as a native Rust binary. By default, it manages a local
Postgres cluster under `~/.pgsandbox/` and chooses a high local port such as
`127.0.0.1:65432`, leaving Docker or another developer database on `5432`
untouched.

The managed local runtime requires `initdb`, `pg_ctl`, and `postgres` on `PATH`.
The `clone_database` MCP tool additionally requires `pg_dump` and `pg_restore`
because it streams a source database dump into a new sandbox.

## Agent-Assisted Setup

Copy this prompt into your coding agent if you want it to install and configure
PGSandbox MCP for you:

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

## Homebrew

```bash
brew install LVTD-LLC/tap/pgsandbox-mcp
pgsandbox-mcp setup --client codex
```

This uses the [LVTD-LLC/homebrew-tap](https://github.com/LVTD-LLC/homebrew-tap) repository, which Homebrew addresses as `LVTD-LLC/tap`.

## GitHub Install Script

For users who do not use Homebrew:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | sh
pgsandbox-mcp setup --client codex
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
pgsandbox-mcp setup --client codex
```

From GitHub without cloning first:

```bash
cargo install --git https://github.com/LVTD-LLC/pgsandbox-mcp --tag v0.1.1
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
