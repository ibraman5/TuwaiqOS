#!/usr/bin/env bash
# Fast patch of D1 networking/identity onto an existing qcow2 (losetup via raw convert).
set -euo pipefail
WORK="${TUWAIQ_WORK:-/work}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
SRC_QCOW="${1:-${WORK}/d1-boot.qcow2}"
OUT_QCOW="${2:-${WORK}/tuwaiqos-d1.qcow2}"
MNT="${WORK}/mnt-d1-patch"

log(){ printf '[patch-d1] %s\n' "$*"; }
die(){ printf '[patch-d1] ERROR: %s\n' "$*" >&2; exit 1; }

[[ -f "${SRC_QCOW}" ]] || die "missing ${SRC_QCOW}"
mkdir -p "${MNT}"
RAW="${WORK}/d1-patch.raw"
rm -f "${RAW}"
qemu-img convert -f qcow2 -O raw "${SRC_QCOW}" "${RAW}"
LOOP="$(losetup -f --show -P "${RAW}")"
cleanup(){ umount "${MNT}" 2>/dev/null || true; losetup -d "${LOOP}" 2>/dev/null || true; }
trap cleanup EXIT
sleep 1
while read -r name majmin type; do
  [[ "${type}" == part ]] || continue
  if [[ ! -e "/dev/${name}" ]]; then
    mknod "/dev/${name}" b "${majmin%%:*}" "${majmin##*:}"
  fi
done < <(lsblk -ln -o NAME,MAJ:MIN,TYPE "${LOOP}")
mount "${LOOP}p3" "${MNT}"
bash "${ROOT_DIR}/product/scripts/apply-branding.sh" "${MNT}"
# Generate netplan if netplan is present
if [[ -x "${MNT}/usr/sbin/netplan" ]] || [[ -e "${MNT}/usr/sbin/netplan" ]]; then
  chroot "${MNT}" netplan generate 2>/dev/null || true
fi
sync
umount "${MNT}"
losetup -d "${LOOP}"
trap - EXIT
rm -f "${OUT_QCOW}"
# Uncompressed qcow2 is more reliable for live QEMU writes on Windows hosts
qemu-img convert -f raw -O qcow2 "${RAW}" "${OUT_QCOW}"
rm -f "${RAW}"
qemu-img info "${OUT_QCOW}"
log "patched ${OUT_QCOW}"
