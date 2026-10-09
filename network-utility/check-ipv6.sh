#!/usr/bin/env bash
# check-ipv6.sh v1.0.0
# Shows whether IPv6 is on or off and which public IP the box uses for
# outbound traffic (it must be the IP registered on the Kite developer console).
set -uo pipefail

EXPECTED_IP="13.206.181.202"

flag=$(cat /proc/sys/net/ipv6/conf/all/disable_ipv6 2>/dev/null || echo "?")
if [[ "$flag" == "1" ]]; then
  echo "IPv6:            DISABLED (disable_ipv6 = 1)"
else
  echo "IPv6:            ENABLED  (disable_ipv6 = $flag)"
fi

addrs=$(ip -6 addr show scope global 2>/dev/null | awk '/inet6/ {print $2}')
echo "IPv6 addresses:  ${addrs:-none}"

if [[ -f /etc/sysctl.d/99-disable-ipv6.conf ]]; then
  echo "After reboot:    stays DISABLED (/etc/sysctl.d/99-disable-ipv6.conf present)"
else
  echo "After reboot:    ENABLED (no /etc/sysctl.d/99-disable-ipv6.conf)"
fi

ip4=$(curl -4 -s -m 5 https://api.ipify.org || true)
echo "Public IPv4:     ${ip4:-unreachable}"
if [[ "$flag" != "1" ]]; then
  ip6=$(curl -6 -s -m 5 https://api64.ipify.org || true)
  echo "Public IPv6:     ${ip6:-unreachable}"
fi

# Which address a default (dual-stack) client like curl/python picks for Kite.
default_ip=$(curl -s -m 5 https://api64.ipify.org || true)
echo "Default route:   ${default_ip:-unreachable}  (what normal programs use)"

if [[ "$default_ip" == "$EXPECTED_IP" ]]; then
  echo "Kite static IP:  OK - outbound traffic uses $EXPECTED_IP"
else
  echo "Kite static IP:  WARNING - outbound traffic uses ${default_ip:-unknown}, not $EXPECTED_IP"
  echo "                 (kite-node >= 2.7.1 still sends orders over IPv4)"
fi
