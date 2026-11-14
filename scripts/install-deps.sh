#!/usr/bin/env bash
# Instala dependências de desenvolvimento no Linux (apt/dnf/pacman) ou macOS (Homebrew).
set -euo pipefail

if [[ "$(uname)" == "Darwin" ]]; then
  command -v brew >/dev/null || { echo "Instale o Homebrew: https://brew.sh"; exit 1; }
  xcode-select -p >/dev/null 2>&1 || xcode-select --install
  brew install ffmpeg-full node rustup-init   # ffmpeg-full: a fórmula `ffmpeg` vem sem drawtext
  command -v cargo >/dev/null || rustup-init -y
elif command -v apt-get >/dev/null; then
  sudo apt-get update
  sudo apt-get install -y ffmpeg build-essential curl pkg-config libssl-dev \
    libwebkit2gtk-4.1-dev libgtk-3-dev librsvg2-dev libayatana-appindicator3-dev
elif command -v dnf >/dev/null; then
  sudo dnf install -y ffmpeg-free webkit2gtk4.1-devel gtk3-devel librsvg2-devel openssl-devel curl gcc-c++
elif command -v pacman >/dev/null; then
  sudo pacman -S --needed ffmpeg webkit2gtk-4.1 gtk3 librsvg base-devel curl openssl
else
  echo "Gerenciador de pacotes não reconhecido; instale FFmpeg e as dependências do Tauri manualmente."
  exit 1
fi

command -v cargo >/dev/null || curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y
command -v node >/dev/null || echo "Instale Node.js ≥ 20 (https://nodejs.org)"

echo
ffmpeg -hide_banner -version | head -1
echo "Pronto. Próximo passo: cd apps/desktop && npm install && npm run tauri dev"
