#!/bin/sh
set -eu

BINARY_NAMES="pgsandbox pgsandbox-mcp"
SERVER_NAME="${PGSANDBOX_MCP_SERVER_NAME:-pgsandbox}"
COMMAND_NAME="${PGSANDBOX_UNINSTALL_COMMAND_NAME:-scripts/uninstall.sh}"
DRY_RUN=0
ASSUME_YES=0
REMOVE_BINARIES=1
REMOVE_CLIENT_CONFIG=1
REMOVE_STATE=1
STOP_LOCAL=1
SEEN_BINARY_PATHS="|"

say() {
  printf '%s\n' "$*"
}

warn() {
  printf 'warning: %s\n' "$*" >&2
}

die() {
  printf 'error: %s\n' "$*" >&2
  exit 1
}

have() {
  command -v "$1" >/dev/null 2>&1
}

usage() {
  cat <<EOF
Usage: $COMMAND_NAME [options]

Remove local PGSandbox MCP installs and reset local PGSandbox state.

Options:
  -y, --yes             Do not prompt before deleting files or uninstalling packages
  --dry-run             Print actions without changing files or uninstalling packages
  --server-name <name>  MCP server entry to remove from client configs (default: pgsandbox)
  --keep-binaries       Do not uninstall packages or remove pgsandbox binaries
  --keep-client-config  Do not remove MCP client config entries
  --keep-state          Do not remove ~/.pgsandbox or socket state
  --no-stop             Do not try to stop the managed local Postgres runtime first
  -h, --help            Show this help

The command removes both CLI names: pgsandbox-mcp and pgsandbox.
It does not uninstall PostgreSQL itself.
EOF
}

while [ "$#" -gt 0 ]; do
  case "$1" in
    -y | --yes)
      ASSUME_YES=1
      ;;
    --dry-run)
      DRY_RUN=1
      ;;
    --server-name)
      shift
      [ "$#" -gt 0 ] || die "--server-name requires a value"
      SERVER_NAME="$1"
      ;;
    --keep-binaries)
      REMOVE_BINARIES=0
      ;;
    --keep-client-config)
      REMOVE_CLIENT_CONFIG=0
      ;;
    --keep-state)
      REMOVE_STATE=0
      ;;
    --no-stop)
      STOP_LOCAL=0
      ;;
    -h | --help | help)
      usage
      exit 0
      ;;
    *)
      die "unknown option: $1"
      ;;
  esac
  shift
done

display_command() {
  first=1
  for arg in "$@"; do
    if [ "$first" -eq 1 ]; then
      first=0
    else
      printf ' '
    fi
    printf "'%s'" "$arg"
  done
}

run_cmd() {
  if [ "$DRY_RUN" -eq 1 ]; then
    printf '+ '
    display_command "$@"
    printf '\n'
    return 0
  fi

  if ! "$@"; then
    printf 'warning: command failed: ' >&2
    display_command "$@" >&2
    printf '\n' >&2
  fi
}

remove_file_if_exists() {
  path="$1"
  if [ -f "$path" ] || [ -L "$path" ]; then
    run_cmd rm -f "$path"
  fi
}

remove_dir_if_exists() {
  path="$1"
  if [ -d "$path" ] || [ -L "$path" ]; then
    run_cmd rm -rf "$path"
  fi
}

confirm_destructive_run() {
  [ "$DRY_RUN" -eq 0 ] || return 0
  [ "$ASSUME_YES" -eq 0 ] || return 0

  say "This will remove PGSandbox binaries, MCP config entry '$SERVER_NAME', and local state."
  say "It will also try the short CLI/package name 'pgsandbox'."
  printf 'Continue? [y/N] '
  if ! read -r answer; then
    exit 1
  fi
  case "$answer" in
    y | Y | yes | YES)
      ;;
    *)
      say "Aborted."
      exit 1
      ;;
  esac
}

stop_local_runtime() {
  [ "$STOP_LOCAL" -eq 1 ] || return 0

  for binary in $BINARY_NAMES; do
    if have "$binary"; then
      if [ "$DRY_RUN" -eq 1 ]; then
        run_cmd "$binary" local stop
      else
        "$binary" local stop >/dev/null 2>&1 || true
      fi
      return 0
    fi
  done
}

homebrew_formula_installed() {
  formula="$1"
  brew list --formula "$formula" >/dev/null 2>&1
}

remove_homebrew_installs() {
  [ "$REMOVE_BINARIES" -eq 1 ] || return 0
  have brew || return 0

  for formula in pgsandbox-mcp pgsandbox; do
    if homebrew_formula_installed "$formula"; then
      run_cmd brew uninstall "$formula"
    fi
  done
}

cargo_package_installed() {
  package="$1"
  cargo install --list 2>/dev/null \
    | sed -n 's/^\([^ ][^ ]*\) v.*/\1/p' \
    | grep -Fx "$package" >/dev/null 2>&1
}

remove_cargo_installs() {
  [ "$REMOVE_BINARIES" -eq 1 ] || return 0
  have cargo || return 0

  for package in $BINARY_NAMES; do
    if cargo_package_installed "$package"; then
      run_cmd cargo uninstall "$package"
    fi
  done
}

npm_package_installed() {
  package="$1"
  npm list -g "$package" --depth=0 >/dev/null 2>&1
}

remove_npm_installs() {
  [ "$REMOVE_BINARIES" -eq 1 ] || return 0
  have npm || return 0

  for package in $BINARY_NAMES; do
    if npm_package_installed "$package"; then
      run_cmd npm uninstall -g "$package"
    fi
  done
}

remove_binary_path() {
  path="$1"
  base="$(basename "$path")"
  case "$base" in
    pgsandbox-mcp | pgsandbox)
      case "$SEEN_BINARY_PATHS" in
        *"|$path|"*)
          return 0
          ;;
      esac
      SEEN_BINARY_PATHS="${SEEN_BINARY_PATHS}${path}|"
      remove_file_if_exists "$path"
      ;;
  esac
}

remove_direct_binaries() {
  [ "$REMOVE_BINARIES" -eq 1 ] || return 0

  if [ -n "${PGSANDBOX_CURRENT_EXE:-}" ]; then
    remove_binary_path "$PGSANDBOX_CURRENT_EXE"
  fi

  for binary in $BINARY_NAMES; do
    if have which; then
      old_ifs="$IFS"
      IFS='
'
      for path in $(which -a "$binary" 2>/dev/null || true); do
        remove_binary_path "$path"
      done
      IFS="$old_ifs"
    fi
  done

  for dir in \
    "${PGSANDBOX_INSTALL_DIR:-}" \
    /usr/local/bin \
    /opt/homebrew/bin; do
    [ -n "$dir" ] || continue
    for binary in $BINARY_NAMES; do
      remove_binary_path "$dir/$binary"
    done
  done

  if [ -n "${HOME:-}" ]; then
    for dir in "$HOME/.local/bin" "$HOME/.cargo/bin"; do
      for binary in $BINARY_NAMES; do
        remove_binary_path "$dir/$binary"
      done
    done
  fi
}

remove_state() {
  [ "$REMOVE_STATE" -eq 1 ] || return 0

  if [ -n "${PGSANDBOX_HOME:-}" ]; then
    remove_dir_if_exists "$PGSANDBOX_HOME"
  elif [ -n "${HOME:-}" ]; then
    remove_dir_if_exists "$HOME/.pgsandbox"
  else
    warn "HOME is not set; skipping default ~/.pgsandbox removal"
  fi
  remove_dir_if_exists /tmp/pgsandbox-sockets
}

cleanup_codex_config() {
  path="$1"
  server_name="$2"
  [ -f "$path" ] || return 0

  CONFIG_PATH="$path" SERVER_NAME="$server_name" DRY_RUN="$DRY_RUN" python3 - <<'PY'
import os
import sys

path = os.environ["CONFIG_PATH"]
server = os.environ["SERVER_NAME"]
dry_run = os.environ["DRY_RUN"] == "1"

def toml_key(value: str) -> str:
    if all(character.isascii() and (character.isalnum() or character in "_-") for character in value):
        return value
    return '"' + value.replace("\\", "\\\\").replace('"', '\\"') + '"'

headers = {
    f"[mcp_servers.{server}]",
    f"[mcp_servers.{toml_key(server)}]",
}

try:
    with open(path, "r", encoding="utf-8") as handle:
        lines = handle.readlines()
except OSError as error:
    print(f"warning: failed to read {path}: {error}", file=sys.stderr)
    raise SystemExit(0)

output = []
skipping = False
removed = False
for line in lines:
    stripped = line.strip()
    if not skipping and stripped in headers:
        skipping = True
        removed = True
        continue
    if skipping and stripped.startswith("["):
        skipping = False
    if not skipping:
        output.append(line)

if not removed:
    raise SystemExit(0)

if dry_run:
    print(f"+ remove mcp_servers.{server} from {path}")
else:
    with open(path, "w", encoding="utf-8") as handle:
        handle.writelines(output)
    print(f"Removed mcp_servers.{server} from {path}")
PY
}

cleanup_json_config() {
  path="$1"
  root_key="$2"
  server_name="$3"
  [ -f "$path" ] || return 0

  CONFIG_PATH="$path" ROOT_KEY="$root_key" SERVER_NAME="$server_name" DRY_RUN="$DRY_RUN" python3 - <<'PY'
import json
import os
import sys

path = os.environ["CONFIG_PATH"]
root_key = os.environ["ROOT_KEY"]
server = os.environ["SERVER_NAME"]
dry_run = os.environ["DRY_RUN"] == "1"

try:
    with open(path, "r", encoding="utf-8") as handle:
        data = json.load(handle)
except json.JSONDecodeError as error:
    print(f"warning: failed to parse JSON config {path}: {error}", file=sys.stderr)
    raise SystemExit(0)
except OSError as error:
    print(f"warning: failed to read {path}: {error}", file=sys.stderr)
    raise SystemExit(0)

root = data.get(root_key)
if not isinstance(root, dict) or server not in root:
    raise SystemExit(0)

if dry_run:
    print(f"+ remove {root_key}.{server} from {path}")
else:
    del root[server]
    temporary_path = f"{path}.tmp"
    with open(temporary_path, "w", encoding="utf-8") as handle:
        json.dump(data, handle, indent=2)
        handle.write("\n")
    os.replace(temporary_path, path)
    print(f"Removed {root_key}.{server} from {path}")
PY
}

cleanup_configs_for_server() {
  server_name="$1"

  cleanup_codex_config "$PWD/.codex/config.toml" "$server_name"
  cleanup_json_config "$PWD/.cursor/mcp.json" mcpServers "$server_name"
  cleanup_json_config "$PWD/.vscode/mcp.json" servers "$server_name"

  if [ -n "${HOME:-}" ]; then
    cleanup_codex_config "$HOME/.codex/config.toml" "$server_name"
    cleanup_json_config "$HOME/.cursor/mcp.json" mcpServers "$server_name"
    cleanup_json_config "$HOME/Library/Application Support/Code/User/mcp.json" servers "$server_name"
    cleanup_json_config "$HOME/.config/Code/User/mcp.json" servers "$server_name"
    cleanup_json_config "$HOME/Library/Application Support/Claude/claude_desktop_config.json" mcpServers "$server_name"
    cleanup_json_config "$HOME/.config/Claude/claude_desktop_config.json" mcpServers "$server_name"
    cleanup_json_config "$HOME/AppData/Roaming/Code/User/mcp.json" servers "$server_name"
    cleanup_json_config "$HOME/AppData/Roaming/Claude/claude_desktop_config.json" mcpServers "$server_name"
  fi

  if [ -n "${APPDATA:-}" ]; then
    cleanup_json_config "$APPDATA/Code/User/mcp.json" servers "$server_name"
    cleanup_json_config "$APPDATA/Claude/claude_desktop_config.json" mcpServers "$server_name"
  fi
}

cleanup_client_configs() {
  [ "$REMOVE_CLIENT_CONFIG" -eq 1 ] || return 0

  if ! have python3; then
    warn "python3 not found; skipping MCP client config cleanup"
    return 0
  fi

  cleanup_configs_for_server "$SERVER_NAME"
  if [ "$SERVER_NAME" != "pgsandbox" ]; then
    cleanup_configs_for_server "pgsandbox"
  fi
  if [ "$SERVER_NAME" != "pgsandbox-mcp" ]; then
    cleanup_configs_for_server "pgsandbox-mcp"
  fi
}

confirm_destructive_run
stop_local_runtime
cleanup_client_configs
remove_homebrew_installs
remove_cargo_installs
remove_npm_installs
remove_direct_binaries
remove_state

if have hash; then
  hash -r 2>/dev/null || true
fi

if [ "$DRY_RUN" -eq 1 ]; then
  say "Dry run complete."
else
  say "PGSandbox uninstall cleanup complete."
  say "Restart any MCP clients that previously loaded the pgsandbox server."
fi
