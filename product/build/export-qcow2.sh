#!/usr/bin/env bash
set -euo pipefail
export DEBIAN_FRONTEND=noninteractive
apt-get update -qq >/dev/null
apt-get install -y -qq qemu-utils e2fsprogs util-linux >/dev/null
DISK=/work/tuwaiqos-d0.raw
test -f "$DISK"
rm -f /work/tuwaiqos-d0.qcow2 /work/tuwaiqos-d0-comp.qcow2
echo "converting raw -> qcow2"
qemu-img convert -O qcow2 "$DISK" /work/tuwaiqos-d0.qcow2
qemu-img check /work/tuwaiqos-d0.qcow2
mkdir -p /src/product/out
rm -f /src/product/out/tuwaiqos-d0.qcow2
cp -f /work/tuwaiqos-d0.qcow2 /src/product/out/tuwaiqos-d0.qcow2
ls -lh /src/product/out/tuwaiqos-d0.qcow2
qemu-img check /src/product/out/tuwaiqos-d0.qcow2
echo EXPORT_OK
