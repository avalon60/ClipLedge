#!/bin/sh
# Author: Clive Bostock
# Date: 10-Sep-2026
# Purpose: Check single-instance lifecycle and fail-closed startup in isolation.
# Usage: xvfb-run -a dbus-run-session --config-file=tests/session.conf -- sh tests/smoke.sh
set -eu
case "${DISPLAY:-}" in :0|:0.0|'') echo 'Use an isolated Xvfb display' >&2; exit 1;; esac
sandbox_dir=$(mktemp -d)
trap 'test -z "${app_pid:-}" || kill "$app_pid" 2>/dev/null || true; rm -rf "$sandbox_dir"' EXIT
export XDG_DATA_HOME="$sandbox_dir/data" XDG_CONFIG_HOME="$sandbox_dir/config"
export GDK_BACKEND=x11 XDG_SESSION_TYPE=x11 GTK_USE_PORTAL=0 GDK_DEBUG=no-portals GIO_USE_VFS=local GTK_A11Y=none GSK_RENDERER=cairo
export XDG_RUNTIME_DIR="$sandbox_dir/runtime"
mkdir -m 700 "$XDG_RUNTIME_DIR"
unset WAYLAND_DISPLAY
binary=${CLIPLEDGE_BINARY:-target/debug/clipledge}
timeout 15 "$binary" --status | grep -q 'running=false'
test ! -e "$XDG_DATA_HOME/clipledge/history.db"
"$binary" --background >"$sandbox_dir/output" 2>&1 &
app_pid=$!
sleep 1
awk '/^VmRSS:/ {print "Isolated idle RSS: " $2 " KiB"}' "/proc/$app_pid/status"
timeout 15 "$binary" --status | grep -q 'running=true'
timeout 15 "$binary" --pause
timeout 15 "$binary" --status | grep -q 'capture=false'
timeout 15 "$binary" --resume
timeout 15 "$binary" --show
timeout 15 "$binary" --hide
timeout 15 "$binary" --quit
wait "$app_pid"
app_pid=
test ! -e "$XDG_DATA_HOME/clipledge/history.db"
if grep -E 'CRITICAL|panicked|ERROR' "$sandbox_dir/output"; then exit 1; fi
printf 'Lifecycle and fail-closed smoke test passed.\n'
