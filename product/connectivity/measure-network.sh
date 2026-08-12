#!/usr/bin/env bash
# In-guest / chroot network measurement for D1 acceptance.
# Measure → record. No arbitrary sysctl tweaks.
set -euo pipefail

OUT="${1:-/var/lib/tuwaiq/connectivity/d1-network-measure.txt}"
mkdir -p "$(dirname "${OUT}")"
exec > >(tee "${OUT}") 2>&1

echo "=== TuwaiqOS D1 network measurement ==="
echo "timestamp_utc=$(date -u +%Y-%m-%dT%H:%M:%SZ)"
echo

echo "## interfaces"
ip -br link || true
ip -br addr || true
echo

echo "## NetworkManager"
systemctl is-active NetworkManager || true
nmcli -t general || true
nmcli -t device || true
nmcli -t connection show --active || true
echo

echo "## DHCP / addressing"
PRIMARY="$(nmcli -t -f DEVICE,STATE device 2>/dev/null | awk -F: '$2=="connected"{print $1; exit}')"
echo "primary=${PRIMARY}"
if [[ -n "${PRIMARY}" ]]; then
  nmcli device show "${PRIMARY}" || true
fi
echo

echo "## routes"
ip route || true
echo

echo "## DNS"
getent hosts example.com || true
getent hosts dns.google || true
if command -v dig >/dev/null; then
  echo "dig @1.1.1.1 example.com:"
  dig +time=3 +tries=1 @1.1.1.1 example.com +short || true
  echo "dig @10.0.2.3 example.com:"
  dig +time=2 +tries=1 @10.0.2.3 example.com +short || true
fi
resolvectl status 2>/dev/null | head -n 40 || true
cat /etc/resolv.conf 2>/dev/null || true
echo

echo "## gateway reachability / latency"
GW="$(ip route | awk '/default/{print $3; exit}')"
echo "gateway=${GW}"
if [[ -n "${GW}" ]]; then
  ping -c 5 -W 2 "${GW}" || true
fi
echo

echo "## external ICMP (may be filtered)"
ping -c 5 -W 2 1.1.1.1 || true
echo

echo "## HTTPS"
if command -v curl >/dev/null; then
  curl -w "http_code=%{http_code} time_total=%{time_total} size=%{size_download}\n" \
    -o /dev/null -sS --max-time 15 https://example.com || true
  # If system resolver failed, retry via Cloudflare DoH-less IP after forcing resolve
  if ! curl -fsS --max-time 5 -o /dev/null https://example.com; then
    echo "retry HTTPS with --resolve using 1.1.1.1 lookup"
    IP="$(dig +time=3 +tries=1 @1.1.1.1 example.com +short | head -n1 || true)"
    if [[ -n "${IP}" ]]; then
      curl -w "http_code=%{http_code} time_total=%{time_total} size=%{size_download}\n" \
        -o /dev/null -sS --max-time 15 --resolve "example.com:443:${IP}" https://example.com || true
    fi
  fi
else
  echo "curl missing"
fi
echo

echo "## listening ports (security audit)"
ss -tulpn || netstat -tulpn || true
echo

echo "## connectivity status json"
if [[ -f /var/lib/tuwaiq/connectivity/status.json ]]; then
  cat /var/lib/tuwaiq/connectivity/status.json
else
  /usr/libexec/tuwaiq/tuwaiq-connectivity-status.sh || true
  cat /var/lib/tuwaiq/connectivity/status.json 2>/dev/null || true
fi
echo
echo "=== end measurement ==="
