#!/usr/bin/env bash
# Verificação completa: testes Rust (com FFmpeg real), frontend e build.
set -euo pipefail
cd "$(dirname "$0")/.."

command -v ffmpeg >/dev/null || { echo "FFmpeg não encontrado no PATH (veja scripts/install-deps.sh)"; exit 1; }

echo "▸ Rust: formatação e testes"
cargo test --workspace --exclude ai-video-studio

echo "▸ Rust: app desktop (compilação)"
cargo build -p ai-video-studio

echo "▸ Frontend"
cd apps/desktop
npm ci --no-audit --no-fund
npm run typecheck
npm test
npx vite build

echo "✔ tudo certo"
