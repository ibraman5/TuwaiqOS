#!/bin/bash
# Surgical D1 closure patch onto existing proof qcow2 (no debootstrap rebuild).
set -euo pipefail
WORK="${TUWAIQ_WORK:-/work}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SRC="${1:-${WORK}/d1-proof.qcow2}"
OUT="${2:-${WORK}/d1-proof.qcow2}"
RAW="${WORK}/d1-closure.raw"
MNT="${WORK}/mnt-closure"

log(){ printf '[d1-closure-patch] %s\n' "$*"; }
need(){ command -v "$1" >/dev/null || { echo "missing $1"; exit 1; }; }
need qemu-img; need losetup; need mount

[[ -f "$SRC" ]] || { echo "missing $SRC"; exit 1; }
rm -f "$RAW"
log "convert qcow2 -> raw"
qemu-img convert -f qcow2 -O raw "$SRC" "$RAW"
LOOP="$(losetup -f --show -P "$RAW")"
cleanup(){ umount "$MNT" 2>/dev/null || true; losetup -d "$LOOP" 2>/dev/null || true; }
trap cleanup EXIT
sleep 1
while read -r name majmin type; do
  [[ "$type" == part ]] || continue
  [[ -e "/dev/$name" ]] || mknod "/dev/$name" b "${majmin%%:*}" "${majmin##*:}"
done < <(lsblk -ln -o NAME,MAJ:MIN,TYPE "$LOOP")
mkdir -p "$MNT"
mount "${LOOP}p3" "$MNT"
bash "${ROOT_DIR}/product/scripts/apply-branding.sh" "$MNT"
# Ensure DisplayServer=x11 remains
grep -q 'DisplayServer=x11' "$MNT/etc/sddm.conf.d/tuwaiqos.conf" || \
  printf '\n[General]\nDisplayServer=x11\n' >> "$MNT/etc/sddm.conf.d/tuwaiqos.conf"
# Verify resolved enablement
ls -la "$MNT/etc/systemd/system/multi-user.target.wants/systemd-resolved.service" || true
ls -la "$MNT/etc/resolv.conf" || true
sync
umount "$MNT"
losetup -d "$LOOP"
trap - EXIT
log "convert raw -> qcow2"
TMP_OUT="${OUT}.new"
rm -f "$TMP_OUT"
qemu-img convert -f raw -O qcow2 "$RAW" "$TMP_OUT"
rm -f "$RAW"
mv -f "$TMP_OUT" "$OUT"
qemu-img info "$OUT"
log "patched $OUT"
