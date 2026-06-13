#!/bin/sh
set -eu

version=$(grep '^version = ' Cargo.toml | head -n 1 | sed 's/version = "\(.*\)"/\1/')
target="${1:-$(rustc -vV | sed -n 's/^host: //p')}"
target_dir="${CARGO_TARGET_DIR:-target}"
binary="$target_dir/$target/release/pgsandbox-mcp"

if [ ! -f "$binary" ]; then
  binary="$target_dir/release/pgsandbox-mcp"
fi

[ -f "$binary" ] || {
  printf 'error: release binary not found. Run cargo build --release first.\n' >&2
  exit 1
}

archive_name="pgsandbox-mcp-${version}-${target}.tar.gz"
archive="dist/${archive_name}"
checksums="dist/pgsandbox-mcp-${version}-checksums.txt"
staging="$(mktemp -d 2>/dev/null || mktemp -d -t pgsandbox-release)"
trap 'rm -rf "$staging"' EXIT INT HUP TERM

mkdir -p dist
cp "$binary" "$staging/pgsandbox-mcp"
chmod 0755 "$staging/pgsandbox-mcp"
tar -czf "$archive" -C "$staging" pgsandbox-mcp

if command -v shasum >/dev/null 2>&1; then
  sha256=$(shasum -a 256 "$archive" | awk '{print $1}')
else
  sha256=$(sha256sum "$archive" | awk '{print $1}')
fi

if [ -f "$checksums" ]; then
  awk -v name="$archive_name" '$2 != name' "$checksums" > "$checksums.tmp" || true
  mv "$checksums.tmp" "$checksums"
fi
printf '%s  %s\n' "$sha256" "$archive_name" >> "$checksums"

printf 'archive:   %s\n' "$archive"
printf 'sha256:    %s\n' "$sha256"
printf 'checksums: %s\n' "$checksums"
