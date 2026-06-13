# PGSandbox MCP

Safe disposable Postgres databases for coding agents.

PGSandbox is a local MCP server that gives agents a narrow, tracked way to create, use, and clean up real Postgres databases. Agents could improvise this with `psql`, `createdb`, and shell scripts. PGSandbox exists so they do not have to improvise with admin credentials every time.

It works against Postgres you already control: a local install, a container-local Postgres, a VPS, or a private development database host. It does not install Postgres or require Docker.
Postgres URL `sslmode` settings are honored, so remote profiles can require TLS with `sslmode=require`.

## Why This Exists

Agents need real databases to validate migrations, reproduce backend bugs, test generated SQL, and build seeded demo states. Without a guardrail, the usual options are risky:

- hand an agent shared development credentials
- let it invent database create/drop commands in a shell
- keep stale test databases around after interrupted sessions
- skip database verification because setup is annoying

PGSandbox makes the safe path shorter:

- create one database and one scoped login role per task
- record every sandbox in metadata before it can be cleaned up
- run SQL through the sandbox role, not the admin connection
- cap TTLs and delete expired resources
- drop only databases PGSandbox created for the selected profile
- return bounded query results instead of dumping unbounded rows

The value is not that agents cannot use Postgres by themselves. The value is that database lifecycle becomes explicit, auditable, and disposable by default.

## Install

The intended local install is a native binary through Homebrew:

```bash
brew install LVTD-LLC/tap/pgsandbox-mcp
pgsandbox-mcp setup --client codex --admin-url postgres://postgres:postgres@localhost:5432/postgres
pgsandbox-mcp doctor
```

The Homebrew formula lives in [LVTD-LLC/homebrew-tap](https://github.com/LVTD-LLC/homebrew-tap). Homebrew exposes that repo as the `LVTD-LLC/tap` tap.

Restart the MCP client after setup. In Codex, run `/mcp` to verify the `pgsandbox` server is available.

If you do not use Homebrew, install the latest GitHub release binary with the
hosted installer:

```bash
curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | sh
pgsandbox-mcp setup --client codex --admin-url postgres://postgres:postgres@localhost:5432/postgres
pgsandbox-mcp doctor
```

The installer downloads a platform-specific release archive, verifies the
checksum when the release includes `pgsandbox-mcp-<version>-checksums.txt`, and
installs to `~/.local/bin` by default. Use `PGSANDBOX_VERSION=0.1.0` to pin a
release or `PGSANDBOX_INSTALL_DIR=/usr/local/bin` to choose a different install
directory.

For development from this repo:

```bash
cargo build
cargo run -- setup --client codex --admin-url postgres://postgres:postgres@localhost:5432/postgres
cargo run -- smoke-test --admin-url postgres://postgres:postgres@localhost:5432/postgres
```

Rust users can also install directly from GitHub:

```bash
cargo install --git https://github.com/LVTD-LLC/pgsandbox-mcp --tag v0.1.0
```

## MCP Client Setup

The setup command writes the right MCP config shape for each supported client:

```bash
pgsandbox-mcp setup --client codex --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
pgsandbox-mcp setup --client cursor --scope project --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
pgsandbox-mcp setup --client vscode --scope project --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
pgsandbox-mcp setup --client claude-desktop --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
pgsandbox-mcp setup --client all --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
```

Supported targets:

- Codex: `~/.codex/config.toml` or project `.codex/config.toml`
- Cursor: `~/.cursor/mcp.json` or project `.cursor/mcp.json`
- VS Code: user `mcp.json` or project `.vscode/mcp.json`
- Claude Desktop: `claude_desktop_config.json`

Use `--dry-run` to print the config without writing files. Passing `--admin-url` writes the admin database URL into the MCP client config so desktop clients do not depend on shell startup files.

## Configuration

The fastest setup is one admin connection string:

```bash
export PGSANDBOX_ADMIN_DATABASE_URL="postgres://postgres:postgres@localhost:5432/postgres"
pgsandbox-mcp
```

For multiple Postgres versions or hosts, use profiles:

```json
{
  "defaultProfile": "local-pg17",
  "profiles": [
    {
      "name": "local-pg17",
      "adminUrl": "postgres://postgres:postgres@localhost:5432/postgres",
      "databasePrefix": "pgsandbox",
      "defaultTtlMinutes": 240,
      "maxTtlMinutes": 1440
    },
    {
      "name": "local-pg16",
      "adminUrl": "postgres://postgres:postgres@localhost:5433/postgres"
    }
  ]
}
```

Then run:

```bash
export PGSANDBOX_CONFIG="./pgsandbox.config.json"
pgsandbox-mcp
```

## MCP Tools

V0 supports:

- `create_database`
- `delete_database`
- `get_connection_string`
- `run_sql`
- `describe_schema`
- `list_databases`
- `cleanup_expired`

See [docs/mcp-tools.md](docs/mcp-tools.md) for details.

## Local Shape

The service uses:

- Rust native binary
- `rmcp` stdio MCP server
- Postgres admin connection with permission to create databases and roles
- metadata table for ownership, TTL, encrypted sandbox credentials, and cleanup state
- optional Docker Compose only for local demo Postgres

Start with [docker-compose.example.yml](docker-compose.example.yml) only if you do not already have local Postgres running.

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
- User SQL runs through generated sandbox credentials.
- Sandbox role passwords are encrypted before being stored in metadata.
- Connection strings are returned only to the caller and are not logged in full.
- The service should run locally or on a private network, not as a public internet-exposed admin surface.

## Status

Early v0. Treat this as local/private infrastructure until the MCP surface and cleanup semantics have more mileage.
