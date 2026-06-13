#!/bin/sh
set -eu

version=$(grep '^version = ' Cargo.toml | head -n 1 | sed 's/version = "\(.*\)"/\1/')
archive="dist/pgsandbox-mcp-${version}.tar.gz"

mkdir -p dist
cp target/release/pgsandbox-mcp dist/pgsandbox-mcp
tar -czf "$archive" -C dist pgsandbox-mcp

if command -v shasum >/dev/null 2>&1; then
  sha256=$(shasum -a 256 "$archive" | awk '{print $1}')
else
  sha256=$(sha256sum "$archive" | awk '{print $1}')
fi

printf 'archive: %s\n' "$archive"
printf 'sha256:  %s\n' "$sha256"
printf 'tap:     LVTD-LLC/homebrew-tap (brew tap name: LVTD-LLC/tap)\n'
printf 'formula: Formula/pgsandbox-mcp.rb\n'
printf 'install: brew install LVTD-LLC/tap/pgsandbox-mcp\n'
