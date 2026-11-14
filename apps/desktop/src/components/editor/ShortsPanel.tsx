import { useQuery, useQueryClient } from "@tanstack/react-query";
import { open as openDialog } from "@tauri-apps/plugin-dialog";
import clsx from "clsx";
import { FolderOpen, Loader2, Smartphone, SquareArrowOutUpRight } from "lucide-react";
import { useEffect, useMemo, useState } from "react";
import type { FramingMode } from "../../bindings/FramingMode";
import type { ShortCandidate } from "../../bindings/ShortCandidate";
import type { ShortCreated } from "../../bindings/ShortCreated";
import type { ShortsOptions } from "../../bindings/ShortsOptions";
import { api, errorMessage } from "../../lib/api";
import { shortDuration } from "../../lib/time";
import { useApp } from "../../store/app";
import { Button, inputCls, Progress } from "../ui";
import { useTranscribeAvailable } from "./AutoEdit";

const FRAMING: { id: FramingMode; label: string; hint: string }[] = [
  { id: "face", label: "Seguir rosto", hint: "Enquadra o rosto principal; troca de plano em cortes de cena ou quando a pessoa se desloca" },
  { id: "center", label: "Centro", hint: "Corte central fixo" },
  { id: "blur_background", label: "Fundo desfocado", hint: "Vídeo inteiro, com a mesma imagem desfocada preenchendo o fundo" },
];
const LAST_DIR = "avs.shorts.dir";

/** Aba "Cortes": sugere trechos autossuficientes e cria um projeto vertical para cada um. */
export function ShortsPanel() {
  const ps = useApp((s) => s.ps)!;
  const toast = useApp((s) => s.toast);
  const jobs = useApp((s) => s.jobs);
  const setPlayhead = useApp((s) => s.setPlayhead);
  const setShortRanges = useApp((s) => s.setShortRanges);
  const openProject = useApp((s) => s.openProject);
  const qc = useQueryClient();
  const id = ps.project.id;
  const tr = useTranscribeAvailable();

  const presets = useQuery({ queryKey: ["presets"], queryFn: api.presets });
  const profiles = useQuery({ queryKey: ["export-profiles"], queryFn: api.exportProfiles });
  const vertical = useMemo(() => (presets.data ?? []).filter((p) => p.resolution.height > p.resolution.width), [presets.data]);

  const [presetId, setPresetId] = useState<string | null>(null);
  const [opts, setOpts] = useState<ShortsOptions | null>(null);
  const [framing, setFraming] = useState<FramingMode>("face");
  const [profileId, setProfileId] = useState<string>("youtube_shorts");
  const [cands, setCands] = useState<ShortCandidate[] | null>(null);
  const [picked, setPicked] = useState<Set<number>>(new Set());
  const [captions, setCaptions] = useState(true);
  const [removePauses, setRemovePauses] = useState(true);
  const [exportNow, setExportNow] = useState(false);
  const [dir, setDir] = useState<string>(() => localStorage.getItem(LAST_DIR) ?? "");
  const [busy, setBusy] = useState(false);
  const [created, setCreated] = useState<ShortCreated[] | null>(null);
  const [jobId, setJobId] = useState<string | null>(null);

  const loadDefaults = (pid: string | null) =>
    api
      .shortsDefaults(id, pid)
      .then((d) => {
        setPresetId(d.preset_id);
        setOpts(d.options);
        setFraming(d.framing);
        if (d.export_profile) setProfileId(d.export_profile);
      })
      .catch((e) => toast("error", errorMessage(e)));
  useEffect(() => {
    loadDefaults(null);
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, [id]);
  useEffect(() => () => setShortRanges(null), [setShortRanges]);

  const job = jobId ? jobs[jobId] : undefined;
  useEffect(() => {
    if (job?.state === "completed") {
      const res = job.result as { shorts?: ShortCreated[]; errors?: string[] } | null;
      setCreated(res?.shorts ?? []);
      (res?.errors ?? []).forEach((e) => toast("error", e));
      setJobId(null);
      qc.invalidateQueries({ queryKey: ["projects"] });
      toast("success", `${res?.shorts?.length ?? 0} corte(s) criado(s) como projetos novos.${exportNow ? " Exportações na fila (veja Trabalhos)." : ""}`);
    } else if (job && (job.state === "failed" || job.state === "failed_recoverable" || job.state === "cancelled")) {
      if (job.error) toast("error", job.error);
      setJobId(null);
    }
  }, [job, qc, toast, exportNow]);

  const showRanges = (list: ShortCandidate[], sel: Set<number>) => setShortRanges(list.filter((c) => sel.has(c.rank)).map((c) => ({ range: c.range, rank: c.rank })));
  const toggle = (rank: number) => {
    const n = new Set(picked);
    if (n.has(rank)) n.delete(rank);
    else n.add(rank);
    setPicked(n);
    if (cands) showRanges(cands, n);
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
  const num = (k: keyof ShortsOptions, v: string) => opts && setOpts({ ...opts, [k]: Number(v) });

  if (!opts || !presetId) return null;
  const chosen = (cands ?? []).filter((c) => picked.has(c.rank));
  return (
    <div className="flex flex-col gap-3">
      <section className="flex flex-col gap-2.5">
        <div className="flex items-center gap-2 px-0.5">
          <Smartphone size={14} className="text-accent" />
          <h3 className="text-xs font-semibold">Cortes verticais</h3>
        </div>
        <p className="px-0.5 text-[11px] leading-snug text-fg-3">Sugere trechos que se sustentam sozinhos. Cada corte vira um projeto novo; este projeto não é alterado.</p>
        <label className="flex flex-col gap-1 text-[11px] text-fg-3">
          Formato
          <select
            className={inputCls}
            value={presetId}
            onChange={(e) => {
              setCands(null);
              setShortRanges(null);
              loadDefaults(e.target.value);
            }}
          >
            {vertical.map((p) => (
              <option key={p.id} value={p.id}>
                {p.name}
              </option>
            ))}
          </select>
        </label>
        <div className="grid grid-cols-4 gap-1.5">
          {(
            [
              ["count", "Qtd.", 1, 10],
              ["min_s", "Mín. (s)", 5, 170],
              ["target_s", "Alvo (s)", 5, 180],
              ["max_s", "Máx. (s)", 10, 180],
            ] as const
          ).map(([k, l, mn, mx]) => (
            <label key={k} className="flex flex-col gap-1 text-[11px] text-fg-3">
              {l}
              <input className={clsx(inputCls, "px-1.5")} type="number" min={mn} max={mx} value={Math.round(opts[k])} onChange={(e) => num(k, e.target.value)} />
            </label>
          ))}
        </div>
        <Button
          size="sm"
          variant="primary"
          disabled={busy}
          onClick={() =>
            run(async () => {
              const c = await api.suggestShorts(id, presetId, opts);
              setCands(c);
              const sel = new Set(c.slice(0, Math.min(3, c.length)).map((x) => x.rank));
              setPicked(sel);
              showRanges(c, sel);
              setCreated(null);
              if (c.length === 0) toast("info", "Nenhum trecho encontrado com essas durações.");
            })
          }
        >
          {busy && !cands ? <Loader2 size={13} className="animate-spin" /> : <Smartphone size={13} />} Sugerir cortes
        </Button>
      </section>

      {cands && cands.length > 0 && (
        <section className="flex flex-col gap-1.5">
          <p className="px-0.5 text-[11px] text-fg-3">Nota editorial (heurística da análise, não previsão de audiência). Marcados em verde na timeline.</p>
          {cands.map((c) => (
            <div key={c.rank} className={clsx("rounded-md border px-2 py-1.5", picked.has(c.rank) ? "border-ok/40 bg-ok/5" : "border-line bg-ink-2/40")}>
              <label className="flex cursor-pointer items-start gap-2">
                <input type="checkbox" className="mt-1" checked={picked.has(c.rank)} onChange={() => toggle(c.rank)} />
                <span className="min-w-0 flex-1">
                  <span className="flex items-baseline gap-1.5">
                    <span className="tabular text-xs font-semibold text-ok">{Math.round(c.score)}</span>
                    <span className="truncate text-xs font-medium text-fg">{c.title}</span>
                  </span>
                  <span className="block text-[11px] text-fg-3 tabular">
                    {shortDuration(c.range.start)}–{shortDuration(c.range.end)} · {shortDuration(c.range.end - c.range.start)}
                  </span>
                  <span className="block text-[11px] leading-snug text-fg-2">{c.explanation}</span>
                </span>
              </label>
              <button className="mt-0.5 ml-6 text-[11px] text-accent hover:underline" onClick={() => setPlayhead(c.range.start)}>
                ir para
              </button>
            </div>
          ))}
        </section>
      )}

      {cands && cands.length > 0 && (
        <section className="flex flex-col gap-2 rounded-lg border border-line p-2.5">
          <div>
            <p className="mb-1 text-[11px] text-fg-3">Enquadramento</p>
            <div className="flex gap-1" role="radiogroup" aria-label="Enquadramento">
              {FRAMING.map((f) => (
                <Button key={f.id} size="sm" role="radio" aria-checked={framing === f.id} title={f.hint} className="flex-1 px-1" variant={framing === f.id ? "primary" : "subtle"} onClick={() => setFraming(f.id)}>
                  {f.label}
                </Button>
              ))}
            </div>
          </div>
          <label className="flex items-center gap-2 text-xs text-fg-2">
            <input type="checkbox" checked={removePauses} onChange={(e) => setRemovePauses(e.target.checked)} /> Remover pausas
          </label>
          <label className={clsx("flex items-start gap-2 text-xs", tr.ok ? "text-fg-2" : "text-fg-3")}>
            <input type="checkbox" className="mt-0.5" disabled={!tr.ok} checked={captions && tr.ok} onChange={(e) => setCaptions(e.target.checked)} />
            <span>
              Legendas automáticas
              {!tr.ok && <span className="block text-[10px]">requer transcrição</span>}
            </span>
          </label>
          <label className="flex items-center gap-2 text-xs text-fg-2">
            <input type="checkbox" checked={exportNow} onChange={(e) => setExportNow(e.target.checked)} /> Exportar ao criar
          </label>
          {exportNow && (
            <div className="flex flex-col gap-1.5 pl-5">
              <select className={inputCls} value={profileId} onChange={(e) => setProfileId(e.target.value)}>
                {(profiles.data ?? [])
                  .filter((p) => !p.resolution || p.resolution.height > p.resolution.width)
                  .map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name}
                    </option>
                  ))}
              </select>
              <div className="flex gap-1">
                <input className={clsx(inputCls, "min-w-0 flex-1")} value={dir} readOnly placeholder="Pasta de destino" />
                <Button
                  size="sm"
                  onClick={async () => {
                    const d = await openDialog({ directory: true, title: "Pasta para os cortes" });
                    if (typeof d === "string") {
                      setDir(d);
                      localStorage.setItem(LAST_DIR, d);
                    }
                  }}
                >
                  <FolderOpen size={13} />
                </Button>
              </div>
            </div>
          )}
          {job ? (
            <div>
              <p className="truncate text-[11px] text-fg-2">{job.stage ?? "Na fila…"}</p>
              <Progress value={job.progress} />
            </div>
          ) : (
            <Button
              size="sm"
              variant="primary"
              disabled={busy || chosen.length === 0 || (exportNow && !dir)}
              onClick={() =>
                run(async () => {
                  const j = await api.createShorts(id, {
                    preset_id: presetId,
                    framing,
                    candidates: chosen,
                    captions: captions && tr.ok,
                    remove_pauses: removePauses,
                    export: exportNow ? { output_dir: dir, profile_id: profileId } : null,
                  });
                  setJobId(j);
                  setCreated(null);
                })
              }
            >
              Criar {chosen.length} corte{chosen.length === 1 ? "" : "s"}
            </Button>
          )}
        </section>
      )}

      {created && created.length > 0 && (
        <section className="flex flex-col gap-1.5">
          <h3 className="px-0.5 text-xs font-semibold text-fg-2">Cortes criados</h3>
          {created.map((c) => (
            <div key={c.project_id} className="rounded-md bg-ink-2/50 px-2 py-1.5 text-xs">
              <div className="flex items-center gap-2">
                <span className="min-w-0 flex-1 truncate font-medium" title={c.name}>
                  {c.name}
                </span>
                <span className="tabular text-fg-3">{shortDuration(c.duration)}</span>
                <Button size="sm" variant="ghost" title="Abrir o projeto do corte" onClick={() => openProject(c.project_id)}>
                  <SquareArrowOutUpRight size={12} /> Abrir
                </Button>
              </div>
              <ul className="mt-0.5 text-[11px] leading-snug text-fg-3">
                {c.steps.slice(0, 3).map((s) => (
                  <li key={s}>{s}</li>
                ))}
                {c.warnings.map((w) => (
                  <li key={w} className="text-warn">
                    {w}
                  </li>
                ))}
              </ul>
            </div>
          ))}
        </section>
      )}
    </div>
  );
}
