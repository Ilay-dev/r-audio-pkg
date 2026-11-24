#!/usr/bin/env bash
set -euo pipefail

# Rustify Private Edition v1.4.0 — 1-Command Build & Install Script for Linux Mint 21 & 22

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
VERSION="1.4.0"
PKG_NAME="rustify-private"
ARCH="amd64"
DIST_DIR="${SCRIPT_DIR}/dist"
DEB_FILE="${DIST_DIR}/${PKG_NAME}_${VERSION}_${ARCH}.deb"

echo "[1/4] Installiere Build- & Runtime-Abhaengigkeiten fuer Linux Mint..."
sudo apt-get update
sudo apt-get install -y \
    build-essential \
    pkg-config \
    libasound2-dev \
    libgtk-3-dev \
    libdbus-1-dev \
    libxkbcommon-dev \
    libxkbcommon-x11-0 \
    libxkbcommon-x11-dev \
    libxcb-render0-dev \
    libxcb-shape0-dev \
    libxcb-xfixes0-dev \
    curl \
    yt-dlp \
    ffmpeg

if ! command -v cargo >/dev/null 2>&1; then
    echo "[2/4] Installiere aktuelle Rust-Toolchain via rustup..."
    curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal
    # shellcheck disable=SC1091
    source "${HOME}/.cargo/env"
fi

echo "[2/4] Kompiliere Rustify Private Edition v${VERSION} (Release)..."
cd "${SCRIPT_DIR}"
cargo build --release --locked

echo "[3/4] Erstelle Debian/Mint-Paket (${DEB_FILE})..."
mkdir -p "${DIST_DIR}"
STAGE_DIR="$(mktemp -d)"
trap 'rm -rf "${STAGE_DIR}"' EXIT

mkdir -p \
    "${STAGE_DIR}/DEBIAN" \
    "${STAGE_DIR}/usr/bin" \
    "${STAGE_DIR}/usr/share/applications" \
    "${STAGE_DIR}/usr/share/icons/hicolor/256x256/apps"

install -Dm755 "${SCRIPT_DIR}/target/release/Rustify" \
    "${STAGE_DIR}/usr/bin/rustify"
install -Dm644 "${SCRIPT_DIR}/assets/rustify_256.png" \
    "${STAGE_DIR}/usr/share/icons/hicolor/256x256/apps/rustify.png"
install -Dm644 "${SCRIPT_DIR}/linux/rustify.desktop" \
    "${STAGE_DIR}/usr/share/applications/rustify.desktop"

cat > "${STAGE_DIR}/DEBIAN/control" <<EOF
Package: ${PKG_NAME}
Version: ${VERSION}
Section: sound
Priority: optional
Architecture: ${ARCH}
Maintainer: Ilay-dev <https://github.com/Ilay-dev/Rustify>
Depends: libasound2 | libasound2t64, libgtk-3-0 | libgtk-3-0t64, libdbus-1-3, libxkbcommon0, libxkbcommon-x11-0, libgl1, curl, yt-dlp, ffmpeg
Recommends: pipewire-pulse | pulseaudio-utils, chromium | chromium-browser | google-chrome-stable
Description: Rustify Private Edition — Minimalist Liquid Glass Music Player & BPM DJ Mixer
 Minimalist Liquid Glass Music Player with BPM DJ Mixing, AudioVisualizer,
 YouTube search/download (yt-dlp), and Spotify, Amazon Music & YouTube playlist sync.
EOF

cat > "${STAGE_DIR}/DEBIAN/postinst" <<'EOF'
#!/usr/bin/env bash
set -e
if command -v gtk-update-icon-cache >/dev/null 2>&1; then
    gtk-update-icon-cache -f -t /usr/share/icons/hicolor >/dev/null 2>&1 || true
fi
if command -v update-desktop-database >/dev/null 2>&1; then
    update-desktop-database /usr/share/applications >/dev/null 2>&1 || true
fi
EOF
chmod 0755 "${STAGE_DIR}/DEBIAN/postinst"

dpkg-deb --build --root-owner-group "${STAGE_DIR}" "${DEB_FILE}"

echo "[4/4] Installiere ${DEB_FILE}..."
sudo apt install -y "${DEB_FILE}"

echo ""
echo "Fertig! Rustify Private Edition v${VERSION} wurde auf Linux Mint installiert."
echo "  Paket:   ${DEB_FILE}"
echo "  Starten: Anwendungsmenue ('Rustify') oder im Terminal: rustify"
echo ""
echo "Hinweis zu optionalen Features:"
echo "  - System-Audio Visualizer nutzt pipewire-pulse / pulseaudio-utils (parec)."
echo "  - Fuer Amazon Music Playlist-Import: sudo apt install -y chromium"
