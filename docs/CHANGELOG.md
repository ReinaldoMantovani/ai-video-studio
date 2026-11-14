# Changelog

## 0.8.0 — Fase 8 (Consolidação)

- **Testes de interface automatizados (`e2e/`)**:
  - o app real é dirigido por WebDriver (`tauri-driver` + WebKitWebDriver), com um cliente sem dependências;
  - 14 cenários: assistente, desfazer/refazer, teclado, versões, edição automática, texto, análise e pausas, exportação validada, recuperação após fechar sem salvar, cortes verticais, reprodução real com troca de janela;
  - capturas e texto da tela em caso de falha.
- **Reprodução do preview no Linux corrigida.** O WebKitGTK não toca `<video>` por `asset://`. O preview agora vem de um servidor local, restrito a 127.0.0.1, com token por execução, só GET/HEAD de mídia dentro da biblioteca e suporte a Range.
  - O diagnóstico da Fase 7 ("faltam codecs") estava errado e foi corrigido na documentação e na mensagem de erro.
- **Troca de janela durante a reprodução corrigida**, achada pelo E2E com reprodução real:
  - o fim natural do arquivo parava a reprodução;
  - o cursor voltava ao início da janela ao trocar de arquivo;
  - o pipeline do WebKitGTK ficava parado após a troca (play só após o `seeked` e vigia de tempo parado).
- **Outros defeitos achados pelo E2E**:
  - "Editar automaticamente" sem análise agora oferece **Analisar e aplicar**, em vez de só mostrar erro;
  - o diálogo de exportação passou a lembrar a última pasta a cada abertura;
  - as datas das versões salvas aparecem em formato brasileiro, não em ISO cru.
- **Instaladores**:
  - `.deb` e `.rpm` com dependência do FFmpeg;
  - AppImage com GStreamer embutido;
  - NSIS/MSI em pt-BR e inglês, DMG para macOS 11+, ícones de todas as plataformas.
  - `.deb`, `.rpm` e AppImage gerados aqui; `.deb` instalado passou nos 14 cenários do E2E; o AppImage passou nos 4 em que foi testado.
- **CI**: formatação, clippy, bindings TypeScript em dia, testes em Linux/Windows/macOS e E2E no Linux com Xvfb.
- **Release** (`release.yml`): instaladores das três plataformas numa tag `v*`, FFmpeg empacotado no Windows e macOS, assinatura e notarização opcionais por secrets (nada no repositório).
- Teste do preview por janelas confere a duração de cada fluxo (vídeo e áudio) do arquivo.
- Documentação revisada: README, DEVELOPMENT, TESTING (seção E2E), SECURITY (servidor de mídia, distribuição), PERFORMANCE e ARCHITECTURE.

## 0.7.0 — Fase 7 (Otimização)

- `avs bench`: benchmark de gravação longa.
  - Gera uma palestra sintética com fala real.
  - Mede importação, análise, pausas, revisão, cortes, scrubbing, desfazer, salvar, exportação de 60 s e de um corte vertical.
  - Registra o pico de memória do app e dos processos filhos por etapa.
- 30 min medidos: importação em 10 s, análise completa em 132 s, pausas, revisão e sugestões em menos de 0,1 s, scrubbing em 137 ms.
- Revisão de qualidade **29× mais rápida** e sugestão de cortes **27× mais rápida** em gravações longas (indexação das séries ordenadas). Teste de 3 h com limites.
- **Preview por janelas** em timelines acima de 4 min: janelas de 2 min em volta do cursor, próxima janela preparada durante a reprodução, cache compartilhado. Reproduzir passou de 4 min 40 s para 26 s numa timeline de 26 min.
- **Timeline** desenha só os clipes perto da tela. Miniaturas e waveform cobrem só a parte visível. Corrigido o canvas acima do limite do navegador em clipes longos e as miniaturas que paravam após 400.
- **Corte vertical com fundo desfocado 32% mais rápido**: desfoque forte em resolução reduzida, 54 dB de PSNR contra o original. Entradas idênticas abertas uma vez (`split`).
- **Paralelismo adaptativo** de renderização: núcleos, memória livre (estimativa calibrada por medições) e encoders por hardware.
- Detecção de cenas sem Python com progresso real.
- Mensagem clara em português quando o sistema não tem codecs para reproduzir o preview (Linux sem GStreamer libav).
- `AVS_LIBRARY` também no app desktop, com o protocolo `asset` restrito à biblioteca em uso.

## 0.6.0 — Fase 6 (Assistente por linguagem natural)

- Aba **Assistente** no editor: o pedido em português vira um plano visível (ações e duração antes → depois) e você escolhe Aplicar ou Descartar. Cada aplicação é um único Ctrl+Z.
- **Interpretação por regras** (offline). Ações:
  - cortar trecho, dividir, apagar clipe;
  - remover pausas, legendas, duração-alvo;
  - velocidade, volume, mudo, fades;
  - texto, zoom, cor, efeitos (desfoque, nitidez, vinheta, P&B, quente/frio);
  - transições, marcadores, congelar, reverso.
- Usa o clipe selecionado e o cursor como contexto. Vários pedidos numa frase.
- **Modelo local opcional (Ollama)**: saída restrita por JSON Schema ao mesmo vocabulário, validada de novo; a interface mostra o que foi enviado (resumo da timeline, sem mídia).
- Proteções: plano desatualizado é recusado; ações desconhecidas do modelo são recusadas; limites de valores.
- Correção (núcleo): mídia com duração fora da grade de quadros deixava uma sobra de menos de um quadro no fim após cortes. Inserir agora arredonda para baixo e a remoção descarta a sobra.

## 0.5.0 — Fase 5 (Cortes verticais)

- Aba **Cortes** no editor:
  - sugestão de trechos autossuficientes (nota, título, justificativa, "ir para", destaque verde na timeline), com quantidade e durações mínima, alvo e máxima ajustáveis;
  - cada corte vira um **projeto vertical novo**; a origem nunca muda e os arquivos derivados são reaproveitados por hard link.
- Enquadramento:
  - **Seguir rosto**: planos estáticos no rosto principal ou no grupo, trocando em cortes de cena ou em deslocamento sustentado;
  - **Centro**;
  - **Fundo desfocado**.
- Cada corte passa pela limpeza automática do preset (pausas, legendas, áudio, ducking, exposição, zoom) e pode ser **exportado em lote** com validação.
- CLI: `avs shorts <projeto> [--preset] [--count] [--framing] [--dry-run] [--export DIR]`.
- Comando de edição `MoveTrack` (reordenar trilhas).
- Justificativas da pontuação mais legíveis (só os fatores; instante e nota aparecem à parte).
- Correções:
  - corte até o fim de uma timeline com duração fora da grade de quadros falhava com "intervalo vazio";
  - a folga de um corte podia invadir o clipe seguinte;
  - um corte com erro interrompia os demais e deixava uma pasta órfã.

## 0.4.0 — Fase 4 (Edição automática)

- **Editar automaticamente** por preset, na aba IA e no novo card do dashboard (projeto → importar → timeline → job de análise + edição). Etapas:
  - legendas e hesitações (com transcrição);
  - pausas conforme o estilo (Conservador/Equilibrado/Agressivo);
  - seleção por pontuação até a duração-alvo, em ordem cronológica e sem esticar;
  - fim da trilha sonora com fade;
  - transições nos saltos;
  - zoom pontual nos picos de movimento;
  - limpeza da fala, ducking da música e correção de exposição.
- Pontuação editorial de trechos (0–100) com os pesos do preset e justificativa.
- Prévia antes de aplicar (trechos em vermelho na timeline, etapas justificadas, notas com "ir para").
- Um único passo no histórico (`EditSession::transaction`). Versões "Original (antes da edição automática)" e "Edição IA vN" salvas automaticamente.
- Revisão automática de qualidade com 8 verificações e "ir para", sem nota perfeita automática.
- Versões nomeadas preservam o rótulo original (acentos e espaços) em `snapshots/labels.json`.
- Correção: ao colocar mídia na timeline, a duração arredondada para quadros não passa mais do fim do arquivo.

## 0.3.0 — Fase 3 (IA local) + início da Fase 4

- Crate `avs-ai`: runtime Python gerenciado (venv por componente), worker JSON-RPC por stdio com progresso e cancelamento, pipeline de análise com cache por impressão digital, cliente Ollama, perfis de hardware e planejadores de edição.
- Worker Python `avs_worker`: Silero VAD via onnxruntime (sem PyTorch), PySceneDetect, OpenCV (movimento, brilho, YuNet), faster-whisper com timestamps por palavra e download de modelos.
- Gerenciador de IA: instalar/remover componentes e modelos Whisper, perfil, idioma, Ollama (estado, modelos, baixar, testar, remover).
- Aba IA no editor: analisar mídias, remover pausas com prévia na timeline, marcar/dividir cenas, gerar legendas da transcrição.
- Alternativas sem Python: fala por energia e cenas pelo FFmpeg, identificadas no resultado.
- Abas do painel esquerdo mostram só o ícone quando inativas.
- Falhas de rede em jobs passam a ser recuperáveis (botão Repetir).

## 0.2.0 — Fase 2 (Editor manual)

- Velocidade 0,25×–4× (áudio sem mudança de tom), reverso, congelar frame (`F`).
- Keyframes de zoom, posição e volume, com easing linear, suave ou degrau; marcadores na timeline e edição no inspector.
- Ferramentas de timeline: seleção (`V`), ripple (`B`), roll (`N`), slip (`Y`), slide (`U`); duplicar (`Ctrl+D`).
- Cor e efeitos: temperatura, LUT 3D, desfoque, nitidez, vinheta.
- Tratamento de áudio por clipe: passa-altas, redução de ruído, compressor e EQ de 3 bandas.
- Legendas SRT/VTT: importar (também por arrastar/importar mídia), exportar, aplicar estilo a todas.
- O nome de um clipe de texto acompanha o texto, salvo se foi renomeado.
- Projetos da versão 0.1 abrem sem conversão (campos novos com valores padrão).

## 0.1.0 — Fase 1 (Fundação)

- Projetos, importação com análise, timeline multitrilha, transições, textos, undo/redo, autosave e recuperação.
- Compilador de renderização único (preview WYSIWYG = exportação), renderização incremental por blocos, exportação validada, encoders por hardware testados.
