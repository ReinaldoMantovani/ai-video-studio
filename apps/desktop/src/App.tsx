import { useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { CircleAlert, CircleCheck, Info, X } from "lucide-react";
import { useEffect } from "react";
import { Dashboard } from "./components/dashboard/Dashboard";
import { Editor } from "./components/editor/Editor";
import { JobsDrawer } from "./components/JobsDrawer";
import { AiManager } from "./components/AiManager";
import type { QualityReport } from "./bindings/QualityReport";
import { api, onAppEvent } from "./lib/api";
import { useApp } from "./store/app";

export default function App() {
  const view = useApp((s) => s.view);
  const qc = useQueryClient();

  // Eventos do backend: jobs, mídia analisada, preview pronto.
  useEffect(() => {
    let refreshTimer: ReturnType<typeof setTimeout> | undefined;
    api.jobs().then((j) => useApp.getState().setJobs(j)).catch(() => {});
    const un = onAppEvent((e) => {
      const s = useApp.getState();
      switch (e.type) {
        case "job": {
          const prev = s.jobs[e.job.id];
          s.upsertJob(e.job);
          if (prev?.state !== e.job.state) {
            if (e.job.state === "completed" && e.job.kind === "export") {
              s.toast("success", `${e.job.title}: concluído e validado`);
              qc.invalidateQueries({ queryKey: ["exports"] });
            }
            if (e.job.state.startsWith("failed")) s.toast("error", `${e.job.title}: ${e.job.error ?? "falhou"}`);
            if (e.job.kind === "import" && e.job.state === "completed") qc.invalidateQueries({ queryKey: ["projects"] });
            if ((e.job.kind === "ai_install" || e.job.kind === "ollama_pull") && e.job.state === "completed") {
              s.toast("success", `${e.job.title}: concluído`);
              qc.invalidateQueries({ queryKey: ["ai-status"] });
            }
            if (e.job.kind === "analysis" && e.job.project_id) qc.invalidateQueries({ queryKey: ["analyses", e.job.project_id] });
            if (e.job.kind === "auto_edit" && e.job.state === "completed") {
              qc.invalidateQueries({ queryKey: ["analyses", e.job.project_id] });
              qc.invalidateQueries({ queryKey: ["projects"] });
              if (s.ps?.project.id === e.job.project_id) {
                const res = e.job.result as { review?: QualityReport; snapshot_after?: string } | null;
                s.refresh().then(() => {
                  s.toast("success", `Edição automática aplicada${res?.snapshot_after ? ` (versão "${res.snapshot_after}")` : ""}. Ctrl+Z desfaz.`);
                  if (res?.review) useApp.getState().showReview(res.review);
                });
              }
            }
          }
          break;
        }
        case "asset":
          if (s.ps?.project.id === e.project_id) {
            clearTimeout(refreshTimer);
            refreshTimer = setTimeout(() => useApp.getState().refresh(), 120);
          }
          break;
        case "preview_ready":
          if (s.ps?.project.id === e.project_id) s.previewReady({ path: e.path, revision: e.revision, range: e.range });
          break;
        case "project_changed":
          break;
        case "analysis_ready":
          qc.invalidateQueries({ queryKey: ["analyses", e.project_id] });
          break;
        case "ai_changed":
          qc.invalidateQueries({ queryKey: ["ai-status"] });
          break;
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, [qc]);

  return (
    <div className="flex h-full flex-col">
      {view.name === "dashboard" ? <Dashboard /> : <Editor />}
      <JobsDrawer />
      <AiManager />
      <Toasts />
    </div>
  );
}

function Toasts() {
  const toasts = useApp((s) => s.toasts);
  const dismiss = useApp((s) => s.dismiss);
  return (
    <div className="pointer-events-none fixed bottom-4 left-1/2 z-[60] flex -translate-x-1/2 flex-col items-center gap-2" aria-live="polite">
      {toasts.map((t) => (
        <div
          key={t.id}
          className={clsx(
            "avs-in pointer-events-auto flex max-w-[560px] items-start gap-2.5 rounded-lg border bg-ink-2 px-3.5 py-2.5 shadow-xl shadow-black/40",
            t.kind === "error" ? "border-bad/40" : t.kind === "success" ? "border-ok/30" : "border-line",
          )}
        >
          {t.kind === "error" ? <CircleAlert size={16} className="mt-0.5 shrink-0 text-bad" /> : t.kind === "success" ? <CircleCheck size={16} className="mt-0.5 shrink-0 text-ok" /> : <Info size={16} className="mt-0.5 shrink-0 text-accent" />}
          <span className="text-[13px] leading-snug select-text">{t.text}</span>
          <button aria-label="Fechar aviso" className="ml-1 text-fg-3 hover:text-fg" onClick={() => dismiss(t.id)}>
            <X size={14} />
          </button>
        </div>
      ))}
    </div>
  );
}
