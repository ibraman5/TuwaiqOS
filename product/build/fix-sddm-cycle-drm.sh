#!/usr/bin/env bash
set -euo pipefail
# Break SDDM systemd ordering cycles + ensure DRM for CanGraphical + keep X11 runtime.
DISK=/work/tuwaiqos-d0.raw
ROOTFS=/work/rootfs
MNT=/work/mnt
OUT=/out

ensure_nodes() {
  local loop="$1"
  partx -u "$loop" 2>/dev/null || true
  while read -r name majmin type; do
    [[ "$type" == "part" ]] || continue
    local node="/dev/$name"
    [[ -e "$node" ]] || mknod "$node" b "${majmin%%:*}" "${majmin##*:}"
  done < <(lsblk -ln -o NAME,MAJ:MIN,TYPE "$loop")
}

fix_target() {
  local T="$1"
  echo "=== fixing $T ==="

  # Remove ALL custom units that created ordering cycles with sddm/graphical
  local units=(
    tuwaiq-sddm-diag.service
    tuwaiq-display-diag.service
    tuwaiq-boot-marker.service
    tuwaiq-graphical-marker.service
    d0-boot-evidence.service
    d0-boot-diag.service
    d0-multiuser-marker.service
  )
  for u in "${units[@]}"; do
    rm -f "$T/etc/systemd/system/$u"
    rm -f "$T/etc/systemd/system/graphical.target.wants/$u"
    rm -f "$T/etc/systemd/system/multi-user.target.wants/$u"
    rm -f "$T/etc/systemd/system/display-manager.service.wants/$u"
  done

  # Ensure display-manager -> sddm and graphical default
  ln -sfn /lib/systemd/system/sddm.service "$T/etc/systemd/system/display-manager.service"
  ln -sfn /lib/systemd/system/graphical.target "$T/etc/systemd/system/default.target"
  mkdir -p "$T/etc/systemd/system/graphical.target.wants"
  ln -sfn /lib/systemd/system/sddm.service \
    "$T/etc/systemd/system/graphical.target.wants/sddm.service" 2>/dev/null || true

  # Autoload DRM modules for QEMU (std VGA = bochs; virtio-vga = virtio_gpu)
  mkdir -p "$T/etc/modules-load.d"
  cat > "$T/etc/modules-load.d/tuwaiq-qemu-drm.conf" <<'EOF'
bochs
virtio_gpu
simpledrm
EOF

  # Soft dependency: wait for DRM device before SDDM if possible
  mkdir -p "$T/etc/systemd/system/sddm.service.d"
  cat > "$T/etc/systemd/system/sddm.service.d/10-wait-drm.conf" <<'EOF'
[Unit]
# Do not After=graphical.target (cycle). Only wait briefly for DRM if present.
Wants=systemd-udev-settle.service
After=systemd-user-sessions.service systemd-logind.service

[Service]
# Ensure modules attempted before start
ExecStartPre=-/sbin/modprobe bochs
ExecStartPre=-/sbin/modprobe virtio_gpu
ExecStartPre=-/bin/sh -c 'for i in 1 2 3 4 5 6 7 8 9 10; do [ -e /dev/dri/card0 ] && exit 0; sleep 0.5; done; exit 0'
EOF

  # Non-cyclic late diag: timer after boot, does not order against sddm start
  mkdir -p "$T/usr/local/sbin" "$T/var/lib"
  cat > "$T/usr/local/sbin/tuwaiq-sddm-diag.sh" <<'EOF'
#!/bin/bash
LOG=/var/lib/tuwaiq-sddm-diag.log
exec >>"$LOG" 2>&1
echo "===== $(date -Is) ====="
systemctl is-active sddm || true
systemctl status sddm --no-pager -l | head -40 || true
journalctl -u sddm -b --no-pager | tail -60 || true
loginctl show-seat seat0 -a 2>/dev/null | head -40 || true
ls -la /dev/dri /dev/fb* 2>&1 || true
pgrep -a Xorg || echo NO_XORG
pgrep -a plasmashell || echo NO_PLASMA
pgrep -a sddm || true
{
  echo TUWAIQ_SDDM_DIAG_BEGIN
  systemctl is-active sddm || true
  loginctl show-seat seat0 -p CanGraphical 2>/dev/null || true
  ls /dev/dri 2>/dev/null || echo NO_DRI
  pgrep -a Xorg || echo NO_XORG
  pgrep -a plasmashell || echo NO_PLASMA
  echo TUWAIQ_SDDM_DIAG_END
} > /dev/ttyS0 2>/dev/null || true
EOF
  chmod +x "$T/usr/local/sbin/tuwaiq-sddm-diag.sh"
  cat > "$T/etc/systemd/system/tuwaiq-sddm-diag.service" <<'EOF'
[Unit]
Description=TuwaiqOS SDDM diagnostics (non-cyclic)
After=multi-user.target
# Intentionally NOT After=display-manager.service / WantedBy=graphical.target

[Service]
Type=oneshot
ExecStartPre=/bin/sleep 25
ExecStart=/usr/local/sbin/tuwaiq-sddm-diag.sh
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
EOF
  mkdir -p "$T/etc/systemd/system/multi-user.target.wants"
  ln -sfn /etc/systemd/system/tuwaiq-sddm-diag.service \
    "$T/etc/systemd/system/multi-user.target.wants/tuwaiq-sddm-diag.service"

  # Keep X11 SDDM config
  mkdir -p "$T/etc/sddm.conf.d"
  cat > "$T/etc/sddm.conf.d/tuwaiqos.conf" <<'EOF'
[General]
DisplayServer=x11
MinimumVT=7

[Theme]
Current=breeze

[Users]
MaximumUid=60000

[X11]
ServerPath=/usr/bin/Xorg
ServerArguments=-nolisten tcp
EOF
  cat > "$T/etc/sddm.conf.d/autologin.conf" <<'EOF'
[Autologin]
User=tuwaiq
Session=plasma
Relogin=false
EOF

  # Prefer vesa for std VGA when DRM absent; modesetting when DRM present
  mkdir -p "$T/etc/X11/xorg.conf.d"
  cat > "$T/etc/X11/xorg.conf.d/20-qemuvga.conf" <<'EOF'
Section "Device"
    Identifier "Card0"
    Driver "vesa"
EndSection
Section "Screen"
    Identifier "Screen0"
    Device "Card0"
    DefaultDepth 24
    SubSection "Display"
        Depth 24
        Modes "1024x768"
    EndSubSection
EndSection
EOF

  # Groups
  if [[ -x /usr/sbin/chroot ]] || command -v chroot >/dev/null; then
    chroot "$T" usermod -aG video,render,input sddm 2>/dev/null || true
    chroot "$T" usermod -aG video,render,input tuwaiq 2>/dev/null || true
  fi

  # Verify kwin_x11 still present
  test -x "$T/usr/bin/kwin_x11"
  test -x "$T/usr/bin/startplasma-x11"
  test -e "$T/usr/lib/xorg/modules/drivers/vesa_drv.so"

  echo "remaining custom units:"
  ls "$T/etc/systemd/system/"*.service 2>/dev/null || true
  ls "$T/etc/systemd/system/graphical.target.wants/" 2>/dev/null || true
}

# Apply to rootfs cache + disk
fix_target "$ROOTFS"

LOOP=$(losetup -f --show --partscan "$DISK")
ensure_nodes "$LOOP"
mapfile -t parts < <(lsblk -ln -o NAME,TYPE "$LOOP" | awk '$2=="part"{print "/dev/" $1}' | sort)
mkdir -p "$MNT"
mount "${parts[2]}" "$MNT"
fix_target "$MNT"
umount "$MNT"
losetup -d "$LOOP"

echo "=== export ==="
rm -f /work/tuwaiqos-d0.qcow2 "$OUT/d0-proof.qcow2"
qemu-img convert -O qcow2 "$DISK" /work/tuwaiqos-d0.qcow2
cp -f /work/tuwaiqos-d0.qcow2 "$OUT/d0-proof.qcow2"
qemu-img check "$OUT/d0-proof.qcow2"
ls -lh "$OUT/d0-proof.qcow2"
echo CYCLE_DRM_FIX_OK
