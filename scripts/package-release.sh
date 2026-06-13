#!/bin/sh
set -eu

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

version=$(grep '^version = ' Cargo.toml | head -n 1 | sed 's/version = "\(.*\)"/\1/')
host_target="$(rustc -vV | sed -n 's/^host: //p')"
explicit_target=false
if [ "$#" -gt 0 ]; then
  target="$1"
  explicit_target=true
elif [ -n "${CARGO_BUILD_TARGET:-}" ]; then
  target="$CARGO_BUILD_TARGET"
  explicit_target=true
else
  target="$host_target"
fi
target_dir="${CARGO_TARGET_DIR:-target}"
binary="$target_dir/$target/release/pgsandbox-mcp"

if [ ! -f "$binary" ]; then
  if [ "$explicit_target" = "false" ] && [ "$target" = "$host_target" ] && [ -f "$target_dir/release/pgsandbox-mcp" ]; then
    binary="$target_dir/release/pgsandbox-mcp"
  else
    die "release binary not found for target $target. Run cargo build --release --target $target first."
  fi
fi

[ -f "$binary" ] || die "release binary not found. Run cargo build --release first."

archive_name="pgsandbox-mcp-${version}-${target}.tar.gz"
archive="dist/${archive_name}"
checksums="dist/pgsandbox-mcp-${version}-checksums.txt"
checksums_tmp="dist/.pgsandbox-mcp-${version}-checksums.$$"
staging="$(mktemp -d 2>/dev/null || mktemp -d -t pgsandbox-release)"
trap 'rm -rf "$staging" "$checksums_tmp"' EXIT INT HUP TERM

mkdir -p dist
cp "$binary" "$staging/pgsandbox-mcp"
chmod 0755 "$staging/pgsandbox-mcp"
tar -czf "$archive" -C "$staging" pgsandbox-mcp

if command -v shasum >/dev/null 2>&1; then
  sha_output=$(shasum -a 256 "$archive") || die "failed to hash $archive with shasum"
elif command -v sha256sum >/dev/null 2>&1; then
  sha_output=$(sha256sum "$archive") || die "failed to hash $archive with sha256sum"
else
  die "shasum or sha256sum is required"
fi
sha256=$(printf '%s\n' "$sha_output" | awk '{print $1}')
[ -n "$sha256" ] || die "could not parse SHA-256 for $archive"

if [ -f "$checksums" ]; then
  awk -v name="$archive_name" '$2 != name' "$checksums" > "$checksums_tmp"
else
  : > "$checksums_tmp"
fi
printf '%s  %s\n' "$sha256" "$archive_name" >> "$checksums_tmp"
mv "$checksums_tmp" "$checksums"

printf 'archive:   %s\n' "$archive"
printf 'sha256:    %s\n' "$sha256"
printf 'checksums: %s\n' "$checksums"
