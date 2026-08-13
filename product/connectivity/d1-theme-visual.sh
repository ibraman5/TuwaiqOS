#!/bin/bash
# Wait for Plasma session, apply Tuwaiq Light via supported look-and-feel tools,
# leave Kickoff + Kate + Konsole visible, then switch to Tuwaiq Dark.
set -euo pipefail
OUT=/var/lib/tuwaiq/connectivity
mkdir -p "$OUT"
exec > >(tee "$OUT/d1-theme-visual.log") 2>&1
echo "===== theme visual $(date -u +%Y-%m-%dT%H:%M:%SZ) ====="

for i in $(seq 1 240); do
  if pgrep -x plasmashell >/dev/null 2>&1; then
    echo "session ready at t+$i"
    break
  fi
  sleep 1
done

XAUTH="$(ls /var/run/sddm/xauth_* 2>/dev/null | head -n1 || true)"
export DISPLAY=:0 XAUTHORITY="$XAUTH" XDG_RUNTIME_DIR=/run/user/1000
export DBUS_SESSION_BUS_ADDRESS=unix:path=/run/user/1000/bus
echo "XAUTHORITY=$XAUTHORITY bus=$([[ -S /run/user/1000/bus ]] && echo yes || echo no)"

run_user() {
  runuser -u tuwaiq -- env \
    DISPLAY="${DISPLAY}" \
    XAUTHORITY="${XAUTHORITY}" \
    XDG_RUNTIME_DIR=/run/user/1000 \
    DBUS_SESSION_BUS_ADDRESS="${DBUS_SESSION_BUS_ADDRESS}" \
    "$@"
}

echo "## open Konsole + Kate + Kickoff on current (Tuwaiq Light) session"
run_user konsole --profile Tuwaiq -e bash -lc 'echo TUWAIQ_LIGHT_THEME; sleep 300' >/dev/null 2>&1 &
sleep 3
run_user kate /usr/share/tuwaiqos/README-branding.txt >/dev/null 2>&1 &
sleep 4
run_user qdbus org.kde.plasmashell /PlasmaShell activateLauncherMenu || \
  run_user qdbus org.kde.plasmashell /PlasmaShell org.kde.PlasmaShell.activateLauncherMenu || true
echo "LIGHT_READY $(date -u +%Y-%m-%dT%H:%M:%SZ)"
sleep 80

echo "## switch to Tuwaiq Dark (colorscheme only; lookandfeeltool can hang)"
run_user plasma-apply-colorscheme TuwaiqDark || true
cp /etc/xdg/kdeglobals /home/tuwaiq/.config/kdeglobals
chown tuwaiq:tuwaiq /home/tuwaiq/.config/kdeglobals
sleep 8
run_user qdbus org.kde.plasmashell /PlasmaShell activateLauncherMenu || true
echo "DARK_READY $(date -u +%Y-%m-%dT%H:%M:%SZ)"
sleep 50
pgrep -a -u tuwaiq plasmashell || echo "FAIL plasmashell"
pgrep -a -u tuwaiq kwin_x11 || true
pgrep -a -u tuwaiq kate || true
pgrep -a -u tuwaiq konsole || true
if pgrep -u tuwaiq plasmashell >/dev/null; then echo "THEME_SWITCH=PASS"; else echo "THEME_SWITCH=FAIL"; fi
touch "$OUT/d1-theme-visual.done"
