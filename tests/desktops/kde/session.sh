#!/bin/sh
# Inside the container: start a headless KWin with KDE's portal, then run
# the test binary given as arguments against it.
set -eu
DBUS_SESSION_BUS_ADDRESS=$(dbus-daemon --session --fork --print-address)
export DBUS_SESSION_BUS_ADDRESS
pipewire >/tmp/pipewire.log 2>&1 &
wireplumber >/tmp/wireplumber.log 2>&1 &
pipewire-pulse >/tmp/pipewire-pulse.log 2>&1 &
kwin_wayland --virtual --width 1920 --height 1080 >/tmp/kwin.log 2>&1 &
for _ in $(seq 1 75); do [ -S "$XDG_RUNTIME_DIR/wayland-0" ] && break; sleep 0.2; done
export WAYLAND_DISPLAY=wayland-0
dbus-update-activation-environment WAYLAND_DISPLAY XDG_CURRENT_DESKTOP QT_QPA_PLATFORM XDG_RUNTIME_DIR
# Nobody is there to click "Share": pre-approve host apps (empty app id)
# for remote desktop, as KDE's portal allows for headless machines.
gdbus call --session --dest org.freedesktop.impl.portal.PermissionStore \
    --object-path /org/freedesktop/impl/portal/PermissionStore \
    --method org.freedesktop.impl.portal.PermissionStore.SetPermission \
    kde-authorized true remote-desktop '' "['yes']" >/dev/null
/usr/libexec/xdg-desktop-portal-kde >/tmp/portal-kde.log 2>&1 &
sleep 1
/usr/libexec/xdg-desktop-portal >/tmp/portal.log 2>&1 &
sleep 2
echo "desktop: $(kwin_wayland --version)"
"$@" || { echo "--- portal-kde log:"; tail -20 /tmp/portal-kde.log; echo "--- kwin log:"; tail -10 /tmp/kwin.log; exit 1; }
