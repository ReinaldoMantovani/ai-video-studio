import { useQuery, useQueryClient } from "@tanstack/react-query";
import { open as openDialog, save as saveDialog } from "@tauri-apps/plugin-dialog";
import clsx from "clsx";
import { AudioLines, Bookmark, Captions, Download, MessageSquareText, Smartphone, Sparkles, FileWarning, Film, FolderInput, History, Image, Link2, Plus, Shuffle, Trash2, Type, Upload } from "lucide-react";
import { useState } from "react";
import type { MediaAsset } from "../../bindings/MediaAsset";
import type { TextStyle } from "../../bindings/TextStyle";
import type { TransitionKind } from "../../bindings/TransitionKind";
import { api, errorMessage, fileUrl } from "../../lib/api";
import { fpsLabel, fromSecs, shortDuration } from "../../lib/time";
import { useApp } from "../../store/app";
import { Button, Empty, IconButton, inputCls, Tabs } from "../ui";
import { appendAsset } from "./actions";
import { AiPanel } from "./AiPanel";
import { ShortsPanel } from "./ShortsPanel";
import { AssistantPanel } from "./AssistantPanel";

type Tab = "media" | "ai" | "assistant" | "shorts" | "text" | "captions" | "transitions" | "history";

export function LeftPanel() {
  const [tab, setTab] = useState<Tab>("media");
  const projectId = useApp((s) => s.ps?.project.id);
  return (
    <div className="flex h-full flex-col bg-ink-1">
      <div className="p-2">
        <Tabs
          value={tab}
          onChange={setTab}
          tabs={[
            { id: "media", label: "Mídia", icon: <Film size={13} /> },
            { id: "ai", label: "IA", icon: <Sparkles size={13} /> },
            { id: "assistant", label: "Assistente", icon: <MessageSquareText size={13} /> },
            { id: "shorts", label: "Cortes", icon: <Smartphone size={13} /> },
            { id: "text", label: "Texto", icon: <Type size={13} /> },
            { id: "captions", label: "Legendas", icon: <Captions size={13} /> },
            { id: "transitions", label: "Transições", icon: <Shuffle size={13} /> },
            { id: "history", label: "Histórico", icon: <History size={13} /> },
          ]}
        />
      </div>
      <div className="min-h-0 flex-1 overflow-y-auto px-2 pb-3">
        {tab === "media" && <MediaBin />}
        {tab === "ai" && <AiPanel />}
        {tab === "assistant" && <AssistantPanel key={projectId} />}
        {tab === "shorts" && <ShortsPanel key={projectId} />}
        {tab === "text" && <TextPanel />}
        {tab === "captions" && <CaptionsPanel />}
        {tab === "transitions" && <TransitionsPanel />}
        {tab === "history" && <HistoryPanel />}
      </div>
    </div>
  );
}

function MediaBin() {
  const ps = useApp((s) => s.ps)!;
  const toast = useApp((s) => s.toast);
  const jobs = useApp((s) => s.jobs);
  const id = ps.project.id;
  const doImport = async (dir: boolean) => {
    const sel = await openDialog({ multiple: !dir, directory: dir, title: dir ? "Importar pasta" : "Importar mídia" });
    if (!sel) return;
    const paths = Array.isArray(sel) ? sel : [sel];
    try {
      const r = await api.importMedia(id, paths);
      r.errors.forEach((e) => toast("error", e));
      r.skipped.forEach((e) => toast("info", `Ignorado: ${e}`));
      await useApp.getState().refresh();
    } catch (e) {
      toast("error", errorMessage(e));
    }
  };
  const analyzing = new Set(
    Object.values(jobs)
      .filter((j) => j.kind === "import" && (j.state === "running" || j.state === "queued"))
      .map((j) => (j.payload as { asset_id?: string })?.asset_id),
  );
  return (
    <div className="flex flex-col gap-2">
      <div className="flex gap-1.5">
        <Button size="sm" variant="primary" className="flex-1" onClick={() => doImport(false)}>
          <Upload size={13} /> Importar
        </Button>
        <Button size="sm" onClick={() => doImport(true)} title="Importar uma pasta inteira">
          <FolderInput size={13} />
        </Button>
      </div>
      {ps.project.assets.length === 0 ? (
        <Empty icon={<Upload size={26} />} title="Nenhuma mídia">
          Importe vídeos, áudios ou imagens, ou arraste arquivos para a janela. MP4, MOV, MKV, WebM, AVI e o que o FFmpeg instalado suportar.
        </Empty>
      ) : (
        ps.project.assets.map((a) => <AssetRow key={a.id} a={a} analyzing={analyzing.has(a.id)} />)
      )}
    </div>
  );
}

function AssetRow({ a, analyzing }: { a: MediaAsset; analyzing: boolean }) {
  const ps = useApp((s) => s.ps)!;
  const toast = useApp((s) => s.toast);
  const missing = a.status.state === "missing";
  const icon = a.kind === "audio" ? <AudioLines size={18} /> : a.kind === "image" ? <Image size={18} /> : <Film size={18} />;
  const relink = async () => {
    const f = await openDialog({ multiple: false, title: `Localizar ${a.name}` });
    if (!f || Array.isArray(f)) return;
    try {
      useApp.getState().setState(await api.relinkAsset(ps.project.id, a.id, f, false));
    } catch (e) {
      toast("error", errorMessage(e));
    }
  };
  return (
    <div
      draggable={!missing}
      onDragStart={(e) => {
        e.dataTransfer.setData("application/x-avs-asset", a.id);
        e.dataTransfer.effectAllowed = "copy";
      }}
      onDoubleClick={() => !missing && appendAsset(a)}
      className={clsx("group flex cursor-grab items-center gap-2.5 rounded-lg border border-transparent p-1.5 hover:border-line hover:bg-ink-2 active:cursor-grabbing", missing && "opacity-70")}
      title="Arraste para a timeline ou clique duas vezes para adicionar ao final"
    >
      <div className="relative flex h-[42px] w-[74px] shrink-0 items-center justify-center overflow-hidden rounded-md bg-ink-3 text-fg-3">
        {a.poster_path ? <img src={fileUrl(a.poster_path)} alt="" className="size-full object-cover" draggable={false} /> : icon}
        {a.kind !== "image" && <span className="absolute right-0.5 bottom-0.5 rounded bg-black/70 px-1 text-[10px] tabular">{shortDuration(a.duration)}</span>}
      </div>
      <div className="min-w-0 flex-1">
        <p className="truncate text-[12.5px]">{a.name}</p>
        <p className="truncate text-[11px] text-fg-3">
          {missing ? (
            <span className="text-bad">Arquivo não encontrado</span>
          ) : analyzing ? (
            <span className="avs-pulse text-accent">Analisando…</span>
          ) : a.video ? (
            `${a.video.width}×${a.video.height} · ${fpsLabel(a.video.fps)} fps${a.video.vfr ? " · VFR" : ""}${a.audio ? "" : " · sem áudio"}${a.proxy_path ? " · proxy" : ""}`
          ) : a.audio ? (
            `${a.audio.codec} · ${a.audio.sample_rate / 1000} kHz`
          ) : (
            a.kind
          )}
        </p>
      </div>
      {missing ? (
        <IconButton label="Localizar arquivo" onClick={relink}>
          <Link2 size={14} />
        </IconButton>
      ) : (
        <div className="flex opacity-0 transition-opacity group-hover:opacity-100">
          <IconButton label="Adicionar ao final da timeline" onClick={() => appendAsset(a)}>
            <Plus size={14} />
          </IconButton>
          <IconButton
            label="Remover do projeto"
            onClick={async () => {
              try {
                useApp.getState().setState(await api.removeAsset(ps.project.id, a.id));
              } catch (e) {
                toast("error", errorMessage(e));
              }
            }}
          >
            <Trash2 size={13} />
          </IconButton>
        </div>
      )}
      {a.status.state === "error" && <FileWarning size={14} className="text-bad" />}
    </div>
  );
}

const TEXT_PRESETS: { name: string; text: string; style: Partial<TextStyle> }[] = [
  { name: "Título", text: "Título", style: { size: 110, y: 0.42, outline_width: 0 } },
  { name: "Legenda", text: "Legenda", style: { size: 64, y: 0.82, outline_width: 4 } },
  { name: "Destaque amarelo", text: "Destaque", style: { size: 80, y: 0.75, color: "#FFE14D", outline_width: 6 } },
  { name: "Caixa", text: "Texto com fundo", style: { size: 56, y: 0.88, outline_width: 0, background: "#000000" } },
];

/** Data de uma versão salva no fuso local ("13/11/2026 18:38"); aceita RFC 3339 e o formato antigo do nome do arquivo. */
export function snapshotDate(v: string): string {
  const old = v.match(/^(\d{4})(\d{2})(\d{2})-(\d{2})(\d{2})/);
  const d = old ? new Date(Date.UTC(+old[1], +old[2] - 1, +old[3], +old[4], +old[5])) : new Date(v);
  if (Number.isNaN(d.getTime())) return v;
  return d.toLocaleString("pt-BR", { day: "2-digit", month: "2-digit", year: "numeric", hour: "2-digit", minute: "2-digit" }).replace(",", "");
}

const BASE_STYLE: TextStyle = { font: "Inter", size: 72, color: "#FFFFFF", outline_color: "#000000", outline_width: 3, background: null, x: 0.5, y: 0.85 };

function TextPanel() {
  const ps = useApp((s) => s.ps)!;
  const exec = useApp((s) => s.exec);
  const add = async (text: string, style: Partial<TextStyle>) => {
    const s = useApp.getState();
    let tr = ps.project.timeline.tracks.find((t) => t.kind === "text" && !t.locked && !t.clips.some((c) => c.start < s.playhead + fromSecs(3) && s.playhead < c.start + c.duration));
    if (!tr) {
      const created = await exec({ op: "add_track", kind: "text", name: null });
      if (!created) return;
      tr = useApp.getState().ps!.project.timeline.tracks.find((t) => t.id === created[0]);
    }
    if (!tr) return;
    const c = await exec({ op: "add_text", track_id: tr.id, start: s.playhead, duration: fromSecs(3), text, style: { ...BASE_STYLE, ...style } });
    if (c?.[0]) useApp.getState().select(c[0]);
  };
  return (
    <div className="flex flex-col gap-2">
      <p className="px-1 text-xs text-fg-3">Adiciona no cursor (3 s). Edite texto, fonte, cor e posição no painel de propriedades.</p>
      {TEXT_PRESETS.map((t) => (
        <button
          key={t.name}
          onClick={() => add(t.text, t.style)}
          className="flex h-16 items-center justify-center rounded-lg border border-line bg-[#0b0c10] font-bold hover:border-accent/50"
          style={{
            color: t.style.color ?? "#fff",
            fontSize: Math.min(24, (t.style.size ?? 72) / 4),
            WebkitTextStroke: t.style.outline_width ? `1px ${t.style.outline_color ?? "#000"}` : undefined,
          }}
        >
          <span className={clsx(t.style.background && "bg-black/80 px-2 py-0.5")}>{t.name}</span>
        </button>
      ))}
    </div>
  );
}

function CaptionsPanel() {
  const ps = useApp((s) => s.ps)!;
  const toast = useApp((s) => s.toast);
  const id = ps.project.id;
  const textTracks = ps.project.timeline.tracks.filter((t) => t.kind === "text");
  const doImport = async () => {
    const f = await openDialog({ multiple: false, title: "Importar legendas", filters: [{ name: "Legendas", extensions: ["srt", "vtt"] }] });
    if (typeof f !== "string") return;
    try {
      const r = await api.importSubtitles(id, f);
      useApp.getState().setState(r.state);
      toast("success", `${r.cues} legenda(s) importada(s)${r.skipped ? `; ${r.skipped} bloco(s) inválido(s) ignorado(s)` : ""}.`);
    } catch (e) {
      toast("error", errorMessage(e));
    }
  };
  const doExport = async (trackId: string, name: string, ext: "srt" | "vtt") => {
    const path = await saveDialog({ title: `Exportar ${ext.toUpperCase()}`, defaultPath: `${name}.${ext}`, filters: [{ name: ext.toUpperCase(), extensions: [ext] }] });
    if (!path) return;
    try {
      const n = await api.exportSubtitles(id, trackId, path);
      toast("success", `${n} legenda(s) exportada(s) em ${ext.toUpperCase()}.`);
    } catch (e) {
      toast("error", errorMessage(e));
    }
  };
  return (
    <div className="flex flex-col gap-3">
      <Button size="sm" variant="primary" onClick={doImport}>
        <Upload size={13} /> Importar SRT ou VTT
      </Button>
      <p className="px-1 text-xs text-fg-3">
        Cada arquivo vira uma trilha de texto editável. Para mudar a aparência de todas, ajuste uma legenda e use "Aplicar estilo a todas" no painel de propriedades.
        A transcrição automática chega com a IA local.
      </p>
      {textTracks.length === 0 ? (
        <p className="px-1 text-xs text-fg-3">Nenhuma trilha de texto no projeto.</p>
      ) : (
        textTracks.map((t) => (
          <div key={t.id} className="rounded-lg border border-line bg-ink-2 p-2.5">
            <div className="flex items-center justify-between">
              <span className="truncate text-xs font-medium">{t.name}</span>
              <span className="text-[11px] text-fg-3">{t.clips.length} texto(s)</span>
            </div>
            <div className="mt-2 flex gap-1.5">
              {(["srt", "vtt"] as const).map((ext) => (
                <Button key={ext} size="sm" className="flex-1" disabled={t.clips.length === 0} onClick={() => doExport(t.id, ps.project.name, ext)}>
                  <Download size={12} /> {ext.toUpperCase()}
                </Button>
              ))}
            </div>
          </div>
        ))
      )}
    </div>
  );
}

const TRANSITIONS: { kind: TransitionKind; name: string }[] = [
  { kind: "crossfade", name: "Dissolver cruzado" },
  { kind: "dissolve", name: "Dissolver granulado" },
  { kind: "fade_black", name: "Passar pelo preto" },
  { kind: "wipe_left", name: "Cortina para a esquerda" },
  { kind: "slide_left", name: "Deslizar para a esquerda" },
];

function TransitionsPanel() {
  const ps = useApp((s) => s.ps)!;
  const sel = useApp((s) => s.selectedClip);
  const exec = useApp((s) => s.exec);
  const toast = useApp((s) => s.toast);
  const [dur, setDur] = useState(0.5);
  const apply = async (kind: TransitionKind) => {
    const tr = ps.project.timeline.tracks.find((t) => t.clips.some((c) => c.id === sel));
    if (!tr || !sel) {
      toast("info", "Selecione na timeline o clipe que vem antes do corte.");
      return;
    }
    const i = tr.clips.findIndex((c) => c.id === sel);
    const next = tr.clips[i + 1];
    if (!next) {
      toast("info", "O clipe selecionado não tem um clipe seguinte na mesma trilha.");
      return;
    }
    await exec({ op: "add_transition", from_clip: sel, to_clip: next.id, kind, duration: fromSecs(dur) });
  };
  return (
    <div className="flex flex-col gap-2">
      <p className="px-1 text-xs text-fg-3">Selecione o clipe antes do corte e escolha a transição. Os clipes precisam estar encostados; o segundo recua para criar a sobreposição.</p>
      <label className="flex items-center justify-between gap-2 px-1 text-xs text-fg-2">
        Duração (s)
        <input type="number" min={0.1} max={5} step={0.1} value={dur} onChange={(e) => setDur(Number(e.target.value))} className={clsx(inputCls, "w-20")} />
      </label>
      {TRANSITIONS.map((t) => (
        <Button key={t.kind} className="justify-start" onClick={() => apply(t.kind)} disabled={!sel}>
          <Shuffle size={13} className="text-accent" /> {t.name}
        </Button>
      ))}
    </div>
  );
}

function HistoryPanel() {
  const ps = useApp((s) => s.ps)!;
  const toast = useApp((s) => s.toast);
  const qc = useQueryClient();
  const snaps = useQuery({ queryKey: ["snapshots", ps.project.id], queryFn: () => api.listSnapshots(ps.project.id) });
  const [label, setLabel] = useState("");
  const entries = ps.history.entries;
  return (
    <div className="flex flex-col gap-4">
      <div>
        <p className="mb-1.5 px-1 text-xs text-fg-3">Alterações desta sessão</p>
        {entries.length === 0 ? (
          <p className="px-1 text-xs text-fg-3">Nenhuma alteração ainda.</p>
        ) : (
          <ol className="flex flex-col gap-0.5">
            {[...entries].reverse().map((e, i) => (
              <li key={i} className={clsx("rounded-md px-2 py-1 text-xs", i === 0 ? "bg-ink-2 text-fg" : "text-fg-2")}>
                {e}
              </li>
            ))}
          </ol>
        )}
      </div>
      <div>
        <p className="mb-1.5 px-1 text-xs text-fg-3">Versões salvas</p>
        <div className="flex gap-1.5">
          <input className={clsx(inputCls, "min-w-0 flex-1")} placeholder="Ex.: Versão para Shorts" value={label} onChange={(e) => setLabel(e.target.value)} />
          <Button
            size="sm"
            onClick={async () => {
              try {
                await api.createSnapshot(ps.project.id, label || "Versão");
                setLabel("");
                qc.invalidateQueries({ queryKey: ["snapshots", ps.project.id] });
              } catch (e) {
                toast("error", errorMessage(e));
              }
            }}
          >
            <Bookmark size={13} /> Salvar
          </Button>
        </div>
        <div className="mt-2 flex flex-col gap-1">
          {snaps.data?.map((s) => (
            <div key={s.file_name} className="flex items-center justify-between rounded-md px-2 py-1.5 hover:bg-ink-2">
              <div className="min-w-0">
                <p className="truncate text-xs">{s.label || "Versão"}</p>
                <p className="text-[11px] text-fg-3 tabular">{snapshotDate(s.created_at)}</p>
              </div>
              <Button
                size="sm"
                variant="ghost"
                onClick={async () => {
                  try {
                    useApp.getState().setState(await api.restoreSnapshot(ps.project.id, s.file_name));
                    toast("success", "Versão restaurada (pode ser desfeita)");
                  } catch (e) {
                    toast("error", errorMessage(e));
                  }
                }}
              >
                Restaurar
              </Button>
            </div>
          ))}
        </div>
      </div>
    </div>
  );
}
