#!/usr/bin/env bash
# Session Signals — guest-side bridge forwarder (NAT WSL / full VMs only).
# NOT needed with mirrored networking.
#
# Listens on the guest's 127.0.0.1:<PORT_LOCAL> and relays to the Windows host's
# portproxy at <GATEWAY>:<PORT_BRIDGE>. This lets the guest's settings.json use
# the stable, standard http://127.0.0.1:4317/hook URL while the moving host IP is
# resolved here at start-up (from the default route) — so a host reboot that
# changes the vEthernet IP only requires a forwarder restart, never a
# settings.json edit.
#
#   guest Claude Code -> 127.0.0.1:4317 (this) -> GATEWAY:4318 -> host portproxy -> 127.0.0.1:4317
#
# Requires socat:  sudo apt-get install -y socat
set -euo pipefail

PORT_LOCAL="${PORT_LOCAL:-4317}"    # what settings.json posts to (matches host app port)
PORT_BRIDGE="${PORT_BRIDGE:-4318}"  # the host portproxy's listen port
GATEWAY="${GATEWAY:-$(ip route show default 2>/dev/null | awk '/default/{print $3; exit}')}"

command -v socat >/dev/null 2>&1 || { echo "socat is required: sudo apt-get install -y socat" >&2; exit 1; }
[[ -n "$GATEWAY" ]] || { echo "Could not resolve host gateway; set GATEWAY=<windows-host-ip>." >&2; exit 1; }

echo "Forwarding 127.0.0.1:${PORT_LOCAL} -> ${GATEWAY}:${PORT_BRIDGE}"
exec socat TCP4-LISTEN:"${PORT_LOCAL}",bind=127.0.0.1,fork,reuseaddr TCP4:"${GATEWAY}":"${PORT_BRIDGE}"
