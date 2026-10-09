#!/usr/bin/env bash
# Rebuild, restart the daemon from target/debug, show the window.
# Usage: scripts/dev.sh            (debug build, info logs in ./dev.log)
#        scripts/dev.sh --no-show  (restart only)
set -euo pipefail
ROOT="$(cd "$(dirname "$0")/.." && pwd)"
BIN="$ROOT/target/debug/parsec"

cargo build --manifest-path "$ROOT/Cargo.toml"
pkill -x parsec 2>/dev/null || true
sleep 0.3
RUST_LOG="${RUST_LOG:-parsec=info}" nohup "$BIN" --background >"$ROOT/dev.log" 2>&1 &
sleep 0.8
if [[ "${1:-}" != "--no-show" ]]; then
  "$BIN" toggle
fi
echo "daemon restarted (log: dev.log). toggle with: $BIN toggle"
