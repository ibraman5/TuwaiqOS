#!/usr/bin/env bash
# D1 acceptance evidence collector — runs once after network is up.
set -euo pipefail

OUT_DIR=/var/lib/tuwaiq/connectivity
mkdir -p "${OUT_DIR}"
LOG="${OUT_DIR}/d1-acceptance.log"
exec >>"${LOG}" 2>&1

echo "===== D1 acceptance $(date -u +%Y-%m-%dT%H:%M:%SZ) ====="

echo "## settle"
sleep 20
free -h || true
uptime || true
echo "MemAvailable_kB=$(awk '/MemAvailable/{print $2}' /proc/meminfo)"
echo "loadavg=$(cut -d' ' -f1-3 /proc/loadavg)"

echo "## services"
systemctl is-active sddm || true
systemctl is-active NetworkManager || true
systemctl is-active tuwaiq-connectivity-status.timer || true
pgrep -a plasmashell || true
pgrep -a Xorg || true

/usr/libexec/tuwaiq/measure-network.sh "${OUT_DIR}/d1-network-measure.txt" || true

echo "## reconnect test"
IFACE="$(nmcli -t -f DEVICE,TYPE,STATE device | awk -F: '$2=="ethernet" && $3=="connected"{print $1; exit}')"
CONN="$(nmcli -t -f NAME,DEVICE connection show --active | awk -F: -v d="${IFACE}" '$2==d{print $1; exit}')"
echo "iface=${IFACE} conn=${CONN}"
if [[ -n "${CONN}" ]]; then
  BEFORE="$(date +%s)"
  nmcli connection down "${CONN}" || true
  sleep 3
  nmcli -t device | tee "${OUT_DIR}/d1-reconnect-down.txt" || true
  nmcli connection up "${CONN}" || nmcli device connect "${IFACE}" || true
  # wait for DHCP
  for i in $(seq 1 30); do
    ST="$(nmcli -t -f DEVICE,STATE device | awk -F: -v d="${IFACE}" '$1==d{print $2}')"
    [[ "${ST}" == "connected" ]] && break
    sleep 1
  done
  AFTER="$(date +%s)"
  echo "reconnect_seconds=$((AFTER-BEFORE))" | tee "${OUT_DIR}/d1-reconnect-timing.txt"
  nmcli -t device | tee "${OUT_DIR}/d1-reconnect-up.txt" || true
  ip -br addr | tee -a "${OUT_DIR}/d1-reconnect-up.txt" || true
else
  echo "SKIP reconnect: no active ethernet connection"
fi

/usr/libexec/tuwaiq/tuwaiq-connectivity-status.sh || true
cp -f /var/lib/tuwaiq/connectivity/status.json "${OUT_DIR}/d1-status-final.json" 2>/dev/null || true

echo "## listening ports final"
ss -tulpn | tee "${OUT_DIR}/d1-listening-ports.txt" || true

echo "===== D1 acceptance done ====="
