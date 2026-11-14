# IA local — modelos, componentes e estratégia

> **Estado (0.3.0):** Fase 3 implementada. Análise de fala, cenas, imagem e rostos verificada com componentes reais. A transcrição está implementada, mas os modelos Whisper (Hugging Face) não puderam ser baixados no ambiente de desenvolvimento: **valide a transcrição na sua máquina** (ver "Validação pendente").

## Princípios

1. Funciona **sem nenhuma chave paga**. Depois de instalar os componentes, funciona **offline**.
2. Nenhuma mídia sai da máquina. As únicas conexões são para **instalar** componentes (PyPI, GitHub para o YuNet, Hugging Face para modelos Whisper) e para o Ollama em `127.0.0.1`. A URL do Ollama é recusada se não for local.
3. A IA **não edita arquivos**: os planejadores produzem `EditCommand`s validados e desfazíveis, mostrados antes de aplicar.
4. Modelos são sugeridos pelo hardware **medido**, com aviso antes de baixar algo pesado demais.
5. Sem componentes de IA, o editor continua 100% funcional e a análise usa alternativas do FFmpeg, identificadas no resultado.

## Arquitetura

```
app (Rust)                                     worker (Python, processo filho)
avs-ai::runtime  ── venv próprio em <dados>/ai/venv; pip por componente
avs-ai::worker   ── JSON-RPC 2.0 por stdin/stdout ──▶ avs_worker
                     $/progress (progresso real)      vad.py        Silero VAD (ONNX, sem PyTorch)
                     $/cancel   (cancelamento)        scenes.py     PySceneDetect
                                                       visual.py     OpenCV: movimento, brilho, YuNet
                                                       transcribe.py faster-whisper + download
avs-ai::analysis ── pipeline + cache por impressão digital (<dados>/ai/analysis/<fp>/)
avs-ai::autoedit ── planos → EditCommand (remover pausas, cenas, legendas)
avs-ai::ollama   ── cliente HTTP local (estado, modelos, download, geração com JSON Schema)
avs-ai::profile  ── Econômico / Equilibrado / Qualidade máxima
```

O worker não abre portas de rede, é encerrado ao fim de cada chamada e recebe apenas caminhos de arquivos do cache do app.

## Componentes (Gerenciador de IA)

| Componente | O que instala | Download | Licença | Verificado aqui |
|---|---|---|---|---|
| Ambiente Python | venv a partir do Python 3.10+ do sistema | ~15 MB | PSF | ✅ |
| Análise de fala e imagem | numpy, onnxruntime, opencv-python-headless, scenedetect; `silero-vad` com `--no-deps` (só o modelo ONNX, **sem PyTorch**) | ~140 MB | MIT / BSD-3 / Apache-2.0 | ✅ |
| Detecção de rostos | YuNet 2023mar (OpenCV Zoo), SHA-256 conferido | 0,2 MB | MIT | ✅ |
| Transcrição | faster-whisper, huggingface_hub | ~120 MB | MIT | ✅ instalado |
| Whisper tiny…large-v3 | modelo CTranslate2 (`Systran/faster-whisper-*`) | 75 MB–3 GB | MIT | ⚠️ download bloqueado no ambiente de desenvolvimento |
| Ollama | externo (ollama.com); o app detecta, lista, baixa, testa e remove modelos | conforme o modelo | por modelo | cliente testado com servidor de protocolo; Ollama real indisponível aqui |

Com todos os componentes de análise, o venv ocupa ~740 MB. Com PyTorch, passaria de vários GB.

## Análise

| Etapa | Primária | Alternativa sem Python | Saída |
|---|---|---|---|
| Volume | Rust (RMS a cada 100 ms do WAV 16 kHz) | — | `loudness` |
| Fala | Silero VAD: janelas de 32 ms, histerese, fala ≥ 250 ms, folga 60 ms | limiar de energia adaptativo (ruído + 12 dB, fala ≥ 250 ms) | `speech.segments`, `speech.method` |
| Cenas | PySceneDetect `AdaptiveDetector` (proxy quando existe) | `select='gt(scene,0.35)'` do FFmpeg | `scenes.cuts`, `scenes.method` |
| Imagem | OpenCV: a cada N s (perfil), movimento entre frames consecutivos, brilho, rostos (YuNet) | — (aviso) | `visual.samples` |
| Transcrição | faster-whisper com timestamps por palavra; idioma automático ou fixo (pt, en, es…) | — (aviso) | `transcript` |

Resultados ficam em cache por impressão digital do arquivo: a mesma gravação não é reanalisada, nem em outro projeto. Uma etapa feita pela alternativa é refeita automaticamente quando o componente principal é instalado.

## Edição automática (Fase 4)

- **Remover pausas**: complemento da fala dentro de cada clipe, com pausa mínima e folga vindas do preset (ex.: Entrevistas 900 ms / 250 ms). Opção de **manter pausas com ação na imagem** (`preserve_silent_action` dos presets de gameplay). Respeita velocidade, evita transições, nunca remove um clipe inteiro. O plano aparece em vermelho na timeline, com o total removido, antes de aplicar. As legendas podem acompanhar os cortes. Tudo vira um único Ctrl+Z.
- **Marcar cenas / dividir nas cenas** do clipe selecionado.
- **Gerar legendas da transcrição**: agrupa palavras em linhas ≤ 42 caracteres e ≤ 4 s, quebrando em pausas e pontuação. Em formatos verticais, posiciona acima da área de controles da plataforma.

### Editar automaticamente por preset

Disponível na aba IA do editor e no card **Edição automática** do dashboard. O dashboard cria o projeto, importa, coloca as mídias na timeline e analisa/edita num job.

- **Estilo**:
  - cortes Conservador, Equilibrado ou Agressivo (multiplicam a pausa mínima do preset por 1,6, 1,0 ou 0,6);
  - narrativa: preservar, resumo, destaques, história curta ou melhores momentos;
  - duração-alvo;
  - efeitos: desligados, sutis, moderados ou intensos;
  - interruptores para cada etapa.
- **Pontuação de trechos** (`scoring.rs`): os trechos de fala são unidos quando ficam a menos de 1,2 s e limitados a 30 s; sem fala, usam-se as cenas ou janelas de 8 s.
  - Fatores medidos: relevância, gancho, completude, expressividade, ritmo, qualidade técnica, ação visual, reação, sequência de eventos, intensidade sonora, sinal de humor e clareza.
  - Os fatores vêm do volume, do movimento, dos rostos, das cenas e da transcrição, e são ponderados pelos `scoring_weights` do preset.
  - Cada trecho recebe uma nota de 0 a 100 e uma justificativa com os 3 fatores principais.
  - É uma **heurística editorial**, não uma previsão de audiência.
- **Seleção**: escolhe os trechos de maior nota até a duração-alvo. No modo "resumo", garante antes o melhor trecho de cada quarto da gravação.
  - Mantém a ordem cronológica e une trechos próximos.
  - Se não houver material suficiente, **avisa e não estica** nada.
- **Sem transcrição**, legendas e hesitações são puladas com aviso. O resto funciona só com a análise de fala, cenas e imagem.
- **Prévia** antes de aplicar: etapas com justificativa, duração antes → depois, trechos removidos em vermelho e nota de cada trecho com "ir para".
- **Aplicar** gera um único passo no histórico (Ctrl+Z desfaz tudo) e salva duas versões, "Original" e "Edição IA vN", na aba Histórico.
- **Revisão de qualidade** (também em "Revisar qualidade"). Verifica:
  - duração × alvo/máximo do preset;
  - cortes no meio de falas;
  - trechos vazios;
  - imagem preta ≥ 1 s e silêncio ≥ 3 s;
  - texto fora da área segura ou sobreposto;
  - trecho repetido e clipes < 0,4 s;
  - risco de distorção e vídeo sem som;
  - proporção com barras.

  Cada ponto tem "ir para". O resumo sem problemas diz "nenhum problema encontrado nos pontos verificados", nunca "perfeito".
- **Ainda não**: refinamento do plano pela LLM local (Ollama) e reenquadramento por rastreamento de rosto. Hoje o reenquadramento é um corte central.

## Cortes verticais (Fase 5)

Na aba **Cortes** do editor (ou `avs shorts`):
- escolha o formato (preset vertical), a quantidade e as durações mínima, alvo e máxima;
- **Sugerir cortes** lista trechos que se sustentam sozinhos, com nota, título (início da transcrição, quando existe), justificativa e "ir para", marcados em verde na timeline;
- marque os cortes, escolha o enquadramento e **Criar**: cada corte vira um projeto novo, editável como qualquer outro, e o projeto de origem não muda.

| Enquadramento | Como funciona | Requer |
|---|---|---|
| Seguir rosto | Planos estáticos centrados no rosto principal (ou no grupo). Troca em cortes de cena ou deslocamento sustentado. Sem rosto, usa o centro e avisa. | Detecção de rostos (YuNet) e análise das mídias |
| Centro | Corte central fixo | — |
| Fundo desfocado | Vídeo inteiro, com a mesma imagem desfocada e escurecida ao fundo | — |

Em cada corte também entram as etapas da edição automática (todas opcionais ou conforme o preset):
- remoção de pausas;
- legendas (com transcrição);
- limpeza da fala;
- música abaixo da fala e encerrada com fade;
- correção de exposição;
- zoom pontual.

Com **Exportar ao criar**, todos vão para a fila com o perfil escolhido e passam pela validação normal.

Limitações atuais:
- o enquadramento não faz panorâmica contínua (cada plano é fixo);
- ainda não há layouts facecam + gameplay nem tela dividida com locutor ativo;
- a detecção de rostos em vídeos reais precisa ser validada na sua máquina. Os testes injetam detecções e conferem o resultado nos pixels renderizados.

## Assistente de edição (Fase 6)

Aba **Assistente** do editor. Você escreve em português, vê o plano (cada ação com o que fará e a duração antes → depois) e escolhe **Aplicar** ou **Descartar**. Cada pedido aplicado é um único Ctrl+Z.

**Interpretação por regras** (padrão, offline, sem IA generativa). Entende pedidos como:
- "corte de 0:10 a 0:20", "tire os primeiros 5 segundos", "divida aqui";
- "remova as pausas maiores que 1,5 segundos", "gere legendas", "deixe com 30 segundos";
- "aumente a música em 3 dB", "silencie a música", "acelere o clipe selecionado 2x", "câmera lenta";
- "zoom de 20% em 0:15", "fade de saída de 2 segundos no final", "transição entre todos os clipes";
- "adicione o texto \"Inscreva-se\" em 0:05 por 3 segundos no topo";
- "preto e branco", "mais claro", "congele a imagem em 0:08", "marque este ponto como \"gancho\"".

Vários pedidos podem ir numa frase.

**Modelo local (Ollama)**, opcional: com um modelo escolhido no Gerenciador de IA e o Ollama rodando, aparece "Interpretar com o modelo local".
- O modelo recebe o pedido e um resumo da timeline (nomes e tempos, sem mídia).
- A resposta é restrita por JSON Schema às mesmas ações e validada de novo antes da prévia.
- A interface mostra o que foi enviado.
- Validado com um servidor que fala o protocolo do Ollama. A qualidade com modelos reais precisa ser avaliada na sua máquina: veja abaixo.

O assistente não exporta, não apaga arquivos e não executa nada no sistema. Ele só propõe edições.

## Perfis de desempenho

| Perfil | Quando é escolhido | Whisper | Amostragem visual | LLM sugerido |
|---|---|---|---|---|
| Econômico | padrão | base | 2 s | ~4B quantizado |
| Equilibrado | ≥ 12 GB RAM e ≥ 6 núcleos, ou GPU ≥ 4 GB | small (CPU) / medium (GPU) | 1 s | 4B–9B |
| Qualidade máxima | GPU CUDA ≥ 8 GB e ≥ 16 GB RAM | medium (CPU) / large-v3 (GPU) | 0,5 s | maior que couber |

O usuário pode trocar o perfil, o idioma e o modelo Whisper. Modelos que usam mais da metade da RAM recebem aviso.

## Validação pendente (faça na sua máquina)

1. Gerenciador de IA → instalar **Transcrição** e **Whisper base** (ou small).
2. Aba IA → marcar **Transcrever fala** → **Analisar mídias**.
3. Selecionar o clipe → **Gerar legendas da transcrição**.
4. Instalar **Detecção de rostos (YuNet)**, reanalisar um vídeo com uma pessoa fora do centro e criar um corte com **Seguir rosto**: o rosto deve ficar centralizado no 9:16.
5. Ollama + modelo (ex.: qwen3:4b) → aba Assistente → marcar "Interpretar com o modelo local" → pedidos livres ("tira a enrolação do começo e põe um título"). Confira o plano antes de aplicar.
6. Opcional: instalar o Ollama, baixar um modelo pelo gerenciador e usar **Testar** (o modelo precisa responder JSON válido).

O código de transcrição e de download segue a API documentada do faster-whisper 1.2 e do huggingface_hub. Os erros reais (rede, modelo ausente) aparecem no gerenciador e na central de processamento.
