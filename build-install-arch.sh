#!/usr/bin/env bash
set -euo pipefail

# Rustify Private Edition v1.4.0 — 1-Command Build & Install Script for Arch Linux

if [[ "${EUID}" -eq 0 ]]; then
    echo "Fehler: Bitte fuehre dieses Skript als normaler Benutzer aus (nicht mit sudo/root)."
    echo "makepkg fragt bei Bedarf automatisch nach sudo."
    exit 1
fi

SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
DIST_DIR="${SCRIPT_DIR}/dist"
mkdir -p "${DIST_DIR}"

echo "[1/3] Pruefe Arch Linux Build-Abhaengigkeiten..."
sudo pacman -S --needed --noconfirm base-devel pkgconf

if ! command -v cargo >/dev/null 2>&1; then
    echo "[1/3] Installiere Rust-Toolchain (rust)..."
    sudo pacman -S --needed --noconfirm rust
fi

echo "[2/3] Baue & installiere Arch-Paket (rustify-1.4.0-1-x86_64.pkg.tar.zst)..."
cd "${SCRIPT_DIR}/linux"
PKGDEST="${DIST_DIR}" makepkg -sfi

echo ""
echo "[3/3] Fertig! Rustify Private Edition v1.4.0 wurde installiert."
echo "      Paket: ${DIST_DIR}/rustify-1.4.0-1-x86_64.pkg.tar.zst"
echo "      Starten ueber das Anwendungsmenue ('Rustify') oder im Terminal mit: rustify"
echo ""
echo "Optionale Pakete (falls noch nicht vorhanden):"
echo "  - System-Audio Visualizer:  sudo pacman -S --needed pipewire-pulse"
echo "  - Amazon Music Import:      sudo pacman -S --needed chromium"
