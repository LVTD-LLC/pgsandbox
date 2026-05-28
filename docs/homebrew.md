# Homebrew Packaging

The target user flow is:

```bash
brew tap LVTD-LLC/tap
brew install pgsandbox-mcp
pgsandbox-mcp setup --client codex --admin-url postgres://postgres:postgres@localhost:5432/postgres
```

## Recommended Release Shape

Use Homebrew as a thin installer for a versioned GitHub release artifact:

1. Build the TypeScript package.
2. Publish the npm package for `npm install -g` and `npx` users.
3. Create a GitHub release tarball that contains an executable `pgsandbox-mcp` Node entrypoint.
4. Update the Homebrew tap formula with the release URL and SHA256.

This avoids asking Homebrew to resolve npm dependencies during install.

## Formula Template

Place this in the tap repo at `Formula/pgsandbox-mcp.rb`:

```ruby
class PgsandboxMcp < Formula
  desc "MCP server for disposable Postgres experimentation databases"
  homepage "https://github.com/LVTD-LLC/pgsandbox-mcp"
  url "https://github.com/LVTD-LLC/pgsandbox-mcp/releases/download/v0.1.0/pgsandbox-mcp-0.1.0.tar.gz"
  sha256 "REPLACE_WITH_RELEASE_TARBALL_SHA256"
  license "MIT"

  depends_on "node"

  def install
    bin.install "pgsandbox-mcp", "pgsandbox-mcp.cjs"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/pgsandbox-mcp --version")
  end
end
```

## Release Checklist

```bash
npm ci
npm test
npm run package:homebrew
```

The package command prints the release archive and SHA256. Upload the archive from `dist/`, then update the formula URL, version, and SHA. Verify from the tap:

```bash
brew install --build-from-source Formula/pgsandbox-mcp.rb
pgsandbox-mcp --version
pgsandbox-mcp doctor
```

## Client Setup After Brew Install

The formula should only install the CLI. Client registration remains explicit:

```bash
pgsandbox-mcp setup --client codex --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
pgsandbox-mcp setup --client cursor --scope project --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
pgsandbox-mcp setup --client vscode --scope project --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
pgsandbox-mcp setup --client claude-desktop --admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"
```
