#!/usr/bin/env bash
# M0 spike: try GNOME's built-in RDP server in extend mode.
#
#   scripts/m0-rdp-extend.sh start <tablet-ip>   back up settings, enable RDP in extend mode
#   scripts/m0-rdp-extend.sh status              show whether the test is running
#   scripts/m0-rdp-extend.sh stop                disable RDP and restore everything
#
# Security while the test runs:
# - a random one-time password replaces your RDP credentials (restored on stop);
# - TLS with the certificate GNOME generated; compare the fingerprint on the tablet;
# - firewall rules let only <tablet-ip> reach the RDP port, and expire on
#   their own after FIREWALL_TIMEOUT even if `stop` never runs;
# - port negotiation is off, so the server cannot move to an unfiltered port.
# Changing the firewall asks for your password through polkit.

set -euo pipefail

SCHEMA=org.gnome.desktop.remote-desktop.rdp
STATE_DIR="${XDG_STATE_HOME:-$HOME/.local/state}/tabextend/m0-rdp"
BACKUP="$STATE_DIR/backup"
FIREWALL_TIMEOUT=2h

die() { echo "error: $*" >&2; exit 1; }

require() {
    local tool
    for tool in grdctl gsettings firewall-cmd; do
        command -v "$tool" >/dev/null || die "$tool not found"
    done
}

# Prints "user<TAB>password" of the current RDP credentials, or nothing.
current_credentials() {
    grdctl status --show-credentials 2>/dev/null | awk -F': ' '
        /^RDP:/   { rdp = 1; next }
        /^[A-Z]/  { rdp = 0 }
        rdp && $1 ~ /Username$/ { user = $2 }
        rdp && $1 ~ /Password$/ { pass = $2 }
        END { if (user != "" && user != "(empty)") printf "%s\t%s\n", user, pass }'
}

random_password() {
    # Read a fixed amount first: `tr < /dev/urandom | head` dies of SIGPIPE under pipefail.
    head -c 512 /dev/urandom | LC_ALL=C tr -dc 'A-Za-z0-9' | cut -c 1-20
}

lan_address() {
    ip -4 route get 1.1.1.1 2>/dev/null | awk '{for (i = 1; i < NF; i++) if ($i == "src") print $(i + 1)}'
}

backup_value() { sed -n "s/^$1=//p" "$BACKUP"; }

firewall_rules() {
    local client=$1 port=$2
    printf '%s\n' \
        "rule family=\"ipv4\" source NOT address=\"$client/32\" port port=\"$port\" protocol=\"tcp\" reject" \
        "rule family=\"ipv6\" port port=\"$port\" protocol=\"tcp\" reject"
}

start() {
    local client=${1:-}
    [[ $client =~ ^([0-9]{1,3}\.){3}[0-9]{1,3}$ ]] ||
        die "usage: $0 start <tablet-ipv4-address>"
    [[ -e "$BACKUP" ]] && die "a test is already running; run '$0 stop' first"
    [[ $(firewall-cmd --state 2>/dev/null) == running ]] ||
        die "firewalld is not running, so the RDP port cannot be restricted to the tablet"

    mkdir -p "$STATE_DIR"
    chmod 700 "$STATE_DIR"
    local port
    port=$(gsettings get $SCHEMA port | awk '{print $NF}')

    # Back up everything we change, credentials included, readable only by you.
    (
        umask 077
        {
            echo "enable=$(gsettings get $SCHEMA enable)"
            echo "screen-share-mode=$(gsettings get $SCHEMA screen-share-mode)"
            echo "view-only=$(gsettings get $SCHEMA view-only)"
            echo "negotiate-port=$(gsettings get $SCHEMA negotiate-port)"
            echo "unit-active=$(systemctl --user is-active gnome-remote-desktop.service || true)"
            echo "client=$client"
            echo "port=$port"
        } > "$BACKUP"
        current_credentials > "$STATE_DIR/credentials"
    )
    trap 'echo "start failed; rolling back" >&2; stop' ERR

    # Firewall first, so the server is never reachable unfiltered.
    local rule
    while IFS= read -r rule; do
        firewall-cmd --quiet --add-rich-rule="$rule" --timeout="$FIREWALL_TIMEOUT"
    done < <(firewall_rules "$client" "$port")

    local user=tabextend-test pass
    pass=$(random_password)
    [[ ${#pass} -eq 20 ]] || false

    grdctl rdp set-credentials "$user" "$pass"
    grdctl rdp disable-view-only
    grdctl rdp disable-port-negotiation
    gsettings set $SCHEMA screen-share-mode extend
    grdctl rdp enable
    systemctl --user start gnome-remote-desktop.service
    trap - ERR

    cat <<EOF

RDP extend test is running.

  Address   $(lan_address):$port   (reachable only from $client)
  Username  $user
  Password  $pass
  TLS fingerprint, check the tablet shows the same before accepting:
            $(grdctl status | awk -F': ' '/TLS fingerprint/ {print $2}')

The firewall rules expire after $FIREWALL_TIMEOUT. When finished: $0 stop
EOF
}

stop() {
    trap - ERR
    [[ -e "$BACKUP" ]] || die "no test is running (no backup at $BACKUP)"
    local rule
    while IFS= read -r rule; do
        firewall-cmd --quiet --remove-rich-rule="$rule" 2>/dev/null || true
    done < <(firewall_rules "$(backup_value client)" "$(backup_value port)")

    grdctl rdp disable
    gsettings set $SCHEMA screen-share-mode "$(backup_value screen-share-mode)"
    gsettings set $SCHEMA view-only "$(backup_value view-only)"
    gsettings set $SCHEMA negotiate-port "$(backup_value negotiate-port)"

    if [[ -s "$STATE_DIR/credentials" ]]; then
        local user pass
        IFS=$'\t' read -r user pass < "$STATE_DIR/credentials"
        grdctl rdp set-credentials "$user" "$pass"
    else
        grdctl rdp clear-credentials
    fi

    [[ $(backup_value enable) == true ]] && grdctl rdp enable
    [[ $(backup_value unit-active) == active ]] ||
        systemctl --user stop gnome-remote-desktop.service

    rm -f "$BACKUP" "$STATE_DIR/credentials"
    echo "RDP test stopped; previous settings restored."
}

status() {
    if [[ -e "$BACKUP" ]]; then
        echo "RDP extend test is running (client $(backup_value client))."
    else
        echo "No RDP test running."
    fi
    grdctl status | sed -n '/^RDP:/,/^VNC:/p' | sed '$d'
    echo "	Screen share mode: $(gsettings get $SCHEMA screen-share-mode)"
    echo "Firewall rich rules:"
    firewall-cmd --list-rich-rules | sed 's/^/	/'
}

require
case "${1:-}" in
    start) start "${2:-}" ;;
    stop) stop ;;
    status) status ;;
    *) echo "usage: $0 start <tablet-ip> | stop | status" >&2; exit 2 ;;
esac
