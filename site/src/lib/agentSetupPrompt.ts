export const agentSetupPrompt = `Install and configure PGSandbox on this machine.

PGSandbox is a local CLI and stdio MCP server for disposable Postgres
databases. It uses a PGSandbox-managed local Postgres cluster by default. It requires local
Postgres server binaries such as initdb, pg_ctl, and postgres on PATH, but it
does not use Docker or touch any existing Postgres service on port 5432.

Do the following:
1. Detect my OS, shell, available package managers, and MCP client. Supported
   clients are codex, cursor, vscode, claude-desktop, and all. If this session
   is clearly running inside one supported MCP client, configure that client
   without asking. If several clients are installed, prefer the active client and
   ask only if you cannot infer where config should be written.
2. Install pgsandbox. The current Homebrew formula is macOS ARM64 only;
   on that platform prefer:
   brew install LVTD-LLC/tap/pgsandbox
   Before a binary install, check release assets for a matching platform archive
   (OS, CPU, and Linux libc); installer support is not proof an asset exists.
   As of November 6, 2026, v0.5.0 has no Linux archive.
   If Homebrew is unavailable and a matching platform archive exists, use:
   curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox/main/scripts/install.sh | sh
   If the platform archive is missing, do not retry it or select another OS.
   With Git, a compatible Rust/C toolchain, pkg-config, and OpenSSL development
   headers available, build the pinned v0.5.0 source instead:
   cargo install --git https://github.com/LVTD-LLC/pgsandbox --rev 540cb5653460b345c3382d26465de54a8670666f --locked
   If build prerequisites are missing, report them before installing packages.
   Ensure the chosen binary directory (~/.local/bin for the script, usually
   ~/.cargo/bin for Cargo) is in the current shell PATH before continuing.
3. Run:
   pgsandbox --version
   If another pgsandbox appears earlier in PATH and is missing, broken, or a
   different version, use the absolute path to the healthy installed binary in
   the setup command with --command.
4. Verify the managed local runtime:
   pgsandbox local start
   pgsandbox doctor
   If initdb, pg_ctl, or postgres is missing, explain that local PostgreSQL
   server binaries must be installed. Do not start Docker, stop Docker
   containers, or bind localhost:5432.
5. Configure the MCP client without an admin URL unless I explicitly gave one:
   pgsandbox setup --client <client>
   Use --scope project for Cursor or VS Code only if I ask for project-local
   config. Otherwise use the default user scope.
6. Verify configuration and Postgres connectivity:
   pgsandbox doctor
   If this fails, explain whether the CLI, local Postgres runtime, MCP config,
   or explicit external Postgres connection failed.
7. Run the disposable end-to-end check:
   pgsandbox smoke-test
   This should create, query, and delete a sandbox database.
8. If the active agent prefers CLI commands instead of MCP tools, verify direct
   CLI access with:
   pgsandbox create-database --name-hint cli-check --ttl-minutes 10
   pgsandbox run-sql --database-id <created-id> --sql "select 1" --readonly
   pgsandbox delete-database --database-id <created-id>
9. Tell me exactly which MCP client config was updated and that I need to restart
   the MCP client. After restart, help me verify that the pgsandbox server is
   available.

Constraints:
- Do not run Docker commands, stop Docker containers, bind localhost:5432, or
  mutate an existing developer database.
- Use the managed local cluster by default. Use PGSANDBOX_ADMIN_DATABASE_URL,
  PGSANDBOX_CONFIG, or --admin-url only when I explicitly ask for an external
  profile.
- Do not inline the full admin URL in commands, docs, git-tracked files, shell
  startup files, or summaries. Local runtime output should mask the password and
  point to ~/.pgsandbox/local-postgres.json for the full private URL.
- Do not leave a smoke-test database behind. If cleanup fails, report the
  database id or name so I can delete it.`;
