#!/usr/bin/env bash
# Surgical D0 display fix on preserved disk image (no package rebuild).
# Switches SDDM to X11 for QEMU SeaBIOS/std-vga, ensures grub.cfg initrd, refreshes qcow2.
set -euo pipefail

WORK="${TUWAIQ_WORK:-/work}"
DISK="${WORK}/tuwaiqos-d0.raw"
ROOTFS="${WORK}/rootfs"
MNT="${WORK}/mnt"
OUT_QCOW="${OUT_QCOW:-/out/tuwaiqos-d0.qcow2}"

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

LOOP="$(losetup --find --show --partscan "${DISK}")"
trap cleanup EXIT
ensure_partition_nodes "${LOOP}"

# Stable GPT order: p1 bios_grub, p2 ESP, p3 root (do not trust unsorted lsblk order)
BIOS_PART="${LOOP}p1"
ESP_PART="${LOOP}p2"
ROOT_PART="${LOOP}p3"
[[ -e "${BIOS_PART}" && -e "${ESP_PART}" && -e "${ROOT_PART}" ]] || die "expected ${LOOP}p1/p2/p3 nodes"

mkdir -p "${MNT}"
mount "${ROOT_PART}" "${MNT}"
mkdir -p "${MNT}/boot/efi"
mount "${ESP_PART}" "${MNT}/boot/efi"

echo "=== before ==="
systemctl --root="${MNT}" get-default || true
cat "${MNT}/etc/sddm.conf.d/"*.conf 2>/dev/null || true

# Prefer X11 for QEMU std/virtio VGA (Wayland needs DRM that often fails here).
mkdir -p "${MNT}/etc/sddm.conf.d" "${ROOTFS}/etc/sddm.conf.d"
cat > "${MNT}/etc/sddm.conf.d/tuwaiqos.conf" <<'EOF'
[General]
DisplayServer=x11

[Theme]
Current=breeze

[Users]
MaximumUid=60000
EOF
cp -f "${MNT}/etc/sddm.conf.d/tuwaiqos.conf" "${ROOTFS}/etc/sddm.conf.d/tuwaiqos.conf"

cat > "${MNT}/etc/sddm.conf.d/autologin.conf" <<'EOF'
[Autologin]
User=tuwaiq
Session=plasma
EOF
cp -f "${MNT}/etc/sddm.conf.d/autologin.conf" "${ROOTFS}/etc/sddm.conf.d/autologin.conf"

# Hostname (was leftover container id)
echo tuwaiqos > "${MNT}/etc/hostname"
echo tuwaiqos > "${ROOTFS}/etc/hostname" 2>/dev/null || true
if ! grep -q 'tuwaiqos' "${MNT}/etc/hosts" 2>/dev/null; then
  echo '127.0.1.1 tuwaiqos' >> "${MNT}/etc/hosts"
fi

# Ensure graphical default + SDDM enabled
ln -sfn /lib/systemd/system/graphical.target "${MNT}/etc/systemd/system/default.target"
ln -sfn /lib/systemd/system/sddm.service "${MNT}/etc/systemd/system/display-manager.service"

# Xorg: try modesetting then vesa for QEMU std VGA
mkdir -p "${MNT}/etc/X11/xorg.conf.d"
cat > "${MNT}/etc/X11/xorg.conf.d/20-qemuvga.conf" <<'EOF'
Section "Device"
    Identifier "QEMUVGA"
    Driver "modesetting"
    Option "AccelMethod" "none"
    Option "SWcursor" "true"
EndSection
EOF
# VESA fallback config (used if modesetting fails via SDDM retry; keep as alternate snippet)
cat > "${MNT}/etc/X11/xorg.conf.d/00-qemufallback-note.conf" <<'EOF'
# Primary device is modesetting; if X fails, rename/swap to Driver "vesa".
EOF

# Sync initrd if missing/empty
if [[ ! -s "${MNT}/boot/initrd.img-6.8.0-137-generic" ]]; then
  echo "syncing initrd from rootfs"
  cp -a "${ROOTFS}/boot/initrd.img-6.8.0-137-generic" "${MNT}/boot/"
fi

ROOT_PARTUUID="$(blkid -s PARTUUID -o value "${ROOT_PART}")"
ESP_PARTUUID="$(blkid -s PARTUUID -o value "${ESP_PART}")"
VMLINUZ="$(ls -1 "${MNT}/boot"/vmlinuz-* | tail -1)"
INITRD="$(ls -1 "${MNT}/boot"/initrd.img-* | tail -1)"
[[ -s "${INITRD}" ]] || die "initrd missing"
VMLINUZ_REL="${VMLINUZ#${MNT}}"
INITRD_REL="${INITRD#${MNT}}"

cat > "${MNT}/etc/fstab" <<EOF
PARTUUID=${ROOT_PARTUUID} / ext4 defaults 0 1
PARTUUID=${ESP_PARTUUID} /boot/efi vfat umask=0077 0 1
EOF

# Keep gfxterm so VGA shows GRUB/kernel; serial remains secondary diagnostic
mkdir -p "${MNT}/boot/grub"
cat > "${MNT}/boot/grub/grub.cfg" <<EOF
set timeout=2
set default=0
insmod all_video
insmod gfxterm
serial --unit=0 --speed=115200
terminal_input console serial
terminal_output gfxterm console serial
menuentry "TuwaiqOS D0" {
  linux ${VMLINUZ_REL} root=PARTUUID=${ROOT_PARTUUID} ro console=tty0 console=ttyS0,115200n8 systemd.show_status=1
  initrd ${INITRD_REL}
}
EOF

grub-install --target=i386-pc --boot-directory="${MNT}/boot" --root-directory="${MNT}" --recheck "${LOOP}"
test -f "${MNT}/boot/grub/i386-pc/core.img" || die "core.img missing"

# Boot evidence unit
mkdir -p "${MNT}/etc/systemd/system" "${MNT}/var/log"
cat > "${MNT}/etc/systemd/system/d0-boot-evidence.service" <<'EOF'
[Unit]
Description=TuwaiqOS D0 boot evidence
After=graphical.target
Wants=graphical.target

[Service]
Type=oneshot
ExecStart=/bin/bash -c 'exec >>/var/log/d0-boot.log 2>&1; date -Is; systemctl is-active graphical.target; systemctl is-active sddm; systemctl is-active display-manager; pgrep -a sddm || true; pgrep -a plasmashell || true; pgrep -a Xorg || true; loginctl list-sessions || true; echo DONE; echo TUWAIQ_D0_GRAPHICAL_EVIDENCE >/dev/ttyS0'
RemainAfterExit=yes

[Install]
WantedBy=graphical.target
EOF
mkdir -p "${MNT}/etc/systemd/system/graphical.target.wants"
ln -sfn /etc/systemd/system/d0-boot-evidence.service \
  "${MNT}/etc/systemd/system/graphical.target.wants/d0-boot-evidence.service"

# Also emit early multi-user marker on serial
cat > "${MNT}/etc/systemd/system/d0-multiuser-marker.service" <<'EOF'
[Unit]
Description=TuwaiqOS D0 multi-user marker
After=multi-user.target

[Service]
Type=oneshot
ExecStart=/bin/sh -c 'echo TUWAIQ_D0_MULTI_USER >/dev/ttyS0'
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
EOF
mkdir -p "${MNT}/etc/systemd/system/multi-user.target.wants"
ln -sfn /etc/systemd/system/d0-multiuser-marker.service \
  "${MNT}/etc/systemd/system/multi-user.target.wants/d0-multiuser-marker.service"

echo "=== after ==="
cat "${MNT}/etc/sddm.conf.d/"*.conf
cat "${MNT}/boot/grub/grub.cfg"
ls -lh "${INITRD}" "${MNT}/boot/grub/i386-pc/core.img"
dpkg --root="${MNT}" -l 'xserver-xorg-video-vesa' 'xserver-xorg-video-all' 2>/dev/null | grep '^ii' || echo "vesa package check done"

cleanup
trap - EXIT

echo "Refreshing compressed qcow2 to ${OUT_QCOW}"
mkdir -p "$(dirname "${OUT_QCOW}")"
TMP_QCOW="${WORK}/tuwaiqos-d0.qcow2.tmp"
rm -f "${TMP_QCOW}"
qemu-img convert -O qcow2 -c "${DISK}" "${TMP_QCOW}"
qemu-img check "${TMP_QCOW}"
cp -f "${TMP_QCOW}" "${OUT_QCOW}"
rm -f "${TMP_QCOW}"
ls -lh "${OUT_QCOW}"
qemu-img check "${OUT_QCOW}"
echo "DISPLAY_FIX_OK"
