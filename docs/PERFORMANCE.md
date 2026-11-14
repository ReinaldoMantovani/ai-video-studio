# Desempenho

Não prometemos que todo vídeo será editado em X minutos: o tempo depende de hardware, codec, resolução e complexidade. O objetivo é minimizar o tempo total sem falhas, e **medir** cada etapa.

## Como medir

```bash
cargo build -p avs-cli --release
# Gravação longa (fala real + pausas): importação, análise, pausas, revisão,
# cortes, scrubbing, desfazer, salvar, exportação de 60 s e de um corte vertical,
# com pico de memória do app e do maior processo filho.
./target/release/avs bench --out /tmp/bench --minutes 30
# Fluxo curto de edição/preview/exportação/reexportação:
./target/release/avs demo --out /tmp/demo --seconds 180
```

Os dois gravam `benchmark.json` com os tempos reais por etapa, o sistema (CPU, RAM, GPU) e a versão do FFmpeg. O `bench` também registra a memória: o pico do processo do app (VmHWM) e o maior pico entre os processos filhos (`getrusage`), por etapa. Use `--library` para medir com os componentes de IA de uma biblioteca já configurada.

## Resultados medidos (referência, não promessa)

Ambiente: VM Linux com **2 vCPUs**, 8 GB RAM, **sem GPU** (somente libx264), FFmpeg 6.1, build release.
Material: gravação sintética 1920×1080 a 30 fps com áudio, mais uma trilha MP3. Edição: remoção de um trecho no meio com ripple, crossfade de 0,5 s, título, música a 25 % com fade. Exportação no perfil YouTube 1080p (`veryfast`, CRF 19, loudnorm –14 LUFS) com **validação completa** (decodificação integral + blackdetect + silencedetect).

| Etapa | Mídia de 60 s (timeline 53,5 s) | Mídia de 180 s (timeline 161,5 s) |
|---|---|---|
| Importação + análise (probe, poster, miniaturas, waveform) | 15,1 s | 32,1 s |
| Aplicar comandos de edição | < 10 ms | < 10 ms |
| Preview completo 540p | 15,3 s | 39,2 s |
| Exportação 1080p + validação | 45,3 s | 123,9 s (≈ 1,3× tempo real) |
| Reexportação após editar só o título | 33,7 s (1/2 blocos reaproveitados) | 46,3 s (**5/6 blocos reaproveitados**) |
| Pico de memória do maior processo filho | — | 425 MB |

Na reexportação, quase todo o tempo restante é a passada de validação (decodificação completa) e o mux final; o vídeo inalterado não é recodificado.

## IA local (medido, mesma VM de 2 vCPUs, sem GPU)

| Etapa | Tempo |
|---|---|
| Instalar "Análise de fala e imagem" (pip, com cache) | ~10 s; venv final de ~740 MB, sem PyTorch |
| Instalar "Transcrição" (pip) | 3,7 s |
| Analisar entrevista de 22 s em 720p (áudio, Silero VAD, PySceneDetect, imagem a cada 2 s com YuNet) | 5,2 s |
| Reanalisar a mesma mídia | instantâneo (cache por impressão digital) |

## Gravação longa — 30 min (Fase 7, medido)

Ambiente: a mesma VM (2 vCPUs, sem GPU), build release, `avs bench --minutes 30`.

Material: uma palestra sintética de 30 min, 1280×720 a 30 fps, 1,2 GB, com fala real do espeak-ng. As pausas são de 0,3 a 2,2 s.

Análise completa com os componentes de IA instalados: Silero, PySceneDetect e OpenCV/YuNet. Resultado bruto em `docs/benchmarks/bench-30min-720p-2vcpu.json`.

| Etapa | Tempo | Observação |
|---|---|---|
| Importar (probe, poster, miniaturas, waveform) | 10,2 s | miniaturas por seek e waveform em streaming: o custo não cresce com a duração como uma decodificação completa |
| Análise (fala, cenas, imagem a cada 2 s) | 132,5 s | 0,07× a duração; 270 trechos de fala, 900 amostras de imagem |
| Remover pausas (plano + aplicação) | < 0,1 s | 134 cortes, 30:00 → 26:24, 135 clipes, um único desfazer |
| Revisão de qualidade / sugestão de cortes | < 0,1 s | |
| Scrubbing (5 frames aleatórios, 540p) | pior: 137 ms | igual ao de um projeto de 2 min (seek por clipe) |
| Desfazer / refazer | 2 ms / 1 ms | |
| Salvar | < 0,1 s | 195 KB |
| Pico de memória do app | 21 MB | |

As exportações dessa rodada concorreram com uma compilação e não são comparáveis; ver a tabela de exportação abaixo.

### Escala de 3 h (teste `avs-ai/tests/scale.rs`, build debug)

A análise sintética tem 1886 trechos de fala, 400 cenas e amostras de imagem a cada 2 s.

| Operação | Antes da Fase 7 | Depois |
|---|---|---|
| Edição automática (pausas + áudio), 943 clipes | 1,2 s | 1,2 s |
| Revisão de qualidade | 1,9 s | **0,07 s** (29×) |
| Sugerir 10 cortes | 3,4 s | **0,13 s** (27×) |
| Desfazer + refazer | < 1 ms | < 1 ms |

As duas lentidões vinham do mesmo padrão: cada trecho percorria todas as amostras de volume (108 mil em 3 h) e todos os trechos de fala, O(trechos × amostras). Agora as séries ordenadas são fatiadas por índice ou busca binária. O teste falha se alguma das duas voltar a passar de 2 s.

## Exportação e memória (medido com `getrusage` por processo FFmpeg)

| Caso | Antes | Depois |
|---|---|---|
| Corte vertical 1080×1920 com fundo desfocado (47 s) | 105,5 s · 910 MB | **71,5 s** (−32%) · 806 MB |
| Trecho de 60 s em 1080p (perfil YouTube, `medium`) + validação | 60,5 s · 624 MB | 62 s · 622 MB (sem mudança; ver abaixo) |

- **Desfoque forte**: o gaussiano grande era aplicado no quadro antes do recorte (3413×1920 num 9:16). Agora a imagem é reduzida, desfocada com σ/d e ampliada de volta ao mesmo tamanho. A diferença para o gaussiano em resolução cheia é de **54 dB de PSNR** (imperceptível; teste `strong_blur_is_downscaled_but_looks_the_same`).
- **Entradas idênticas** (fundo + principal do mesmo trecho) são abertas uma vez e divididas com `split`.
- **De onde vêm os ~620 MB** de um bloco 1080p, medido isolando cada fator:
  - decodificação e grafo: ~210 MB;
  - libx264 `medium`: +~420 MB (lookahead e quadros de referência); `veryfast`: +~210 MB.

  É o custo da qualidade do perfil, não desperdício. Testamos e **descartamos** separar áudio e vídeo em entradas distintas: 174 vs 183 MB, sem ganho.
- **Paralelismo adaptativo**: o número de blocos simultâneos considera os núcleos (1 processo x264 já usa vários) e a memória disponível. Cada processo tem uma estimativa calibrada pelas medições acima (área do quadro × preset), e o total fica em até 60% da RAM livre. Encoders por hardware: 1 por vez.

  Exemplos: 1080p `medium` com 1,5 GB livres → 1 bloco por vez; 4K com 6 GB → 1; 16 núcleos com bastante RAM → 4.

## Preview em timelines longas

Antes, abrir um projeto gerava o preview da **timeline inteira**. Em 26 min de timeline (2 vCPUs), isso levava **4 min 40 s** até poder reproduzir; em 3 h, seria cerca de meia hora.

Agora, timelines acima de 4 min usam janelas de 2 min em volta do cursor, alinhadas à grade de blocos de 5 s:
- a primeira janela ficou pronta em **26 s** (no teste com mídia 320×180: 14,7 s);
- durante a reprodução, a próxima janela é preparada 40 s antes do fim da atual e a troca é automática;
- a sobreposição entre janelas sai do cache (teste `long_timeline_preview_is_windowed_and_reuses_cache`: 3 blocos reaproveitados);
- timelines curtas continuam com o preview inteiro.

A troca de janelas durante a reprodução real é verificada pelo E2E `reproducao_e_preview_por_janelas`:
- timeline de 5 min, cursor em 1:50;
- reprodução atravessando 2:00 e passando para o arquivo da janela seguinte, sem voltar o cursor.

**Correção de diagnóstico (Fase 8).** Na Fase 7, atribuímos a falta de reprodução no Linux à ausência de codecs do GStreamer. Estava errado:
- o WebKitGTK não carrega `<video>` pelo protocolo `asset://` do Tauri (erro 4, `networkState` 3);
- o mesmo arquivo tocava normalmente por `file://`.

No Linux, o preview agora é servido por um servidor local de mídia (ver `SECURITY.md`). A mensagem de erro de reprodução deixou de afirmar que faltam codecs: ela mostra o erro real e só sugere `gstreamer1.0-libav` como possibilidade.

A reprodução real revelou mais dois defeitos, já corrigidos:
- **Fim do arquivo.** O WebKit encerra o arquivo da janela ~0,45 s antes do fim nominal (os fluxos têm 120 s; o teste confere cada um). O `pause` do fim natural parava a reprodução em vez de trocar de janela. Agora quem decide é `onEnded`.
- **Pipeline parado após a troca.** Logo depois de trocar de arquivo, o WebKitGTK às vezes fica "tocando" sem avançar (`paused=false`, `readyState` 4, tempo parado). O player só dá play depois do `seeked`. Há também um vigia: com o tempo parado por 800 ms, ele faz pause/play.

## Timeline com centenas de clipes

- Só os clipes perto da tela são desenhados. A janela é quantizada em blocos de 1024 px, para não redesenhar a cada pixel de rolagem.
- Miniaturas e waveform cobrem só a parte visível de cada clipe. Antes:
  - o canvas da waveform tinha a largura do clipe inteiro: com zoom, um clipe de 30 min passava de 100 mil px e ultrapassava o limite do navegador;
  - as miniaturas paravam após 400 a partir do início, deixando o resto de um clipe longo em branco.
- Verificado na interface com a timeline de 26 min e 135 clipes do `bench`.

## Análise sem Python

A detecção de cenas pelo FFmpeg agora reporta progresso real (`-progress`). Os cortes vão para um arquivo de metadados. Antes, um arquivo de horas ficava em 0% até o fim.

## O que já está otimizado

- **Seek por clipe**: cada clipe abre o arquivo com `-ss` próprio. Um trecho na hora 2 custa o mesmo que um no minuto 0.
- **Renderização incremental por blocos** com cache por hash: edições pequenas só renderizam os blocos afetados. Pausar ou retomar reaproveita o trabalho feito.
- **Memória limitada**: um bloco por vez (ou N em paralelo), independentemente da duração da timeline; waveform decodificada em streaming (8 kHz mono, constante).
- **Miniaturas por seek** (custo constante por miniatura) com paralelismo limitado aos núcleos.
- **Proxies 540p** (H.264, GOP 15, FPS constante) gerados automaticamente para material > 1080p, HEVC, ProRes, DNxHD, AV1, 10 bits ou VFR; usados no preview, nunca na exportação.
- **Frame de scrubbing** renderizado só para o intervalo de 1 frame, com cache por (revisão, frame, altura).
- **Desfoque forte em resolução reduzida**, entradas idênticas compartilhadas e paralelismo pela memória disponível (Fase 7, acima).
- **Fila com prioridades**: preview (2) > análise (4) > exportação (5), com no máximo uma exportação simultânea para não esgotar RAM/VRAM.
- **Encoders por hardware** testados de verdade e usados automaticamente quando funcionam (NVENC, Quick Sync, AMF, VideoToolbox), com fallback para software.

## Ainda não feito (precisa de hardware ou de decisão)

- Decodificação por hardware (`-hwaccel`) e filtros na GPU: sem GPU neste ambiente para validar. Não ligamos algo que não podemos testar.
- Validação por amostragem em exportações de horas: a validação completa (decodificação integral) continua padrão. Trocar por amostragem reduz a garantia e deve ser uma escolha explícita do usuário.
- Benchmark real de 1 a 3 h (aqui faltam disco e tempo). A escala de 3 h está coberta pelo teste de algoritmos acima. Para medir na sua máquina: `avs bench --minutes 180`.
