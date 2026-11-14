import { useMutation, useQuery, useQueryClient } from "@tanstack/react-query";
import { open as openDialog, ask } from "@tauri-apps/plugin-dialog";
import clsx from "clsx";
import { Clapperboard, Cpu, Sparkles, Film, FolderOpen, HardDrive, Layers, MemoryStick, MonitorPlay, Plus, Trash2, Wand2, Zap } from "lucide-react";
import { useMemo, useState } from "react";
import type { EditorialPreset } from "../../bindings/EditorialPreset";
import type { ProjectSummary } from "../../bindings/ProjectSummary";
import { api, errorMessage, fileUrl } from "../../lib/api";
import { bytes, fpsLabel, shortDuration } from "../../lib/time";
import { useApp } from "../../store/app";
import { activeJobs, JobRow, useNow } from "../JobsDrawer";
import { Button, Dialog, Empty, Field, inputCls, Progress } from "../ui";
import { PresetsDialog } from "./PresetsDialog";
import type { AutoEditOptions } from "../../bindings/AutoEditOptions";
import { AutoEditOptionsForm, optionsFromRules } from "../editor/AutoEdit";

export function Dashboard() {
  const [newOpen, setNewOpen] = useState(false);
  const [presetsOpen, setPresetsOpen] = useState(false);
  const [autoOpen, setAutoOpen] = useState(false);
  const projects = useQuery({ queryKey: ["projects"], queryFn: api.listProjects });
  const exports = useQuery({ queryKey: ["exports"], queryFn: () => api.listExports(null) });
  const presets = useQuery({ queryKey: ["presets"], queryFn: api.presets });
  const jobs = useApp((s) => s.jobs);
  const openProject = useApp((s) => s.openProject);
  const running = activeJobs(jobs);
  const now = useNow();
  const recent = projects.data?.[0];

  return (
    <div className="flex h-full flex-col">
      <TopBar />
      <div className="grid min-h-0 flex-1 grid-cols-[1fr_340px] gap-0">
        <main className="min-h-0 overflow-y-auto px-8 py-7">
          <section className="grid grid-cols-[minmax(0,1.15fr)_minmax(0,1fr)_minmax(0,1fr)] gap-4">
            <button
              onClick={() => setNewOpen(true)}
              className="group relative flex min-h-[168px] flex-col justify-between overflow-hidden rounded-[14px] border border-accent/30 bg-[linear-gradient(135deg,#2a2160_0%,#191a3a_55%,#14161c_100%)] p-6 text-left transition-colors hover:border-accent/60"
            >
              <div className="flex size-11 items-center justify-center rounded-xl bg-accent text-white shadow-lg shadow-accent/30">
                <Plus size={22} />
              </div>
              <div>
                <h1 className="text-[22px] font-semibold tracking-tight">Novo projeto</h1>
                <p className="mt-1 max-w-sm text-fg-2">Escolha um preset de formato e importe seus vídeos. Os arquivos originais nunca são alterados.</p>
              </div>
              <Clapperboard className="absolute -right-6 -bottom-8 text-accent/10 transition-transform group-hover:scale-105" size={180} strokeWidth={1} />
            </button>
            <button
              onClick={() => setAutoOpen(true)}
              className="flex min-h-[168px] flex-col justify-between rounded-[14px] border border-line bg-ink-1 p-5 text-left transition-colors hover:border-accent/50 hover:bg-ink-2"
            >
              <div className="flex size-11 items-center justify-center rounded-xl bg-accent-soft text-accent">
                <Wand2 size={21} />
              </div>
              <div>
                <h2 className="text-[17px] font-semibold tracking-tight">Edição automática</h2>
                <p className="mt-1 text-fg-2">Escolha os vídeos e um preset: a IA local analisa, corta e monta uma primeira versão para você revisar.</p>
              </div>
            </button>
            <div className="flex flex-col rounded-[14px] border border-line bg-ink-1 p-5">
              <p className="text-xs text-fg-3">Continuar editando</p>
              {recent ? (
                <button onClick={() => openProject(recent.id)} className="mt-3 flex flex-1 items-center gap-4 rounded-lg text-left hover:bg-ink-2">
                  <Thumb p={recent} className="h-[68px] w-[120px] shrink-0" />
                  <div className="min-w-0">
                    <p className="truncate text-[15px] font-semibold">{recent.name}</p>
                    <p className="mt-1 text-xs text-fg-2 tabular">
                      {shortDuration(recent.duration)} · {recent.width}×{recent.height}
                    </p>
                    <p className="mt-0.5 text-xs text-fg-3">{new Date(recent.updated_at).toLocaleString("pt-BR")}</p>
                  </div>
                </button>
              ) : (
                <p className="mt-3 text-fg-2">Seus projetos aparecem aqui depois de criados.</p>
              )}
            </div>
          </section>

          <section className="mt-9">
            <div className="mb-3 flex items-baseline justify-between">
              <h2 className="text-[15px] font-semibold">Projetos recentes</h2>
              <span className="text-xs text-fg-3">{projects.data?.length ?? 0} projetos</span>
            </div>
            {projects.isLoading ? null : projects.data && projects.data.length > 0 ? (
              <div className="grid grid-cols-[repeat(auto-fill,minmax(220px,1fr))] gap-4">
                {projects.data.map((p) => (
                  <ProjectCard key={p.id} p={p} presetName={presets.data?.find((x) => x.id === p.preset_id)?.name} />
                ))}
              </div>
            ) : (
              <div className="rounded-[14px] border border-dashed border-line">
                <Empty icon={<Film size={30} />} title="Nenhum projeto ainda">
                  Crie um projeto para começar a editar.
                </Empty>
              </div>
            )}
          </section>

          <section className="mt-9 mb-6">
            <h2 className="mb-3 text-[15px] font-semibold">Vídeos exportados</h2>
            {exports.data && exports.data.length > 0 ? (
              <div className="overflow-hidden rounded-[12px] border border-line">
                {exports.data.slice(0, 12).map((e) => {
                  const warns = (e.report as { warnings?: number })?.warnings ?? 0;
                  return (
                    <div key={e.id} className="flex items-center gap-4 border-b border-line bg-ink-1 px-4 py-2.5 last:border-0">
                      <MonitorPlay size={16} className="text-accent" />
                      <span className="min-w-0 flex-1 truncate select-text">{e.path.split(/[\\/]/).pop()}</span>
                      <span className="text-xs text-fg-2 tabular">{shortDuration(e.duration)}</span>
                      <span className="text-xs text-fg-2 tabular">
                        {e.width}×{e.height}
                      </span>
                      <span className="w-16 text-right text-xs text-fg-2 tabular">{bytes(e.size_bytes)}</span>
                      <span className={clsx("w-24 text-xs", warns ? "text-warn" : "text-ok")}>{warns ? `${warns} alerta(s)` : "validado"}</span>
                      <Button size="sm" variant="ghost" onClick={() => api.openExport(e.path, false).catch((er) => useApp.getState().toast("error", errorMessage(er)))}>
                        Abrir
                      </Button>
                      <Button size="sm" variant="ghost" onClick={() => api.openExport(e.path, true).catch((er) => useApp.getState().toast("error", errorMessage(er)))}>
                        <FolderOpen size={14} />
                      </Button>
                    </div>
                  );
                })}
              </div>
            ) : (
              <p className="text-fg-3">Nenhum vídeo exportado ainda.</p>
            )}
          </section>
        </main>

        <aside className="flex min-h-0 flex-col gap-5 overflow-y-auto border-l border-line bg-ink-1 px-5 py-6">
          <SystemPanel />
          <div>
            <h3 className="mb-2.5 text-[13px] font-semibold">Trabalhos em andamento</h3>
            {running.length === 0 ? <p className="text-xs text-fg-3">Nenhum trabalho em execução.</p> : <div className="flex flex-col gap-2">{running.map((j) => <JobRow key={j.id} j={j} now={now} />)}</div>}
          </div>
          <div>
            <h3 className="mb-2.5 text-[13px] font-semibold">IA local</h3>
            <button onClick={() => useApp.getState().setAiOpen(true)} className="flex w-full items-center gap-3 rounded-lg border border-line bg-ink-2 p-3 text-left hover:bg-ink-3">
              <Sparkles size={18} className="text-accent" />
              <div>
                <p className="font-medium">Gerenciador de IA</p>
                <p className="text-xs text-fg-3">Fala, cenas, rostos, transcrição e Ollama</p>
              </div>
            </button>
          </div>
          <div>
            <h3 className="mb-2.5 text-[13px] font-semibold">Presets</h3>
            <button onClick={() => setPresetsOpen(true)} className="flex w-full items-center gap-3 rounded-lg border border-line bg-ink-2 p-3 text-left hover:bg-ink-3">
              <Layers size={18} className="text-accent" />
              <div>
                <p className="font-medium">{presets.data?.length ?? 0} presets de edição</p>
                <p className="text-xs text-fg-3">Formatos, ritmo, legendas, áudio e exportação</p>
              </div>
            </button>
          </div>
        </aside>
      </div>
      <NewProjectDialog open={newOpen} onClose={() => setNewOpen(false)} presets={presets.data ?? []} />
      <AutoEditDialog open={autoOpen} onClose={() => setAutoOpen(false)} presets={presets.data ?? []} />
      <PresetsDialog open={presetsOpen} onClose={() => setPresetsOpen(false)} presets={presets.data ?? []} />
    </div>
  );
}

export function TopBar({ children }: { children?: React.ReactNode }) {
  const jobs = useApp((s) => s.jobs);
  const setOpen = useApp((s) => s.setJobsOpen);
  const open = useApp((s) => s.jobsOpen);
  const n = activeJobs(jobs).length;
  const go = useApp((s) => s.go);
  return (
    <header className="flex h-11 shrink-0 items-center gap-3 border-b border-line bg-ink-1 px-3">
      <button onClick={() => go({ name: "dashboard" })} className="flex items-center gap-2 rounded-md px-1.5 py-1 hover:bg-ink-2" title="Início">
        <span className="flex size-6 items-center justify-center rounded-md bg-accent-strong">
          <Zap size={13} className="text-white" fill="currentColor" />
        </span>
        <span className="text-[13px] font-semibold tracking-tight">AI Video Studio</span>
      </button>
      <div className="flex min-w-0 flex-1 items-center gap-2">{children}</div>
      <button
        onClick={() => setOpen(!open)}
        className={clsx("flex h-7 items-center gap-2 rounded-md border px-2.5 text-xs transition-colors", n ? "border-accent/40 bg-accent-soft text-fg" : "border-line text-fg-2 hover:bg-ink-2")}
      >
        {n > 0 && <span className="size-1.5 rounded-full bg-accent avs-pulse" />}
        Processamento{n > 0 ? ` (${n})` : ""}
      </button>
    </header>
  );
}

function Thumb({ p, className }: { p: ProjectSummary; className?: string }) {
  return (
    <div className={clsx("overflow-hidden rounded-lg bg-ink-2", className)}>
      {p.thumbnail ? (
        <img src={fileUrl(p.thumbnail)} alt="" className="size-full object-cover" draggable={false} />
      ) : (
        <div className="flex size-full items-center justify-center text-fg-3">
          <Film size={22} />
        </div>
      )}
    </div>
  );
}

function ProjectCard({ p, presetName }: { p: ProjectSummary; presetName?: string }) {
  const openProject = useApp((s) => s.openProject);
  const qc = useQueryClient();
  const del = useMutation({
    mutationFn: async () => {
      if (await ask(`Excluir o projeto "${p.name}"? Os vídeos originais não serão apagados.`, { title: "Excluir projeto", kind: "warning" })) {
        await api.deleteProject(p.id);
      }
    },
    onSuccess: () => qc.invalidateQueries({ queryKey: ["projects"] }),
    onError: (e) => useApp.getState().toast("error", errorMessage(e)),
  });
  const status: Record<string, string> = { idle: "Vazio", editing: "Em edição", saved: "Salvo" };
  return (
    <div className="group relative overflow-hidden rounded-[12px] border border-line bg-ink-1 transition-colors hover:border-fg-3/50">
      <button onClick={() => openProject(p.id)} className="block w-full text-left">
        <Thumb p={p} className="aspect-video w-full rounded-none" />
        <div className="p-3">
          <p className="truncate font-semibold">{p.name}</p>
          <p className="mt-1 text-xs text-fg-2 tabular">
            {shortDuration(p.duration)} · {p.width}×{p.height} · {fpsLabel(parseFps(p.fps))} fps
          </p>
          <div className="mt-2 flex items-center justify-between text-[11px] text-fg-3">
            <span className="truncate">{presetName ?? "Sem preset"}</span>
            <span>{status[p.status] ?? p.status}</span>
          </div>
          <p className="mt-1 text-[11px] text-fg-3">Editado em {new Date(p.updated_at).toLocaleDateString("pt-BR")}</p>
        </div>
      </button>
      <button
        aria-label={`Excluir ${p.name}`}
        onClick={() => del.mutate()}
        className="absolute top-2 right-2 rounded-md bg-black/60 p-1.5 text-fg-2 opacity-0 transition-opacity group-hover:opacity-100 hover:text-bad focus:opacity-100"
      >
        <Trash2 size={14} />
      </button>
    </div>
  );
}

function parseFps(s: string) {
  const [n, d] = s.split("/").map(Number);
  return { num: n || 30, den: d || 1 };
}

function SystemPanel() {
  const info = useQuery({ queryKey: ["system"], queryFn: api.systemInfo, refetchInterval: 3000 });
  const enc = useQuery({ queryKey: ["encoders"], queryFn: api.encoders, staleTime: Infinity });
  const i = info.data;
  const hw = enc.data?.filter((e) => e.hardware && e.working) ?? [];
  return (
    <div>
      <h3 className="mb-2.5 text-[13px] font-semibold">Sistema</h3>
      {!i ? (
        <p className="text-xs text-fg-3">Lendo hardware…</p>
      ) : (
        <div className="flex flex-col gap-3">
          <Meter icon={<Cpu size={14} />} label={`CPU · ${i.cpu_cores} núcleos`} value={i.cpu_usage_pct / 100} detail={`${i.cpu_usage_pct.toFixed(0)}%`} title={i.cpu_brand} />
          <Meter icon={<MemoryStick size={14} />} label="Memória" value={i.memory_used / Math.max(1, i.memory_total)} detail={`${bytes(i.memory_used)} / ${bytes(i.memory_total)}`} />
          {i.gpus.map((g) => (
            <Meter
              key={g.name}
              icon={<MonitorPlay size={14} />}
              label={g.name}
              value={(g.utilization_pct ?? 0) / 100}
              detail={g.memory_total_mb ? `${g.memory_used_mb ?? 0} / ${g.memory_total_mb} MB` : ""}
            />
          ))}
          <div className="flex items-center justify-between text-xs">
            <span className="flex items-center gap-1.5 text-fg-2">
              <HardDrive size={14} /> Espaço livre
            </span>
            <span className="tabular">{bytes(i.disk_free)}</span>
          </div>
          <div className="rounded-lg bg-ink-2 p-2.5 text-xs">
            <p className="text-fg-2">Aceleração de vídeo</p>
            <p className="mt-1 text-fg">
              {enc.isLoading ? "Testando encoders…" : hw.length ? hw.map((e) => e.name).join(", ") : "Nenhum encoder por hardware funcional; usando CPU (libx264/libx265)."}
            </p>
            <p className="mt-1.5 text-fg-3">FFmpeg {i.ffmpeg_version}</p>
          </div>
        </div>
      )}
    </div>
  );
}

function Meter({ icon, label, value, detail, title }: { icon: React.ReactNode; label: string; value: number; detail: string; title?: string }) {
  return (
    <div title={title}>
      <div className="mb-1.5 flex items-center justify-between text-xs">
        <span className="flex min-w-0 items-center gap-1.5 truncate text-fg-2">
          {icon}
          {label}
        </span>
        <span className="tabular">{detail}</span>
      </div>
      <Progress value={value} tone={value > 0.9 ? "bad" : "accent"} />
    </div>
  );
}

function NewProjectDialog({ open, onClose, presets }: { open: boolean; onClose: () => void; presets: EditorialPreset[] }) {
  const [name, setName] = useState("");
  const [presetId, setPresetId] = useState<string>("youtube_shorts");
  const [busy, setBusy] = useState(false);
  const openProject = useApp((s) => s.openProject);
  const toast = useApp((s) => s.toast);
  const qc = useQueryClient();
  const groups = useMemo(() => {
    const m = new Map<string, EditorialPreset[]>();
    presets.forEach((p) => m.set(p.category, [...(m.get(p.category) ?? []), p]));
    return [...m.entries()];
  }, [presets]);
  const sel = presets.find((p) => p.id === presetId);

  const create = async (withFiles: boolean) => {
    setBusy(true);
    try {
      const files = withFiles
        ? ((await openDialog({ multiple: true, title: "Importar vídeos, áudios ou imagens" })) as string[] | null)
        : null;
      if (withFiles && !files) return;
      const ps = await api.createProject(name || "Projeto sem título", presetId || null, null);
      if (files?.length) {
        const r = await api.importMedia(ps.project.id, files);
        r.errors.forEach((e) => toast("error", e));
      }
      await qc.invalidateQueries({ queryKey: ["projects"] });
      await openProject(ps.project.id);
      onClose();
      setName("");
    } catch (e) {
      toast("error", errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Novo projeto"
      width={620}
      footer={
        <>
          <Button variant="ghost" onClick={() => create(false)} disabled={busy}>
            Criar vazio
          </Button>
          <Button variant="primary" onClick={() => create(true)} disabled={busy}>
            Criar e importar vídeos
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <Field label="Nome">
          <input autoFocus className={inputCls} value={name} onChange={(e) => setName(e.target.value)} placeholder="Projeto sem título" onKeyDown={(e) => e.key === "Enter" && create(false)} />
        </Field>
        <Field label="Preset de formato">
          <select className={inputCls} value={presetId} onChange={(e) => setPresetId(e.target.value)}>
            {groups.map(([cat, list]) => (
              <optgroup key={cat} label={cat}>
                {list.map((p) => (
                  <option key={p.id} value={p.id}>
                    {p.name} — {p.aspect}
                  </option>
                ))}
              </optgroup>
            ))}
          </select>
        </Field>
        {sel && (
          <div className="rounded-lg border border-line bg-ink-2 p-3.5">
            <p className="text-fg-2">{sel.description}</p>
            <div className="mt-3 grid grid-cols-3 gap-3 text-xs">
              <Spec k="Quadro" v={`${sel.resolution.width}×${sel.resolution.height} (${sel.aspect})`} />
              <Spec k="FPS" v={sel.fps ? fpsLabel(sel.fps) : "da origem"} />
              <Spec k="Duração" v={sel.duration.max_s ? `até ${sel.duration.max_s} s` : "livre"} />
            </div>
          </div>
        )}
      </div>
    </Dialog>
  );
}

/** Fluxo completo: projeto com preset → importar → colocar na timeline → job de edição automática. */
function AutoEditDialog({ open, onClose, presets }: { open: boolean; onClose: () => void; presets: EditorialPreset[] }) {
  const [name, setName] = useState("");
  const [presetId, setPresetId] = useState<string>("youtube_shorts");
  const [opts, setOpts] = useState<AutoEditOptions | null>(null);
  const [busy, setBusy] = useState(false);
  const openProject = useApp((s) => s.openProject);
  const toast = useApp((s) => s.toast);
  const qc = useQueryClient();
  const sel = presets.find((p) => p.id === presetId);
  const groups = useMemo(() => {
    const m = new Map<string, EditorialPreset[]>();
    presets.forEach((p) => m.set(p.category, [...(m.get(p.category) ?? []), p]));
    return [...m.entries()];
  }, [presets]);
  // Opções seguem as regras do preset escolhido (o usuário ajusta depois).
  const rules = useQuery({ queryKey: ["preset-rules", presetId], queryFn: () => api.presetAutoRules(presetId), enabled: open && !!presetId });
  const current = opts ?? (rules.data ? optionsFromRules(rules.data) : null);

  const start = async () => {
    if (!sel || !current) return;
    setBusy(true);
    try {
      const files = (await openDialog({ multiple: true, title: "Escolha os vídeos (e, se quiser, uma música)" })) as string[] | null;
      if (!files?.length) return;
      const ps = await api.createProject(name || `Edição automática — ${sel.name}`, sel.id, null);
      const pid = ps.project.id;
      const r = await api.importMedia(pid, files);
      r.errors.forEach((e) => toast("error", e));
      if (r.assets.length === 0) {
        toast("error", "Nenhum arquivo de mídia pôde ser importado.");
        await openProject(pid);
        return;
      }
      await api.placeAssets(pid, r.assets.map((a) => a.id));
      await api.autoEditStart(pid, current);
      await qc.invalidateQueries({ queryKey: ["projects"] });
      await openProject(pid);
      toast("info", "Analisando e editando em segundo plano. O resultado aparece na timeline quando terminar.");
      onClose();
      setName("");
      setOpts(null);
    } catch (e) {
      toast("error", errorMessage(e));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="Edição automática"
      width={640}
      footer={
        <>
          <Button variant="ghost" onClick={onClose} disabled={busy}>
            Cancelar
          </Button>
          <Button variant="primary" onClick={start} disabled={busy || !sel}>
            <Wand2 size={14} /> Escolher vídeos e editar
          </Button>
        </>
      }
    >
      <div className="flex flex-col gap-4">
        <div className="grid grid-cols-2 gap-3">
          <Field label="Nome do projeto">
            <input className={inputCls} value={name} onChange={(e) => setName(e.target.value)} placeholder={sel ? `Edição automática — ${sel.name}` : ""} />
          </Field>
          <Field label="Preset">
            <select className={inputCls} value={presetId} onChange={(e) => (setPresetId(e.target.value), setOpts(null))}>
              {groups.map(([cat, list]) => (
                <optgroup key={cat} label={cat}>
                  {list.map((p) => (
                    <option key={p.id} value={p.id}>
                      {p.name} — {p.aspect}
                    </option>
                  ))}
                </optgroup>
              ))}
            </select>
          </Field>
        </div>
        {sel && <p className="-mt-1 text-xs text-fg-3">{sel.description}</p>}
        {current && <AutoEditOptionsForm value={current} onChange={setOpts} />}
        <p className="text-xs text-fg-3">
          Tudo roda no seu computador. Nada é inventado: a edição só corta, ordena e trata o material que você escolher. Uma versão &quot;Original&quot; é salva antes e o
          resultado pode ser desfeito com Ctrl+Z.
        </p>
      </div>
    </Dialog>
  );
}

function Spec({ k, v }: { k: string; v: string }) {
  return (
    <div>
      <p className="text-fg-3">{k}</p>
      <p className="mt-0.5 text-fg">{v}</p>
    </div>
  );
}
