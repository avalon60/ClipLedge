#!/bin/sh
# Author: Clive Bostock
# Date: 10-Sep-2026
# Purpose: Run integration tests with a disposable keyring, bus, and display.
# Usage: xvfb-run -a dbus-run-session --config-file=tests/session.conf -- sh tests/desktop-session.sh
set -eu
case "${DISPLAY:-}" in :0|:0.0|'') echo 'Use an isolated Xvfb display' >&2; exit 1;; esac
session_dir=$(mktemp -d)
trap 'test -z "${keyring_pid:-}" || kill "$keyring_pid" 2>/dev/null || true; rm -rf "$session_dir"' EXIT
export XDG_DATA_HOME="$session_dir/data" XDG_CONFIG_HOME="$session_dir/config" XDG_CACHE_HOME="$session_dir/cache" XDG_RUNTIME_DIR="$session_dir/runtime"
export GDK_BACKEND=x11 XDG_SESSION_TYPE=x11 GTK_USE_PORTAL=0 GDK_DEBUG=no-portals GIO_USE_VFS=local GTK_A11Y=none GSK_RENDERER=cairo CLIPLEDGE_TEST_ISOLATED=1
unset WAYLAND_DISPLAY GNOME_KEYRING_CONTROL SSH_AUTH_SOCK
mkdir -p "$XDG_DATA_HOME" "$XDG_CONFIG_HOME" "$XDG_CACHE_HOME"
mkdir -m 700 "$XDG_RUNTIME_DIR"
printf 'disposable-test-password' | gnome-keyring-daemon --unlock --foreground --components=secrets --control-directory="$XDG_RUNTIME_DIR/keyring" >"$session_dir/keyring.log" 2>&1 &
keyring_pid=$!
sleep 1
cargo test --locked --offline --test desktop_session -- --ignored --test-threads=1 --nocapture
