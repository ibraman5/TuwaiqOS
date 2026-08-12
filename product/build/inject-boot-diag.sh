#!/usr/bin/env bash
set -euo pipefail
DISK=/work/tuwaiqos-d0.raw
LOOP=$(losetup --find --show --partscan "$DISK")
cleanup() {
  sync || true
  umount /work/mnt/boot/efi 2>/dev/null || true
  umount /work/mnt 2>/dev/null || true
  losetup -d "$LOOP" 2>/dev/null || true
}
trap cleanup EXIT

partx -u "$LOOP" 2>/dev/null || true
while read -r name majmin type; do
  [[ "$type" == "part" ]] || continue
  node="/dev/$name"
  if [[ ! -e "$node" ]]; then
    maj="${majmin%%:*}"; min="${majmin##*:}"
    mknod "$node" b "$maj" "$min"
    echo "created $node"
  fi
done < <(lsblk -ln -o NAME,MAJ:MIN,TYPE "$LOOP")

mapfile -t parts < <(lsblk -ln -o NAME,TYPE "$LOOP" | awk '$2=="part"{print "/dev/" $1}')
ESP="${parts[1]}"; ROOT="${parts[2]}"
mkdir -p /work/mnt
mount "$ROOT" /work/mnt
mkdir -p /work/mnt/boot/efi
mount "$ESP" /work/mnt/boot/efi || true

echo "=== SDDM conf ==="
ls /work/mnt/etc/sddm.conf.d/ || true
cat /work/mnt/etc/sddm.conf.d/* 2>/dev/null || true
echo "=== default target ==="
readlink -f /work/mnt/etc/systemd/system/default.target 2>/dev/null || true
ls -l /work/mnt/etc/systemd/system/default.target 2>/dev/null || true
echo "=== sessions ==="
ls /work/mnt/usr/share/xsessions 2>/dev/null || true
ls /work/mnt/usr/share/wayland-sessions 2>/dev/null || true
echo "=== graphics pkgs ==="
chroot /work/mnt dpkg-query -W mesa-vulkan-drivers libgl1-mesa-dri xserver-xorg-core xserver-xorg-video-modesetting plasma-workspace 2>&1 | cat || true
echo "=== dri ==="
ls /work/mnt/usr/lib/x86_64-linux-gnu/dri 2>/dev/null | head || true
echo "=== hostname/os ==="
cat /work/mnt/etc/hostname || true
grep PRETTY /work/mnt/etc/os-release || true

cat > /work/mnt/usr/local/sbin/d0-boot-diag.sh <<'EOF'
#!/bin/bash
exec >>/var/log/d0-boot-diag.txt 2>&1
echo "==== $(date -Is) ===="
echo "HOST=$(hostname)"
systemctl is-system-running || true
systemctl get-default || true
for u in graphical.target multi-user.target sddm.service display-manager.service; do
  echo -n "$u: "
  systemctl is-active "$u" || true
  systemctl status "$u" --no-pager -l 2>/dev/null | head -n 25 || true
  echo "----"
done
echo "=== journal sddm ==="
journalctl -b -u sddm --no-pager 2>/dev/null | tail -n 100 || true
echo "=== errors ==="
journalctl -b -p err --no-pager 2>/dev/null | tail -n 80 || true
echo "=== /dev/dri ==="
ls -la /dev/dri || true
echo "=== processes ==="
ps aux | egrep 'sddm|Xorg|Xwayland|plasma|kwin' | grep -v egrep || true
echo DIAG_COMPLETE
EOF
chmod +x /work/mnt/usr/local/sbin/d0-boot-diag.sh

cat > /work/mnt/etc/systemd/system/d0-boot-diag.service <<'EOF'
[Unit]
Description=TuwaiqOS D0 boot diagnostics
After=multi-user.target
Wants=multi-user.target

[Service]
Type=oneshot
ExecStartPre=/bin/sleep 25
ExecStart=/usr/local/sbin/d0-boot-diag.sh
RemainAfterExit=yes

[Install]
WantedBy=multi-user.target
EOF
mkdir -p /work/mnt/etc/systemd/system/multi-user.target.wants
ln -sf /etc/systemd/system/d0-boot-diag.service \
  /work/mnt/etc/systemd/system/multi-user.target.wants/d0-boot-diag.service

mkdir -p /work/mnt/etc/sddm.conf.d
cat > /work/mnt/etc/sddm.conf.d/autologin.conf <<'EOF'
[Autologin]
User=tuwaiq
Session=plasma
Relogin=false

[General]
DisplayServer=x11
EOF

# Ensure Xorg stack present enough for QEMU (install into rootfs if missing, copy critical bits)
if ! chroot /work/mnt dpkg-query -W xserver-xorg-core >/dev/null 2>&1; then
  echo "NOTE: xserver-xorg-core missing on disk image"
fi

cleanup
trap - EXIT
echo "Refreshing qcow2"
qemu-img convert -O qcow2 "$DISK" /work/tuwaiqos-d0.qcow2
cp -f /work/tuwaiqos-d0.qcow2 /src/product/out/tuwaiqos-d0.qcow2
ls -lh /src/product/out/tuwaiqos-d0.qcow2
echo INJECT_OK
