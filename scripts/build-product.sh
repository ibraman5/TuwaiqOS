#!/usr/bin/env bash
# Host entry: build TuwaiqOS Product D0 image via Docker.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
cd "${ROOT_DIR}"

need() { command -v "$1" >/dev/null 2>&1 || { echo "missing dependency: $1" >&2; exit 1; }; }
need docker

echo "[build-product] verifying docker"
docker info >/dev/null

echo "[build-product] building builder image"
docker build -t tuwaiqos-product-builder:d0 -f product/build/Dockerfile product/build

mkdir -p product/out product/build/work
echo "[build-product] ensuring Docker volume tuwaiqos-d0-work"
docker volume create tuwaiqos-d0-work >/dev/null

# Debootstrap must run on a Linux filesystem volume — not a Windows bind mount.
echo "[build-product] running privileged disk build (may take a long time)"
docker run --rm --privileged \
  -v "${ROOT_DIR}:/src" \
  -v tuwaiqos-d0-work:/work \
  -e TUWAIQ_WORK=/work \
  -e TUWAIQ_DEBOOTSTRAP_RETRIES=8 \
  -e TUWAIQ_UBUNTU_MIRROR=http://azure.archive.ubuntu.com/ubuntu \
  -w /src \
  tuwaiqos-product-builder:d0 \
  bash product/build/build-rootfs-disk.sh

echo "[build-product] artifacts:"
ls -lh product/out || true
