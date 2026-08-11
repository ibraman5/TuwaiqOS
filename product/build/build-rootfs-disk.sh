#!/usr/bin/env bash
# Build a bootable TuwaiqOS Product D0 disk image inside Linux/Docker.
# Output: product/out/tuwaiqos-d0.raw (+ .qcow2 when qemu-img exists)
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT_DIR="${ROOT_DIR}/product/out"
WORK="${ROOT_DIR}/product/build/work"
ROOTFS="${WORK}/rootfs"
DISK="${OUT_DIR}/tuwaiqos-d0.raw"
PKG_LIST="${ROOT_DIR}/product/packages/d0-ubuntu2404.list"
DISK_SIZE_GB="${TUWAIQ_DISK_SIZE_GB:-8}"

export DEBIAN_FRONTEND=noninteractive

log() { printf '[build-product] %s\n' "$*"; }
die() { printf '[build-product] ERROR: %s\n' "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "missing dependency: $1"; }

log "repo=${ROOT_DIR}"
need debootstrap
need chroot
need tar
need truncate
need parted
need losetup
need mkfs.ext4
need mount
need grub-install
need rsync

mkdir -p "${OUT_DIR}" "${WORK}"
rm -rf "${ROOTFS}"
mkdir -p "${ROOTFS}"

log "debootstrap ubuntu 24.04 (noble)"
debootstrap --arch=amd64 --variant=minbase \
  --components=main,universe \
  --include=systemd-sysv,sudo,locales \
  noble "${ROOTFS}" http://archive.ubuntu.com/ubuntu

log "configure apt sources"
cat > "${ROOTFS}/etc/apt/sources.list" <<'EOF'
deb http://archive.ubuntu.com/ubuntu noble main restricted universe multiverse
deb http://archive.ubuntu.com/ubuntu noble-updates main restricted universe multiverse
deb http://archive.ubuntu.com/ubuntu noble-security main restricted universe multiverse
EOF

log "install D0 package set"
mapfile -t PKGS < <(grep -vE '^\s*(#|$)' "${PKG_LIST}")
chroot "${ROOTFS}" apt-get update
chroot "${ROOTFS}" apt-get install -y --no-install-recommends "${PKGS[@]}"
chroot "${ROOTFS}" apt-get clean

log "create default user tuwaiq (password: tuwaiq) — change after first boot"
chroot "${ROOTFS}" useradd -m -s /bin/bash -G sudo,video,audio,plugdev tuwaiq || true
echo 'tuwaiq:tuwaiq' | chroot "${ROOTFS}" chpasswd
mkdir -p "${ROOTFS}/etc/sddm.conf.d"
cat > "${ROOTFS}/etc/sddm.conf.d/autologin.conf" <<'EOF'
[Autologin]
User=tuwaiq
Session=plasma
EOF

log "enable graphical target + sddm"
chroot "${ROOTFS}" systemctl set-default graphical.target || true
chroot "${ROOTFS}" systemctl enable sddm.service NetworkManager.service || true

log "apply Tuwaiq branding"
bash "${ROOT_DIR}/product/scripts/apply-branding.sh" "${ROOTFS}"

# Prefer Wayland Plasma session for user
mkdir -p "${ROOTFS}/home/tuwaiq/.config"
cat > "${ROOTFS}/home/tuwaiq/.config/startkderc" <<'EOF'
[General]
systemdBoot=false
EOF
chroot "${ROOTFS}" chown -R tuwaiq:tuwaiq /home/tuwaiq

log "create disk image (${DISK_SIZE_GB}G)"
rm -f "${DISK}"
truncate -s "${DISK_SIZE_GB}G" "${DISK}"
parted -s "${DISK}" mklabel gpt
parted -s "${DISK}" mkpart ESP fat32 1MiB 512MiB
parted -s "${DISK}" set 1 esp on
parted -s "${DISK}" mkpart root ext4 512MiB 100%

LOOP="$(losetup --find --show --partscan "${DISK}")"
cleanup() {
  sync || true
  umount "${WORK}/mnt/boot/efi" 2>/dev/null || true
  umount "${WORK}/mnt/dev" 2>/dev/null || true
  umount "${WORK}/mnt/proc" 2>/dev/null || true
  umount "${WORK}/mnt/sys" 2>/dev/null || true
  umount "${WORK}/mnt" 2>/dev/null || true
  losetup -d "${LOOP}" 2>/dev/null || true
}
trap cleanup EXIT

# Wait for partition nodes
for _ in $(seq 1 20); do
  [[ -e "${LOOP}p1" && -e "${LOOP}p2" ]] && break
  sleep 0.2
done
[[ -e "${LOOP}p2" ]] || die "loop partitions not found for ${LOOP}"

mkfs.vfat -F32 "${LOOP}p1"
mkfs.ext4 -F "${LOOP}p2"

mkdir -p "${WORK}/mnt"
mount "${LOOP}p2" "${WORK}/mnt"
mkdir -p "${WORK}/mnt/boot/efi"
mount "${LOOP}p1" "${WORK}/mnt/boot/efi"

log "copy rootfs"
rsync -aHAX --info=progress2 "${ROOTFS}/" "${WORK}/mnt/"

echo "${LOOP}p2 / ext4 defaults 0 1" > "${WORK}/mnt/etc/fstab"
echo "${LOOP}p1 /boot/efi vfat umask=0077 0 1" >> "${WORK}/mnt/etc/fstab"
# Rewrite fstab with stable PARTUUIDs
P1="$(blkid -s PARTUUID -o value "${LOOP}p1")"
P2="$(blkid -s PARTUUID -o value "${LOOP}p2")"
cat > "${WORK}/mnt/etc/fstab" <<EOF
PARTUUID=${P2} / ext4 defaults 0 1
PARTUUID=${P1} /boot/efi vfat umask=0077 0 1
EOF

mount --bind /dev "${WORK}/mnt/dev"
mount --bind /proc "${WORK}/mnt/proc"
mount --bind /sys "${WORK}/mnt/sys"

log "install GRUB (UEFI + BIOS where possible)"
chroot "${WORK}/mnt" apt-get update
chroot "${WORK}/mnt" apt-get install -y --no-install-recommends grub-efi-amd64 grub-pc-bin grub-efi-amd64-bin shim-signed || true
chroot "${WORK}/mnt" grub-install --target=x86_64-efi --efi-directory=/boot/efi --bootloader-id=tuwaiqos --recheck || true
# BIOS fallback for QEMU -bios default
chroot "${WORK}/mnt" grub-install --target=i386-pc --recheck "${LOOP}" || true
chroot "${WORK}/mnt" update-grub || true

cleanup
trap - EXIT

if command -v qemu-img >/dev/null 2>&1; then
  log "writing qcow2"
  qemu-img convert -O qcow2 "${DISK}" "${OUT_DIR}/tuwaiqos-d0.qcow2"
fi

SIZE_BYTES="$(stat -c%s "${DISK}" 2>/dev/null || wc -c < "${DISK}")"
log "OUTPUT_PATH=${DISK}"
log "OUTPUT_SIZE_BYTES=${SIZE_BYTES}"
log "done"
