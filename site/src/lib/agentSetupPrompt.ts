export const agentSetupPrompt = `Install and configure PGSandbox MCP on this machine.

PGSandbox MCP is a local stdio MCP server for disposable Postgres databases. It
needs an existing Postgres admin connection that can create databases and roles.
It does not install Postgres and does not require Docker.

Do the following:
1. Detect my OS, shell, available package managers, and MCP client. Supported
   clients are codex, cursor, vscode, claude-desktop, and all. If you cannot
   infer the target MCP client, ask me which one to configure.
2. If PGSANDBOX_ADMIN_DATABASE_URL is not already set, ask me to provide the
   Postgres admin URL through the agent's secret input or an interactive shell
   prompt. If neither is available and I paste it in chat, treat it as sensitive
   and never repeat it except with the password masked.
3. Install pgsandbox-mcp. Prefer:
   brew install LVTD-LLC/tap/pgsandbox-mcp
   If Homebrew is unavailable, use:
   curl -fsSL https://raw.githubusercontent.com/LVTD-LLC/pgsandbox-mcp/main/scripts/install.sh | sh
   If the install script uses ~/.local/bin, make sure pgsandbox-mcp is available
   in the current shell PATH before continuing.
4. Run:
   pgsandbox-mcp --version
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
- Do not inline the full admin URL in commands, docs, git-tracked files, shell
  startup files, or summaries. Use "$PGSANDBOX_ADMIN_DATABASE_URL" in commands.
  The MCP setup command may write the admin URL only to the selected local MCP
  client config.
- Do not leave a smoke-test database behind. If cleanup fails, report the
  database id or name so I can delete it.`;
