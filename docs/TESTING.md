# Testes

```bash
scripts/verify.sh            # tudo: cargo test --workspace, vitest, typecheck, build do frontend
cargo test --workspace       # Rust (precisa de FFmpeg no PATH)
cd apps/desktop && npm test  # frontend
```

Os testes de integração **geram mídia sintética com o próprio FFmpeg** (`testsrc2`, `sine`, `color`) e verificam os arquivos produzidos (pixels, duração, streams), não apenas códigos de retorno.

## Cobertura atual

46 testes de comportamento em Rust (vários são fluxos completos com FFmpeg real e com o worker Python real), verificações de geração dos contratos TypeScript (`export_bindings_*`) e 6 testes do frontend.

### `avs-core` (unitários, determinísticos)
- tempo exato: uma hora de frames NTSC somados um a um sem desvio; taxas comuns dividem o flick; parse e formatação de timecode
- split / undo / redo; trim do início move o ponto de entrada na origem
- **cenário E**: "remova 1:20–1:30 e coloque uma transição suave" → `DeleteRange` + `AddTransition`, desfazer e refazer
- comando inválido (sobreposição, além do fim da mídia) é rejeitado **sem alterar o projeto**
- ripple insert/delete; trilha bloqueada bloqueia edições
- restaurar versão (snapshot) é desfazível; versão inválida é rejeitada sem alterar o projeto
- velocidade: duração muda, origem consumida conferida; divisão de clipe acelerado; slip limitado à mídia
- reverso: divisão mantém a ordem correta da origem; reverso longo recusado
- roll, slide e duplicar com valores exatos; roll além da origem recusado
- congelar frame insere e empurra a trilha
- keyframes: criar, substituir, remover; zoom animado < 100% recusado; aparar preserva o valor na borda
- legendas: trilha a partir de SRT (legendas sobrepostas encurtadas, vazias ignoradas), aplicar estilo a todas
- projetos salvos antes dos novos campos continuam abrindo
- SRT/VTT: leitura tolerante (BOM, CRLF, tags, configurações VTT, blocos quebrados) e ida e volta exata
- ida e volta JSON sem perdas; exportação dos bindings TypeScript

### `avs-media` (integração com FFmpeg real)
- probe: dimensões, FPS, duração, áudio, imagem vs. vídeo vs. áudio; arquivo corrompido → `InvalidMedia` claro
- miniaturas, waveform (picos conferidos contra a amplitude conhecida), poster
- **exportação validada**: timeline com transição, lacuna, imagem, texto com caracteres especiais (`: ' % " \ [ ] ; { }`), música com fades, em caminho com espaço, apóstrofo e dois-pontos → todas as verificações passam; cores conferidas pixel a pixel (vermelho no clipe B, preto na lacuna, azul na imagem, mistura na transição)
- **WYSIWYG**: frame do scrubbing × frame exportado nas mesmas posições → PSNR > 30 dB
- **blocos = passada única**: blocos de 1,3 s (cortando transições e fades ao meio) × passada única → PSNR > 35 dB
- **renderização incremental**: editar só o último trecho reaproveita 3 de 4 blocos; desfazer reaproveita 4 de 4
- **cancelamento**: nenhum arquivo final e nenhum bloco parcial no cache
- detecção de encoders por codificação real
- **`tests/effects.rs`** (pixels e áudio do arquivo exportado):
  - 2×: origem 1 s (vermelho) aos 0,5 s e 3 s (azul) aos 1,5 s · reverso começa azul e termina vermelho, mesmo cortado em blocos de 1,5 s · câmera lenta 0,5× com áudio · frame congelado azul e **silencioso**
  - zoom 1→2× com pan: lado esquerdo vermelho no início e azul no fim; **blocos de 0,7 s × passada única: PSNR > 35 dB**
  - LUT 3D que inverte cores (vermelho → ciano), vinheta escurece os cantos, 3000 K aquece o cinza; LUT inexistente recusada
  - rampa de volume por keyframe (início ≥ 20 dB mais baixo que o fim) com passa-altas, redução de ruído, EQ e compressor

### `avs-store`
- WAL ativo, migrações idempotentes, configurações
- gravação atômica; arquivo corrompido → recuperação pelo `.bak`; snapshots
- schema futuro rejeitado com mensagem clara
- sanitização de nomes (inclusive nomes reservados do Windows) e proteção contra path traversal

### `avs-app` (fluxo completo)
- **cenário D**: criar projeto com preset → importar pasta (sem duplicar) → miniaturas/waveform → inserir, dividir, apagar, desfazer, transição, texto, música → frame → preview → exportar validado → nome sanitizado → reexportar sem sobrescrever, com 100% dos blocos reaproveitados
- **cenário G**: encerrar sem salvar → reiniciar → projeto recuperado do autosave idêntico → salvar → reabrir limpo
- pausa durante a exportação → retomada reaproveitando blocos; cancelamento de job
- legendas: importar SRT pela importação de mídia, exportar VTT, rejeitar extensão errada, desfazer a importação
- **edição automática pelo fluxo do dashboard** (`dashboard_auto_edit_flow_job_snapshots_review_and_export`):
  - preset YouTube Shorts; vídeo horizontal com fala sintética real e pausas, mais música;
  - `place_assets` usa "Preencher" e uma duração em quadros que não passa da mídia (bug real encontrado e corrigido);
  - a prévia sem análise é recusada; o job analisa e aplica;
  - o relatório tem pausas e ducking e explica que a transcrição não está instalada;
  - a revisão tem ≥ 6 verificações;
  - as versões "Original" e "Edição IA v1" são salvas (com acentos e espaços preservados);
  - a prévia não altera a revisão;
  - a **exportação do resultado passa na validação**;
  - um Ctrl+Z volta ao original
- IA pela camada de serviço: análise como job, regras do preset Entrevistas, plano e aplicação de pausas (tempo removido exato), marcador no corte de cena, erro claro sem transcrição, Ollama remoto recusado

### `avs-ai` (IA local)
- remover pausas: cortes exatos com folga (início/fim sem folga), pausa curta mantida; aplicado num projeto real com legendas acompanhando; um único desfazer
- pausa com ação visual mantida; mapeamento correto com o clipe a 2×
- marcadores e divisão nas cenas; agrupamento de palavras em legendas (quebra por pausa e pontuação)
- sem Python: fala por energia em **fala sintética real (espeak-ng)** e cenas pelo FFmpeg, com aviso; cache instantâneo na segunda vez
- **integração real** (`AVS_AI_INTEGRATION=1`): cria o venv, instala o componente de análise via pip e baixa o YuNet com SHA-256 conferido. Depois o worker detecta a fala sintética (Silero, início em ~2 s), o corte de cena em 4 s (PySceneDetect) e o brilho por amostra (YuNet ativo). O progresso é monotônico, a transcrição ausente é reportada, e o cancelamento de uma chamada longa ao worker funciona
- cliente Ollama: estado, modelos, download com progresso, geração com JSON Schema e teste, contra um servidor local que fala o protocolo do Ollama; Ollama parado gera estado claro
- perfis de hardware e aviso de modelo pesado

### `avs-ai` — Fase 4 (`tests/autoedit.rs`)
- pontuação: trecho de fala completo e expressivo vence o parado; notas entre 0 e 100; justificativa cita o fator dominante
- edição automática num projeto real:
  - o tempo removido é exato (pausas com folga só do lado da fala);
  - a duração da trilha confere com remoções e transições;
  - há etapas de pausas, transições, zoom, áudio, ducking e cor;
  - o zoom cai no pico de ação (origem 35–37 s) e nunca fica abaixo de 100%;
  - a música é reduzida 12 dB sob a fala;
  - há uma única entrada no histórico, e desfazer restaura tudo
- melhores momentos para 20 s: escolhe exatamente os 2 trechos de ação, em ordem cronológica; alvo maior que o material não estica
- Conservador > Equilibrado > Agressivo em duração final
- sem análise: erro claro e nada muda
- revisão:
  - aponta duração acima do máximo (falha), corte no meio da fala no instante certo, texto fora da área segura e textos sobrepostos;
  - um projeto limpo dá "pass" sem prometer perfeição

### Fase 5 — cortes (`avs-ai/tests/shorts.rs`, `avs-app/tests/studio.rs`)

Sugestão:
- 10 min de fala com dois momentos agitados resultam em 4 cortes sem sobreposição, em ordem de nota;
- cada corte respeita mín./máx. e começa 150 ms antes de uma frase e termina 300 ms depois de outra;
- os dois melhores caem nos momentos agitados;
- material curto gera 1 corte com aviso honesto;
- a folga nunca invade o clipe seguinte. Esse bug foi encontrado na verificação pela interface (um "flash" de 5 quadros) e corrigido.

Enquadramento:
- `fill_offset` coloca o assunto no centro, conferido com a mesma geometria do compilador, e limita nas bordas;
- os planos seguem o rosto, trocam no corte de cena e no deslocamento sustentado, e ignoram o tremor;
- sem rosto, o resultado é um plano central.

Fluxo completo com render real:
- vídeo 16:9 com o "rosto" a 25% da largura e fala sintética;
- sem análise, o erro é claro; com a análise, os rostos são injetados no cache (o YuNet roda no worker Python);
- 2 cortes viram projetos 1080×1920 com versão salva e exportação em lote validada;
- **no arquivo exportado, o assunto está no centro (±6%)**;
- no "Centro", o assunto lateral fica fora do quadro;
- no "Fundo desfocado", a trilha de fundo fica abaixo, com desfoque e muda, e o vídeo inteiro aparece por cima;
- a timeline da origem continua idêntica;
- regressão: corte até o fim de uma timeline com duração fora da grade de quadros ("intervalo vazio", encontrado pela interface e corrigido).

### Fase 6 — assistente (`avs-ai/tests/assistant.rs`, `avs-app/tests/studio.rs`)

Interpretador por regras:
- 27 pedidos reais em português viram os intents esperados (tempos `1:12,5`, `1 minuto e 15`, `aqui`, `no final`; alvos; aspas; vários pedidos numa frase);
- pedido fora do escopo responde "Não entendi" com exemplos; texto sem aspas recebe a dica.

Execução num projeto real:
- 4 pedidos numa frase viram **um** passo no histórico: tempo removido exato, música +6 dB, texto criado, fade no último clipe; desfazer restaura tudo;
- erros específicos (instante fora do vídeo, sem seleção, "Passo 2: velocidade 9×", sem transcrição, "nada é esticado") não alteram nada;
- duração-alvo, transições e zoom funcionam.

Modelo local:
- o schema vai no `format`; o prompt tem seleção e cursor, sem caminhos de arquivo;
- ação fora do vocabulário (`run_shell`), JSON inválido e tipo errado são recusados.

Serviço com mídia real:
- prévia sem alterar a revisão, aplicação, **exportação validada** e desfazer;
- plano desatualizado recusado (`stale_plan`);
- LLM indisponível com o motivo; LLM local pelo protocolo do Ollama vira plano, informa o que foi enviado e é aplicado; ação maliciosa do modelo recusada.

Regressão de núcleo: mídia com duração fora da grade de quadros não deixa mais uma "lasca" de 1 ms no fim. Isso foi encontrado pela interface na Fase 6.

### Fase 7 — desempenho

Algoritmos em escala (`avs-ai/tests/scale.rs`):
- 3 h de análise sintética (1886 trechos): edição automática com 943 clipes, revisão, 10 sugestões, desfazer/refazer e serialização;
- limites que falham se a revisão ou as sugestões voltarem a ficar quadráticas (antes: 1,9 s e 3,4 s; agora: 0,07 s e 0,13 s em debug).

Render (`avs-media/tests/effects.rs`):
- o desfoque forte usa redução/ampliação e fica a **54 dB de PSNR** do gaussiano em resolução cheia;
- um trecho compartilhado por duas trilhas abre uma entrada só (`split`), com as cores e o áudio certos.

Outros:
- paralelismo adaptativo (`render.rs`): estimativa de memória calibrada pelas medições; limites por núcleos, memória e hardware;
- preview por janelas (`studio.rs`):
  - janela alinhada à grade, sem passar do fim;
  - arquivo de 120 s; evento com a janela;
  - a janela seguinte reaproveita 3 blocos do cache;
  - as duas janelas coexistem;
- lógica de janela atual e próxima no store (vitest, 4 testes);
- detecção de cenas sem Python: progresso real e monotônico.

### Fase 8 — E2E da interface (`e2e/`)

O app real é dirigido por WebDriver: `tauri-driver` + `WebKitWebDriver`, com um cliente W3C sem dependências (`e2e/webdriver.mjs`).
- A biblioteca é preenchida pela CLI `avs` com mídia sintética do FFmpeg:
  - 12 s com silêncios em 3–5 s e 8–10 s;
  - um projeto de 5 min.
- Cada cenário roda contra a interface, sem atalhos internos.
- Em caso de falha, ficam em `e2e/artifacts/` uma captura de tela e o texto da tela.

```bash
sudo apt install webkit2gtk-driver xvfb && cargo install tauri-driver --locked
cd apps/desktop && npm run build && cd ../..
cargo build --release -p ai-video-studio --features custom-protocol -p avs-cli
xvfb-run -a node e2e/run.mjs                    # todos
node e2e/run.mjs --only exportar,texto          # alguns
node e2e/run.mjs --app /usr/bin/ai-video-studio --installed   # contra o pacote instalado
```

| Cenário | O que verifica |
|---|---|
| `dashboard` | ações principais e projetos da biblioteca |
| `gerenciador_ia_e_presets` | Gerenciador de IA abre com os componentes; catálogo de presets |
| `criar_projeto_vazio` | criação pelo diálogo e editor vazio |
| `assistente_dividir_desfazer_refazer` | pedido em português → plano → aplicar → desfazer e refazer |
| `assistente_cortar_trecho` | "corte de 0:02 a 0:04" (0:12 → 0:10) e pedido fora do escopo explicado |
| `selecao_teclado_e_inspector` | clique seleciona e mostra o inspector; Delete apaga; Ctrl+Z restaura |
| `versoes_salvas` | salvar versão nomeada com data legível, editar e restaurar a versão |
| `editar_automaticamente_e_revisao` | sem análise → "Analisar e aplicar" → revisão de qualidade |
| `texto` | inserir um título pela aba Texto |
| `analise_e_pausas` | análise real, plano de pausas, aplicar (0:12 → 0:08/0:09) |
| `exportar` | exportação pela interface, aviso de validação e arquivo com vídeo e áudio |
| `recuperacao_apos_fechar_sem_salvar` | marcador sem salvar, encerrar o app, reabrir e recuperar |
| `reproducao_e_preview_por_janelas` | reprodução **real** e troca de janela em 2:00 numa timeline de 5 min |
| `cortes_verticais` | sugerir, criar e abrir um corte 1080×1920 |

Resultados neste ambiente (Xvfb, 2 vCPUs):
- **14/14** com o binário de release;
- **14/14** contra o `.deb` 0.8.0 instalado (`--installed`);
- 4/4 contra o AppImage extraído (dashboard, presets, exportação e reprodução, com o GStreamer empacotado).

Defeitos reais encontrados pelo E2E e corrigidos:
- a pasta de exportação lida só na montagem do diálogo;
- "Editar automaticamente" sem análise mostrava só um erro (agora oferece analisar);
- datas das versões em ISO cru;
- **a reprodução no Linux**: `asset://`, fim de arquivo e pipeline parado, descritos em `PERFORMANCE.md`.

### Frontend (vitest)
- conversões de tempo idênticas às do Rust, timecode por frame, snap magnético

### Verificação manual da interface
O app foi executado sob Xvfb com a biblioteca preenchida pela CLI (`docs/screenshots/`): dashboard, abertura de projeto, seleção, divisão com `S`, preview automático com progresso real, diálogo de exportação e recuperação após `kill` do processo. Na Fase 4, o fluxo completo pela interface:
- card "Edição automática" → preset YouTube Shorts → seletor de arquivos nativo;
- projeto criado com as mídias na timeline e o vídeo horizontal preenchendo o 9:16;
- job de análise + edição (22,7 s → 13,1 s, música cortada com fade e reduzida sob a fala);
- revisão de qualidade abrindo sozinha, com o aviso honesto de duração abaixo do alvo de 45 s;
- Ctrl+Z voltando ao original;
- prévia "Agressivo" com os trechos em vermelho;
- "Aplicar edição" salvando "Edição IA v2" (`docs/screenshots/edicao-automatica-*.png`, `revisao-qualidade.png`).

Na Fase 7, com a biblioteca do `avs bench` (`AVS_LIBRARY`):
- timeline de 26 min com 135 clipes desenhando miniaturas e waveform;
- o preview do projeto abriu em janela (0:00–2:00) pronta em 26 s, contra 4 min 40 s do preview completo.

Nessa fase, concluímos que a reprodução não funcionava por falta de codecs H.264. O diagnóstico estava errado: a causa era o protocolo `asset://` no WebKitGTK. A correção e o teste estão na Fase 8.

Na Fase 6, a aba **Assistente**:
- o pedido "adicione o texto \"Bem-vindos\" em 0:01 por 3 segundos no topo, aplique fade de entrada de 1 segundo e deixe mais claro" virou 3 ações na prévia;
- aplicado, o texto aparece no topo do frame renderizado e a entrada começa no preto (`docs/screenshots/assistente.png`).

Na Fase 5, a aba **Cortes**:
- sugestões com nota e justificativa, marcadas em verde na timeline;
- criação de 2 e de 3 cortes ("Fundo desfocado" e "Seguir rosto", este com o aviso de rosto não detectado em material sem pessoas);
- abertura de um corte com a trilha de fundo e a música encerrada no fim.

Achados corrigidos nessa verificação:
- folga invadindo o clipe seguinte;
- "intervalo vazio";
- painel mantendo o estado de outro projeto;
- status "Vazio" no dashboard;
- aviso de rosto impreciso (`docs/screenshots/cortes-*.png`).

A CLI `avs shorts` analisou, sugeriu, criou e exportou um corte 1080×1920 validado em 28 s.

Na Fase 3: análise de uma entrevista sintética (Silero: 4 trechos de fala, 1 corte de cena), plano de pausas em vermelho na timeline (4 cortes, 8,2 s), aplicação (21,97 s → 13,77 s), instalação da transcrição pelo Gerenciador de IA e falha clara ao baixar o Whisper (403 da rede do sandbox).

## Mapa dos cenários da seção 30

| Cenário | Situação |
|---|---|
| A — gameplay de 30 min com edição automática | 🟡 edição automática pronta e testada com material sintético; falta a medição com gravação real de 30 min (`avs demo --seconds N` cobre o pipeline de arquivos longos) |
| B — cortes verticais | ✅ sugestão, enquadramento, criação e exportação em lote validadas com render real (`studio.rs`, `shorts.rs`) e pela interface; falta validar o YuNet com rostos reais |
| C — podcast | 🟡 fala/pausas/legendas e cortes com rosto prontos; identificação de locutor e tela dividida pendentes |
| D — edição manual | ✅ `studio.rs` + E2E (seleção, divisão, texto, inspector) |
| E — edição por comando + undo/redo + exportação | ✅ pedidos em português → plano → um passo no histórico → exportação validada (`assistant.rs`, `studio.rs`) |
| F — arquivos grandes | ✅ `avs bench` de 30 min medido; escala de 3 h coberta por teste de algoritmos; preview por janelas; memória por processo medida e paralelismo limitado pela RAM. Falta só um benchmark real de 3 h numa máquina com disco suficiente |
| G — interrupção | ✅ `studio.rs` + E2E `recuperacao_apos_fechar_sem_salvar` |
| H — modelos indisponíveis | ✅ sem Ollama e sem venv: editor manual intacto, análise com alternativas do FFmpeg e avisos claros (testes + UI) |

## Próximos testes
- E2E no Windows e no macOS: o `tauri-driver` não tem WebDriver para o WKWebView, e no Windows exige o `msedgedriver` da mesma versão do WebView2. Hoje o CI roda o E2E só no Linux; Windows e macOS rodam os testes Rust e do frontend.
- benchmark noturno com arquivos de 1 a 3 h e medição de pico de RSS
- validar os instaladores de Windows e macOS (gerados pelo workflow de release; não foram executados neste ambiente)
