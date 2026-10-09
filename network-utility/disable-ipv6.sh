#!/usr/bin/env bash
# disable-ipv6.sh v1.0.1
# Turns IPv6 off on this box (now and after reboot) so every outbound
# connection - including Kite order calls - uses the registered static IPv4.
# Safe for this host: SSH and Redis also listen on IPv4.
# Run it while no trading program is running. Undo with enable-ipv6.sh.
set -euo pipefail

CONF=/etc/sysctl.d/99-disable-ipv6.conf

if pgrep -f "kite-node native-sats-live" >/dev/null; then
  echo "A live SATS run is active. Stop it (Ctrl+C) before changing the network." >&2
  exit 1
fi

echo "Writing $CONF"
sudo tee "$CONF" >/dev/null <<'EOF'
# Written by RustNautilasProject/network-utility/disable-ipv6.sh
net.ipv6.conf.all.disable_ipv6 = 1
net.ipv6.conf.default.disable_ipv6 = 1
net.ipv6.conf.lo.disable_ipv6 = 1
EOF

echo "Applying sysctl settings"
sudo sysctl --system >/dev/null

echo "Done. Verifying:"
exec "$(dirname "$(readlink -f "$0")")/check-ipv6.sh"
