import { getCurrentWebview } from "@tauri-apps/api/webview";
import clsx from "clsx";
import { ArrowLeft, Download, Redo2, Save, Undo2 } from "lucide-react";
import { useEffect, useState } from "react";
import { Panel, PanelGroup, PanelResizeHandle } from "react-resizable-panels";
import { useQuery } from "@tanstack/react-query";
import { api, errorMessage } from "../../lib/api";
import { useApp } from "../../store/app";
import { TopBar } from "../dashboard/Dashboard";
import { Button, IconButton } from "../ui";
import { QualityReviewDialog } from "./AutoEdit";
import { ExportDialog } from "./ExportDialog";
import { Inspector } from "./Inspector";
import { LeftPanel } from "./LeftPanel";
import { Player } from "./Player";
import { Timeline } from "./Timeline";
import { splitAtPlayhead, deleteSelection, duplicateSelection, freezeAtPlayhead } from "./actions";

function Handle({ dir }: { dir: "h" | "v" }) {
  return (
    <PanelResizeHandle
      className={clsx("group relative bg-ink-0 transition-colors data-[resize-handle-state=drag]:bg-accent/40", dir === "h" ? "w-1" : "h-1")}
    />
  );
}

export function Editor() {
  const ps = useApp((s) => s.ps);
  const [exportOpen, setExportOpen] = useState(false);
  const presets = useQuery({ queryKey: ["presets"], queryFn: api.presets });

  useKeyboard(() => setExportOpen(true));
  useOsDragDrop();

  if (!ps) return null;
  const p = ps.project;
  const preset = presets.data?.find((x) => x.id === p.preset_id);

  return (
    <div className="flex h-full flex-col">
      <TopBar>
        <EditorTitle />
        <div className="ml-2 flex items-center gap-0.5">
          <IconButton label={`Desfazer${ps.history.undo_label ? `: ${ps.history.undo_label}` : ""} (Ctrl+Z)`} disabled={!ps.history.can_undo} onClick={() => useApp.getState().undo()}>
            <Undo2 size={15} />
          </IconButton>
          <IconButton label={`Refazer${ps.history.redo_label ? `: ${ps.history.redo_label}` : ""} (Ctrl+Shift+Z)`} disabled={!ps.history.can_redo} onClick={() => useApp.getState().redo()}>
            <Redo2 size={15} />
          </IconButton>
        </div>
        <span className={clsx("ml-2 text-xs", ps.dirty ? "text-warn" : "text-fg-3")}>{ps.dirty ? "Alterações não salvas" : "Salvo"}</span>
        <div className="flex-1" />
        {preset && (
          <span className="rounded-md border border-line px-2 py-1 text-xs text-fg-2" title={preset.description}>
            {preset.name}
          </span>
        )}
        <span className="text-xs text-fg-3 tabular">
          {p.settings.width}×{p.settings.height}
        </span>
        <Button size="sm" variant="ghost" onClick={() => useApp.getState().save()} title="Salvar (Ctrl+S)">
          <Save size={14} /> Salvar
        </Button>
        <Button size="sm" variant="primary" onClick={() => setExportOpen(true)} disabled={p.timeline.tracks.every((t) => t.clips.length === 0)}>
          <Download size={14} /> Exportar
        </Button>
      </TopBar>
      <PanelGroup direction="vertical" autoSaveId="avs-editor-v" className="min-h-0 flex-1">
        <Panel defaultSize={62} minSize={30}>
          <PanelGroup direction="horizontal" autoSaveId="avs-editor-h">
            <Panel defaultSize={22} minSize={14} collapsible>
              <LeftPanel />
            </Panel>
            <Handle dir="h" />
            <Panel defaultSize={56} minSize={30}>
              <Player safeArea={preset?.safety.safe_area ?? null} />
            </Panel>
            <Handle dir="h" />
            <Panel defaultSize={22} minSize={14} collapsible>
              <Inspector />
            </Panel>
          </PanelGroup>
        </Panel>
        <Handle dir="v" />
        <Panel defaultSize={38} minSize={18}>
          <Timeline />
        </Panel>
      </PanelGroup>
      <ExportDialog open={exportOpen} onClose={() => setExportOpen(false)} />
      <QualityReviewDialog />
    </div>
  );
}

function EditorTitle() {
  const ps = useApp((s) => s.ps)!;
  const [editing, setEditing] = useState(false);
  const [name, setName] = useState(ps.project.name);
  useEffect(() => setName(ps.project.name), [ps.project.name]);
  const commit = async () => {
    setEditing(false);
    if (name.trim() && name !== ps.project.name) {
      try {
        useApp.getState().setState(await api.renameProject(ps.project.id, name));
      } catch (e) {
        useApp.getState().toast("error", errorMessage(e));
      }
    }
  };
  return (
    <div className="flex items-center gap-1">
      <IconButton
        label="Voltar ao início"
        onClick={() => {
          useApp.getState().go({ name: "dashboard" });
        }}
      >
        <ArrowLeft size={15} />
      </IconButton>
      {editing ? (
        <input
          autoFocus
          value={name}
          onChange={(e) => setName(e.target.value)}
          onBlur={commit}
          onKeyDown={(e) => e.key === "Enter" && commit()}
          className="h-7 w-56 rounded-md border border-accent bg-ink-2 px-2 text-[13px] focus:outline-none"
        />
      ) : (
        <button onDoubleClick={() => setEditing(true)} className="max-w-64 truncate rounded-md px-1.5 py-1 text-[13px] font-medium hover:bg-ink-2" title="Clique duas vezes para renomear">
          {ps.project.name}
        </button>
      )}
    </div>
  );
}

/** Atalhos globais do editor (ignorados enquanto se digita em campos). */
function useKeyboard(openExport: () => void) {
  useEffect(() => {
    const onKey = (e: KeyboardEvent) => {
      const t = e.target as HTMLElement;
      if (t.closest("input, textarea, select, [contenteditable]")) return;
      const s = useApp.getState();
      const mod = e.ctrlKey || e.metaKey;
      const fps = s.ps?.project.settings.fps;
      if (mod && e.key.toLowerCase() === "z") {
        e.preventDefault();
        if (e.shiftKey) s.redo();
        else s.undo();
      } else if (mod && e.key.toLowerCase() === "y") {
        e.preventDefault();
        s.redo();
      } else if (mod && e.key.toLowerCase() === "s") {
        e.preventDefault();
        s.save();
      } else if (mod && e.key.toLowerCase() === "e") {
        e.preventDefault();
        openExport();
      } else if (!mod && !e.shiftKey && ["v", "b", "n", "y", "u"].includes(e.key.toLowerCase())) {
        const map = { v: "select", b: "ripple", n: "roll", y: "slip", u: "slide" } as const;
        s.setTool(map[e.key.toLowerCase() as keyof typeof map]);
      } else if (mod && e.key.toLowerCase() === "d") {
        e.preventDefault();
        duplicateSelection();
      } else if (!mod && e.key.toLowerCase() === "f") {
        e.preventDefault();
        freezeAtPlayhead();
      } else if (!mod && e.key.toLowerCase() === "s") {
        e.preventDefault();
        splitAtPlayhead();
      } else if (e.key === "Delete" || e.key === "Backspace") {
        e.preventDefault();
        deleteSelection(e.shiftKey);
      } else if (!mod && e.key.toLowerCase() === "m") {
        s.exec({ op: "add_marker", at: s.playhead, label: "Marcador" });
      } else if ((e.key === "ArrowLeft" || e.key === "ArrowRight") && fps) {
        e.preventDefault();
        const step = Math.floor((fps.den * 705_600_000) / fps.num) * (e.shiftKey ? Math.round(fps.num / fps.den) : 1);
        s.setPlayhead(s.playhead + (e.key === "ArrowLeft" ? -step : step));
      } else if (e.key === "Home") {
        s.setPlayhead(0);
      } else if (e.key === "Escape") {
        s.select(null);
      } else if (e.key === "+" || e.key === "=") {
        s.setZoom(s.pxPerSec * 1.25);
      } else if (e.key === "-") {
        s.setZoom(s.pxPerSec / 1.25);
      }
    };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [openExport]);
}

/** Arrastar arquivos do sistema para a janela importa para o projeto aberto. */
function useOsDragDrop() {
  useEffect(() => {
    const un = getCurrentWebview().onDragDropEvent(async (ev) => {
      if (ev.payload.type !== "drop") return;
      const s = useApp.getState();
      const id = s.ps?.project.id;
      if (!id || ev.payload.paths.length === 0) return;
      try {
        const r = await api.importMedia(id, ev.payload.paths);
        r.errors.forEach((e) => s.toast("error", e));
        if (r.assets.length) s.toast("info", `${r.assets.length} arquivo(s) importado(s); gerando miniaturas e waveform.`);
        await s.refresh();
      } catch (e) {
        s.toast("error", errorMessage(e));
      }
    });
    return () => {
      un.then((f) => f());
    };
  }, []);
}
