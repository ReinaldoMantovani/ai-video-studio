# Arquitetura — AI Video Studio

Este documento responde aos nove itens da seção 34 do documento de requisitos: arquitetura definitiva, justificativa da stack, estrutura de diretórios, modelo de dados, contratos entre módulos, estratégia de renderização, estratégia de IA local, plano de implementação e critérios de teste e desempenho.

Estado atual: **Fases 1 (Fundação), 2 (Editor manual), 3 (IA local), 4 (Edição automática), 5 (Cortes verticais), 6 (Assistente por linguagem natural) e 7 (Otimização) concluídas**. O planejamento da Fase 4 é determinístico; o refinamento opcional por LLM local ainda está pendente. Ver a seção 8.

---

## 1. Arquitetura definitiva

```
┌────────────────────────────── Aplicação desktop (Tauri 2) ──────────────────────────────┐
│  WebView: React + TypeScript + Tailwind + Zustand + TanStack Query                      │
│  Dashboard · Editor (player, timeline, inspector, mídia) · Exportação · Processamento   │
│                    ▲ invoke (comandos tipados)      ▲ eventos "avs://event"             │
└────────────────────┼────────────────────────────────┼───────────────────────────────────┘
                     │                                │
┌────────────────────┴──────── avs-app (camada de serviço, Rust) ─────────────────────────┐
│ Projetos abertos (EditSession) · Importação · Fila persistente de jobs (prioridade,     │
│ pausa, cancelamento, retomada) · Preview · Exportação validada · Presets · Sistema      │
└───────┬───────────────────────┬─────────────────────────┬───────────────────────────────┘
        │                       │                         │
┌───────┴────────┐   ┌──────────┴───────────┐   ┌─────────┴──────────┐   ┌───────────────────┐
│ avs-core       │   │ avs-media            │   │ avs-store          │   │ workers/python    │
│ tempo exato,   │   │ FFmpeg/FFprobe:      │   │ SQLite WAL,        │   │ (Fase 3) Whisper, │
│ modelo, valida-│   │ probe, derivados,    │   │ .avsproj atômico,  │   │ VAD, cenas, visão │
│ ção, comandos, │   │ compilador de render,│   │ snapshots,         │   │ Ollama — processo │
│ undo/redo      │   │ render por blocos,   │   │ autosave, .bak     │   │ isolado, JSON-RPC │
│ (sem I/O)      │   │ validação, hwaccel   │   │                    │   │ via stdio         │
└────────────────┘   └──────────────────────┘   └────────────────────┘   └───────────────────┘
                                 ▲
                   avs-cli (mesma camada de serviço; automação, CI e benchmarks)
```

Separação exigida pela seção 4, módulo a módulo:

| Responsabilidade | Onde |
|---|---|
| Interface gráfica | `apps/desktop/src` |
| Gestão de projetos | `avs-app` (`Studio::create/open/save…`) + `avs-store` |
| Motor de edição | `avs-core` (`EditCommand`, `apply`, `EditSession`) |
| Motor de renderização | `avs-media::compile` + `avs-media::render` |
| Processamento de áudio | `avs-media::compile` (mix, fades, ganho), `derive` (waveform); DSP avançado na Fase 2 |
| Visão computacional | `workers/python/avs_worker` (OpenCV, YuNet, PySceneDetect) |
| Orquestração de IA | `avs-ai` (runtime, worker, análise, Ollama, perfis) + `avs-app::ai` (jobs) |
| Seleção automática de momentos | `avs-ai::scoring` (pontuação 0–100 com os pesos do preset e justificativa), `avs-ai::autoplan` ("Editar automaticamente"), `avs-ai::review` (revisão de qualidade) |
| Gerador de cortes | `avs-ai::shorts` (sugestão de cortes e reenquadramento por planos) + `avs-app::shorts` (um projeto vertical por corte, exportação em lote) |
| Exportação | `avs-media::render` + `avs-media::validate` + `avs-app::run_export` |
| Gerenciamento de modelos/recursos | `avs-ai::runtime` (componentes), `avs-ai::profile`, `avs-app::system`, `hwaccel` |

Não há microsserviços, contêineres nem servidor de banco: um processo desktop, processos FFmpeg filhos e, a partir da Fase 3, workers Python filhos.

## 2. Justificativa da stack

Mantive a stack recomendada. Abaixo, as escolhas que vão além do documento e o motivo de cada uma.

- **Tauri 2 + Rust**: binário pequeno, sem Chromium embutido, acesso nativo rápido. O motor inteiro (`avs-*`) é Rust puro e não depende do Tauri: a CLI usa o mesmo código, e os testes rodam sem interface.
- **Tempo em *flicks* (`i64`, 1/705 600 000 s)**: divide exatamente 24, 25, 30, 50, 60 fps, os NTSC (x/1001) e 44,1/48 kHz. Somas de duração nunca acumulam erro; posições viram frames e amostras inteiros só na borda (compilador). Alternativa descartada: `f64` em segundos, que acumula erro de arredondamento (requisito da seção 28).
- **Compilador de render próprio sobre filtergraph do FFmpeg**, em vez de MLT, GStreamer ou OpenTimelineIO como motor. O FFmpeg já é obrigatório. Um compilador determinístico (timeline → grafo) dá controle total de sincronismo e permite renderizar intervalos com exatidão, que é a base do cache por blocos e do WYSIWYG. O OpenTimelineIO entra como formato de intercâmbio (importação e exportação, Fase 2), não como motor.
- **Contratos tipados gerados (`ts-rs`)**: os tipos TypeScript em `apps/desktop/src/bindings` são gerados das structs Rust. Frontend e backend não divergem.
- **SQLite (WAL) para índice + JSON versionado para o projeto**: o banco serve para listar e filtrar (projetos, jobs, exportações); o `.avsproj` é a fonte da verdade, portátil e legível. Gravação atômica (temp + fsync + rename) e `.bak`.
- **Python apenas nos workers de IA (Fase 3)**: o ecossistema de modelos (Faster-Whisper, Silero, PySceneDetect, OpenCV, MediaPipe, ONNX) está em Python. Cada worker é um processo isolado que conversa por JSON-RPC em stdio, sem porta de rede aberta. Isso cumpre "proteção de endpoints locais" por construção.

## 3. Estrutura de diretórios

```
ai-video-studio/
├── Cargo.toml                 # workspace Rust
├── crates/
│   ├── avs-core/              # tempo, modelo, validação, comandos, histórico (sem I/O)
│   ├── avs-media/             # FFmpeg: probe, derivados, compilador, render, validação, hwaccel
│   │   └── tests/engine.rs    # integração com FFmpeg real
│   ├── avs-store/             # SQLite WAL, arquivos de projeto, snapshots
│   ├── avs-app/               # serviço: projetos, jobs, preview, export, presets, sistema
│   │   └── tests/studio.rs    # fluxo completo, pausa/retomada, recuperação
│   └── avs-cli/               # binário `avs`
├── apps/desktop/
│   ├── src/                   # React (bindings/ gerados, lib/, store/, components/)
│   └── src-tauri/             # comandos Tauri, capacidades, config de bundle
├── presets/
│   ├── editorial/NN_<id>.json # 21 presets editoriais versionados
│   └── export_profiles.json   # perfis de exportação versionados
├── workers/python/            # contrato dos workers de IA (Fase 3)
├── scripts/                   # instalação de dependências, FFmpeg, verificação
└── docs/                      # este e os demais documentos
```

Biblioteca do usuário (dados), por projeto:

```
<app-data>/library.db                         # índice SQLite (WAL)
<app-data>/presets/editorial/*.json           # presets do usuário
<app-data>/projects/<id>/project.avsproj      # documento do projeto
                        /project.avsproj.bak  # versão anterior
                        /autosave.avsproj     # salvamento automático (recuperação)
                        /snapshots/*.avsproj  # versões nomeadas
                        /proxies/<asset>.mp4
                        /cache/assets/<asset>/{poster.jpg,thumbs.jpg,waveform.u8}
                        /cache/chunks/{preview,export}/<hash>.mov   # blocos renderizados
                        /renders/preview-r<rev>.mp4
```

Os arquivos originais **não são copiados**. O projeto guarda caminho + impressão digital (tamanho + SHA-256 do primeiro e do último MiB). Isso permite relocalizar mídia movida automaticamente (busca na pasta de origem) ou manualmente.

## 4. Modelo de dados

Definido em `crates/avs-core/src/model.rs`, com tipos TypeScript gerados em `apps/desktop/src/bindings/`.

| Entidade | Status | Observações |
|---|---|---|
| `Project` | ✅ | `schema_version`, configurações, mídias, timeline, preset |
| `MediaAsset` | ✅ | kind, caminho, impressão digital, streams, VFR, rotação, proxy, miniaturas, waveform, status |
| `Timeline` / `Track` / `Clip` | ✅ | trilhas de vídeo/texto/áudio; clipe = mídia (`source_in`) ou texto |
| `Transition` | ✅ | entre clipes consecutivos que se sobrepõem exatamente pela duração |
| `Effect` | ✅ | `VideoEffects` (temperatura, LUT, desfoque, nitidez, vinheta), `ColorAdjust`, `AudioEffects` (passa-altas, ruído, compressor, EQ), velocidade/reverso/congelamento |
| `Keyframe` | ✅ | `Keyframes { scale, position_x, position_y, volume }` com easing linear/suave/degrau; tempos relativos ao clipe |
| `Subtitle` | ✅ | legendas são clipes de texto numa trilha; `subtitles.rs` lê/escreve SRT e VTT; palavra a palavra virá da transcrição (Fase 3) |
| `Preset` (`EditorialPreset`) | ✅ | JSON versionado e validado |
| `ExportProfile` | ✅ | JSON versionado |
| `EditCommand` | ✅ | 16 operações validadas, base do assistente de IA |
| `ProjectSnapshot` | ✅ | `snapshots/*.avsproj` |
| `RenderJob` (`JobRecord`) | ✅ | persistido em SQLite |
| `AIModel` (`AiComponent`), `AnalysisResult`, `PausePlan` | ✅ | `avs-ai` |
| `AIProvider`, `HighlightCandidate`, `EditPlan` (LLM) | ⏳ Fases 4–5 | provedores pagos opcionais na Fase 4+ |

Invariantes verificados por `validate_project` após cada comando (o comando é descartado se falhar) e antes de cada render:
duração positiva; origem dentro da mídia (exceto imagens); tipo de clipe compatível com a trilha; sobreposição só entre vizinhos com transição de duração idêntica; nunca três clipes sobrepostos; fades ≤ duração; volume, opacidade e escala em faixas válidas; resolução par.

## 5. Contratos entre módulos

- **UI → serviço**: comandos Tauri (`apps/desktop/src-tauri/src/lib.rs`). Todos rodam em `spawn_blocking`. Erros chegam como `AppError { code, message }`, com a mensagem real (inclusive o final do stderr do FFmpeg).
- **Serviço → UI**: evento `avs://event` com `AppEvent = job | asset | project_changed | preview_ready`. O progresso é limitado a cerca de 10 eventos por segundo por job e persistido a cada 2 s.
- **Edição**: só por `EditCommand` (JSON com `op`). `apply()` trabalha numa cópia, valida e só então confirma (atômico). `EditSession` mantém undo/redo; `execute_batch` agrupa várias operações numa única entrada desfazível.
- **Render**: `compile(project, range, options) → CompiledGraph`; `render(RenderRequest) → RenderOutcome`; `validate_output(path, Expectation) → ValidationReport`.
- **Workers de IA (Fase 3)**: processo filho, JSON-RPC 2.0 por stdio (ver `workers/python/README.md`). O LLM nunca executa comandos do sistema: o assistente (Fase 6) só pode devolver `EditCommand`s, que passam pela mesma validação de qualquer edição manual.

## 6. Estratégia de renderização

**Um único compilador** (`avs-media/src/compile.rs`) atende a preview, frame do scrubbing, preview por trecho e exportação. Por isso a preview mostra o mesmo que será exportado. Isso é verificado em teste: o frame do scrubbing comparado ao frame do arquivo exportado dá PSNR acima de 30 dB, com a diferença vindo apenas da compressão.

Como uma timeline vira grafo:

1. Cada clipe visível no intervalo vira uma entrada `-ss <origem> -t <duração>` própria (seek rápido). O custo não cresce com a posição do trecho num arquivo de horas.
2. Vídeo: `fps` → escala/recorte/pad calculados em Rust (encaixar ou preencher, escala, posição) → cor (`eq`) → opacidade → fades com alfa → `tpad` + `trim` para o **número exato de frames**. Clipes de mídia curta ou VFR não dessincronizam a timeline.
3. Cada trilha vira uma sequência: lacunas transparentes, `concat` entre clipes encostados e `xfade` nas transições. As trilhas são empilhadas com `overlay` sobre o fundo do projeto, e os textos entram via `drawtext` (fonte resolvida, texto em `textfile` com `expansion=none`, sem interpolação de texto do usuário no grafo).
4. Áudio: reamostragem → `apad` + `atrim` para o **número exato de amostras** → ganho → fades do clipe e das transições → `adelay` em amostras → `amix` sobre um silêncio-base de duração exata.
5. A janela de compilação é expandida para nunca cortar uma transição ou um fade ao meio, e depois aparada de volta ao intervalo pedido. Renderizar `[a, b)` dá exatamente os pixels desse trecho na renderização completa.

**Velocidade, reverso e congelamento**: o trecho de origem consumido é `duração × velocidade`. O vídeo usa `setpts`, o áudio uma cadeia de `atempo` (sem mudar o tom). O reverso usa `reverse`/`areverse` sobre o trecho exato e é limitado a 120 s de origem por clipe, porque esses filtros mantêm o trecho em memória. O congelamento lê um único frame e o repete com `tpad`, sem áudio.

**Keyframes**: cada curva vira uma expressão por partes, avaliada por frame, em função do tempo *relativo ao clipe*. Por isso continua exata quando um bloco de renderização começa no meio do clipe (testado contra passada única: PSNR > 35 dB). Zoom e posição animados usam `zoompan` sobre o quadro já encaixado; limitação: o zoom animado vai de 100% a 500% (zoom-out animado ainda não é suportado). O volume animado usa `volume` com `eval=frame`.

**Efeitos**: aplicados depois da escala, antes do canal alfa, com intensidades proporcionais à altura de saída. Assim o preview (540p) e a exportação (1080p+) têm a mesma aparência. Arquivos de LUT entram na chave de cache: editar o `.cube` invalida os blocos.

**Renderização incremental por blocos** (`render.rs`): a timeline é dividida em blocos de N frames (5 s na preview, 30 s na exportação). A chave de cada bloco é o SHA-256 de grafo, argumentos, identidade das entradas (caminho, tamanho, mtime), versão do FFmpeg e configurações do encoder. Blocos inalterados são reaproveitados. O vídeo dos blocos é unido sem recodificar (demuxer `concat` com durações exatas). O áudio fica em PCM nos blocos (sem *priming* de AAC entre blocos) e é codificado uma vez no final. Benefícios:
- uma edição pequena só renderiza os blocos afetados (testado: 3 de 4 reaproveitados);
- pausar e retomar uma exportação, ou retomar depois de reiniciar o app, reaproveita o que já foi feito;
- a memória fica limitada ao bloco atual, mesmo em timelines de horas com centenas de clipes;
- blocos podem ser renderizados em paralelo (limite configurável).

**Aceleração por hardware**: `hwaccel::detect_encoders` não confia no nome da GPU. Cada encoder candidato (NVENC, Quick Sync, AMF, VideoToolbox, além dos de software) é testado codificando frames reais. Se o encoder de hardware falhar durante a exportação, ela é repetida automaticamente em software.

**Validação** (`validate.rs`): existência, contêiner legível, resolução, FPS, codec, duração (tolerância de 2 frames), contagem de frames, presença de áudio, diferença de duração entre streams e uma passada de decodificação completa com `blackdetect` e `silencedetect`. Frames pretos e silêncio só são alertados quando caem onde a timeline tem conteúdo. Uma exportação só é marcada como concluída sem nenhuma falha; se falhar, o arquivo é renomeado para `*.invalido.mp4` e o erro real é exibido.

## 7. Estratégia de IA local (Fases 3–6)

- **Workers Python isolados** iniciados sob demanda, um por capacidade (transcrição, VAD, cenas, visão), com JSON-RPC em stdio, cancelamento via sinal e limites de memória e concorrência controlados pela fila de jobs existente (que já tem prioridades e limite de exportações simultâneas).
- **Transcrição**: Faster-Whisper (CTranslate2; int8 na CPU, float16 na GPU), com whisper.cpp como alternativa sem Python. Timestamps por palavra alimentam as legendas.
- **VAD**: Silero VAD (ONNX). **Cenas**: PySceneDetect (ContentDetector/AdaptiveDetector). **Visão**: OpenCV + MediaPipe para rostos e pose; amostragem adaptativa de frames representativos (por cena e por pico de movimento), nunca todos os frames.
- **LLM**: Ollama via HTTP local (127.0.0.1). Modelos escolhidos pelo perfil de hardware detectado (econômico, equilibrado ou qualidade máxima), com aviso antes de baixar um modelo maior do que a máquina suporta. Antes de fixar nomes de modelo, a Fase 3 vai verificar disponibilidade, licença e desempenho reais.
- **Planejamento → operações**: o LLM recebe um resumo estruturado (transcrição segmentada, cenas, pontuações) e devolve um `EditPlan`, que é convertido em `EditCommand`s. Esses comandos passam pela mesma validação e pelo mesmo undo/redo de qualquer edição manual. Nada de editar arquivos por tentativa e erro.
- **Provedores pagos (opcional)**: as chaves ficam no cofre do sistema operacional (keyring), nunca no frontend nem nos logs. Nenhuma mídia sai da máquina sem consentimento explícito por envio.

Detalhes em `docs/AI_MODELS.md`.

## 8. Plano de implementação

| Fase | Conteúdo | Estado |
|---|---|---|
| 1 Fundação | estrutura, interface, importação, FFmpeg, persistência, preview, timeline, exportação real | ✅ concluída |
| 2 Editor manual | multitrilha, trim, split, mover, duplicar, ripple/roll/slip/slide, transições, textos, velocidade, reverso, congelar, keyframes, cor/LUT/efeitos, legendas SRT/VTT, DSP de áudio, undo/redo ✅ · pendente: máscaras, chroma key, estabilização, speed ramp contínuo, OTIO | ✅ (itens avançados pendentes) |
| 3 IA local | runtime Python gerenciado, worker JSON-RPC, Gerenciador de IA, Silero VAD, PySceneDetect, OpenCV/YuNet, faster-whisper, cliente Ollama, perfis de hardware, cache de análise ✅ · transcrição a validar fora do sandbox | ✅ |
| 4 Edição automática | remover pausas e hesitações, cenas, legendas, pontuação editorial de trechos, "Editar automaticamente" por preset (seleção para a duração-alvo, transições, zoom pontual, tratamento de áudio, ducking, exposição, fim da trilha sonora), prévia, um único desfazer, versões antes/depois, revisão automática de qualidade, fluxo a partir do dashboard ✅ · refinamento opcional do plano por LLM local | ✅ (LLM pendente) |
| 5 Cortes | sugestão de cortes autossuficientes com nota e justificativa, um projeto por corte, enquadramento (seguir rosto por planos, centro, fundo desfocado), limpeza automática, legendas, exportação em lote, CLI `avs shorts` ✅ · panorâmica contínua, layouts facecam+gameplay e tela dividida, locutor ativo | ✅ (layouts avançados pendentes) |
| 6 Assistente | pedidos em português → intents de vocabulário fechado → `EditCommand` validados. Interpretação por regras (offline, sem modelo) ou por LLM local (Ollama + JSON Schema). Usa seleção e cursor como contexto, prévia numa cópia, um único desfazer e recusa de plano desatualizado ✅ · conversa com memória de contexto entre pedidos | ✅ |
| 7 Otimização | proxies, cache por blocos, detecção de hardware ✅. Fase 7: `avs bench` (gravação longa com pico de memória por etapa), análise de 30 min medida, algoritmos de 3 h indexados (revisão 29× e sugestões 27× mais rápidas), preview por janelas em timelines longas, timeline desenhando só o visível, desfoque forte reduzido (corte vertical −32%), paralelismo pela memória disponível, progresso na detecção de cenas sem Python ✅ · `-hwaccel`/filtros na GPU (sem GPU para validar) | ✅ |
| 8 Consolidação | E2E da interface com `tauri-driver` (14 cenários, incluindo reprodução real e troca de janela), servidor local de mídia para o `<video>` no Linux, instaladores `.deb`/`.rpm`/AppImage gerados e testados aqui, configuração de NSIS/MSI/DMG, CI com E2E no Linux, workflow de release com assinatura por secrets, documentação revisada ✅ · instaladores de Windows e macOS gerados só no CI (não executados aqui) | ✅ |

As 8 fases do plano estão concluídas.

Próximos passos recomendados:
- Gerar uma tag `v0.8.0` para rodar o workflow de release e testar os instaladores de Windows e macOS em máquinas reais.
- Validar a transcrição (Whisper) fora do sandbox.

Em máquinas com GPU, vale medir Whisper e encoders por hardware com `avs bench`. Na Fase 5 ainda faltam os layouts avançados: facecam + gameplay, tela dividida para podcast com locutor ativo e panorâmica contínua. Em paralelo, falta o refinamento opcional do plano pela LLM local: o Ollama receberia os trechos pontuados e devolveria só uma reordenação ou seleção restrita por JSON Schema, validada como `EditCommand`. Esse refinamento precisa ser validado numa máquina com Ollama.

### Fluxo de "Editar automaticamente" (Fase 4)

1. `Studio::place_assets` coloca as mídias na timeline. Usa "Preencher" quando o preset pede reenquadramento e a proporção difere.
2. O job `auto_edit` analisa o que falta. Usa o cache por impressão digital e só transcreve se o preset precisar e a transcrição estiver instalada.
3. `auto_edit_apply` salva a versão "Original (antes da edição automática)". Em seguida roda `autoplan::run` dentro de `EditSession::transaction` (uma entrada no histórico) e salva a versão "Edição IA vN".
4. `autoplan::run` aplica as etapas nesta ordem, cada uma com justificativa no relatório:
   - legendas (antes dos cortes, para acompanhá-los);
   - hesitações (transcrição);
   - pausas (regras do preset × intensidade);
   - seleção para a duração-alvo, em ordem cronológica, unindo trechos a menos de 1,5 s;
   - remoção do fim para o início com ripple na trilha principal e nas de texto;
   - fim da trilha sonora com fade;
   - transições nos saltos de conteúdo;
   - zoom nos picos de movimento;
   - tratamento da fala;
   - ducking por keyframes de volume;
   - exposição.
5. `review::review` aponta problemas com o instante, para "ir para". A revisão nunca dá nota perfeita.

### Cortes verticais (Fase 5)

1. `shorts::suggest` parte dos trechos pontuados da trilha principal, já na timeline. Cada trecho é uma semente que cresce para a direita até o alvo e, se ainda estiver curto, para a esquerda até o mínimo. A janela só atravessa intervalos de até 6 s e nunca passa do máximo.
   - Nota do corte: 35% do gancho (primeiro trecho), 45% da média ponderada pela duração e 20% de completude.
   - A seleção é gulosa e sem sobreposição.
   - A folga (150 ms antes e 300 ms depois) nunca atravessa para outro clipe, para não sobrar um "flash" de poucos quadros.
2. `Studio::create_short` cria cada corte como um projeto novo. A origem nunca é alterada.
   - O projeto copia a timeline e as mídias; os arquivos derivados são reaproveitados por hard link.
   - Remove o que está fora do trecho em todas as trilhas e aplica o enquadramento.
   - Roda `autoplan::run` com as regras do preset vertical: pausas, legendas, áudio, ducking e exposição; sem nova seleção e sem transições.
   - Salva a versão "Corte gerado" e, se pedido, enfileira a exportação (uma por vez).
   - Um corte com erro não impede os outros, e não deixa pasta pela metade.
3. Enquadramento (`shorts::plan_reframe`): "Preencher" com deslocamento estático por plano, exato no compilador.
   - O plano muda em cortes de cena ou quando o rosto principal se desloca mais de 30% da largura do quadro por 2 amostras seguidas, com plano mínimo de 2 s; o tremor é ignorado.
   - Vários rostos que cabem juntos no quadro são centralizados como grupo.
   - "Fundo desfocado" usa uma trilha abaixo (`MoveTrack`) com a mesma imagem em "Preencher", desfoque e mudo.

### Assistente (Fase 6)

`avs-ai::assistant`:
- **`parse_rules`**: interpretador determinístico de português.
  - Divide o pedido em orações por `;`, `depois`, `em seguida` e `, <verbo>` / `e <verbo>`.
  - Reconhece tempos (`1:05`, `1:12,5`, `30 segundos`, `1 minuto e 15`, `aqui`, `no final`), alvos (selecionado, música, fala, todos) e texto entre aspas.
  - Explica o que não entendeu, com exemplos.
- **`llm_request` / `parse_llm`**: prompt com o resumo da timeline e JSON Schema com as 18 ações; a resposta é validada.
- **`execute`**: converte os intents em `EditCommand`s, com mensagens específicas ("Passo 2: velocidade 9× fora do limite").

`avs-app::assistant`:
- `assistant_plan` faz a prévia numa cópia e devolve a revisão;
- `assistant_apply` reaplica numa transação só se a revisão for a mesma.

A pré-visualização (`auto_edit_preview`) roda o mesmo `autoplan::run` numa cópia do projeto. Assim, o que se vê em vermelho na timeline é exatamente o que será aplicado.

## 9. Critérios de teste e desempenho

Ver `TESTING.md` e `PERFORMANCE.md`. Em resumo:
- unitários determinísticos para tempo, comandos e validação; integração com **FFmpeg real** para probe, derivados, export, WYSIWYG, cache incremental e cancelamento; fluxo completo da camada de serviço, incluindo pausa, retomada e recuperação após reinício;
- benchmarks com tempos reais por etapa (`avs demo --seconds N` grava `benchmark.json`), sem prometer tempos absolutos.
