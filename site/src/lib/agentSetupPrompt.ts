export const agentSetupPrompt = `Install and configure PGSandbox MCP on this machine.

PGSandbox MCP is a local stdio MCP server for disposable Postgres databases. It
uses a PG Sandbox-managed local Postgres cluster by default. It requires local
Postgres server binaries such as initdb, pg_ctl, and postgres on PATH, but it
does not use Docker or touch any existing Postgres service on port 5432.

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
   If initdb, pg_ctl, or postgres is missing, explain that local PostgreSQL
   server binaries must be installed. Do not start Docker, stop Docker
   containers, or bind localhost:5432.
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
