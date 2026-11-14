import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import { AlertTriangle, CheckCircle2, ClipboardCheck, Loader2, Wand2, XCircle } from "lucide-react";
import { useEffect, useState } from "react";
import type { AutoEditOptions } from "../../bindings/AutoEditOptions";
import type { AutoEditPreview } from "../../bindings/AutoEditPreview";
import type { CheckLevel } from "../../bindings/CheckLevel";
import type { CutIntensity } from "../../bindings/CutIntensity";
import type { PresetRules } from "../../bindings/PresetRules";
import { api, errorMessage } from "../../lib/api";
import { fromSecs, secs, shortDuration } from "../../lib/time";
import { useApp } from "../../store/app";
import { Button, Dialog, inputCls, Progress } from "../ui";

const CUTS: { id: CutIntensity; label: string; hint: string }[] = [
  { id: "conservative", label: "Conservador", hint: "corta só pausas longas" },
  { id: "balanced", label: "Equilibrado", hint: "regras do preset" },
  { id: "aggressive", label: "Agressivo", hint: "ritmo mais rápido" },
];
const EFFECTS: [string, string][] = [
  ["off", "Desligados"],
  ["subtle", "Sutis"],
  ["moderate", "Moderados"],
  ["intense", "Intensos"],
];
const NARRATIVE: [string, string][] = [
  ["preserve", "Preservar tudo (só limpar)"],
  ["summarize", "Resumo da gravação"],
  ["highlight_events", "Acontecimentos de destaque"],
  ["short_story", "História curta"],
  ["best_moments", "Melhores momentos"],
];

/** Opções iniciais a partir das regras do preset (o usuário pode mudar tudo). */
export function optionsFromRules(r: PresetRules): AutoEditOptions {
  return {
    cuts: "balanced",
    effects: r.effect_intensity,
    narrative: r.narrative,
    target_duration: r.target_s != null ? fromSecs(r.target_s) : null,
    remove_pauses: r.remove_silence,
    remove_fillers: r.remove_fillers,
    captions: r.captions,
    transitions: r.transition_frequency !== "rare",
    zoom: r.effects.includes("zoom"),
    audio_treatment: r.noise_reduction || r.voice_priority,
    ducking: r.music_ducking_db < 0,
    color_correction: r.effects.includes("color_correction") || r.effects.includes("color_grade"),
  };
}

/** Transcrição instalada (componente + modelo Whisper do perfil)? */
export function useTranscribeAvailable() {
  const status = useQuery({ queryKey: ["ai-status"], queryFn: api.aiStatus, staleTime: 30000 });
  const st = status.data;
  const has = (c: string) => st?.components.find((x) => x.id === c)?.installed ?? false;
  const whisper = st?.effective.whisper_model ?? "small";
  return { ok: has("transcription") && has(`whisper:${whisper}`), whisper };
}

export function AutoEditOptionsForm({ value, onChange, compact }: { value: AutoEditOptions; onChange: (o: AutoEditOptions) => void; compact?: boolean }) {
  const tr = useTranscribeAvailable();
  const set = (p: Partial<AutoEditOptions>) => onChange({ ...value, ...p });
  const target = value.target_duration != null ? Math.round(secs(value.target_duration)) : "";
  const check = (key: keyof AutoEditOptions, label: string, opts?: { needsText?: boolean }) => {
    const disabled = opts?.needsText && !tr.ok;
    return (
      <label className={clsx("flex items-start gap-2 text-xs", disabled ? "text-fg-3" : "text-fg-2")} title={disabled ? `Precisa da transcrição (Gerenciador de IA → Transcrição + Whisper ${tr.whisper})` : undefined}>
        <input type="checkbox" className="mt-0.5" disabled={disabled} checked={!!value[key] && !disabled} onChange={(e) => set({ [key]: e.target.checked } as Partial<AutoEditOptions>)} />
        <span>
          {label}
          {disabled && <span className="block text-[10px] text-fg-3">requer transcrição</span>}
        </span>
      </label>
    );
  };
  return (
    <div className="flex flex-col gap-3">
      <div>
        <p className="mb-1 px-0.5 text-[11px] text-fg-3">Cortes</p>
        <div className="flex gap-1" role="radiogroup" aria-label="Intensidade dos cortes">
          {CUTS.map((c) => (
            <Button key={c.id} size="sm" role="radio" aria-checked={value.cuts === c.id} title={c.hint} className="flex-1" variant={value.cuts === c.id ? "primary" : "subtle"} onClick={() => set({ cuts: c.id })}>
              {c.label}
            </Button>
          ))}
        </div>
      </div>
      <div className={clsx("grid gap-2", compact ? "grid-cols-1" : "grid-cols-2")}>
        <label className="flex flex-col gap-1 text-[11px] text-fg-3">
          Narrativa
          <select className={inputCls} value={value.narrative ?? "preserve"} onChange={(e) => set({ narrative: e.target.value })}>
            {NARRATIVE.map(([k, l]) => (
              <option key={k} value={k}>
                {l}
              </option>
            ))}
          </select>
        </label>
        <label className="flex flex-col gap-1 text-[11px] text-fg-3">
          Efeitos
          <select className={inputCls} value={value.effects ?? "subtle"} onChange={(e) => set({ effects: e.target.value })}>
            {EFFECTS.map(([k, l]) => (
              <option key={k} value={k}>
                {l}
              </option>
            ))}
          </select>
        </label>
        <label className="flex flex-col gap-1 text-[11px] text-fg-3">
          Duração-alvo (s)
          <input
            className={inputCls}
            type="number"
            min={5}
            max={7200}
            placeholder="sem alvo"
            value={target}
            onChange={(e) => set({ target_duration: e.target.value === "" ? null : fromSecs(Math.max(5, Number(e.target.value))) })}
          />
        </label>
      </div>
      <div className={clsx("grid gap-x-3 gap-y-1.5", compact ? "grid-cols-1" : "grid-cols-2")}>
        {check("remove_pauses", "Remover pausas")}
        {check("remove_fillers", "Remover hesitações", { needsText: true })}
        {check("captions", "Legendas automáticas", { needsText: true })}
        {check("transitions", "Transições nos saltos")}
        {check("zoom", "Zoom pontual na ação")}
        {check("audio_treatment", "Limpar o áudio da fala")}
        {check("ducking", "Música abaixo da fala")}
        {check("color_correction", "Corrigir exposição")}
      </div>
    </div>
  );
}

/** Seção "Editar automaticamente" da aba IA: prévia → aplicar (1 passo no histórico). */
export function AutoEditSection() {
  const ps = useApp((s) => s.ps)!;
  const toast = useApp((s) => s.toast);
  const jobs = useApp((s) => s.jobs);
  const setAutoPlan = useApp((s) => s.setAutoPlan);
  const setPlayhead = useApp((s) => s.setPlayhead);
  const id = ps.project.id;
  const [rules, setRules] = useState<PresetRules | null>(null);
  const [opts, setOpts] = useState<AutoEditOptions | null>(null);
  const [preview, setPreview] = useState<AutoEditPreview | null>(null);
  const [busy, setBusy] = useState(false);
  const running = Object.values(jobs).find((j) => j.kind === "auto_edit" && j.project_id === id && (j.state === "running" || j.state === "queued"));
  const revision = ps.history.revision;

  useEffect(() => {
    api
      .autoEditRules(id)
      .then((r) => {
        setRules(r);
        setOpts(optionsFromRules(r));
      })
      .catch(() => {});
  }, [id]);
  // Qualquer edição invalida a prévia.
  useEffect(() => {
    setPreview(null);
  }, [revision]);

  const hasMedia = ps.project.timeline.tracks.some((t) => t.clips.some((c) => c.content.type === "media"));
  const change = (o: AutoEditOptions) => {
    setOpts(o);
    setPreview(null);
    setAutoPlan(null);
  };
  const run = async (f: () => Promise<void>) => {
    setBusy(true);
    try {
      await f();
    } catch (e) {
      toast("error", errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  if (!rules || !opts) return null;
  return (
    <section className="flex flex-col gap-2.5 rounded-lg border border-accent/25 bg-accent-soft/40 p-2.5">
      <div className="flex items-center gap-2 px-0.5">
        <Wand2 size={14} className="text-accent" />
        <h3 className="text-xs font-semibold text-fg">Editar automaticamente</h3>
        <span className="ml-auto truncate text-[11px] text-fg-3" title="Regras do preset do projeto">
          {rules.name}
        </span>
      </div>
      <AutoEditOptionsForm value={opts} onChange={change} compact />
      {running ? (
        <div className="px-0.5">
          <p className="truncate text-[11px] text-fg-2">{running.stage ?? "Na fila…"}</p>
          <Progress value={running.progress} />
        </div>
      ) : (
        <Button
          size="sm"
          disabled={busy || !hasMedia}
          title={hasMedia ? "" : "Coloque vídeos ou áudios na timeline"}
          onClick={() =>
            run(async () => {
              const p = await api.autoEditPreview(id, opts);
              setPreview(p);
              setAutoPlan(p.report.removed);
            })
          }
        >
          {busy ? <Loader2 size={13} className="animate-spin" /> : <Wand2 size={13} />} Pré-visualizar plano
        </Button>
      )}
      {preview && !running && (
        <div className="flex flex-col gap-2 rounded-md bg-ink-1 p-2.5 text-xs">
          <p className="text-fg">
            {shortDuration(preview.report.before)} → <strong>{shortDuration(preview.report.after)}</strong>
            {preview.report.removed.length > 0 && <span className="text-fg-3"> · trechos removidos em vermelho na timeline</span>}
          </p>
          <ol className="flex flex-col gap-1.5">
            {preview.report.steps.map((s) => (
              <li key={s.kind} className="leading-snug">
                <span className="font-medium text-fg">{s.title}:</span> <span className="text-fg-2">{s.detail}</span>
              </li>
            ))}
          </ol>
          {[...preview.report.warnings, ...preview.missing_analysis.map((n) => `'${n}' ainda não foi analisado: será analisado ao aplicar.`)].map((w) => (
            <p key={w} className="flex gap-1.5 text-warn">
              <AlertTriangle size={12} className="mt-0.5 shrink-0" /> {w}
            </p>
          ))}
          {preview.report.moments.length > 0 && (
            <details className="text-fg-2">
              <summary className="cursor-pointer text-fg-2 hover:text-fg">Pontuação dos trechos ({preview.report.moments.length})</summary>
              <p className="mt-1 text-[10px] text-fg-3">Heurística editorial a partir da análise — não é previsão de visualizações.</p>
              <ul className="mt-1.5 flex max-h-56 flex-col gap-1 overflow-y-auto pr-1">
                {preview.report.moments.map((m, i) => (
                  <li key={i}>
                    <button className="flex w-full items-start gap-2 rounded px-1 py-0.5 text-left hover:bg-ink-2" onClick={() => setPlayhead(m.range.start)}>
                      <span className={clsx("w-8 shrink-0 text-right tabular font-semibold", m.kept ? "text-ok" : "text-fg-3")}>{Math.round(m.score)}</span>
                      <span className="min-w-0 flex-1">
                        <span className="tabular text-fg-3">{shortDuration(m.range.start)}</span> {m.explanation}
                        {!m.kept && <span className="text-fg-3"> · fora do corte</span>}
                      </span>
                    </button>
                  </li>
                ))}
              </ul>
            </details>
          )}
          <div className="mt-1 flex gap-1.5">
            {preview.missing_analysis.length > 0 ? (
              <Button
                size="sm"
                variant="primary"
                className="flex-1"
                disabled={busy}
                onClick={() =>
                  run(async () => {
                    await api.autoEditStart(id, opts);
                    setPreview(null);
                    setAutoPlan(null);
                    toast("info", "Analisando e editando em segundo plano (veja Trabalhos).");
                  })
                }
              >
                Analisar e aplicar
              </Button>
            ) : (
              <Button
                size="sm"
                variant="primary"
                className="flex-1"
                disabled={busy || preview.report.steps.length === 0}
                onClick={() =>
                  run(async () => {
                    const r = await api.autoEditApply(id, opts);
                    useApp.getState().setState(r.state);
                    setPreview(null);
                    setAutoPlan(null);
                    toast("success", `Edição aplicada. Versões salvas: "${r.result.snapshot_before}" e "${r.result.snapshot_after}". Ctrl+Z desfaz.`);
                    useApp.getState().showReview(await api.qualityReview(id));
                  })
                }
              >
                Aplicar edição
              </Button>
            )}
            <Button size="sm" variant="ghost" onClick={() => (setPreview(null), setAutoPlan(null))}>
              Descartar
            </Button>
          </div>
        </div>
      )}
      <Button size="sm" variant="ghost" disabled={busy} onClick={() => run(async () => useApp.getState().showReview(await api.qualityReview(id)))}>
        <ClipboardCheck size={13} /> Revisar qualidade
      </Button>
    </section>
  );
}

const LEVEL: Record<CheckLevel, { icon: typeof CheckCircle2; cls: string; label: string }> = {
  pass: { icon: CheckCircle2, cls: "text-ok", label: "ok" },
  warn: { icon: AlertTriangle, cls: "text-warn", label: "atenção" },
  fail: { icon: XCircle, cls: "text-bad", label: "problema" },
};

/** Revisão automática de qualidade, com "ir para" em cada ponto. */
export function QualityReviewDialog() {
  const review = useApp((s) => s.review);
  const open = useApp((s) => s.reviewOpen);
  const close = useApp((s) => s.closeReview);
  const setPlayhead = useApp((s) => s.setPlayhead);
  const select = useApp((s) => s.select);
  if (!review) return null;
  const L = LEVEL[review.level];
  return (
    <Dialog open={open} onClose={close} title="Revisão de qualidade" width={560}>
      <div className="flex flex-col gap-3">
        <div className={clsx("flex items-center gap-2 rounded-lg border px-3 py-2", review.level === "pass" ? "border-ok/30" : review.level === "warn" ? "border-warn/30" : "border-bad/40")}>
          <L.icon size={16} className={L.cls} />
          <p className="text-[13px]">{review.summary}</p>
        </div>
        {review.checks.map((c) => {
          const C = LEVEL[c.level];
          return (
            <div key={c.id}>
              <div className="flex items-center gap-2">
                <C.icon size={14} className={C.cls} />
                <span className="text-[13px] font-medium">{c.title}</span>
                <span className="ml-auto text-[11px] text-fg-3">{c.note}</span>
              </div>
              {c.issues.length > 0 && (
                <ul className="mt-1 ml-6 flex flex-col gap-0.5">
                  {c.issues.map((i, k) => (
                    <li key={k} className="flex items-center gap-2 text-xs text-fg-2">
                      <span className="min-w-0 flex-1">{i.message}</span>
                      {i.at != null && (
                        <Button
                          size="sm"
                          variant="ghost"
                          onClick={() => {
                            setPlayhead(i.at!);
                            if (i.clip_id) select(i.clip_id);
                            close();
                          }}
                        >
                          ir para
                        </Button>
                      )}
                    </li>
                  ))}
                </ul>
              )}
            </div>
          );
        })}
      </div>
    </Dialog>
  );
}
