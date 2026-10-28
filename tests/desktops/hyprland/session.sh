#!/bin/sh
# Inside the container: start Hyprland (nested in a headless labwc, as
# Hyprland has no headless mode), then run the test binary given as
# arguments against it.
set -eu
DBUS_SESSION_BUS_ADDRESS=$(dbus-daemon --session --fork --print-address)
export DBUS_SESSION_BUS_ADDRESS
pipewire >/tmp/pipewire.log 2>&1 &
wireplumber >/tmp/wireplumber.log 2>&1 &
pipewire-pulse >/tmp/pipewire-pulse.log 2>&1 &
labwc >/tmp/labwc.log 2>&1 &
for _ in $(seq 1 50); do [ -S "$XDG_RUNTIME_DIR/wayland-0" ] && break; sleep 0.2; done
WAYLAND_DISPLAY=wayland-0 Hyprland >/tmp/hyprland.log 2>&1 &
for _ in $(seq 1 75); do
    for sock in "$XDG_RUNTIME_DIR"/hypr/*/.socket.sock; do [ -S "$sock" ] && break 2; done
    sleep 0.2
done
sleep 1
HYPRLAND_INSTANCE_SIGNATURE=$(ls "$XDG_RUNTIME_DIR/hypr/")
# Hyprland's own socket: the one that is not labwc's wayland-0.
for sock in "$XDG_RUNTIME_DIR"/wayland-*; do
    name=$(basename "$sock")
    case "$name" in wayland-0 | *.lock) ;; *) WAYLAND_DISPLAY=$name ;; esac
done
export HYPRLAND_INSTANCE_SIGNATURE WAYLAND_DISPLAY
echo "desktop: $(hyprctl version | head -1)"
"$@"
