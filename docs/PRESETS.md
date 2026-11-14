# Presets

Os presets ficam em **arquivos JSON versionados**, não em condicionais no código:

- `presets/editorial/NN_<id>.json`: 21 presets editoriais (empacotados com o app)
- `presets/export_profiles.json`: perfis de exportação
- `<app-data>/presets/editorial/*.json`: presets do usuário (criados, duplicados ou importados)

Cada arquivo é validado ao carregar (`EditorialPreset::validate`): versão do schema, `id` seguro para nome de arquivo, resolução par e pesos de pontuação somando 1,0. Um arquivo inválido é **reportado** (`avs presets` e log), nunca ignorado em silêncio.

## O que já usa os presets (Fase 1)

- **Novo projeto**: resolução, proporção e FPS do projeto vêm do preset (`project_settings`).
- **Exportação**: o perfil sugerido é `export_profile` do preset.
- **Player**: as guias de área segura vêm de `safety.safe_area`.
- **Navegador de presets** no dashboard: exibe todo o conteúdo do arquivo.

Ritmo, regras de corte, legendas, reenquadramento, narrativa e pesos de pontuação são consumidos pelo motor de edição automática (Fase 4) e pelo gerador de cortes (Fase 5). Os campos já estão definidos e preenchidos para que esses motores não precisem de novos formatos.

## Campos

| Campo | Significado |
|---|---|
| `schema_version` | versão do formato (atual: 1) |
| `id`, `name`, `description`, `category`, `platforms` | identificação |
| `aspect`, `resolution`, `fps` | quadro de saída; `fps: null` = preservar o FPS da origem |
| `duration` | `min_s`, `max_s` (limite editorial), `target_s` |
| `pacing` | `calm` · `natural` · `dynamic` · `very_dynamic` |
| `cuts` | remoção de silêncio, limiar, folga, **`preserve_silent_action`** (não cortar ação silenciosa em gameplay), palavras de preenchimento, regras |
| `audio` | loudness-alvo (LUFS), pico verdadeiro, ducking de música, redução de ruído, prioridade de voz |
| `captions` | ativação, estilo, posição, caracteres por linha |
| `effects`, `transitions` | permitidos, intensidade, duração padrão, frequência |
| `reframe` | modo (`auto_focus`, `speaker_tracking`, `action_tracking`, `facecam_gameplay`…), preservação de HUD, layouts |
| `narrative` | modo (`preserve`, `summarize`, `highlight_events`, `short_story`, `best_moments`), gancho, regras |
| `safety` | área segura (% por borda) e regras (não inventar, não prometer visualizações…) |
| `export_profile` | perfil de exportação sugerido |
| `scoring_weights` | pesos iniciais da pontuação editorial — **heurística de engenharia a calibrar, não previsão de audiência** |

## Presets incluídos

| # | id | Formato | Destaque |
|---|---|---|---|
| 01 | `gameplay_youtube` | 16:9, FPS da origem | não remove silêncio com ação; música abaixo da narração |
| 02 | `gameplay_highlights` | 16:9 | montagem dinâmica, slow motion moderado |
| 03 | `gameplay_vertical` | 9:16 | action tracking, preserva HUD, facecam + gameplay |
| 04 | `youtube_shorts` | 9:16 1080×1920 | até 180 s, gancho, narrativa completa |
| 05 | `instagram_reels` | 9:16 | alvo editorial curto, separado do limite da plataforma |
| 06 | `facebook_reels` | 9:16 | legendas para assistir sem áudio |
| 07 | `tiktok_dinamico` | 9:16 | ritmo muito dinâmico, karaokê |
| 08 | `podcast_viral` | 9:16 | perguntas e respostas completas, speaker tracking |
| 09 | `entrevistas` | 16:9 | pausas longas removidas, contexto preservado |
| 10 | `cortes_humor` | 9:16 | preparação + punchline + reação |
| 11 | `documentario` | 16:9 | precisão factual; nada gerado apresentado como real |
| 12 | `tutorial` | 16:9 | capítulos, zoom em ações |
| 13 | `review_produtos` | 16:9 | demonstração → vantagens → limitações |
| 14 | `vlog` | 16:9 | natural, correção básica de cor |
| 15 | `viagem_cinematografica` | 16:9 | transições longas, planos contemplativos |
| 16 | `musica_lofi` | 16:9 | ritmo lento, loops, sem cortes abruptos |
| 17 | `cortes_lives` | 9:16 | múltiplos cortes de gravações longas |
| 18 | `esportes` | 16:9 | replays, slow motion contextual, sequência preservada |
| 19 | `noticias` | 16:9 | não alterar sentido; distinguir análise de fato |
| 20 | `linkedin_corporativo` | 16:9 | –16 LUFS, visual sóbrio |
| 21 | `personalizado` | 16:9 | base neutra para duplicar |

## Perfis de exportação

`youtube_1080p`, `youtube_4k`, `youtube_shorts`, `instagram_reels`, `facebook_reels`, `tiktok`, `linkedin`, `high_quality`, `hevc_archive`, `web_vp9`.

Especificações de plataformas mudam. Por isso os perfis ficam num arquivo separado e versionado, que pode ser atualizado sem recompilar. Os valores de loudness (–14 LUFS, –16 no LinkedIn) são **escolhas editoriais iniciais**, não especificações oficiais.

## Criando um preset

Copie `21_personalizado.json` para `<app-data>/presets/editorial/meu_preset.json`, altere `id` e `name` e ajuste os campos. Ele aparece no próximo início. Programaticamente: `Studio::save_user_preset` (comando Tauri `save_user_preset`). A UI de edição de presets chega com a Fase 4.
