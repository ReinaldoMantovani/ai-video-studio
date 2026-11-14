import { useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { AlertTriangle, CheckCircle2, Cpu, Download, FlaskConical, Loader2, Server, Trash2 } from "lucide-react";
import { useState } from "react";
import type { AiComponent } from "../bindings/AiComponent";
import type { AiProfile } from "../bindings/AiProfile";
import type { AiSettings } from "../bindings/AiSettings";
import { api, errorMessage } from "../lib/api";
import { bytes } from "../lib/time";
import { useApp } from "../store/app";
import { Button, Dialog, Field, inputCls, Progress } from "./ui";

const PROFILE_LABEL: Record<AiProfile, string> = { economy: "Econômico", balanced: "Equilibrado", max: "Qualidade máxima" };
const LANGS: [string | null, string][] = [
  [null, "Detectar automaticamente"],
  ["pt", "Português"],
  ["en", "Inglês"],
  ["es", "Espanhol"],
];

/** Gerenciador de IA: componentes locais, modelos, perfil de desempenho e Ollama. */
export function AiManager() {
  const open = useApp((s) => s.aiOpen);
  const setOpen = useApp((s) => s.setAiOpen);
  const toast = useApp((s) => s.toast);
  const jobs = useApp((s) => s.jobs);
  const qc = useQueryClient();
  const status = useQuery({ queryKey: ["ai-status"], queryFn: api.aiStatus, enabled: open, refetchInterval: open ? 15000 : false });
  const st = status.data;
  const [pull, setPull] = useState("");
  const [testing, setTesting] = useState<string | null>(null);

  const lastError = (title: string) => {
    const list = Object.values(jobs).filter((j) => j.kind === "ai_install" && j.title === title).sort((a, b) => b.created_at.localeCompare(a.created_at));
    return list[0]?.state.startsWith("failed") ? list[0].error : null;
  };
  const running = (title: string) => Object.values(jobs).find((j) => (j.kind === "ai_install" || j.kind === "ollama_pull") && j.title === title && (j.state === "running" || j.state === "queued"));
  const installed = (id: string) => st?.components.find((c) => c.id === id)?.installed ?? false;
  const saveSettings = async (patch: Partial<AiSettings>) => {
    if (!st) return;
    try {
      await api.setAiSettings({ ...st.settings, ...patch });
      qc.invalidateQueries({ queryKey: ["ai-status"] });
    } catch (e) {
      toast("error", errorMessage(e));
    }
  };

  return (
    <Dialog open={open} onClose={() => setOpen(false)} title="Gerenciador de IA" width={860}>
      {!st ? (
        <div className="flex items-center gap-2 py-10 text-fg-2">
          <Loader2 size={16} className="animate-spin" /> Verificando componentes e hardware…
        </div>
      ) : (
        <div className="flex flex-col gap-6">
          <p className="text-xs text-fg-2">
            Tudo roda neste computador. Nenhuma mídia é enviada para a internet. Os componentes ficam em <span className="select-text text-fg">{st.library_path}</span>.
          </p>

          <section>
            <h3 className="mb-2 flex items-center gap-2 text-[13px] font-semibold">
              <Cpu size={14} className="text-accent" /> Perfil de desempenho
            </h3>
            <div className="grid grid-cols-3 gap-3">
              <Field label="Perfil" hint={`Hardware: ${st.effective.reason}`}>
                <select className={inputCls} value={st.settings.profile ?? "auto"} onChange={(e) => saveSettings({ profile: e.target.value === "auto" ? null : (e.target.value as AiProfile) })}>
                  <option value="auto">Automático ({PROFILE_LABEL[st.recommended]})</option>
                  {(Object.keys(PROFILE_LABEL) as AiProfile[]).map((p) => (
                    <option key={p} value={p}>
                      {PROFILE_LABEL[p]}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="Idioma da transcrição">
                <select className={inputCls} value={st.settings.language ?? ""} onChange={(e) => saveSettings({ language: e.target.value || null })}>
                  {LANGS.map(([v, l]) => (
                    <option key={l} value={v ?? ""}>
                      {l}
                    </option>
                  ))}
                </select>
              </Field>
              <Field label="Modelo de transcrição">
                <select className={inputCls} value={st.settings.whisper_model ?? ""} onChange={(e) => saveSettings({ whisper_model: e.target.value || null })}>
                  <option value="">Do perfil (Whisper {st.effective.whisper_model})</option>
                  {st.components
                    .filter((c) => c.id.startsWith("whisper:"))
                    .map((c) => (
                      <option key={c.id} value={c.id.slice(8)}>
                        {c.name}
                        {c.installed ? "" : " (não instalado)"}
                      </option>
                    ))}
                </select>
              </Field>
            </div>
            <p className="mt-2 text-[11px] text-fg-3">
              Amostragem visual a cada {st.effective.visual_interval} s · sugestão de LLM: {st.effective.llm_hint}.
            </p>
          </section>

          <section>
            <h3 className="mb-2 text-[13px] font-semibold">Componentes locais</h3>
            {!st.system_python && !installed("runtime") && (
              <div className="mb-3 flex gap-2 rounded-lg border border-warn/30 bg-warn/10 p-3 text-xs text-warn">
                <AlertTriangle size={15} className="shrink-0" />
                Python 3.10+ não encontrado. Instale-o em python.org (ou defina AVS_PYTHON) para habilitar a IA local. O editor continua funcionando sem IA.
              </div>
            )}
            <div className="overflow-hidden rounded-lg border border-line">
              {st.components.map((c) => (
                <ComponentRow
                  key={c.id}
                  c={c}
                  ramMb={st.hardware.ram_mb}
                  blocked={c.requires.filter((r) => !installed(r))}
                  job={running(`Instalar ${c.name}`)}
                  error={lastError(`Instalar ${c.name}`)}
                  onInstall={async () => {
                    try {
                      await api.aiInstall(c.id);
                    } catch (e) {
                      toast("error", errorMessage(e));
                    }
                  }}
                  onRemove={async () => {
                    try {
                      await api.aiRemove(c.id);
                      qc.invalidateQueries({ queryKey: ["ai-status"] });
                    } catch (e) {
                      toast("error", errorMessage(e));
                    }
                  }}
                />
              ))}
            </div>
          </section>

          <section>
            <h3 className="mb-2 flex items-center gap-2 text-[13px] font-semibold">
              <Server size={14} className="text-accent" /> Ollama (modelos de linguagem locais)
            </h3>
            {st.ollama.running ? (
              <div className="flex flex-col gap-2">
                <p className="text-xs text-ok">
                  Em execução em {st.ollama.url} · versão {st.ollama.version}
                </p>
                {st.ollama.models.length === 0 && <p className="text-xs text-fg-3">Nenhum modelo baixado.</p>}
                {st.ollama.models.map((m) => (
                  <div key={m.name} className="flex items-center gap-3 rounded-md bg-ink-2 px-3 py-2 text-xs">
                    <input type="radio" name="llm" aria-label={`Usar ${m.name}`} checked={st.settings.ollama_model === m.name} onChange={() => saveSettings({ ollama_model: m.name })} />
                    <span className="flex-1 font-medium">{m.name}</span>
                    <span className="text-fg-3">
                      {m.parameter_size} {m.quantization}
                    </span>
                    <span className="w-16 text-right text-fg-3 tabular">{bytes(m.size_bytes)}</span>
                    <Button
                      size="sm"
                      variant="ghost"
                      disabled={testing === m.name}
                      onClick={async () => {
                        setTesting(m.name);
                        try {
                          await api.ollamaTest(m.name);
                          toast("success", `${m.name} respondeu corretamente em JSON.`);
                        } catch (e) {
                          toast("error", errorMessage(e));
                        } finally {
                          setTesting(null);
                        }
                      }}
                    >
                      {testing === m.name ? <Loader2 size={12} className="animate-spin" /> : <FlaskConical size={12} />} Testar
                    </Button>
                    <Button size="sm" variant="ghost" aria-label={`Remover ${m.name}`} onClick={() => api.ollamaDelete(m.name).then(() => qc.invalidateQueries({ queryKey: ["ai-status"] })).catch((e) => toast("error", errorMessage(e)))}>
                      <Trash2 size={12} />
                    </Button>
                  </div>
                ))}
                <div className="flex gap-2">
                  <input className={clsx(inputCls, "flex-1")} placeholder="Nome do modelo no Ollama (ex.: qwen3:4b)" value={pull} onChange={(e) => setPull(e.target.value)} />
                  <Button
                    disabled={!pull.trim()}
                    onClick={async () => {
                      try {
                        await api.ollamaPull(pull.trim());
                        setPull("");
                        useApp.getState().setJobsOpen(true);
                      } catch (e) {
                        toast("error", errorMessage(e));
                      }
                    }}
                  >
                    <Download size={13} /> Baixar
                  </Button>
                </div>
                <p className="text-[11px] text-fg-3">Confira a licença e o tamanho do modelo antes de baixar. O LLM será usado no planejamento da edição automática (próxima fase).</p>
              </div>
            ) : (
              <p className="text-xs text-fg-2">
                O Ollama não está em execução neste computador. Instale-o em ollama.com e abra o aplicativo; esta tela detecta automaticamente. Ele é opcional: análise de fala, cenas e transcrição funcionam sem ele.
              </p>
            )}
          </section>
        </div>
      )}
    </Dialog>
  );
}

function ComponentRow({
  c,
  ramMb,
  blocked,
  job,
  error,
  onInstall,
  onRemove,
}: {
  error?: string | null;
  c: AiComponent;
  ramMb: number;
  blocked: string[];
  job?: { progress: number; stage: string | null };
  onInstall: () => void;
  onRemove: () => void;
}) {
  const heavy = c.memory_mb > ramMb / 2;
  return (
    <div className="flex items-start gap-3 border-b border-line bg-ink-1 px-4 py-3 last:border-0">
      <div className="min-w-0 flex-1">
        <div className="flex items-center gap-2">
          <span className="font-medium">{c.name}</span>
          {c.installed && <CheckCircle2 size={13} className="text-ok" />}
          {c.version && <span className="text-[11px] text-fg-3">{c.version}</span>}
        </div>
        <p className="mt-0.5 text-xs text-fg-2">{c.description}</p>
        <p className="mt-1 text-[11px] text-fg-3">
          {c.download_mb ? `~${c.download_mb} MB` : ""}
          {c.memory_mb ? ` · ~${c.memory_mb} MB de memória` : ""}
          {c.languages.length ? ` · ${c.languages.join(", ")}` : ""} · licença {c.license}
        </p>
        {c.note && <p className="mt-1 text-[11px] text-fg-2">{c.note}</p>}
        {heavy && !c.installed && <p className="mt-1 text-[11px] text-warn">Pesado para este computador ({Math.round(ramMb / 1024)} GB de RAM): pode ficar muito lento.</p>}
        {error && !job && !c.installed && <p className="mt-1 text-[11px] text-bad select-text">Última tentativa falhou: {error}</p>}
        {job && (
          <div className="mt-2">
            <Progress value={job.progress} />
            <p className="mt-1 text-[11px] text-fg-3">{job.stage}</p>
          </div>
        )}
      </div>
      <div className="shrink-0">
        {c.installed ? (
          c.id !== "runtime" && (
            <Button size="sm" variant="ghost" onClick={onRemove}>
              Remover
            </Button>
          )
        ) : (
          <Button size="sm" variant="primary" disabled={!!job || blocked.length > 0} title={blocked.length ? `Requer: ${blocked.join(", ")}` : undefined} onClick={onInstall}>
            {job ? <Loader2 size={12} className="animate-spin" /> : <Download size={12} />} {blocked.length ? "Requer outro componente" : "Instalar"}
          </Button>
        )}
      </div>
    </div>
  );
}
