#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"
VERSION="${1:-1.4.0}"
PKG_NAME="rustify-private"
ARCH="amd64"
BIN_PATH="${REPO_ROOT}/target/release/Rustify"

if [[ ! -f "${BIN_PATH}" ]]; then
    echo "ERROR: Release binary not found at ${BIN_PATH}" >&2
    exit 1
fi

STAGE="$(mktemp -d)"
trap 'rm -rf "${STAGE}"' EXIT

install -Dm755 "${BIN_PATH}" "${STAGE}/usr/bin/rustify"
install -Dm644 "${REPO_ROOT}/linux/rustify.desktop" "${STAGE}/usr/share/applications/rustify.desktop"
install -Dm644 "${REPO_ROOT}/assets/rustify_256.png" "${STAGE}/usr/share/icons/hicolor/256x256/apps/rustify.png"

INSTALLED_SIZE="$(du -sk "${STAGE}/usr" | awk '{print $1}')"

mkdir -p "${STAGE}/DEBIAN"
cat > "${STAGE}/DEBIAN/control" <<EOF
Package: ${PKG_NAME}
Version: ${VERSION}
Section: sound
Priority: optional
Architecture: ${ARCH}
Maintainer: Ilay-dev
Depends: libasound2 | libasound2t64, libgtk-3-0 | libgtk-3-0t64, libdbus-1-3, libxkbcommon0, libxkbcommon-x11-0, libgl1, curl, yt-dlp, ffmpeg
Recommends: pipewire-pulse | pulseaudio-utils, chromium | chromium-browser | google-chrome-stable
Installed-Size: ${INSTALLED_SIZE}
Description: Rustify — Local Music Player & DJ Mixer
EOF

cat > "${STAGE}/DEBIAN/postinst" <<'EOF'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
exit 0
EOF

cat > "${STAGE}/DEBIAN/postrm" <<'EOF'
#!/bin/sh
set -e
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database -q /usr/share/applications || true
fi
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -q -t -f /usr/share/icons/hicolor || true
fi
exit 0
EOF

chmod 0644 "${STAGE}/DEBIAN/control"
chmod 0755 "${STAGE}/DEBIAN/postinst" "${STAGE}/DEBIAN/postrm"

mkdir -p "${REPO_ROOT}/dist"
OUT_DEB="${REPO_ROOT}/dist/${PKG_NAME}_${VERSION}_${ARCH}.deb"
dpkg-deb --build --root-owner-group "${STAGE}" "${OUT_DEB}"
ls -lh "${OUT_DEB}"
