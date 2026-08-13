#!/usr/bin/env bash
# D1 closure: DNS clean-boot proof using normal system resolver only.
# Forbidden in PASS criteria: dig @server, curl --resolve, manual DNS server args.
set -euo pipefail

OUT_DIR=/var/lib/tuwaiq/connectivity
mkdir -p "${OUT_DIR}"
LOG="${OUT_DIR}/d1-dns-closure.log"
RESULT="${OUT_DIR}/d1-dns-closure.result"
exec > >(tee "${LOG}") 2>&1

pass=0
fail=0
ok() { echo "PASS: $*"; pass=$((pass+1)); }
bad() { echo "FAIL: $*"; fail=$((fail+1)); }

echo "===== D1 DNS closure $(date -u +%Y-%m-%dT%H:%M:%SZ) ====="

# Wait for NM + resolved
for i in $(seq 1 60); do
  systemctl is-active NetworkManager >/dev/null 2>&1 && \
  systemctl is-active systemd-resolved >/dev/null 2>&1 && break
  sleep 1
done

echo "## NetworkManager"
systemctl is-active NetworkManager && ok "NetworkManager active" || bad "NetworkManager inactive"
nmcli -t general || true
nmcli -t device || true

echo "## systemd-resolved"
systemctl is-active systemd-resolved && ok "systemd-resolved active" || bad "systemd-resolved inactive"
systemctl status systemd-resolved --no-pager -l | head -n 30 || true

echo "## /etc/resolv.conf"
ls -la /etc/resolv.conf || true
if [[ -L /etc/resolv.conf ]]; then
  echo "symlink -> $(readlink /etc/resolv.conf)"
  case "$(readlink /etc/resolv.conf)" in
    *systemd/resolve/stub-resolv.conf*|*systemd/resolve/resolv.conf*)
      ok "resolv.conf points at systemd-resolved"
      ;;
    *)
      bad "resolv.conf symlink unexpected: $(readlink /etc/resolv.conf)"
      ;;
  esac
else
  bad "resolv.conf is not a symlink"
fi
echo "--- resolv.conf content ---"
cat /etc/resolv.conf || true

echo "## resolvectl status"
resolvectl status || bad "resolvectl status failed"

echo "## enp0s3 DNS (nmcli)"
IFACE="$(nmcli -t -f DEVICE,STATE device | awk -F: '$2=="connected"{print $1; exit}')"
echo "iface=${IFACE}"
if [[ -n "${IFACE}" ]]; then
  nmcli device show "${IFACE}" | grep -E 'IP4.DNS|IP6.DNS|IP4.ADDRESS|IP4.GATEWAY|GENERAL.CONNECTION' || true
  ok "interface connected: ${IFACE}"
else
  bad "no connected interface"
fi

echo "## normal getaddrinfo (getent) — no dig @server"
if getent ahosts example.com | head -n 5; then
  ok "getent ahosts example.com"
else
  bad "getent ahosts example.com"
fi
if getent hosts example.com | head -n 5; then
  ok "getent hosts example.com"
else
  bad "getent hosts example.com"
fi

echo "## resolvectl query — system resolver path"
if resolvectl query example.com; then
  ok "resolvectl query example.com"
else
  bad "resolvectl query example.com"
fi

echo "## normal HTTPS — no curl --resolve"
if curl -I -sS --max-time 20 https://example.com | head -n 15; then
  ok "curl -I https://example.com"
else
  bad "curl -I https://example.com"
fi
HTTP_CODE="$(curl -o /dev/null -sS -w '%{http_code}' --max-time 20 https://example.com || echo 000)"
echo "http_code=${HTTP_CODE}"
if [[ "${HTTP_CODE}" =~ ^2 ]]; then
  ok "HTTPS ${HTTP_CODE}"
else
  bad "HTTPS ${HTTP_CODE}"
fi

echo "## reconnect cycle + DNS still works"
CONN="$(nmcli -t -f NAME,DEVICE connection show --active | awk -F: -v d="${IFACE}" '$2==d{print $1; exit}')"
echo "conn=${CONN}"
if [[ -n "${CONN}" ]]; then
  nmcli connection down "${CONN}" || true
  sleep 2
  nmcli connection up "${CONN}" || nmcli device connect "${IFACE}" || true
  for i in $(seq 1 30); do
    ST="$(nmcli -t -f DEVICE,STATE device | awk -F: -v d="${IFACE}" '$1==d{print $2}')"
    [[ "${ST}" == "connected" ]] && break
    sleep 1
  done
  sleep 2
  if getent ahosts example.com | head -n 3 && curl -I -sS --max-time 20 https://example.com | head -n 5; then
    ok "post-reconnect DNS+HTTPS"
  else
    bad "post-reconnect DNS+HTTPS"
  fi
else
  bad "no connection for reconnect test"
fi

echo "## listening ports"
ss -tulpn | tee "${OUT_DIR}/d1-dns-closure-listening.txt" || true

echo "===== SUMMARY pass=${pass} fail=${fail} ====="
if [[ "${fail}" -eq 0 ]]; then
  echo PASS > "${RESULT}"
  echo "DNS_CLOSURE=PASS"
  exit 0
fi
echo FAIL > "${RESULT}"
echo "DNS_CLOSURE=FAIL"
exit 1
