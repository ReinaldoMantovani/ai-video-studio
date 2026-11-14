import clsx from "clsx";
import { FolderOpen, Pause, Play, RotateCcw, Square, Trash2, X } from "lucide-react";
import { useEffect, useState } from "react";
import type { JobRecord } from "../bindings/JobRecord";
import { api, errorMessage } from "../lib/api";
import { useApp } from "../store/app";
import { Empty, IconButton, Progress } from "./ui";

export const STATE_LABEL: Record<string, string> = {
  queued: "Aguardando",
  running: "Em execução",
  paused: "Pausado",
  completed: "Finalizado",
  cancelled: "Cancelado",
  failed_recoverable: "Erro recuperável",
  failed: "Erro definitivo",
};

const KIND_LABEL: Record<string, string> = { import: "Preparação de mídia", preview: "Preview", export: "Exportação", analysis: "Análise com IA", ai_install: "Instalação de IA", ollama_pull: "Modelo Ollama" };

function elapsed(j: JobRecord, now: number): string {
  if (!j.started_at) return "";
  const end = j.finished_at ? Date.parse(j.finished_at) : now;
  const s = Math.max(0, Math.round((end - Date.parse(j.started_at)) / 1000));
  return s >= 60 ? `${Math.floor(s / 60)} min ${s % 60} s` : `${s} s`;
}

/** Tempo restante: só exibido quando há progresso suficiente para estimar. */
function remaining(j: JobRecord, now: number): string | null {
  if (j.state !== "running" || !j.started_at || j.progress < 0.05) return null;
  const el = (now - Date.parse(j.started_at)) / 1000;
  const left = Math.round(el / j.progress - el);
  return left >= 60 ? `~${Math.round(left / 60)} min restantes` : `~${left} s restantes`;
}

export function activeJobs(jobs: Record<string, JobRecord>) {
  return Object.values(jobs).filter((j) => j.state === "running" || j.state === "queued");
}

export function JobRow({ j, now }: { j: JobRecord; now: number }) {
  const toast = useApp((s) => s.toast);
  const act = (a: "cancel" | "pause" | "resume" | "retry" | "remove") => api.jobAction(j.id, a).catch((e) => toast("error", errorMessage(e)));
  const result = j.result as { path?: string } | null;
  const failed = j.state.startsWith("failed");
  return (
    <div className="flex flex-col gap-2 rounded-lg border border-line bg-ink-2 p-3">
      <div className="flex items-start justify-between gap-2">
        <div className="min-w-0">
          <p className="truncate font-medium">{j.title}</p>
          <p className="text-xs text-fg-3">
            {KIND_LABEL[j.kind] ?? j.kind} · <span className={clsx(failed && "text-bad", j.state === "completed" && "text-ok")}>{STATE_LABEL[j.state] ?? j.state}</span>
            {j.stage && j.state === "running" ? ` · ${j.stage}` : ""}
          </p>
        </div>
        <div className="flex shrink-0 gap-0.5">
          {j.state === "running" && j.kind === "export" && (
            <IconButton label="Pausar" onClick={() => act("pause")}>
              <Pause size={14} />
            </IconButton>
          )}
          {j.state === "paused" && (
            <IconButton label="Continuar" onClick={() => act("resume")}>
              <Play size={14} />
            </IconButton>
          )}
          {(failed || j.state === "cancelled") && j.kind === "export" && (
            <IconButton label="Repetir" onClick={() => act("retry")}>
              <RotateCcw size={14} />
            </IconButton>
          )}
          {(j.state === "running" || j.state === "queued" || j.state === "paused") && (
            <IconButton label="Cancelar" onClick={() => act("cancel")}>
              <Square size={13} />
            </IconButton>
          )}
          {j.state === "completed" && j.kind === "export" && result?.path && (
            <IconButton label="Mostrar na pasta" onClick={() => api.openExport(result.path!, true).catch((e) => toast("error", errorMessage(e)))}>
              <FolderOpen size={14} />
            </IconButton>
          )}
          {!["running", "queued"].includes(j.state) && (
            <IconButton label="Remover da lista" onClick={() => act("remove").then(() => api.jobs().then(useApp.getState().setJobs))}>
              <Trash2 size={14} />
            </IconButton>
          )}
        </div>
      </div>
      {(j.state === "running" || j.state === "paused") && <Progress value={j.progress} />}
      <div className="flex justify-between text-[11px] text-fg-3 tabular">
        <span>{elapsed(j, now)}</span>
        <span>{j.state === "running" ? `${(j.progress * 100).toFixed(0)}%` : ""} {remaining(j, now) ?? ""}</span>
      </div>
      {j.error && <p className="select-text text-xs leading-snug text-bad">{j.error}</p>}
    </div>
  );
}

export function useNow(ms = 1000) {
  const [now, setNow] = useState(Date.now());
  useEffect(() => {
    const t = setInterval(() => setNow(Date.now()), ms);
    return () => clearInterval(t);
  }, [ms]);
  return now;
}

export function JobsDrawer() {
  const open = useApp((s) => s.jobsOpen);
  const setOpen = useApp((s) => s.setJobsOpen);
  const jobs = useApp((s) => s.jobs);
  const now = useNow();
  if (!open) return null;
  const list = Object.values(jobs).sort((a, b) => b.created_at.localeCompare(a.created_at));
  return (
    <aside className="avs-in fixed top-12 right-3 bottom-3 z-40 flex w-[380px] flex-col rounded-xl border border-line bg-ink-1 shadow-2xl shadow-black/50" aria-label="Central de processamento">
      <div className="flex items-center justify-between border-b border-line px-4 py-3">
        <h2 className="font-semibold">Processamento</h2>
        <IconButton label="Fechar" onClick={() => setOpen(false)}>
          <X size={16} />
        </IconButton>
      </div>
      <div className="flex flex-1 flex-col gap-2 overflow-y-auto p-3">
        {list.length === 0 ? (
          <Empty icon={<Play size={28} />} title="Nenhum trabalho ainda">
            Importações, previews e exportações aparecem aqui com o progresso real de cada etapa.
          </Empty>
        ) : (
          list.map((j) => <JobRow key={j.id} j={j} now={now} />)
        )}
      </div>
    </aside>
  );
}
