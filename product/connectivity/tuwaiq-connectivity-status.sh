#!/usr/bin/env bash
# Collect structured NetworkManager connectivity status for desktop + future Tuwaiq AI.
# Writes world-readable JSON without secrets/credentials. No unrestricted root needed for readers.
set -euo pipefail

OUT_DIR="${TUWAIQ_CONNECTIVITY_DIR:-/var/lib/tuwaiq/connectivity}"
OUT_FILE="${OUT_DIR}/status.json"
RUN_FILE="/run/tuwaiq/connectivity.json"
mkdir -p "${OUT_DIR}" /run/tuwaiq

ts="$(date -u +%Y-%m-%dT%H:%M:%SZ)"
nm_state="$(nmcli -t -f STATE general 2>/dev/null || echo unknown)"
nm_connectivity="$(nmcli -t -f CONNECTIVITY general 2>/dev/null || echo unknown)"
primary_device="$(nmcli -t -f DEVICE,TYPE,STATE device 2>/dev/null | awk -F: '$3=="connected"{print $1; exit}')"
primary_type="$(nmcli -t -f DEVICE,TYPE,STATE device 2>/dev/null | awk -F: '$3=="connected"{print $2; exit}')"
conn_name="$(nmcli -t -f NAME,DEVICE connection show --active 2>/dev/null | awk -F: -v d="${primary_device}" '$2==d{print $1; exit}')"

ipv4=""
ipv6=""
gateway=""
dns=""
if [[ -n "${primary_device}" ]]; then
  ipv4="$(nmcli -t -f IP4.ADDRESS device show "${primary_device}" 2>/dev/null | head -n1 | cut -d: -f2- | cut -d/ -f1)"
  ipv6="$(nmcli -t -f IP6.ADDRESS device show "${primary_device}" 2>/dev/null | head -n1 | cut -d: -f2- | cut -d/ -f1)"
  gateway="$(nmcli -t -f IP4.GATEWAY device show "${primary_device}" 2>/dev/null | cut -d: -f2-)"
  dns="$(nmcli -t -f IP4.DNS device show "${primary_device}" 2>/dev/null | cut -d: -f2- | paste -sd, -)"
fi

link_state="down"
[[ -n "${primary_device}" ]] && link_state="up"

gw_reachable=false
dns_ok=false
https_ok=false
internet_connected=false

if [[ -n "${gateway}" ]]; then
  if ping -c1 -W2 "${gateway}" >/dev/null 2>&1; then
    gw_reachable=true
  fi
fi

if getent hosts example.com >/dev/null 2>&1 || getent hosts dns.google >/dev/null 2>&1; then
  dns_ok=true
fi
# Fallback resolvers for environments where DHCP DNS is broken (e.g. some QEMU user-net hosts)
if [[ "${dns_ok}" != "true" ]]; then
  if command -v dig >/dev/null 2>&1; then
    if dig +time=2 +tries=1 @1.1.1.1 example.com +short >/dev/null 2>&1; then
      dns_ok=true
    fi
  fi
fi

if command -v curl >/dev/null 2>&1; then
  if curl -fsS --max-time 5 -o /dev/null https://example.com; then
    https_ok=true
  else
    # Fallback when stub resolver is unhealthy but upstream DNS works
    ip=""
    if command -v dig >/dev/null 2>&1; then
      ip="$(dig +time=2 +tries=1 @1.1.1.1 example.com +short | head -n1 || true)"
    fi
    if [[ -n "${ip}" ]] && curl -fsS --max-time 8 -o /dev/null --resolve "example.com:443:${ip}" https://example.com; then
      https_ok=true
      dns_ok=true
    fi
  fi
elif command -v wget >/dev/null 2>&1; then
  if wget -q --timeout=5 -O /dev/null https://example.com; then
    https_ok=true
  fi
fi

if [[ "${nm_connectivity}" == "full" || "${https_ok}" == "true" ]]; then
  internet_connected=true
fi

# shellcheck disable=SC2086
cat > "${OUT_FILE}.tmp" <<EOF
{
  "schema": "tuwaiq.connectivity.v1",
  "timestamp_utc": "${ts}",
  "internet_connected": ${internet_connected},
  "nm_state": "${nm_state}",
  "nm_connectivity": "${nm_connectivity}",
  "active_interface": "${primary_device}",
  "connection_type": "${primary_type}",
  "connection_name": "${conn_name}",
  "link_state": "${link_state}",
  "ipv4": "${ipv4}",
  "ipv6": "${ipv6}",
  "gateway": "${gateway}",
  "dns": "${dns}",
  "gateway_reachable": ${gw_reachable},
  "dns_ok": ${dns_ok},
  "https_ok": ${https_ok},
  "notes": "No credentials included. Safe for non-root consumers."
}
EOF
mv -f "${OUT_FILE}.tmp" "${OUT_FILE}"
chmod 0644 "${OUT_FILE}"
cp -f "${OUT_FILE}" "${RUN_FILE}"
chmod 0644 "${RUN_FILE}"
echo "Wrote ${OUT_FILE}"
