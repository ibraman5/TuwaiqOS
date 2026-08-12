#!/usr/bin/env bash
# Surgically apply D1 onto preserved D0 disk (raw via losetup), then emit D1 qcow2.
# No nbd required (Docker Desktop / Windows-friendly).
set -euo pipefail

WORK="${TUWAIQ_WORK:-/work}"
ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
ROOTFS="${WORK}/rootfs"
DISK_RAW="${WORK}/tuwaiqos-d0.raw"
DISK_QCOW="${WORK}/tuwaiqos-d1.qcow2"
MNT="${WORK}/mnt-d1"

log() { printf '[apply-d1] %s\n' "$*"; }
die() { printf '[apply-d1] ERROR: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || die "missing dependency: $1"; }

need losetup
need mount
need qemu-img
need lsblk

[[ -d "${ROOTFS}" ]] || die "missing rootfs at ${ROOTFS}"
[[ -f "${DISK_RAW}" ]] || die "missing raw disk at ${DISK_RAW}"

ensure_partition_nodes() {
  local loop="$1"
  if command -v partx >/dev/null 2>&1; then
    partx -a "${loop}" 2>/dev/null || partx -u "${loop}" 2>/dev/null || true
  fi
  blockdev --rereadpt "${loop}" 2>/dev/null || true
  while read -r name majmin type; do
    [[ "${type}" == "part" ]] || continue
    local node="/dev/${name}"
    if [[ ! -e "${node}" ]]; then
      local maj="${majmin%%:*}" min="${majmin##*:}"
      mknod "${node}" b "${maj}" "${min}"
      log "created ${node} (${maj}:${min})"
    fi
  done < <(lsblk -ln -o NAME,MAJ:MIN,TYPE "${loop}")
}

apply_into() {
  local target="$1"
  bash "${ROOT_DIR}/product/scripts/apply-branding.sh" "${target}"
}

log "apply branding into work rootfs"
apply_into "${ROOTFS}"

# Install extras into work rootfs (best-effort)
mount --bind /dev "${ROOTFS}/dev" 2>/dev/null || true
mount --bind /proc "${ROOTFS}/proc" 2>/dev/null || true
mount --bind /sys "${ROOTFS}/sys" 2>/dev/null || true
chroot "${ROOTFS}" apt-get update -qq 2>/dev/null || true
chroot "${ROOTFS}" apt-get install -y -qq curl ufw iputils-ping dnsutils 2>/dev/null || log "rootfs apt extras skipped"
apply_into "${ROOTFS}"
umount "${ROOTFS}/dev" 2>/dev/null || true
umount "${ROOTFS}/proc" 2>/dev/null || true
umount "${ROOTFS}/sys" 2>/dev/null || true

log "mount raw disk root and apply D1"
mkdir -p "${MNT}"
LOOP="$(losetup -f --show -P "${DISK_RAW}")"
cleanup() {
  umount "${MNT}/dev" 2>/dev/null || true
  umount "${MNT}/proc" 2>/dev/null || true
  umount "${MNT}/sys" 2>/dev/null || true
  umount "${MNT}" 2>/dev/null || true
  losetup -d "${LOOP}" 2>/dev/null || true
}
trap cleanup EXIT
ensure_partition_nodes "${LOOP}"
sleep 1

ROOTPART=""
for cand in "${LOOP}p3" "${LOOP}p2" "${LOOP}p1"; do
  if [[ -e "${cand}" ]]; then
    FSTYPE="$(blkid -o value -s TYPE "${cand}" 2>/dev/null || true)"
    if [[ "${FSTYPE}" == "ext4" || "${FSTYPE}" == "btrfs" || "${FSTYPE}" == "xfs" ]]; then
      ROOTPART="${cand}"
      break
    fi
  fi
done
[[ -n "${ROOTPART}" ]] || die "could not find root filesystem on ${DISK_RAW}"
log "root partition=${ROOTPART} loop=${LOOP}"
mount "${ROOTPART}" "${MNT}"

apply_into "${MNT}"

# Bring curl/ufw into the disk image
mount --bind /dev "${MNT}/dev" 2>/dev/null || true
mount --bind /proc "${MNT}/proc" 2>/dev/null || true
mount --bind /sys "${MNT}/sys" 2>/dev/null || true
chroot "${MNT}" apt-get update -qq 2>/dev/null || true
chroot "${MNT}" apt-get install -y -qq curl ufw iputils-ping dnsutils 2>/dev/null || log "disk apt extras skipped"
apply_into "${MNT}"

# Preserve D0 display fixes from rootfs if present
for f in \
  etc/X11/xorg.conf.d/20-modesetting.conf \
  etc/sddm.conf.d/tuwaiqos.conf
do
  if [[ -f "${ROOTFS}/${f}" ]]; then
    install -d "$(dirname "${MNT}/${f}")"
    cp -a "${ROOTFS}/${f}" "${MNT}/${f}"
  fi
done

# Force X11 in SDDM for D0 regression
mkdir -p "${MNT}/etc/sddm.conf.d"
if [[ -f "${MNT}/etc/sddm.conf.d/tuwaiqos.conf" ]]; then
  if ! grep -q 'DisplayServer=x11' "${MNT}/etc/sddm.conf.d/tuwaiqos.conf"; then
    printf '\n[General]\nDisplayServer=x11\n' >> "${MNT}/etc/sddm.conf.d/tuwaiqos.conf"
  fi
fi

sync
umount "${MNT}/dev" 2>/dev/null || true
umount "${MNT}/proc" 2>/dev/null || true
umount "${MNT}/sys" 2>/dev/null || true
umount "${MNT}"
losetup -d "${LOOP}"
trap - EXIT

log "convert raw -> ${DISK_QCOW} (compressed qcow2)"
rm -f "${DISK_QCOW}"
qemu-img convert -c -f raw -O qcow2 "${DISK_RAW}" "${DISK_QCOW}"
qemu-img info "${DISK_QCOW}"
log "D1 image ready: ${DISK_QCOW}"
echo "${DISK_QCOW}"
