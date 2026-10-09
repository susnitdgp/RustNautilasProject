#!/usr/bin/env bash
# enable-ipv6.sh v1.0.0
# Undoes disable-ipv6.sh: removes the persistent setting and turns IPv6 back on now.
# Run it while no trading program is running.
set -euo pipefail

CONF=/etc/sysctl.d/99-disable-ipv6.conf

if pgrep -f "kite-node native-sats-live" >/dev/null; then
  echo "A live SATS run is active. Stop it (Ctrl+C) before changing the network." >&2
  exit 1
fi

if [[ -f "$CONF" ]]; then
  echo "Removing $CONF"
  sudo rm -f "$CONF"
fi

echo "Turning IPv6 back on"
sudo sysctl -w net.ipv6.conf.all.disable_ipv6=0 \
               net.ipv6.conf.default.disable_ipv6=0 \
               net.ipv6.conf.lo.disable_ipv6=0 >/dev/null
sudo sysctl --system >/dev/null

echo "Done (the IPv6 address may take a few seconds to come back). Verifying:"
sleep 5
exec "$(dirname "$(readlink -f "$0")")/check-ipv6.sh"
