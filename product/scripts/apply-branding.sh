#!/usr/bin/env bash
# Apply TuwaiqOS branding into a target root filesystem ($1).
set -euo pipefail

ROOT="${1:-}"
if [[ -z "${ROOT}" || ! -d "${ROOT}" ]]; then
  echo "usage: apply-branding.sh <rootfs>" >&2
  exit 2
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PRODUCT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
REPO_ROOT="$(cd "${PRODUCT_ROOT}/.." && pwd)"

# Prefer repo-relative product tree when invoked from scripts/
if [[ -d "${REPO_ROOT}/product/branding" ]]; then
  BRAND="${REPO_ROOT}/product/branding"
  CFG="${REPO_ROOT}/product/config"
  DESKTOP="${REPO_ROOT}/product/desktop"
else
  BRAND="${PRODUCT_ROOT}/branding"
  CFG="${PRODUCT_ROOT}/config"
  DESKTOP="${PRODUCT_ROOT}/desktop"
fi

install -d "${ROOT}/usr/share/plasma/look-and-feel"
cp -a "${BRAND}/plasma/look-and-feel/org.tuwaiqos.desktop" \
  "${ROOT}/usr/share/plasma/look-and-feel/"

install -d "${ROOT}/usr/share/wallpapers"
cp -a "${BRAND}/wallpapers/TuwaiqOS" "${ROOT}/usr/share/wallpapers/"

install -d "${ROOT}/etc/sddm.conf.d"
cp "${DESKTOP}/sddm/tuwaiqos.conf" "${ROOT}/etc/sddm.conf.d/tuwaiqos.conf"

# os-release identity (do not erase useful Ubuntu fields; overlay Tuwaiq naming)
if [[ -f "${ROOT}/etc/os-release" ]]; then
  cp "${ROOT}/etc/os-release" "${ROOT}/etc/os-release.ubuntu-base"
fi
cat > "${ROOT}/etc/os-release" <<'EOF'
PRETTY_NAME="TuwaiqOS"
NAME="TuwaiqOS"
ID=tuwaiqos
ID_LIKE="ubuntu debian"
VERSION_ID="d0"
VERSION="D0 (Desktop Foundation)"
HOME_URL="https://github.com/italamrii/TuwaiqOS"
SUPPORT_URL="https://github.com/italamrii/TuwaiqOS"
BUG_REPORT_URL="https://github.com/italamrii/TuwaiqOS/issues"
PRIVACY_POLICY_URL="https://github.com/italamrii/TuwaiqOS"
UBUNTU_CODENAME=noble
EOF

install -d "${ROOT}/etc/xdg/tuwaiqos"
cp "${CFG}/tuwaiqos.conf" "${ROOT}/etc/xdg/tuwaiqos/tuwaiqos.conf"
cp "${CFG}/locale.conf" "${ROOT}/etc/locale.conf"

# Default Plasma wallpaper hint for new users
install -d "${ROOT}/etc/xdg/plasma-org.kde.plasma.desktop-appletsrc.d"
cat > "${ROOT}/usr/share/tuwaiqos/README-branding.txt" <<'EOF'
TuwaiqOS D0 branding applied.
Look-and-feel: org.tuwaiqos.desktop
Wallpaper: TuwaiqOS
Session: Plasma Wayland via SDDM
EOF

# Ensure skel has a minimal plasma config selecting wallpaper when possible
install -d "${ROOT}/etc/skel/.config"
cat > "${ROOT}/etc/skel/.config/plasma-org.kde.plasma.desktop-appletsrc" <<'EOF'
[Containments][1][Wallpaper][org.kde.image][General]
Image=file:///usr/share/wallpapers/TuwaiqOS/contents/images/1920x1080.svg
EOF

echo "Tuwaiq branding applied to ${ROOT}"
