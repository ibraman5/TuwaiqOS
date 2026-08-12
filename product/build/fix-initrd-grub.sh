#!/usr/bin/env bash
# Sync initrd from preserved rootfs onto disk image, rewrite grub.cfg, reinstall BIOS GRUB, refresh qcow2.
set -euo pipefail

WORK="${TUWAIQ_WORK:-/work}"
DISK="${WORK}/tuwaiqos-d0.raw"
ROOTFS="${WORK}/rootfs"
MNT="${WORK}/mnt"
OUT_QCOW="/src/product/out/tuwaiqos-d0.qcow2"

die() { echo "ERROR: $*" >&2; exit 1; }

ensure_partition_nodes() {
  local loop="$1"
  partx -u "${loop}" 2>/dev/null || true
  blockdev --rereadpt "${loop}" 2>/dev/null || true
  while read -r name majmin type; do
    [[ "${type}" == "part" ]] || continue
    local node="/dev/${name}"
    if [[ ! -e "${node}" ]]; then
      local maj="${majmin%%:*}" min="${majmin##*:}"
      mknod "${node}" b "${maj}" "${min}"
      echo "created ${node}"
    fi
  done < <(lsblk -ln -o NAME,MAJ:MIN,TYPE "${loop}")
}

cleanup() {
  sync || true
  umount "${MNT}/dev/pts" 2>/dev/null || true
  umount "${MNT}/dev" 2>/dev/null || true
  umount "${MNT}/proc" 2>/dev/null || true
  umount "${MNT}/sys" 2>/dev/null || true
  umount "${MNT}/boot/efi" 2>/dev/null || true
  umount "${MNT}" 2>/dev/null || true
  losetup -d "${LOOP}" 2>/dev/null || true
}

[[ -f "${DISK}" ]] || die "missing ${DISK}"
[[ -s "${ROOTFS}/boot/initrd.img-6.8.0-137-generic" ]] || die "rootfs initrd missing"

LOOP="$(losetup --find --show --partscan "${DISK}")"
trap cleanup EXIT
ensure_partition_nodes "${LOOP}"

mapfile -t parts < <(lsblk -ln -o NAME,TYPE "${LOOP}" | awk '$2=="part"{print "/dev/" $1}')
((${#parts[@]} >= 3)) || die "expected 3 partitions"
ESP_PART="${parts[1]}"
ROOT_PART="${parts[2]}"

mkdir -p "${MNT}"
mount "${ROOT_PART}" "${MNT}"
mkdir -p "${MNT}/boot/efi"
mount "${ESP_PART}" "${MNT}/boot/efi"

echo "Syncing initrd + initramfs-tools bits from rootfs"
cp -a "${ROOTFS}/boot/initrd.img-6.8.0-137-generic" "${MNT}/boot/"
cp -a "${ROOTFS}/boot/initrd.img" "${MNT}/boot/" 2>/dev/null || true
# Ensure update-initramfs tooling exists for future boots (minimal package sync)
if [[ ! -x "${MNT}/usr/sbin/update-initramfs" ]]; then
  tar -C "${ROOTFS}" -cf - \
    usr/sbin/update-initramfs \
    usr/share/initramfs-tools \
    usr/lib/initramfs-tools \
    etc/initramfs-tools \
    2>/dev/null | tar -C "${MNT}" -xf - || true
fi

VMLINUZ="$(ls -1 "${MNT}/boot"/vmlinuz-* | tail -1)"
INITRD="$(ls -1 "${MNT}/boot"/initrd.img-* | tail -1)"
[[ -s "${INITRD}" ]] || die "initrd still missing on disk"
ROOT_PARTUUID="$(blkid -s PARTUUID -o value "${ROOT_PART}")"
ESP_PARTUUID="$(blkid -s PARTUUID -o value "${ESP_PART}")"
VMLINUZ_REL="${VMLINUZ#${MNT}}"
INITRD_REL="${INITRD#${MNT}}"

cat > "${MNT}/etc/fstab" <<EOF
PARTUUID=${ROOT_PARTUUID} / ext4 defaults 0 1
PARTUUID=${ESP_PARTUUID} /boot/efi vfat umask=0077 0 1
EOF

mkdir -p "${MNT}/boot/grub"
cat > "${MNT}/boot/grub/grub.cfg" <<EOF
set timeout=3
set default=0
serial --unit=0 --speed=115200
terminal_input console serial
terminal_output console serial
menuentry "TuwaiqOS D0" {
  linux ${VMLINUZ_REL} root=PARTUUID=${ROOT_PARTUUID} ro quiet splash console=tty0 console=ttyS0,115200n8
  initrd ${INITRD_REL}
}
EOF

echo "=== grub.cfg ==="
cat "${MNT}/boot/grub/grub.cfg"
ls -lh "${INITRD}"

grub-install --target=i386-pc --boot-directory="${MNT}/boot" --root-directory="${MNT}" --recheck "${LOOP}"
test -f "${MNT}/boot/grub/i386-pc/core.img" || die "core.img missing"
grep -q "initrd ${INITRD_REL}" "${MNT}/boot/grub/grub.cfg" || die "grub.cfg missing initrd"

cleanup
trap - EXIT

echo "Refreshing qcow2"
qemu-img convert -O qcow2 "${DISK}" "${WORK}/tuwaiqos-d0.qcow2"
cp -f "${WORK}/tuwaiqos-d0.qcow2" "${OUT_QCOW}"
ls -lh "${OUT_QCOW}"
echo "FIX_OK"
