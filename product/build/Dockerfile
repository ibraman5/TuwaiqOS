# TuwaiqOS Product D0 image builder
# Requires: privileged container, loop devices, network for apt.
FROM ubuntu:24.04

ENV DEBIAN_FRONTEND=noninteractive

RUN apt-get update && apt-get install -y --no-install-recommends \
    debootstrap \
    rsync \
    parted \
    gdisk \
    dosfstools \
    e2fsprogs \
    util-linux \
    mount \
    grub-common \
    grub-pc-bin \
    grub-efi-amd64-bin \
    grub-efi-amd64 \
    shim-signed \
    qemu-utils \
    ca-certificates \
    curl \
    xz-utils \
    systemd-sysv \
    && rm -rf /var/lib/apt/lists/*

WORKDIR /src
CMD ["bash", "product/build/build-rootfs-disk.sh"]
