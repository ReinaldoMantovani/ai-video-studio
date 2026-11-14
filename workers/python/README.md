# Workers de IA (`avs_worker`)

Pacote Python executado pelo app como **processo filho** (`python -m avs_worker`), dentro do venv gerenciado em `<dados do app>/ai/venv`. Não abre portas de rede.

## Instalação (feita pelo Gerenciador de IA)

| Arquivo | Componente |
|---|---|
| `requirements-core.txt` | Análise de fala e imagem (numpy, onnxruntime, OpenCV, PySceneDetect) |
| `requirements-vad-model.txt` | instalado com `--no-deps`: apenas o modelo ONNX do Silero VAD, sem PyTorch |
| `requirements-transcribe.txt` | Transcrição (faster-whisper, huggingface_hub) |

Uso manual para desenvolvimento:

```bash
python3 -m venv .venv
.venv/bin/pip install -r requirements-core.txt
.venv/bin/pip install --no-deps -r requirements-vad-model.txt
echo '{"jsonrpc":"2.0","id":1,"method":"capabilities","params":{}}' | .venv/bin/python -m avs_worker
```

## Protocolo

JSON-RPC 2.0, uma mensagem por linha em stdin/stdout. Logs vão para stderr.

```jsonc
// app → worker
{"jsonrpc":"2.0","id":1,"method":"vad","params":{"wav":"/…/audio16k.wav"}}
// worker → app (progresso real, ≥ 0,5 % de diferença)
{"jsonrpc":"2.0","method":"$/progress","params":{"id":1,"stage":"detectando fala","fraction":0.42}}
// app → worker, a qualquer momento
{"jsonrpc":"2.0","method":"$/cancel"}
// resposta
{"jsonrpc":"2.0","id":1,"result":{"duration":8.59,"speech":[[1.956,6.332]],"speech_ratio":0.51}}
// erro com código estável
{"jsonrpc":"2.0","id":1,"error":{"code":"model_missing","message":"…"}}
```

| Método | Parâmetros | Resultado |
|---|---|---|
| `capabilities` | `models_dir` | versões instaladas, modelo Silero, modelos Whisper, CUDA, provedores ONNX |
| `vad` | `wav` (16 kHz mono), limiares opcionais | `speech: [[início, fim]]`, `speech_ratio` |
| `scenes` | `video`, `detector` (adaptive/content) | `cuts`, `shots`, `fps` |
| `visual` | `video`, `interval`, `faces`, `model_dir` | `samples: [{t, motion, luma, faces}]` |
| `transcribe` | `wav`, `models_dir`, `model`, `language` | `language`, `segments` com `words` |
| `download_whisper` | `model`, `models_dir` | caminho e bytes (download atômico via `.partial`) |

Códigos de erro: `cancelled`, `component_missing`, `model_missing`, `network`, `invalid_media`, `invalid_audio`, `invalid_params`, `busy`, `internal`.
