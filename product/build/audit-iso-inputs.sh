#!/bin/bash
# Read-only audit of the existing rootfs to plan the Developer ISO build.
R=/work/rootfs

echo "=== os-release ==="
head -3 "$R/etc/os-release" 2>/dev/null || echo "MISSING rootfs"

echo "=== key binaries ==="
for b in usr/bin/git usr/bin/ssh usr/bin/konsole usr/bin/plasmashell usr/bin/sddm usr/sbin/sddm \
         usr/bin/nmcli usr/bin/vim usr/bin/nano usr/bin/kate usr/bin/dolphin; do
  if [ -e "$R/$b" ]; then echo "HAVE $b"; else echo "MISS $b"; fi
done

echo "=== live boot support ==="
for p in casper live-boot live-boot-initramfs-tools squashfs-tools; do
  if [ -d "$R/usr/share/doc/$p" ]; then echo "HAVE $p"; else echo "MISS $p"; fi
done

echo "=== kernel/initrd ==="
ls -1 "$R"/boot/vmlinuz-* "$R"/boot/initrd.img-* 2>/dev/null || echo "none"

echo "=== branding present ==="
for f in usr/share/tuwaiqos/icons/tuwaiq-mark.svg usr/share/wallpapers/TuwaiqOS/metadata.desktop \
         usr/share/color-schemes/TuwaiqDark.colors usr/share/color-schemes/TuwaiqLight.colors \
         etc/sddm.conf.d/tuwaiqos.conf; do
  if [ -e "$R/$f" ]; then echo "HAVE $f"; else echo "MISS $f"; fi
done

echo "=== rootfs size ==="
du -sh "$R" 2>/dev/null | tail -1

echo "=== volume space ==="
df -h /work | tail -1

echo "AUDIT_DONE"
