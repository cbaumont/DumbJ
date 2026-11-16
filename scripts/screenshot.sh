#!/usr/bin/env bash
# Takes a screenshot of DumbJ running under a headless sway compositor.
#
# Usage: scripts/screenshot.sh OUTPUT.png [PATH]
#   PATH   folder or file DumbJ opens
#
# Environment:
#   SCALE  output scale, e.g. 1.25 or 1.5 (default 1)
#   SIZE   output resolution in physical pixels (default 1600x1000)
#   KEYS   wtype arguments sent after start-up, e.g. '-M ctrl -k l -m ctrl -s 300'
#          (send them in one string: each wtype run installs its own keymap)
#   MID    shell command run after KEYS, e.g. to change a file on disk
#   KEYS2  wtype arguments sent after MID
#   X11    set to 1 to run DumbJ as an X11 client through sway's Xwayland
#   DUMBJ  binary to run (default target/debug/dumbj)
#
# Needs sway, grim and wtype (Fedora: dnf install sway grim wtype).
set -euo pipefail

output=${1:?usage: scripts/screenshot.sh OUTPUT.png [PATH]}
shift
root=$(cd "$(dirname "$0")/.." && pwd)
dumbj=${DUMBJ:-$root/target/debug/dumbj}
work=$(mktemp -d)
sway_pid=
app_pid=
cleanup() {
    [ -n "$app_pid" ] && kill "$app_pid" 2>/dev/null
    [ -n "$sway_pid" ] && kill "$sway_pid" 2>/dev/null
    rm -rf "$work"
}
trap cleanup EXIT

cat >"$work/sway.conf" <<CONF
output HEADLESS-1 resolution ${SIZE:-1600x1000} scale ${SCALE:-1}
default_border none
CONF

# A private runtime dir keeps this compositor apart from the desktop session.
export XDG_RUNTIME_DIR="$work/run"
mkdir -m 700 "$XDG_RUNTIME_DIR"
env -u WAYLAND_DISPLAY -u DISPLAY WLR_BACKENDS=headless WLR_LIBINPUT_NO_DEVICES=1 \
    sway -c "$work/sway.conf" >"$work/sway.log" 2>&1 &
sway_pid=$!
for _ in $(seq 50); do
    [ -S "$XDG_RUNTIME_DIR/wayland-1" ] && break
    sleep 0.1
done
export WAYLAND_DISPLAY=wayland-1

if [ "${X11:-0}" = 1 ]; then
    # sway starts Xwayland on demand and gives DISPLAY to the programs it launches.
    SWAYSOCK=$(echo "$XDG_RUNTIME_DIR"/sway-ipc.*.sock)
    export SWAYSOCK
    swaymsg exec -- env -u WAYLAND_DISPLAY "$(printf '%q ' "$dumbj" "$@")" >/dev/null
else
    env -u DISPLAY "$dumbj" "$@" &
    app_pid=$!
fi
sleep "${STARTUP_WAIT:-3}"
if [ -n "${KEYS:-}" ]; then
    # The first key after wtype installs its keymap is lost, so lead with a spare
    # Shift tap.
    # shellcheck disable=SC2086  # KEYS is a list of wtype arguments
    wtype -M shift -m shift -s 200 $KEYS
    sleep 0.5
fi
if [ -n "${MID:-}" ]; then
    eval "$MID"
    sleep 0.5
fi
if [ -n "${KEYS2:-}" ]; then
    # shellcheck disable=SC2086
    wtype -M shift -m shift -s 200 $KEYS2
    sleep 0.5
fi
grim "$output"
