import { useQuery, useQueryClient } from "@tanstack/react-query";
import clsx from "clsx";
import { AudioLines, Captions, CheckCircle2, Clapperboard, Loader2, ScanFace, Scissors, Settings2, Sparkles } from "lucide-react";
import { useEffect, useState } from "react";
import type { PauseRules } from "../../bindings/PauseRules";
import { api, errorMessage } from "../../lib/api";
import { fromSecs, secs } from "../../lib/time";
import { useApp } from "../../store/app";
import { Button, Progress, Slider } from "../ui";
import { AutoEditSection } from "./AutoEdit";

const METHOD: Record<string, string> = { silero: "Silero VAD", energy: "nível de áudio", pyscenedetect: "PySceneDetect", ffmpeg: "FFmpeg" };

/** Aba IA do editor: análise local e ações de edição automática com prévia. */
export function AiPanel() {
  const ps = useApp((s) => s.ps)!;
  const toast = useApp((s) => s.toast);
  const jobs = useApp((s) => s.jobs);
  const selected = useApp((s) => s.selectedClip);
  const plan = useApp((s) => s.pausePlan);
  const setPlan = useApp((s) => s.setPausePlan);
  const setAiOpen = useApp((s) => s.setAiOpen);
  const qc = useQueryClient();
  const id = ps.project.id;
  const status = useQuery({ queryKey: ["ai-status"], queryFn: api.aiStatus, staleTime: 30000 });
  const analyses = useQuery({ queryKey: ["analyses", id], queryFn: () => api.analyses(id) });
  const [transcribe, setTranscribe] = useState(false);
  const [rules, setRules] = useState<PauseRules | null>(null);
  const [scope, setScope] = useState<"selected" | "all">("all");
  const [syncText, setSyncText] = useState(true);
  const [busy, setBusy] = useState(false);

  useEffect(() => {
    api.pauseRules(id).then(setRules).catch(() => {});
  }, [id]);

  const st = status.data;
  const has = (c: string) => st?.components.find((x) => x.id === c)?.installed ?? false;
  const whisper = st?.effective.whisper_model ?? "small";
  const canTranscribe = has("transcription") && has(`whisper:${whisper}`);
  const analyzing = Object.values(jobs).filter((j) => j.kind === "analysis" && j.project_id === id && (j.state === "running" || j.state === "queued"));
  const byAsset = new Map((analyses.data ?? []).map((a) => [a.asset_id, a.result]));
  const selClip = ps.project.timeline.tracks.flatMap((t) => t.clips).find((c) => c.id === selected);
  const selAsset = selClip?.content.type === "media" ? selClip.content.asset_id : null;
  const selResult = selAsset ? byAsset.get(selAsset) : undefined;

  const run = async (f: () => Promise<unknown>) => {
    setBusy(true);
    try {
      await f();
    } catch (e) {
      toast("error", errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <div className="flex flex-col gap-4">
      {st && (!has("analysis") || !has("faces")) && (
        <div className="rounded-lg border border-accent/30 bg-accent-soft p-3 text-xs">
          <p className="text-fg">
            {has("analysis") ? "Instale a detecção de rostos para análise visual completa." : "Sem os componentes de IA, a análise usa alternativas do FFmpeg (menos precisas)."}
          </p>
          <Button size="sm" className="mt-2" onClick={() => setAiOpen(true)}>
            <Settings2 size={12} /> Abrir Gerenciador de IA
          </Button>
        </div>
      )}

      <AutoEditSection />

      <section className="flex flex-col gap-2">
        <h3 className="px-1 text-xs font-semibold text-fg-2">Análise</h3>
        <label className={clsx("flex items-center gap-2 px-1 text-xs", canTranscribe ? "text-fg-2" : "text-fg-3")} title={canTranscribe ? "" : `Instale 'Transcrição' e o modelo Whisper ${whisper} no Gerenciador de IA`}>
          <input type="checkbox" disabled={!canTranscribe} checked={transcribe && canTranscribe} onChange={(e) => setTranscribe(e.target.checked)} />
          Transcrever fala (Whisper {whisper}){!canTranscribe && " — não instalado"}
        </label>
        <Button
          size="sm"
          variant="primary"
          disabled={analyzing.length > 0 || ps.project.assets.length === 0}
          onClick={() =>
            run(async () => {
              await api.analyzeMedia(id, [], transcribe && canTranscribe, false);
            })
          }
        >
          {analyzing.length ? <Loader2 size={13} className="animate-spin" /> : <Sparkles size={13} />} {analyzing.length ? "Analisando…" : "Analisar mídias"}
        </Button>
        {analyzing.map((j) => (
          <div key={j.id} className="px-1">
            <p className="truncate text-[11px] text-fg-3">
              {j.title} · {j.stage}
            </p>
            <Progress value={j.progress} />
          </div>
        ))}
        {ps.project.assets
          .filter((a) => a.kind !== "image")
          .map((a) => {
            const r = byAsset.get(a.id);
            return (
              <div key={a.id} className="rounded-md bg-ink-2 px-2.5 py-2 text-[11px]">
                <div className="flex items-center gap-1.5">
                  {r ? <CheckCircle2 size={12} className="text-ok" /> : <span className="size-3 rounded-full border border-fg-3" />}
                  <span className="truncate font-medium text-fg">{a.name}</span>
                </div>
                {r && (
                  <div className="mt-1 flex flex-col gap-0.5 text-fg-2">
                    {r.speech && (
                      <span className="flex items-center gap-1">
                        <AudioLines size={11} /> {r.speech.segments.length} trecho(s) de fala · {Math.round(r.speech.ratio * 100)}% · {METHOD[r.speech.method]}
                      </span>
                    )}
                    {r.scenes && (
                      <span className="flex items-center gap-1">
                        <Clapperboard size={11} /> {r.scenes.cuts.length} corte(s) de cena · {METHOD[r.scenes.method]}
                      </span>
                    )}
                    {r.visual && (
                      <span className="flex items-center gap-1">
                        <ScanFace size={11} /> {r.visual.samples.filter((s) => s.faces.length).length} amostra(s) com rosto de {r.visual.samples.length}
                      </span>
                    )}
                    {r.transcript && (
                      <span className="flex items-center gap-1">
                        <Captions size={11} /> transcrito ({r.transcript.language}, {r.transcript.segments.length} frases)
                      </span>
                    )}
                    {r.warnings.map((w) => (
                      <span key={w} className="text-warn">
                        {w}
                      </span>
                    ))}
                  </div>
                )}
              </div>
            );
          })}
      </section>

      <section className="flex flex-col gap-2.5">
        <h3 className="px-1 text-xs font-semibold text-fg-2">Remover pausas</h3>
        {rules && (
          <>
            <Slider
              label="Pausa mínima"
              min={0.2}
              max={3}
              step={0.1}
              value={secs(rules.min_silence)}
              format={(v) => `${v.toFixed(1)} s`}
              onChange={(v) => setRules({ ...rules, min_silence: fromSecs(v) })}
              onCommit={() => setPlan(null)}
            />
            <Slider
              label="Folga em volta da fala"
              min={0}
              max={0.6}
              step={0.05}
              value={secs(rules.padding)}
              format={(v) => `${v.toFixed(2)} s`}
              onChange={(v) => setRules({ ...rules, padding: fromSecs(v) })}
              onCommit={() => setPlan(null)}
            />
            <label className="flex items-center gap-2 px-1 text-xs text-fg-2">
              <input type="checkbox" checked={rules.preserve_action} onChange={(e) => (setRules({ ...rules, preserve_action: e.target.checked }), setPlan(null))} />
              Manter pausas com ação na imagem
            </label>
            <div className="flex gap-1">
              {(["all", "selected"] as const).map((s) => (
                <Button key={s} size="sm" className="flex-1" variant={scope === s ? "primary" : "subtle"} onClick={() => (setScope(s), setPlan(null))} disabled={s === "selected" && !selClip}>
                  {s === "all" ? "Todos os clipes" : "Clipe selecionado"}
                </Button>
              ))}
            </div>
            <Button
              size="sm"
              disabled={busy}
              onClick={() =>
                run(async () => {
                  const p = await api.planPauses(id, scope === "selected" && selClip ? [selClip.id] : null, rules);
                  setPlan(p);
                  if (p.cuts.length === 0) toast("info", "Nenhuma pausa encontrada com essas regras.");
                })
              }
            >
              <Scissors size={13} /> Calcular pausas
            </Button>
          </>
        )}
        {plan && plan.cuts.length > 0 && (
          <div className="rounded-lg border border-bad/30 bg-bad/10 p-2.5 text-xs">
            <p className="text-fg">
              {plan.cuts.length} corte(s), {secs(plan.removed).toFixed(1)} s a remover
            </p>
            {plan.kept_action > 0 && <p className="text-fg-2">{plan.kept_action} pausa(s) mantida(s) por ter ação visual</p>}
            {plan.skipped.map((s) => (
              <p key={s} className="text-warn">
                {s}
              </p>
            ))}
            <p className="mt-1 text-fg-3">Os trechos aparecem em vermelho na timeline.</p>
            <label className="mt-2 flex items-center gap-2 text-fg-2">
              <input type="checkbox" checked={syncText} onChange={(e) => setSyncText(e.target.checked)} />
              Legendas e textos acompanham os cortes
            </label>
            <div className="mt-2 flex gap-1.5">
              <Button
                size="sm"
                variant="primary"
                className="flex-1"
                disabled={busy}
                onClick={() =>
                  run(async () => {
                    const s = await api.applyPauses(id, plan, syncText);
                    setPlan(null);
                    useApp.getState().setState(s);
                    toast("success", `Pausas removidas (${plan.cuts.length} cortes). Ctrl+Z desfaz tudo.`);
                  })
                }
              >
                Aplicar
              </Button>
              <Button size="sm" variant="ghost" onClick={() => setPlan(null)}>
                Descartar
              </Button>
            </div>
          </div>
        )}
      </section>

      <section className="flex flex-col gap-2">
        <h3 className="px-1 text-xs font-semibold text-fg-2">Clipe selecionado</h3>
        {!selClip ? (
          <p className="px-1 text-xs text-fg-3">Selecione um clipe de mídia na timeline.</p>
        ) : !selResult ? (
          <p className="px-1 text-xs text-fg-3">A mídia deste clipe ainda não foi analisada.</p>
        ) : (
          <>
            <div className="flex gap-1.5">
              <Button size="sm" className="flex-1" disabled={busy || !selResult.scenes} onClick={() => run(async () => useApp.getState().setState(await api.applyScenes(id, selClip.id, false)))}>
                Marcar cenas
              </Button>
              <Button size="sm" className="flex-1" disabled={busy || !selResult.scenes} onClick={() => run(async () => useApp.getState().setState(await api.applyScenes(id, selClip.id, true)))}>
                Dividir nas cenas
              </Button>
            </div>
            <Button
              size="sm"
              disabled={busy || !selResult.transcript}
              title={selResult.transcript ? "" : "Analise com 'Transcrever fala' marcado"}
              onClick={() =>
                run(async () => {
                  useApp.getState().setState(await api.captionsFromTranscript(id, selClip.id, 42));
                  qc.invalidateQueries({ queryKey: ["analyses", id] });
                  toast("success", "Legendas criadas numa nova trilha (editáveis).");
                })
              }
            >
              <Captions size={13} /> Gerar legendas da transcrição
            </Button>
          </>
        )}
      </section>
    </div>
  );
}
