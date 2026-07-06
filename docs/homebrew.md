# Homebrew Packaging

The target user flow is:

```bash
brew install LVTD-LLC/tap/pgsandbox-mcp
pgsandbox-mcp setup --client codex
pgsandbox-mcp doctor
```

## Recommended Release Shape

Use Homebrew as a thin installer for a versioned GitHub release artifact:

1. Build the Rust release binary.
2. Create a GitHub release tarball that contains the executable `pgsandbox-mcp` binary.
3. Update `Formula/pgsandbox-mcp.rb` in [LVTD-LLC/homebrew-tap](https://github.com/LVTD-LLC/homebrew-tap) with the release URL and SHA256.

This avoids asking users to install Node, npm, or a package manager runtime for a local MCP server.
The formula installs only `pgsandbox-mcp`; users still need local PostgreSQL
server binaries such as `initdb`, `pg_ctl`, and `postgres` for the managed local
runtime. PGSandbox checks `PATH` plus common Homebrew and Postgres.app install
locations, so keg-only installs from `postgresql@18` through `postgresql@13`
can still work without a shell-specific PATH edit when their server binaries
are present.

## Formula Template

Place this in [LVTD-LLC/homebrew-tap](https://github.com/LVTD-LLC/homebrew-tap) at `Formula/pgsandbox-mcp.rb`:

```ruby
class PgsandboxMcp < Formula
  desc "MCP server for disposable Postgres experimentation databases"
  homepage "https://github.com/LVTD-LLC/pgsandbox-mcp"
  url "https://github.com/LVTD-LLC/pgsandbox-mcp/releases/download/v0.1.3/pgsandbox-mcp-0.1.3.tar.gz"
  sha256 "REPLACE_WITH_RELEASE_TARBALL_SHA256"
  license "MIT"

  def install
    bin.install "pgsandbox-mcp"
  end

  test do
    assert_match version.to_s, shell_output("#{bin}/pgsandbox-mcp --version")
  end
end
```

## Release Checklist

```bash
cargo test
npm run package:homebrew
npm run package:release
```

The Homebrew package command prints the release archive and SHA256 for the tap
formula. The release package command creates a platform-specific archive named
`pgsandbox-mcp-<version>-<target>.tar.gz` plus
`pgsandbox-mcp-<version>-checksums.txt` for the GitHub install script. Upload
the archives from `dist/` before publishing the GitHub release.

The published release starts the `Update Homebrew tap` workflow, which opens a
PR in `LVTD-LLC/homebrew-tap` updating `Formula/pgsandbox-mcp.rb` to the
versioned release URL and computed SHA256.

Homebrew users cannot receive a new version until that tap PR is merged. If
`brew upgrade LVTD-LLC/tap/pgsandbox-mcp` reports that the installed version is
already current, the tap formula still points at that version.

The workflow requires a `HOMEBREW_TAP_PAT` repository secret in
`LVTD-LLC/pgsandbox-mcp`. Use a fine-grained token with `Contents: Read and
write` and `Pull requests: Read and write` access to `LVTD-LLC/homebrew-tap`, or
an equivalent classic PAT.

After the Homebrew tap PR merges, verify from the tap checkout:

```bash
brew install --build-from-source Formula/pgsandbox-mcp.rb
pgsandbox-mcp --version
pgsandbox-mcp doctor
```

## Client Setup After Brew Install

The formula should only install the CLI. Client registration remains explicit:

```bash
pgsandbox-mcp setup --client codex
pgsandbox-mcp setup --client cursor --scope project
pgsandbox-mcp setup --client vscode --scope project
pgsandbox-mcp setup --client claude-desktop
```

Use `--admin-url "$PGSANDBOX_ADMIN_DATABASE_URL"` only when intentionally
configuring an external Postgres profile instead of the managed local default.
