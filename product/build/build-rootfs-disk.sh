#!/usr/bin/env bash
# Build a bootable TuwaiqOS Product D0 disk image inside Linux/Docker.
# Output: product/out/tuwaiqos-d0.raw (+ .qcow2 when qemu-img exists)
#
# IMPORTANT: debootstrap/rootfs must live on a Linux filesystem (Docker volume
# or container-local path). Bind-mounted Windows/NTFS paths fail package extract
# ("tar failed") because of device nodes / permissions.
set -euo pipefail

ROOT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
OUT_DIR="${ROOT_DIR}/product/out"
# Prefer Linux-backed work dir (docker volume at /work). Fall back only for native Linux hosts.
WORK="${TUWAIQ_WORK:-/work}"
if [[ ! -d "${WORK}" ]] || [[ "${WORK}" == /work && ! -w /work ]]; then
  WORK="${ROOT_DIR}/product/build/work"
fi
ROOTFS="${WORK}/rootfs"
DISK_BUILD="${WORK}/tuwaiqos-d0.raw"
DISK="${OUT_DIR}/tuwaiqos-d0.raw"
PKG_LIST="${ROOT_DIR}/product/packages/d0-ubuntu2404.list"
DISK_SIZE_GB="${TUWAIQ_DISK_SIZE_GB:-12}"
MIRROR="${TUWAIQ_UBUNTU_MIRROR:-http://azure.archive.ubuntu.com/ubuntu}"
DEBOOTSTRAP_RETRIES="${TUWAIQ_DEBOOTSTRAP_RETRIES:-3}"
RESUME="${TUWAIQ_RESUME:-auto}"
STAGE_FILE="${WORK}/.d0-stage"

export DEBIAN_FRONTEND=noninteractive

log() { printf '[build-product] %s\n' "$*"; }
die() { printf '[build-product] ERROR: %s\n' "$*" >&2; exit 1; }

need() { command -v "$1" >/dev/null 2>&1 || die "missing dependency: $1"; }

rootfs_ready() {
  [[ -f "${ROOTFS}/etc/os-release" && -f "${ROOTFS}/var/lib/dpkg/status" ]]
}

packages_ready() {
  [[ -f "${STAGE_FILE}" ]] && grep -qx 'packages-installed' "${STAGE_FILE}" && \
    chroot "${ROOTFS}" dpkg-query -W plasma-desktop sddm linux-image-virtual >/dev/null 2>&1
}

chroot_mount() {
  mount --bind /dev "${ROOTFS}/dev" 2>/dev/null || true
  mount --bind /proc "${ROOTFS}/proc" 2>/dev/null || true
  mount --bind /sys "${ROOTFS}/sys" 2>/dev/null || true
  mkdir -p "${ROOTFS}/dev/pts"
  mount -t devpts devpts "${ROOTFS}/dev/pts" 2>/dev/null || true
}

chroot_umount() {
  umount "${ROOTFS}/dev/pts" 2>/dev/null || true
  umount "${ROOTFS}/dev" 2>/dev/null || true
  umount "${ROOTFS}/proc" 2>/dev/null || true
  umount "${ROOTFS}/sys" 2>/dev/null || true
}

sync_disks() {
  if command -v sync >/dev/null 2>&1; then
    sync
  fi
}

copy_rootfs_to_disk() {
  local attempt=1
  while (( attempt <= 3 )); do
    log "copy rootfs to disk (attempt ${attempt}/3 via tar)"
    if tar -C "${ROOTFS}" -cpf - . | tar -C "${WORK}/mnt" -xpf - ; then
      sync_disks
      echo "rootfs-on-disk" > "${WORK}/.d0-rootfs-on-disk"
      return 0
    fi
    log "tar copy failed on attempt ${attempt}"
    attempt=$((attempt + 1))
    sleep 5
  done
  die "rootfs copy failed after 3 attempts"
}

verify_disk_partition_table() {
  local disk="$1"
  log "verify GPT layout on ${disk}"
  parted -s "${disk}" unit MiB print
  parted -s "${disk}" print 2>/dev/null | grep -qi 'bios_grub' || \
    die "missing bios_grub partition (required for SeaBIOS+GPT)"
  local count
  count="$(parted -s "${disk}" unit s print | awk '/^ [0-9]+/{c++} END{print c+0}')"
  [[ "${count}" -ge 3 ]] || die "disk image missing expected 3 GPT partitions (found ${count})"
}

disk_layout_valid() {
  [[ -f "${DISK_BUILD}" ]] || return 1
  parted -s "${DISK_BUILD}" print 2>/dev/null | grep -qi 'bios_grub' || return 1
  local count
  count="$(parted -s "${DISK_BUILD}" unit s print 2>/dev/null | awk '/^ [0-9]+/{c++} END{print c+0}')"
  [[ "${count}" -ge 3 ]]
}

# Docker builder containers often lack udev: kernel partitions exist (lsblk) but
# /dev/loopNpM nodes may be missing. Discover via lsblk and mknod when needed.
ensure_partition_nodes() {
  local loop="$1"
  if command -v partx >/dev/null 2>&1; then
    partx -a "${loop}" 2>/dev/null || partx -u "${loop}" 2>/dev/null || true
  fi
  blockdev --rereadpt "${loop}" 2>/dev/null || true
  partprobe "${loop}" 2>/dev/null || true

  while read -r name majmin type; do
    [[ "${type}" == "part" ]] || continue
    local node="/dev/${name}"
    if [[ ! -e "${node}" ]]; then
      local maj="${majmin%%:*}" min="${majmin##*:}"
      mknod "${node}" b "${maj}" "${min}"
      log "created missing partition node ${node} (${maj}:${min})"
    fi
  done < <(lsblk -ln -o NAME,MAJ:MIN,TYPE "${loop}")
}

discover_loop_partitions() {
  local disk="$1"
  local attempt=1
  local loop=""
  while (( attempt <= 3 )); do
    loop="$(losetup --find --show --partscan "${disk}")"
    log "attached loop ${loop} (attempt ${attempt}/3)"
    log "losetup -l:"; losetup -l "${loop}" || losetup -a || true
    ensure_partition_nodes "${loop}"
    # Sort by name so p1/p2/p3 order is stable (unsorted lsblk can reorder).
    mapfile -t _parts < <(lsblk -ln -o NAME,TYPE "${loop}" | awk '$2=="part"{print "/dev/" $1}' | sort)
    if ((${#_parts[@]} >= 3)) && [[ -e "${_parts[0]}" && -e "${_parts[1]}" && -e "${_parts[2]}" ]]; then
      LOOP="${loop}"
      BIOS_PART="${_parts[0]}"
      ESP_PART="${_parts[1]}"
      ROOT_PART="${_parts[2]}"
      log "partition map BIOS=${BIOS_PART} ESP=${ESP_PART} ROOT=${ROOT_PART}"
      lsblk -ln -o NAME,MAJ:MIN,TYPE,SIZE "${loop}"
      return 0
    fi
    if ((${#_parts[@]} >= 2)) && [[ -e "${_parts[0]}" && -e "${_parts[1]}" ]]; then
      log "WARN: only ${#_parts[@]} partitions (legacy layout without bios_grub)"
    fi
    log "partition nodes not ready; lsblk for ${loop}:"
    lsblk "${loop}" || lsblk || true
    ls -la "${loop}"* 2>/dev/null || true
    losetup -d "${loop}" 2>/dev/null || true
    loop=""
    attempt=$((attempt + 1))
    sleep 2
  done
  die "loop partitions not found for disk image (no usable /dev nodes after 3 attempts)"
}

mount_disk_partitions() {
  mkdir -p "${WORK}/mnt"
  mount "${ROOT_PART}" "${WORK}/mnt"
  mkdir -p "${WORK}/mnt/boot/efi"
  mount "${ESP_PART}" "${WORK}/mnt/boot/efi"
}

disk_image_has_rootfs() {
  [[ -f "${WORK}/.d0-rootfs-on-disk" ]] && \
    [[ -f "${WORK}/mnt/etc/os-release" ]] && \
    [[ -d "${WORK}/mnt/usr/share/plasma" ]] && \
    { [[ -x "${WORK}/mnt/usr/sbin/sddm" ]] || [[ -x "${WORK}/mnt/usr/bin/sddm" ]]; }
}

force_rootfs_recopy() {
  [[ "${TUWAIQ_FORCE_ROOTFS_RECOPY:-0}" == "1" ]] && return 0
  [[ ! -f "${WORK}/.d0-rootfs-on-disk" ]]
}

log "repo=${ROOT_DIR}"
log "work=${WORK} (must be Linux FS for debootstrap)"
log "mirror=${MIRROR} resume=${RESUME}"
need debootstrap
need chroot
need tar
need truncate
need parted
need losetup
need mkfs.ext4
need mkfs.vfat
need mount
need grub-install
need rsync

mkdir -p "${OUT_DIR}" "${WORK}"

if [[ "${RESUME}" == "auto" ]] && rootfs_ready; then
  log "resume: reusing existing rootfs at ${ROOTFS}"
else
  rm -rf "${ROOTFS}"
  mkdir -p "${ROOTFS}"

  log "debootstrap ubuntu 24.04 (noble)"
  attempt=1
  while true; do
    rm -rf "${ROOTFS}"
    mkdir -p "${ROOTFS}"
    if debootstrap --arch=amd64 --variant=minbase \
      --components=main,universe \
      --include=systemd-sysv,sudo,locales \
      noble "${ROOTFS}" "${MIRROR}"; then
      break
    fi
    if (( attempt >= DEBOOTSTRAP_RETRIES )); then
      die "debootstrap failed after ${DEBOOTSTRAP_RETRIES} attempts (network/mirror)"
    fi
    log "debootstrap attempt ${attempt} failed; retrying in $((attempt * 15))s"
    sleep $((attempt * 15))
    attempt=$((attempt + 1))
  done

  log "configure apt sources"
  cat > "${ROOTFS}/etc/apt/sources.list" <<EOF
deb ${MIRROR} noble main restricted universe multiverse
deb ${MIRROR} noble-updates main restricted universe multiverse
deb http://security.ubuntu.com/ubuntu noble-security main restricted universe multiverse
EOF
fi

if ! packages_ready; then
  log "install D0 package set"
  mapfile -t PKGS < <(grep -vE '^\s*(#|$)' "${PKG_LIST}" | tr -d '\r')
  [[ ${#PKGS[@]} -gt 0 ]] || die "empty package list: ${PKG_LIST}"
  log "packages=${#PKGS[@]} first=${PKGS[0]}"

  chroot_mount
  trap chroot_umount EXIT

  # Ensure sources exist when resuming an interrupted build.
  if [[ ! -f "${ROOTFS}/etc/apt/sources.list" ]]; then
    cat > "${ROOTFS}/etc/apt/sources.list" <<EOF
deb ${MIRROR} noble main restricted universe multiverse
deb ${MIRROR} noble-updates main restricted universe multiverse
deb http://security.ubuntu.com/ubuntu noble-security main restricted universe multiverse
EOF
  fi

  chroot "${ROOTFS}" dpkg --configure -a || true
  chroot "${ROOTFS}" apt-get -o Acquire::Retries=3 -o Acquire::http::Timeout=60 update
  if ! ls "${ROOTFS}/var/lib/apt/lists/"*_main_binary-amd64_Packages >/dev/null 2>&1; then
    die "apt indexes missing main amd64 Packages after update"
  fi
  chroot "${ROOTFS}" apt-cache policy linux-image-virtual | head -n 8 || true

  attempt=1
  while true; do
    if chroot "${ROOTFS}" apt-get -o Acquire::Retries=3 install -y --no-install-recommends "${PKGS[@]}"; then
      break
    fi
    if (( attempt >= 3 )); then
      die "apt install failed after 3 attempts"
    fi
    log "apt install attempt ${attempt} failed; running fix-broken then retrying"
    chroot "${ROOTFS}" apt-get -o Acquire::Retries=3 -f install -y || true
    chroot "${ROOTFS}" dpkg --configure -a || true
    attempt=$((attempt + 1))
    sleep $((attempt * 10))
  done

  chroot "${ROOTFS}" apt-get clean
  chroot_umount
  trap - EXIT
  echo 'packages-installed' > "${STAGE_FILE}"
else
  log "resume: D0 package set already installed"
fi

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

log "create disk image (${DISK_SIZE_GB}G) on Linux work volume — GPT bios_grub + ESP + root"

cleanup() {
  sync_disks
  umount "${WORK}/mnt/dev/pts" 2>/dev/null || true
  umount "${WORK}/mnt/boot/efi" 2>/dev/null || true
  umount "${WORK}/mnt/dev" 2>/dev/null || true
  umount "${WORK}/mnt/proc" 2>/dev/null || true
  umount "${WORK}/mnt/sys" 2>/dev/null || true
  umount "${WORK}/mnt" 2>/dev/null || true
  losetup -d "${LOOP}" 2>/dev/null || true
}

if disk_layout_valid; then
  log "resume: reusing bios_grub GPT disk ${DISK_BUILD}"
  verify_disk_partition_table "${DISK_BUILD}"
  BIOS_PART=""
  ESP_PART=""
  ROOT_PART=""
  discover_loop_partitions "${DISK_BUILD}"
  trap cleanup EXIT
  mount_disk_partitions
  if disk_image_has_rootfs && ! force_rootfs_recopy; then
    log "resume: verified rootfs already on disk image; skipping tar copy"
  else
    if force_rootfs_recopy; then
      log "force recopy: wiping root partition and copying rootfs via tar"
      umount "${WORK}/mnt/boot/efi" 2>/dev/null || true
      umount "${WORK}/mnt" 2>/dev/null || true
      mkfs.ext4 -F "${ROOT_PART}"
      mount "${ROOT_PART}" "${WORK}/mnt"
      mkdir -p "${WORK}/mnt/boot/efi"
      mount "${ESP_PART}" "${WORK}/mnt/boot/efi"
      rm -f "${WORK}/.d0-rootfs-on-disk"
    fi
    copy_rootfs_to_disk
  fi
else
  log "creating new bios_grub GPT disk (legacy layout missing or absent)"
  rm -f "${DISK_BUILD}" "${WORK}/tuwaiqos-d0.qcow2"
  truncate -s "${DISK_SIZE_GB}G" "${DISK_BUILD}"
  parted -s "${DISK_BUILD}" mklabel gpt
  parted -s "${DISK_BUILD}" mkpart bios_grub 1MiB 2MiB
  parted -s "${DISK_BUILD}" set 1 bios_grub on
  parted -s "${DISK_BUILD}" mkpart ESP fat32 2MiB 514MiB
  parted -s "${DISK_BUILD}" set 2 esp on
  parted -s "${DISK_BUILD}" mkpart root ext4 514MiB 100%
  verify_disk_partition_table "${DISK_BUILD}"

  BIOS_PART=""
  ESP_PART=""
  ROOT_PART=""
  discover_loop_partitions "${DISK_BUILD}"
  trap cleanup EXIT

  mkfs.vfat -F32 "${ESP_PART}"
  mkfs.ext4 -F "${ROOT_PART}"
  mount_disk_partitions
  copy_rootfs_to_disk
fi

# Rewrite fstab with stable PARTUUIDs
P1="$(blkid -s PARTUUID -o value "${ESP_PART}")"
P2="$(blkid -s PARTUUID -o value "${ROOT_PART}")"
cat > "${WORK}/mnt/etc/fstab" <<EOF
PARTUUID=${P2} / ext4 defaults 0 1
PARTUUID=${P1} /boot/efi vfat umask=0077 0 1
EOF

mount --bind /dev "${WORK}/mnt/dev"
mount --bind /proc "${WORK}/mnt/proc"
mount --bind /sys "${WORK}/mnt/sys"
mkdir -p "${WORK}/mnt/dev/pts"
mount -t devpts devpts "${WORK}/mnt/dev/pts"

log "install GRUB BIOS (SeaBIOS + bios_grub — no blocklist --force)"
mkdir -p "${WORK}/mnt/boot/grub"

log "enable serial console for diagnostics (keeps graphical target)"
if [[ -f "${WORK}/mnt/etc/default/grub" ]]; then
  sed -i 's/^GRUB_CMDLINE_LINUX_DEFAULT=.*/GRUB_CMDLINE_LINUX_DEFAULT="quiet splash console=tty0 console=ttyS0,115200n8"/' \
    "${WORK}/mnt/etc/default/grub" || true
  grep -q '^GRUB_TERMINAL=' "${WORK}/mnt/etc/default/grub" || \
    echo 'GRUB_TERMINAL="console serial"' >> "${WORK}/mnt/etc/default/grub"
  grep -q '^GRUB_SERIAL_COMMAND=' "${WORK}/mnt/etc/default/grub" || \
    echo 'GRUB_SERIAL_COMMAND="serial --unit=0 --speed=115200"' >> "${WORK}/mnt/etc/default/grub"
fi

grub-install --target=i386-pc --boot-directory="${WORK}/mnt/boot" --root-directory="${WORK}/mnt" \
  --recheck "${LOOP}"

# Ensure initrd exists (minbase/package resumes can leave /boot without initrd.img).
if ! ls "${WORK}/mnt/boot"/initrd.img-* >/dev/null 2>&1; then
  log "generating initramfs (missing initrd.img-*)"
  chroot "${WORK}/mnt" update-initramfs -c -k all || \
    chroot "${WORK}/mnt" update-initramfs -u -k all || \
    die "update-initramfs failed and no initrd present"
fi

VMLINUZ="$(ls -1 "${WORK}/mnt/boot"/vmlinuz-* 2>/dev/null | tail -1 || true)"
INITRD="$(ls -1 "${WORK}/mnt/boot"/initrd.img-* 2>/dev/null | tail -1 || true)"
[[ -n "${VMLINUZ}" ]] || die "no vmlinuz found under ${WORK}/mnt/boot"
[[ -n "${INITRD}" && -s "${INITRD}" ]] || die "no initrd.img found under ${WORK}/mnt/boot"
VMLINUZ_REL="${VMLINUZ#${WORK}/mnt}"
INITRD_REL="${INITRD#${WORK}/mnt}"
log "writing grub.cfg kernel=${VMLINUZ_REL} initrd=${INITRD_REL} root=PARTUUID=${P2}"
cat > "${WORK}/mnt/boot/grub/grub.cfg" <<EOF
set timeout=3
set default=0
serial --unit=0 --speed=115200
terminal_input console serial
terminal_output console serial
menuentry "TuwaiqOS D0" {
  linux ${VMLINUZ_REL} root=PARTUUID=${P2} ro quiet splash console=tty0 console=ttyS0,115200n8
  initrd ${INITRD_REL}
}
EOF

test -s "${WORK}/mnt/boot/grub/grub.cfg" || die "grub.cfg not generated"
grep -q "initrd ${INITRD_REL}" "${WORK}/mnt/boot/grub/grub.cfg" || die "grub.cfg missing initrd path"
grep -q "PARTUUID=${P2}" "${WORK}/mnt/boot/grub/grub.cfg" || die "grub.cfg missing root PARTUUID"
test -f "${WORK}/mnt/boot/grub/i386-pc/core.img" || die "grub core.img missing after BIOS install"
log "grub.cfg:"; head -20 "${WORK}/mnt/boot/grub/grub.cfg"
log "GRUB BIOS install OK (core.img present, no --force)"

cleanup
trap - EXIT

log "finalize disk image on work volume (host export deferred)"
if command -v qemu-img >/dev/null 2>&1; then
  log "writing qcow2 on work volume"
  qemu-img convert -O qcow2 "${DISK_BUILD}" "${WORK}/tuwaiqos-d0.qcow2"
fi

SIZE_BYTES="$(stat -c%s "${DISK_BUILD}")"
log "OUTPUT_PATH=${DISK_BUILD}"
log "OUTPUT_SIZE_BYTES=${SIZE_BYTES}"
log "HOST_EXPORT=deferred"
echo 'disk-finalized' >> "${STAGE_FILE}"
log "done"
