# Segurança

## Implementado

| Ameaça | Mitigação |
|---|---|
| Injeção de comandos via nomes de arquivo | FFmpeg/FFprobe executados com `Command::args` (sem shell); filtergraphs grandes vão para arquivo de script |
| Injeção no filtergraph | caminhos escapados em dois níveis (`escape_filter_value`, testado com `: ' [ ] , ;`); texto do usuário vai por `textfile` com `expansion=none`; cores aceitas só como hex validado (`ff_color`) |
| Path traversal | `safe_join` rejeita `..`, caminhos absolutos e prefixos; nomes de exportação sanitizados (separadores, caracteres reservados, `CON`/`NUL`…); exportação exige pasta absoluta |
| Escrita de arquivos pelo frontend | exportação de legendas só grava `.srt`/`.vtt` em caminho absoluto escolhido no diálogo do sistema; LUTs só são lidas (`.cube`/`.3dl` existentes) e passam pelo escape do filtergraph |
| Sobrescrita acidental | exportações nunca sobrescrevem (`nome (2).mp4`); o arquivo final só aparece depois de concluído (`.part` + rename) |
| Leitura arbitrária de arquivos pela WebView | protocolo `asset` limitado à pasta `projects` da biblioteca em uso; abrir ou revelar arquivos só por comando Rust que confere o histórico de exportações; capacidades Tauri mínimas (diálogos + eventos) |
| Conteúdo remoto na WebView | CSP restritiva: só `self`, `asset:` e `ipc:`, e `media-src` também em `http://127.0.0.1:*` (servidor de mídia abaixo); nenhuma origem externa |
| Servidor local de mídia (Linux) | O WebKitGTK não reproduz `<video>` por `asset://`, então o preview é servido por HTTP local (`crates/avs-app/src/media_server.rs`).
- Escuta só em `127.0.0.1`, numa porta efêmera.
- Exige um token aleatório de 128 bits no caminho, gerado a cada execução e comparado em tempo constante.
- Aceita só GET/HEAD e só extensões de mídia.
- O caminho é canonizado (sem `..`, links resolvidos) e precisa estar dentro da pasta `projects` da biblioteca.
- Sem listagem de diretórios e sem CORS.

Outro programa da máquina sem o token recebe 403. Testado em `tests/media_server.rs`: token errado, fora da raiz, `..`, extensão não permitida e Range. |
| Corrupção do projeto | gravação atômica com fsync; `.bak` da versão anterior; autosave separado; schema versionado (rejeita versões futuras) |
| Arquivos corrompidos ou maliciosos | probe com timeout; erros reais de FFprobe; mídia sem dimensões ou duração é rejeitada |
| Esgotamento de recursos | uma exportação por vez; paralelismo limitado (blocos e miniaturas); waveform em streaming (memória constante); timeout em probes e extrações |
| Cancelamento inseguro | processo filho finalizado; parciais removidos; nunca entram no cache |
| Envio silencioso de mídia | mídia nunca é enviada; rede só para **instalar** componentes de IA escolhidos pelo usuário (PyPI, GitHub/YuNet com SHA-256, Hugging Face/Whisper) e para o Ollama, aceito apenas em 127.0.0.1/localhost |
| Assistente por linguagem natural | o pedido vira **intents** de um vocabulário fechado (18 ações), não comandos.
- Com o modelo local, a saída é restrita por JSON Schema (`format` do Ollama) e validada de novo no app: ação desconhecida, campo com tipo errado ou mais de 12 ações → recusa com o motivo, sem "conserto" silencioso.
- Tempos e valores têm limites: instante dentro do vídeo, velocidade de 0,25× a 4×, texto de até 200 caracteres, pedido de até 1000.
- Tudo roda numa **cópia** para a prévia e é aplicado só com confirmação, numa transação validada; um plano feito sobre outra revisão do projeto é recusado.
- O modelo recebe apenas o pedido e um resumo textual da timeline (nomes e tempos dos clipes, sem caminhos e sem mídia), e a interface mostra isso a cada uso.
- Sem o modelo, as regras não enviam nada para fora do processo. |
| Workers de IA | processo filho sem porta de rede (JSON-RPC por stdio), venv isolado (`PYTHONNOUSERSITE`), encerrado ao fim de cada chamada; o LLM não executa comandos — só gera dados convertidos em `EditCommand` validados |

## Planejado (com as fases de IA)

- Chaves de provedores pagos no cofre do SO (`keyring`: Credential Manager, Keychain, Secret Service), nunca no frontend nem nos logs, exibidas mascaradas.
- Consentimento explícito antes de qualquer envio de mídia a um serviço externo.
- Verificação de integridade (SHA-256) dos downloads de modelos que ainda não a têm.

## Distribuição

- O workflow `release.yml` assina os instaladores só com certificados vindos de secrets do GitHub (Developer ID + notarização no macOS, Authenticode no Windows).
- Nenhuma chave fica no repositório. A sobreposição de configuração com a impressão digital do certificado é criada só durante o build e está no `.gitignore`.
- Sem os secrets, os instaladores saem sem assinatura, e o SmartScreen e o Gatekeeper vão avisar.

## Reportar vulnerabilidades

Abra uma issue privada ou envie e-mail ao mantenedor. Não publique detalhes antes de haver correção.
