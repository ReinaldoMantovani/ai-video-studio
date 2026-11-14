#!/usr/bin/env bash
# Copia o ffmpeg/ffprobe instalados para os recursos do app (empacotamento).
# Atenção à licença: builds com libx264/libx265 são GPL.
set -euo pipefail
dest="$(dirname "$0")/../apps/desktop/src-tauri/resources/ffmpeg"
mkdir -p "$dest"
for t in ffmpeg ffprobe; do
  src="$(command -v "$t")"
  cp "$src" "$dest/"
  echo "copiado: $src"
done
