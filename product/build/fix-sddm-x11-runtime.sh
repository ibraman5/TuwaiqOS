#!/usr/bin/env bash
set -euo pipefail
# Minimal D0 SDDM→X11 fix on preserved disk+rootfs. No full desktop reinstall.
WORK=/work
ROOTFS=/work/rootfs
DISK=/work/tuwaiqos-d0.raw
MNT=/work/mnt
OUT=/out

die() { echo "ERROR: $*" >&2; exit 1; }

ensure_nodes() {
  local loop="$1"
  partx -u "$loop" 2>/dev/null || true
  while read -r name majmin type; do
    [[ "$type" == "part" ]] || continue
    local node="/dev/$name"
    if [[ ! -e "$node" ]]; then
      mknod "$node" b "${majmin%%:*}" "${majmin##*:}"
      echo "created $node"
    fi
  done < <(lsblk -ln -o NAME,MAJ:MIN,TYPE "$loop")
}

install_runtime() {
  local target="$1"
  echo "=== install runtime into $target ==="
  mount --bind /dev "$target/dev"
  mount --bind /proc "$target/proc"
  mount --bind /sys "$target/sys"
  mkdir -p "$target/dev/pts"
  mount -t devpts devpts "$target/dev/pts"
  cp /etc/resolv.conf "$target/etc/resolv.conf" || true

  # Ensure Azure/Ubuntu mirrors reachable
  chroot "$target" apt-get update -qq
  # ONLY missing runtime for Plasma X11 under QEMU
  DEBIAN_FRONTEND=noninteractive chroot "$target" apt-get install -y --no-install-recommends \
    kwin-x11 \
    xserver-xorg-video-vesa \
    xserver-xorg-video-fbdev \
    xinit \
    x11-xserver-utils

  # Groups for display access
  chroot "$target" usermod -aG video,render,input sddm || true
  chroot "$target" usermod -aG video,render,input tuwaiq || true

  # SDDM X11 explicit config
  mkdir -p "$target/etc/sddm.conf.d"
  cat > "$target/etc/sddm.conf.d/tuwaiqos.conf" <<'EOF'
[General]
DisplayServer=x11
MinimumVT=7
Numlock=none

[Theme]
Current=breeze

[Users]
MaximumUid=60000

[X11]
ServerPath=/usr/bin/Xorg
SessionCommand=/etc/sddm/Xsession
DisplayCommand=/usr/share/sddm/scripts/Xsetup
DisplayStopCommand=/usr/share/sddm/scripts/Xstop
ServerArguments=-nolisten tcp
EOF

  cat > "$target/etc/sddm.conf.d/autologin.conf" <<'EOF'
[Autologin]
User=tuwaiq
Session=plasma
Relogin=false
EOF

  # Prefer modesetting (present); keep vesa as secondary Device section via separate conf
  mkdir -p "$target/etc/X11/xorg.conf.d"
  cat > "$target/etc/X11/xorg.conf.d/20-qemuvga.conf" <<'EOF'
Section "Device"
    Identifier "Card0"
    Driver "modesetting"
    Option "AccelMethod" "none"
    Option "SWcursor" "true"
EndSection
EOF
  cat > "$target/etc/X11/xorg.conf.d/10-vesa-fallback-note.conf" <<'EOF'
# If modesetting fails under QEMU std VGA, replace Driver above with "vesa".
EOF

  # Persist full SDDM diagnosis next boot
  mkdir -p "$target/usr/local/sbin" "$target/var/lib" \
    "$target/etc/systemd/system/graphical.target.wants"
  cat > "$target/usr/local/sbin/tuwaiq-sddm-diag.sh" <<'EOF'
#!/bin/bash
LOG=/var/lib/tuwaiq-sddm-diag.log
exec >>"$LOG" 2>&1
echo "===== $(date -Is) ====="
systemctl status sddm --no-pager -l || true
journalctl -u sddm -b --no-pager || true
journalctl -b -p warning..alert --no-pager | tail -80 || true
loginctl list-seats || true
loginctl seat-status seat0 || true
loginctl show-seat seat0 -a || true
ls -la /dev/dri /dev/fb* 2>&1 || true
command -v Xorg; command -v startplasma-x11; command -v kwin_x11; command -v startplasma-wayland
ls /usr/share/xsessions/ /usr/share/wayland-sessions/ 2>&1 || true
id sddm; getent group video render input
pgrep -a Xorg || true
pgrep -a sddm || true
pgrep -a plasmashell || true
pgrep -a kwin || true
systemctl is-active systemd-logind || true
# Mirror summary to serial
{
  echo TUWAIQ_SDDM_DIAG_BEGIN
  systemctl is-active sddm || true
  loginctl show-seat seat0 -p CanGraphical -p IdleHint 2>/dev/null || true
  ls /dev/dri 2>/dev/null || echo NO_DRI
  pgrep -a Xorg || echo NO_XORG
  pgrep -a plasmashell || echo NO_PLASMA
  echo TUWAIQ_SDDM_DIAG_END
} > /dev/ttyS0 2>/dev/null || true
EOF
  chmod +x "$target/usr/local/sbin/tuwaiq-sddm-diag.sh"
  cat > "$target/etc/systemd/system/tuwaiq-sddm-diag.service" <<'EOF'
[Unit]
Description=TuwaiqOS SDDM diagnostics
After=display-manager.service
Wants=display-manager.service

[Service]
Type=oneshot
ExecStartPre=/bin/sleep 20
ExecStart=/usr/local/sbin/tuwaiq-sddm-diag.sh
RemainAfterExit=yes

[Install]
WantedBy=graphical.target
EOF
  ln -sfn /etc/systemd/system/tuwaiq-sddm-diag.service \
    "$target/etc/systemd/system/graphical.target.wants/tuwaiq-sddm-diag.service"

  chroot "$target" systemctl enable sddm.service || true
  chroot "$target" systemctl set-default graphical.target || true

  echo "=== verify after install ==="
  chroot "$target" dpkg -l kwin-x11 xserver-xorg-video-vesa xinit 2>/dev/null | grep '^ii' || true
  ls -la "$target/usr/bin/kwin_x11" "$target/usr/bin/startplasma-x11" "$target/usr/bin/Xorg"
  ls "$target/usr/lib/xorg/modules/drivers/"
  getent -R "$target" group video render | cat || grep -E '^video:|^render:|^input:' "$target/etc/group"

  umount "$target/dev/pts" || true
  umount "$target/dev" "$target/proc" "$target/sys" || true
}

echo "Free disk in container:"; df -h /work

# 1) Install into preserved rootfs cache
install_runtime "$ROOTFS"

# 2) Install into disk image
LOOP=$(losetup -f --show --partscan "$DISK")
trap 'umount "$MNT/dev/pts" 2>/dev/null; umount "$MNT/dev" "$MNT/proc" "$MNT/sys" "$MNT/boot/efi" "$MNT" 2>/dev/null; losetup -d "$LOOP" 2>/dev/null' EXIT
ensure_nodes "$LOOP"
mapfile -t parts < <(lsblk -ln -o NAME,TYPE "$LOOP" | awk '$2=="part"{print "/dev/" $1}' | sort)
ROOT_PART="${parts[2]}"
ESP_PART="${parts[1]}"
mkdir -p "$MNT"
mount "$ROOT_PART" "$MNT"
mkdir -p "$MNT/boot/efi"
mount "$ESP_PART" "$MNT/boot/efi" 2>/dev/null || true
install_runtime "$MNT"

# Keep grub.cfg intact (do not touch GRUB install)
test -s "$MNT/boot/grub/grub.cfg"
grep -q 'initrd' "$MNT/boot/grub/grub.cfg"

umount "$MNT/boot/efi" 2>/dev/null || true
umount "$MNT"
losetup -d "$LOOP"
trap - EXIT

echo "=== export qcow2 ==="
rm -f /work/tuwaiqos-d0.qcow2
qemu-img convert -O qcow2 "$DISK" /work/tuwaiqos-d0.qcow2
qemu-img check /work/tuwaiqos-d0.qcow2
rm -f "$OUT/d0-proof.qcow2"
cp -f /work/tuwaiqos-d0.qcow2 "$OUT/d0-proof.qcow2"
ls -lh "$OUT/d0-proof.qcow2"
qemu-img check "$OUT/d0-proof.qcow2"
echo X11_RUNTIME_FIX_OK
