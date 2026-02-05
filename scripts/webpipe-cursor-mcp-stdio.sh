#!/usr/bin/env bash
set -euo pipefail

# Cursor MCP entrypoint for webpipe.
# Goal: "latest and greatest" from local source with minimal friction.
#
# Strategy:
# - Start fast (Cursor can time out on long compiles)
# - Rebuild in the background when possible
# - Keep stdout reserved for MCP frames (no build noise on stdout)

ROOT="/Users/arc/Documents/dev/webpipe"
TARGET_DIR="${WEBPIPE_CARGO_TARGET_DIR:-$ROOT/target-webpipe}"
PROFILE="${WEBPIPE_CURSOR_PROFILE:-debug}" # debug | release
LOG="${WEBPIPE_CURSOR_BUILD_LOG:-$HOME/Library/Logs/webpipe-cursor-build.log}"

cd "$ROOT"

export CARGO_TARGET_DIR="$TARGET_DIR"

mkdir -p "$(dirname "$LOG")" || true
# Keep the log “current” so stale build errors don’t confuse Cursor debugging.
: >"$LOG" || true

find_cargo() {
  if command -v cargo >/dev/null 2>&1; then
    command -v cargo
    return 0
  fi
  if [ -x "$HOME/.cargo/bin/cargo" ]; then
    echo "$HOME/.cargo/bin/cargo"
    return 0
  fi
  if [ -x "/Users/arc/.cargo/bin/cargo" ]; then
    echo "/Users/arc/.cargo/bin/cargo"
    return 0
  fi
  return 1
}

if ! CARGO_BIN="$(find_cargo)"; then
  echo "webpipe: cargo not found (PATH too minimal?)." >&2
  echo "Expected cargo at: \$HOME/.cargo/bin/cargo" >&2
  exit 127
fi

build_args=(build -p webpipe-mcp --bin webpipe)
bin_path="$CARGO_TARGET_DIR/debug/webpipe"
if [ "$PROFILE" = "release" ]; then
  build_args+=(--release)
  bin_path="$CARGO_TARGET_DIR/release/webpipe"
fi

# If we already have a binary, start immediately (don’t block MCP startup),
# and kick off a best-effort rebuild for the *next* restart.
if [ -x "$bin_path" ]; then
  ("$CARGO_BIN" "${build_args[@]}" >>"$LOG" 2>&1) || true
  exec "$bin_path" mcp-stdio
fi

# First run: no binary yet → build once in the foreground (log output to file).
if ! "$CARGO_BIN" "${build_args[@]}" >>"$LOG" 2>&1; then
  echo "webpipe build failed; see $LOG" >&2
  # Print the last chunk for quick diagnosis in Cursor UI.
  tail -n 80 "$LOG" >&2 || true
  exit 1
fi

exec "$bin_path" mcp-stdio

