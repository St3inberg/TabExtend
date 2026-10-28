#!/bin/sh
# Inside the container: start a headless Sway session, then run the test
# binary given as arguments against it.
set -eu
export XDG_RUNTIME_DIR=/run/user/0
DBUS_SESSION_BUS_ADDRESS=$(dbus-daemon --session --fork --print-address)
export DBUS_SESSION_BUS_ADDRESS
# Sound: the tablet's speaker needs a running sound server.
pipewire >/tmp/pipewire.log 2>&1 &
wireplumber >/tmp/wireplumber.log 2>&1 &
pipewire-pulse >/tmp/pipewire-pulse.log 2>&1 &
sway >/tmp/sway.log 2>&1 &
for _ in $(seq 1 50); do
    for sock in "$XDG_RUNTIME_DIR"/sway-ipc.*; do [ -S "$sock" ] && break 2; done
    sleep 0.2
done
SWAYSOCK=$(ls "$XDG_RUNTIME_DIR"/sway-ipc.*)
export SWAYSOCK WAYLAND_DISPLAY=wayland-1
echo "desktop: $(sway --version)"
"$@"
