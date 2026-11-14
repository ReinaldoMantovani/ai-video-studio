# Desenvolvimento

## Pré-requisitos

| Ferramenta | Versão | Observação |
|---|---|---|
| Rust | ≥ 1.80 (testado 1.97) | `rustup` |
| Node.js | ≥ 20 (testado 22) | npm |
| FFmpeg + FFprobe | ≥ 6.0 (testado 6.1) | no `PATH`, ou `AVS_FFMPEG_DIR`, ou empacotado em `src-tauri/resources/ffmpeg/` |
| Dependências do Tauri | — | Linux: webkit2gtk-4.1, gtk3, librsvg; Windows: WebView2; macOS: Xcode CLT |

Instalação automática: `scripts/install-deps.sh` (Linux/macOS) ou `scripts/install-deps.ps1` (Windows).

## Comandos

```bash
# tudo de uma vez: testes Rust + frontend + build
scripts/verify.sh

# app desktop em modo desenvolvimento (hot reload do frontend)
cd apps/desktop && npm install && npm run tauri dev

# instaladores/pacotes (ver README: deb, rpm, appimage, nsis, msi, dmg)
cd apps/desktop && npm run tauri build -- --bundles deb,rpm,appimage

# E2E da interface (Linux): webkit2gtk-driver + tauri-driver + Xvfb
cargo install tauri-driver --locked
cd apps/desktop && npm run build && cd ../..
cargo build --release -p ai-video-studio --features custom-protocol -p avs-cli
xvfb-run -a node e2e/run.mjs [--only cenario1,cenario2] [--keep]

# testes
cargo test --workspace                 # inclui integração com FFmpeg real
cd apps/desktop && npm test            # vitest
cd apps/desktop && npm run typecheck

# regenerar os tipos TypeScript a partir do Rust (após mudar structs)
cd apps/desktop && npm run bindings

# CLI
cargo run -p avs-cli --release -- --help
cargo run -p avs-cli --release -- demo --out /tmp/avs-demo --seconds 60
```

Variáveis de ambiente úteis:

| Variável | Efeito |
|---|---|
| `AVS_FFMPEG_DIR` | pasta com `ffmpeg`/`ffprobe` a usar |
| `AVS_LIBRARY` | biblioteca da CLI e do app desktop (padrão: diretório de dados do usuário) |
| `AVS_PRESETS_DIR` | pasta de presets alternativa |
| `RUST_LOG=avs=debug` | logs estruturados (inclui argumentos de cada FFmpeg) |

## Convenções

- **Toda alteração de timeline passa por `EditCommand`.** Não altere `Project.timeline` diretamente na camada de serviço (exceto `EditSession::replace`, usado para restaurar versões).
- **Tempo é sempre `Time` (flicks).** Converta para segundos só na borda (argumentos do FFmpeg e UI).
- **Nada de shell.** Processos são criados com `Command::args`. Valores que vão para filtergraph passam por `escape_filter_value`, e cores por `ff_color`.
- **Nada simulado.** Progresso vem do `-progress` do FFmpeg; sucesso vem de validação real.
- **Novos tipos compartilhados**: derive `TS` + `#[ts(export)]`, rode `npm run bindings` e versione os `.ts` gerados.

## Adicionando uma operação de edição

1. Adicione a variante em `EditCommand` (`crates/avs-core/src/command.rs`) e o rótulo em `label()`.
2. Implemente em `apply_inner`. Se necessário, estenda `validate.rs` com o novo invariante.
3. Escreva um teste em `crates/avs-core/src/tests.rs` (aplicar, desfazer, refazer, rejeitar caso inválido).
4. Se a operação afeta a renderização, cubra no compilador e adicione um caso em `crates/avs-media/tests/engine.rs`.
5. `npm run bindings`. A UI passa a enxergar o novo tipo.

## Reprodução do preview no Linux

O WebKitGTK não reproduz `<video>` vindo do protocolo `asset://`. No Linux, o app sobe um servidor HTTP local, protegido por token e restrito à biblioteca (`media_server.rs`, detalhes em `SECURITY.md`). O frontend pega a URL base com o comando `media_base_url`. No Windows e no macOS, a URL base é nula e o preview usa o `asset://` normalmente.

O player tem duas proteções para quirks do WebKitGTK, cobertas pelo E2E `reproducao_e_preview_por_janelas`:
- `onEnded` decide a troca de janela;
- um vigia reinicia o pipeline se o tempo parar durante a reprodução.
