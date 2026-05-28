# Install And Setup

## npm

```bash
npm install -g pgsandbox-mcp
pgsandbox-mcp setup --client codex --admin-url postgres://postgres:postgres@localhost:5432/postgres
```

## npx

```bash
npx -y pgsandbox-mcp setup --client codex --admin-url postgres://postgres:postgres@localhost:5432/postgres
```

For `npx`, the generated MCP config should usually be edited to pin a version or replaced after installing the package globally.

## Homebrew

```bash
brew tap LVTD-LLC/tap
brew install pgsandbox-mcp
pgsandbox-mcp setup --client codex --admin-url postgres://postgres:postgres@localhost:5432/postgres
```

## Verify

```bash
pgsandbox-mcp doctor
pgsandbox-mcp smoke-test
```

Then restart your MCP client and ask it to create a disposable Postgres sandbox.
