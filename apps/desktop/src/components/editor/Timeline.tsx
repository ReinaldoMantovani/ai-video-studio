import { useQuery } from "@tanstack/react-query";
import clsx from "clsx";
import {
  AudioLines,
  Columns2,
  Diamond,
  Eye,
  GalleryHorizontal,
  MousePointer2,
  MoveHorizontal,
  Rewind,
  Snowflake,
  StretchHorizontal,
  EyeOff,
  Film,
  Lock,
  LockOpen,
  Magnet,
  Maximize2,
  Scissors,
  Trash2,
  Type,
  Volume2,
  VolumeX,
  ZoomIn,
  ZoomOut,
} from "lucide-react";
import { memo, useCallback, useEffect, useLayoutEffect, useMemo, useRef, useState } from "react";
import type { Clip } from "../../bindings/Clip";
import type { MediaAsset } from "../../bindings/MediaAsset";
import type { Project } from "../../bindings/Project";
import type { Track } from "../../bindings/Track";
import { api, fileUrl } from "../../lib/api";
import { frameDuration, fromSecs, secs, shortDuration, snap, snapToFrame } from "../../lib/time";
import { type Tool, useApp } from "../../store/app";
import { IconButton } from "../ui";
import { deleteSelection, splitAtPlayhead } from "./actions";

const HEADER_W = 176;
const RULER_H = 28;
const LANE_H: Record<string, number> = { video: 66, text: 34, audio: 50 };
import type { TrackPatch } from "../../bindings/TrackPatch";
const NULL_TRACK_PATCH: TrackPatch = { name: null, muted: null, hidden: null, locked: null, volume: null };

type Drag = {
  mode: "move" | "trim-l" | "trim-r" | "slip" | "slide" | "roll";
  clip: Clip;
  /** roll: o clipe vizinho do outro lado do corte */
  neighbor?: Clip;
  /** roll: posição original do corte */
  edge?: number;
  ripple?: boolean;
  trackId: string;
  x0: number;
  moved: boolean;
  start: number;
  duration: number;
  sourceIn: number;
  targetTrack: string;
  guide: number | null;
};

export function Timeline() {
  const ps = useApp((s) => s.ps)!;
  const pps = useApp((s) => s.pxPerSec);
  const setZoom = useApp((s) => s.setZoom);
  const playhead = useApp((s) => s.playhead);
  const setPlayhead = useApp((s) => s.setPlayhead);
  const exec = useApp((s) => s.exec);
  const pausePlan = useApp((s) => s.pausePlan);
  const autoPlan = useApp((s) => s.autoPlan);
  const shortRanges = useApp((s) => s.shortRanges);
  const p = ps.project;
  const fps = p.settings.fps;
  const scroller = useRef<HTMLDivElement>(null);
  const [view, setView] = useState({ left: 0, width: 1200 });
  const [magnet, setMagnet] = useState(true);
  const [drag, setDrag] = useState<Drag | null>(null);
  const dragRef = useRef<Drag | null>(null);
  dragRef.current = drag;

  const duration = useMemo(() => p.timeline.tracks.reduce((m, t) => t.clips.reduce((mm, c) => Math.max(mm, c.start + c.duration), m), 0), [p.timeline]);
  const contentW = Math.max(view.width - HEADER_W, (secs(duration) + 30) * pps);
  // Janela visível (px da faixa) em blocos de 1024 px com margem: só o que está
  // perto da tela é desenhado. Quantizada para não re-renderizar a cada pixel de rolagem.
  const VIS_STEP = 1024;
  const visFrom = Math.max(0, Math.floor((view.left - 600) / VIS_STEP) * VIS_STEP);
  const visTo = Math.ceil((view.left + view.width + 600) / VIS_STEP) * VIS_STEP;
  const xOf = useCallback((t: number) => secs(t) * pps, [pps]);
  const tOf = useCallback((x: number) => fromSecs(Math.max(0, x) / pps), [pps]);

  useLayoutEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const upd = () => setView({ left: el.scrollLeft, width: el.clientWidth });
    upd();
    const ro = new ResizeObserver(upd);
    ro.observe(el);
    el.addEventListener("scroll", upd, { passive: true });
    return () => {
      ro.disconnect();
      el.removeEventListener("scroll", upd);
    };
  }, []);

  // Mantém o playhead visível durante a reprodução.
  useEffect(() => {
    const el = scroller.current;
    if (!el || dragRef.current) return;
    const x = xOf(playhead);
    if (x < el.scrollLeft || x > el.scrollLeft + el.clientWidth - HEADER_W - 40) el.scrollLeft = Math.max(0, x - 80);
  }, [playhead, xOf]);

  // Ctrl + roda = zoom em torno do cursor.
  useEffect(() => {
    const el = scroller.current;
    if (!el) return;
    const onWheel = (e: WheelEvent) => {
      if (!(e.ctrlKey || e.metaKey)) return;
      e.preventDefault();
      const rect = el.getBoundingClientRect();
      const x = e.clientX - rect.left - HEADER_W + el.scrollLeft;
      const t = x / useApp.getState().pxPerSec;
      const next = Math.min(600, Math.max(2, useApp.getState().pxPerSec * (e.deltaY < 0 ? 1.15 : 1 / 1.15)));
      setZoom(next);
      requestAnimationFrame(() => (el.scrollLeft = Math.max(0, t * next - (e.clientX - rect.left - HEADER_W))));
    };
    el.addEventListener("wheel", onWheel, { passive: false });
    return () => el.removeEventListener("wheel", onWheel);
  }, [setZoom]);

  const fit = () => {
    const w = (scroller.current?.clientWidth ?? 1200) - HEADER_W - 40;
    if (duration > 0) setZoom(w / secs(duration));
  };

  // Alvos de snap: bordas de clipes, playhead, marcadores, início.
  const snapTargets = useCallback(
    (exclude: string) => {
      const out = [0, useApp.getState().playhead, ...p.timeline.markers.map((m) => m.at)];
      p.timeline.tracks.forEach((t) => t.clips.forEach((c) => c.id !== exclude && out.push(c.start, c.start + c.duration)));
      return out;
    },
    [p.timeline],
  );

  const assetOf = useCallback((c: Clip) => (c.content.type === "media" ? p.assets.find((a) => a.id === (c.content as { asset_id: string }).asset_id) : undefined), [p.assets]);

  const beginDrag = (e: React.PointerEvent, clip: Clip, track: Track, part: "body" | "left" | "right") => {
    if (track.locked || e.button !== 0) return;
    e.stopPropagation();
    (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
    useApp.getState().select(clip.id);
    const tool = useApp.getState().tool;
    let mode: Drag["mode"] = part === "body" ? "move" : part === "left" ? "trim-l" : "trim-r";
    let neighbor: Clip | undefined;
    let edge: number | undefined;
    if (part === "body" && (tool === "slip" || tool === "slide")) mode = tool;
    if (part !== "body" && tool === "roll") {
      const i = track.clips.findIndex((c) => c.id === clip.id);
      const other = part === "right" ? track.clips[i + 1] : track.clips[i - 1];
      const touching = other && (part === "right" ? other.start === clip.start + clip.duration : other.start + other.duration === clip.start);
      if (touching) {
        mode = "roll";
        neighbor = other;
        edge = part === "right" ? clip.start + clip.duration : clip.start;
      }
    }
    setDrag({
      mode,
      clip,
      neighbor,
      edge,
      ripple: tool === "ripple",
      trackId: track.id,
      x0: e.clientX,
      moved: false,
      start: clip.start,
      duration: clip.duration,
      sourceIn: clip.content.type === "media" ? clip.content.source_in : 0,
      targetTrack: track.id,
      guide: null,
    });
  };

  const onDragMove = (e: React.PointerEvent) => {
    const d = dragRef.current;
    if (!d) return;
    const dx = e.clientX - d.x0;
    if (!d.moved && Math.abs(dx) < 3) return;
    const dt = fromSecs(dx / pps);
    const tol = fromSecs(8 / pps);
    const targets = magnet ? snapTargets(d.clip.id) : [];
    const c = d.clip;
    const srcIn = c.content.type === "media" ? c.content.source_in : 0;
    const a = assetOf(c);
    const fd = frameDuration(fps);
    let next: Drag = { ...d, moved: true, guide: null };
    if (d.mode === "move") {
      let s = Math.max(0, c.start + dt);
      const a1 = snap(s, targets, tol);
      const a2 = snap(s + c.duration, targets, tol);
      if (a1.target !== null && (a2.target === null || Math.abs(a1.t - s) <= Math.abs(a2.t - (s + c.duration)))) {
        s = a1.t;
        next.guide = a1.target;
      } else if (a2.target !== null) {
        s = a2.t - c.duration;
        next.guide = a2.target;
      }
      next.start = snapToFrame(Math.max(0, s), fps);
      // Troca de trilha (mesmo tipo).
      const el = document.elementFromPoint(e.clientX, e.clientY)?.closest("[data-track-id]") as HTMLElement | null;
      const tid = el?.dataset.trackId;
      const src = p.timeline.tracks.find((t) => t.id === d.trackId)!;
      const dst = p.timeline.tracks.find((t) => t.id === tid);
      if (dst && !dst.locked && (dst.kind === src.kind || (src.kind === "video" && dst.kind === "audio" && a?.audio))) next.targetTrack = dst.id;
    } else if (d.mode === "slip") {
      // O conteúdo acompanha o mouse; posição e duração não mudam.
      const span = c.freeze ? 0 : Math.round(c.duration * c.speed);
      const maxIn = a && a.kind !== "image" ? Math.max(0, a.duration - span) : 0;
      next.sourceIn = snapToFrame(Math.min(Math.max(0, srcIn - Math.round(dt * c.speed)), maxIn), fps);
    } else if (d.mode === "slide") {
      next.start = snapToFrame(Math.max(0, c.start + dt), fps);
    } else if (d.mode === "roll") {
      const sn = snap(d.edge! + dt, targets, tol);
      next.guide = snapToFrame(sn.t, fps);
    } else if (d.mode === "trim-l") {
      const end = c.start + c.duration;
      let s = c.start + dt;
      const sn = snap(s, targets, tol);
      s = sn.t;
      next.guide = sn.target;
      const minStart = a && a.kind !== "image" ? c.start - srcIn : 0;
      s = Math.min(Math.max(s, minStart, 0), end - fd);
      s = snapToFrame(s, fps);
      next = { ...next, start: s, duration: end - s, sourceIn: srcIn + (s - c.start) };
    } else {
      let e2 = c.start + c.duration + dt;
      const sn = snap(e2, targets, tol);
      e2 = sn.t;
      next.guide = sn.target;
      const maxEnd = a && a.kind !== "image" ? c.start + (a.duration - srcIn) : Infinity;
      e2 = snapToFrame(Math.max(Math.min(e2, maxEnd), c.start + fd), fps);
      next = { ...next, duration: e2 - c.start };
    }
    setDrag(next);
  };

  const onDragEnd = async () => {
    const d = dragRef.current;
    setDrag(null);
    if (!d || !d.moved) return;
    const c = d.clip;
    if (d.mode === "slip") {
      if (d.sourceIn !== (c.content.type === "media" ? c.content.source_in : 0))
        await exec({ op: "slip_clip", clip_id: c.id, delta: d.sourceIn - (c.content.type === "media" ? c.content.source_in : 0) });
      return;
    }
    if (d.mode === "slide") {
      if (d.start !== c.start) await exec({ op: "slide_clip", clip_id: c.id, delta: d.start - c.start });
      return;
    }
    if (d.mode === "roll") {
      if (d.guide != null && d.guide !== d.edge && d.neighbor) {
        const leftFirst = d.neighbor.start > c.start;
        await exec({ op: "roll_edit", left_clip: leftFirst ? c.id : d.neighbor.id, right_clip: leftFirst ? d.neighbor.id : c.id, delta: d.guide - d.edge! });
      }
      return;
    }
    if (d.mode === "move") {
      if (d.start === c.start && d.targetTrack === d.trackId) return;
      await exec({ op: "move_clip", clip_id: c.id, track_id: d.targetTrack === d.trackId ? null : d.targetTrack, start: d.start });
    } else if (d.mode === "trim-l") {
      if (d.start !== c.start) await exec({ op: "trim_clip", clip_id: c.id, new_start: d.start, new_end: null, ripple: !!d.ripple });
    } else if (d.start + d.duration !== c.start + c.duration) {
      await exec({ op: "trim_clip", clip_id: c.id, new_start: null, new_end: c.start + d.duration, ripple: !!d.ripple });
    }
  };

  // Scrub na régua.
  const scrubbing = useRef(false);
  const scrubTo = (clientX: number) => {
    const el = scroller.current!;
    const rect = el.getBoundingClientRect();
    const x = clientX - rect.left - HEADER_W + el.scrollLeft;
    setPlayhead(snapToFrame(Math.min(tOf(x), Math.max(0, duration - frameDuration(fps))), fps));
  };

  const onDrop = async (e: React.DragEvent, track: Track) => {
    const id = e.dataTransfer.getData("application/x-avs-asset");
    if (!id) return;
    e.preventDefault();
    const a = p.assets.find((x) => x.id === id);
    if (!a) return;
    const rect = (e.currentTarget as HTMLElement).getBoundingClientRect();
    let t = tOf(e.clientX - rect.left);
    if (magnet) t = snap(t, snapTargets(""), fromSecs(10 / pps)).t;
    let target = track;
    if (track.kind === "text" || (track.kind === "video" && a.kind === "audio")) {
      const alt = p.timeline.tracks.find((x) => x.kind === (a.kind === "audio" ? "audio" : "video") && !x.locked);
      if (!alt) return;
      target = alt;
    }
    const created = await exec({ op: "insert_clip", track_id: target.id, asset_id: a.id, start: t, source_in: null, duration: null, ripple: false });
    if (created?.[0]) useApp.getState().select(created[0]);
  };

  const phX = xOf(playhead);

  return (
    <div className="flex h-full flex-col bg-ink-1">
      <Toolbar magnet={magnet} setMagnet={setMagnet} fit={fit} duration={duration} />
      <div ref={scroller} className="relative min-h-0 flex-1 overflow-auto" onPointerMove={onDragMove} onPointerUp={onDragEnd}>
        <div className="relative" style={{ width: contentW + HEADER_W }}>
          {/* Régua */}
          <div className="sticky top-0 z-20 flex" style={{ height: RULER_H }}>
            <div className="sticky left-0 z-30 shrink-0 border-r border-b border-line bg-ink-1" style={{ width: HEADER_W }} />
            <div
              className="relative flex-1 cursor-col-resize border-b border-line bg-ink-1"
              onPointerDown={(e) => {
                scrubbing.current = true;
                (e.currentTarget as HTMLElement).setPointerCapture(e.pointerId);
                scrubTo(e.clientX);
              }}
              onPointerMove={(e) => scrubbing.current && scrubTo(e.clientX)}
              onPointerUp={() => (scrubbing.current = false)}
            >
              <Ruler pps={pps} left={view.left} width={view.width} />
              {p.timeline.markers.map((m) => (
                <button
                  key={m.id}
                  title={`${m.label} — clique duas vezes para remover`}
                  onPointerDown={(e) => e.stopPropagation()}
                  onClick={() => setPlayhead(m.at)}
                  onDoubleClick={() => exec({ op: "remove_marker", marker_id: m.id })}
                  className="absolute bottom-0 -translate-x-1/2"
                  style={{ left: xOf(m.at) }}
                >
                  <span className="block size-0 border-x-[5px] border-t-[7px] border-x-transparent border-t-accent" />
                </button>
              ))}
            </div>
          </div>

          {/* Trilhas */}
          {p.timeline.tracks.map((t) => (
            <div key={t.id} className="flex border-b border-line/70" style={{ height: LANE_H[t.kind] }}>
              <TrackHeader track={t} />
              <div
                data-track-id={t.id}
                className={clsx("relative flex-1", drag?.targetTrack === t.id && drag.mode === "move" ? "bg-accent/[0.06]" : "bg-ink-0/40", t.locked && "bg-[repeating-linear-gradient(135deg,transparent_0_6px,#ffffff06_6px_12px)]")}
                onPointerDown={(e) => {
                  if (e.button === 0) {
                    useApp.getState().select(null);
                    scrubTo(e.clientX);
                  }
                }}
                onDragOver={(e) => e.dataTransfer.types.includes("application/x-avs-asset") && e.preventDefault()}
                onDrop={(e) => onDrop(e, t)}
              >
                {t.clips.map((c) => {
                  const dd = drag && drag.clip.id === c.id ? drag : null;
                  if (dd && dd.targetTrack !== t.id) return null;
                  // Fora da janela visível (e sem arrasto): não desenha.
                  if (!dd && (xOf(c.start + c.duration) < visFrom || xOf(c.start) > visTo)) return null;
                  return <ClipView key={c.id} clip={c} track={t} asset={assetOf(c)} pps={pps} drag={dd} project={p} onBegin={beginDrag} visFrom={visFrom} visTo={visTo} />;
                })}
                {drag && drag.targetTrack === t.id && drag.trackId !== t.id && (
                  <ClipView clip={drag.clip} track={t} asset={assetOf(drag.clip)} pps={pps} drag={drag} project={p} onBegin={() => {}} visFrom={visFrom} visTo={visTo} />
                )}
                {pausePlan?.cuts
                  .filter((c) => c.track_id === t.id)
                  .map((c, i) => (
                    <div
                      key={i}
                      title={`Pausa a remover (${(secs(c.range.end - c.range.start)).toFixed(2)} s)`}
                      className="pointer-events-none absolute inset-y-0 z-[6] border-x border-bad bg-bad/35"
                      style={{ left: xOf(c.range.start), width: Math.max(2, xOf(c.range.end - c.range.start)) }}
                    />
                  ))}
                {t.kind === "video" &&
                  shortRanges?.map((r) => (
                    <div
                      key={`short${r.rank}`}
                      title={`Corte ${r.rank}`}
                      className="pointer-events-none absolute inset-y-0 z-[6] border-x-2 border-ok bg-ok/20"
                      style={{ left: xOf(r.range.start), width: Math.max(2, xOf(r.range.end - r.range.start)) }}
                    >
                      <span className="absolute top-0.5 left-1 rounded bg-ok px-1 text-[10px] font-semibold text-ink-0">{r.rank}</span>
                    </div>
                  ))}
                {t.kind !== "audio" &&
                  autoPlan?.map((r, i) => (
                    <div
                      key={`auto${i}`}
                      title={`Trecho que a edição automática remove (${secs(r.end - r.start).toFixed(2)} s)`}
                      className="pointer-events-none absolute inset-y-0 z-[6] border-x border-bad bg-bad/35"
                      style={{ left: xOf(r.start), width: Math.max(2, xOf(r.end - r.start)) }}
                    />
                  ))}
                {p.timeline.transitions
                  .filter((tr) => tr.track_id === t.id)
                  .map((tr) => {
                    const b = t.clips.find((c) => c.id === tr.to_clip);
                    if (!b) return null;
                    return <TransitionMark key={tr.id} id={tr.id} x={xOf(b.start)} w={xOf(tr.duration)} />;
                  })}
              </div>
            </div>
          ))}
          <div className="flex" style={{ height: 40 }}>
            <div className="sticky left-0 shrink-0 bg-ink-1" style={{ width: HEADER_W }} />
          </div>

          {/* Guia de snap e playhead */}
          {drag?.guide != null && <div className="pointer-events-none absolute top-0 bottom-0 z-10 w-px bg-warn/80" style={{ left: HEADER_W + xOf(drag.guide) }} />}
          <div className="pointer-events-none absolute top-0 bottom-0 z-20" style={{ left: HEADER_W + phX }}>
            <div className="absolute top-0 -left-[6px] h-3 w-3 rotate-45 rounded-[2px] bg-accent" style={{ top: RULER_H - 9 }} />
            <div className="h-full w-px bg-accent shadow-[0_0_8px_#8b7cf6]" />
          </div>
        </div>
      </div>
    </div>
  );
}

function Toolbar({ magnet, setMagnet, fit, duration }: { magnet: boolean; setMagnet: (v: boolean) => void; fit: () => void; duration: number }) {
  const pps = useApp((s) => s.pxPerSec);
  const setZoom = useApp((s) => s.setZoom);
  const sel = useApp((s) => s.selectedClip ?? s.selectedTransition);
  const exec = useApp((s) => s.exec);
  return (
    <div className="flex h-10 shrink-0 items-center gap-1 border-b border-line px-2">
      <IconButton label="Dividir no cursor (S)" onClick={() => splitAtPlayhead()}>
        <Scissors size={15} />
      </IconButton>
      <IconButton label="Apagar seleção (Delete) · Shift+Delete fecha a lacuna" disabled={!sel} onClick={() => deleteSelection(false)}>
        <Trash2 size={15} />
      </IconButton>
      <div className="mx-1.5 h-5 w-px bg-line" />
      <span className="mr-0.5 text-xs text-fg-3">Trilha</span>
      <IconButton label="Adicionar trilha de vídeo" onClick={() => exec({ op: "add_track", kind: "video", name: null })}>
        <Film size={14} />
      </IconButton>
      <IconButton label="Adicionar trilha de texto" onClick={() => exec({ op: "add_track", kind: "text", name: null })}>
        <Type size={14} />
      </IconButton>
      <IconButton label="Adicionar trilha de áudio" onClick={() => exec({ op: "add_track", kind: "audio", name: null })}>
        <AudioLines size={14} />
      </IconButton>
      <div className="mx-1.5 h-5 w-px bg-line" />
      <ToolPicker />
      <div className="mx-1.5 h-5 w-px bg-line" />
      <IconButton label="Ímã (encaixe magnético)" active={magnet} onClick={() => setMagnet(!magnet)}>
        <Magnet size={15} />
      </IconButton>
      <div className="flex-1" />
      <span className="mr-3 text-xs text-fg-3 tabular">Duração {shortDuration(duration)}</span>
      <IconButton label="Diminuir zoom (−)" onClick={() => setZoom(pps / 1.4)}>
        <ZoomOut size={15} />
      </IconButton>
      <input aria-label="Zoom da timeline" type="range" min={Math.log(2)} max={Math.log(600)} step={0.01} value={Math.log(pps)} onChange={(e) => setZoom(Math.exp(Number(e.target.value)))} className="w-28" />
      <IconButton label="Aumentar zoom (+)" onClick={() => setZoom(pps * 1.4)}>
        <ZoomIn size={15} />
      </IconButton>
      <IconButton label="Ajustar à janela" onClick={fit}>
        <Maximize2 size={14} />
      </IconButton>
    </div>
  );
}

const TOOLS: { id: Tool; label: string; key: string; icon: React.ReactNode }[] = [
  { id: "select", label: "Seleção — mover e aparar", key: "V", icon: <MousePointer2 size={14} /> },
  { id: "ripple", label: "Ripple — aparar fechando/abrindo espaço", key: "B", icon: <StretchHorizontal size={14} /> },
  { id: "roll", label: "Roll — mover o corte entre dois clipes", key: "N", icon: <Columns2 size={14} /> },
  { id: "slip", label: "Slip — deslizar o conteúdo dentro do clipe", key: "Y", icon: <MoveHorizontal size={14} /> },
  { id: "slide", label: "Slide — mover o clipe entre os vizinhos", key: "U", icon: <GalleryHorizontal size={14} /> },
];

function ToolPicker() {
  const tool = useApp((s) => s.tool);
  const setTool = useApp((s) => s.setTool);
  return (
    <div role="radiogroup" aria-label="Ferramenta de edição" className="flex rounded-md bg-ink-0 p-0.5">
      {TOOLS.map((t) => (
        <IconButton key={t.id} role="radio" aria-checked={tool === t.id} label={`${t.label} (${t.key})`} active={tool === t.id} onClick={() => setTool(t.id)} className="size-6">
          {t.icon}
        </IconButton>
      ))}
    </div>
  );
}

const STEPS = [0.1, 0.25, 0.5, 1, 2, 5, 10, 15, 30, 60, 120, 300, 600, 1800, 3600];

/** Régua: desenha só os ticks visíveis (barato mesmo em timelines de horas). */
const Ruler = memo(function Ruler({ pps, left, width }: { pps: number; left: number; width: number }) {
  const step = STEPS.find((s) => s * pps >= 80) ?? 3600;
  const minor = step / 5;
  const t0 = Math.floor(left / pps / step) * step;
  const t1 = (left + width) / pps;
  const ticks: { x: number; major: boolean; label?: string }[] = [];
  for (let t = t0; t <= t1 + step; t += minor) {
    const major = Math.abs(t / step - Math.round(t / step)) < 1e-6;
    const tt = Math.round(t * 1000) / 1000;
    const label = major ? (step < 1 ? `${Math.floor(tt / 60)}:${(tt % 60).toFixed(2).padStart(5, "0")}` : shortDuration(fromSecs(tt))) : undefined;
    ticks.push({ x: t * pps, major, label });
  }
  return (
    <div className="pointer-events-none absolute inset-0">
      {ticks.map((k, i) => (
        <div key={i} className="absolute bottom-0" style={{ left: k.x }}>
          <div className={clsx("w-px", k.major ? "h-2.5 bg-fg-3" : "h-1.5 bg-line")} />
          {k.label && <span className="absolute bottom-3 left-1 text-[10px] text-fg-3 tabular whitespace-nowrap">{k.label}</span>}
        </div>
      ))}
    </div>
  );
});

function TrackHeader({ track }: { track: Track }) {
  const exec = useApp((s) => s.exec);
  const patch = (p: Partial<TrackPatch>) => exec({ op: "update_track", track_id: track.id, patch: { ...NULL_TRACK_PATCH, ...p } });
  const Icon = track.kind === "video" ? Film : track.kind === "audio" ? AudioLines : Type;
  return (
    <div className="sticky left-0 z-10 flex shrink-0 items-center gap-1.5 border-r border-line bg-ink-1 pr-1 pl-2.5" style={{ width: HEADER_W }}>
      <Icon size={13} className={track.kind === "video" ? "text-clip-video-edge" : track.kind === "audio" ? "text-clip-audio-edge" : "text-clip-text-edge"} />
      <span className="min-w-0 flex-1 truncate text-xs text-fg-2">{track.name}</span>
      {track.kind !== "audio" && (
        <IconButton label={track.hidden ? "Mostrar trilha" : "Ocultar trilha"} onClick={() => patch({ hidden: !track.hidden })} className="size-6">
          {track.hidden ? <EyeOff size={13} /> : <Eye size={13} />}
        </IconButton>
      )}
      {track.kind !== "text" && (
        <IconButton label={track.muted ? "Ativar som" : "Silenciar trilha"} onClick={() => patch({ muted: !track.muted })} className="size-6">
          {track.muted ? <VolumeX size={13} className="text-warn" /> : <Volume2 size={13} />}
        </IconButton>
      )}
      <IconButton label={track.locked ? "Desbloquear" : "Bloquear trilha"} onClick={() => patch({ locked: !track.locked })} className="size-6">
        {track.locked ? <Lock size={12} className="text-warn" /> : <LockOpen size={12} />}
      </IconButton>
    </div>
  );
}

function TransitionMark({ id, x, w }: { id: string; x: number; w: number }) {
  const sel = useApp((s) => s.selectedTransition === id);
  const selectTransition = useApp((s) => s.selectTransition);
  return (
    <button
      title="Transição — clique para selecionar"
      onPointerDown={(e) => {
        e.stopPropagation();
        selectTransition(id);
      }}
      className={clsx(
        "absolute top-1/2 z-[5] h-5 -translate-y-1/2 rounded-sm border bg-[linear-gradient(135deg,#ffffff30_25%,transparent_25%,transparent_50%,#ffffff30_50%,#ffffff30_75%,transparent_75%)] bg-[length:8px_8px]",
        sel ? "border-white bg-accent/70" : "border-white/50 bg-black/50",
      )}
      style={{ left: x, width: Math.max(10, w) }}
    />
  );
}

const ClipView = memo(function ClipView({
  clip,
  track,
  asset,
  pps,
  drag,
  project,
  onBegin,
  visFrom,
  visTo,
}: {
  clip: Clip;
  track: Track;
  asset?: MediaAsset;
  pps: number;
  drag: Drag | null;
  project: Project;
  onBegin: (e: React.PointerEvent, c: Clip, t: Track, part: "body" | "left" | "right") => void;
  visFrom: number;
  visTo: number;
}) {
  const selected = useApp((s) => s.selectedClip === clip.id);
  const start = drag && drag.mode !== "roll" ? drag.start : clip.start;
  const dur = drag && drag.mode !== "roll" ? drag.duration : clip.duration;
  const sourceIn = drag ? drag.sourceIn : clip.content.type === "media" ? clip.content.source_in : 0;
  const x = secs(start) * pps;
  const w = Math.max(3, secs(dur) * pps);
  const h = LANE_H[track.kind];
  const kind = clip.content.type === "text" ? "text" : track.kind;
  const tone =
    kind === "text"
      ? "bg-clip-text border-clip-text-edge/60"
      : kind === "audio"
        ? "bg-clip-audio border-clip-audio-edge/50"
        : "bg-clip-video border-clip-video-edge/50";
  const thumbH = kind === "video" ? (asset?.audio ? h - 24 : h - 8) : 0;
  // Parte do clipe perto da tela (px relativos ao clipe): miniaturas e
  // waveform só dessa parte — canvas limitado à tela mesmo em clipes de horas.
  const lx0 = Math.max(0, Math.min(w, visFrom - x));
  const lx1 = Math.max(lx0, Math.min(w, visTo - x));

  return (
    <div
      className={clsx(
        "group absolute top-[3px] bottom-[3px] overflow-hidden rounded-[6px] border",
        tone,
        selected && "ring-2 ring-white/90 ring-offset-0",
        drag?.moved && "opacity-90 shadow-lg shadow-black/50",
        track.locked ? "cursor-not-allowed" : "cursor-grab active:cursor-grabbing",
        asset?.status.state === "missing" && "bg-[repeating-linear-gradient(135deg,#f2667a22_0_6px,transparent_6px_12px)]",
      )}
      style={{ left: x, width: w }}
      onPointerDown={(e) => onBegin(e, clip, track, "body")}
      title={`${clip.name} — ${shortDuration(clip.duration)}`}
    >
      {kind === "video" && asset && thumbH > 0 && <Thumbs asset={asset} clip={clip} sourceIn={sourceIn} from={lx0} to={lx1} height={thumbH} pps={pps} />}
      {(kind === "audio" || (kind === "video" && asset?.audio)) && asset?.waveform_path && (
        <div className="absolute inset-x-0 bottom-0" style={{ height: kind === "audio" ? h - 18 : 16 }}>
          {clip.freeze || lx1 <= lx0 ? null : (
            <Waveform
              projectId={project.id}
              asset={asset}
              // Trecho de origem da parte visível (no reverso, espelhado).
              from={clip.reverse ? sourceIn + Math.round((dur - fromSecs(lx1 / pps)) * clip.speed) : sourceIn + Math.round(fromSecs(lx0 / pps) * clip.speed)}
              dur={Math.round(fromSecs((lx1 - lx0) / pps) * clip.speed)}
              reverse={clip.reverse}
              left={lx0}
              width={lx1 - lx0}
              muted={clip.audio_muted || track.muted}
              gain={clip.volume}
            />
          )}
        </div>
      )}
      <div className={clsx("pointer-events-none absolute top-0 left-0 flex max-w-full items-center gap-1 px-1.5 text-[11px] leading-4", kind === "video" ? "rounded-br-md bg-black/55" : "")}>
        {clip.freeze && <Snowflake size={10} className="shrink-0 text-white" />}
        {clip.reverse && <Rewind size={10} className="shrink-0 text-white" />}
        {clip.speed !== 1 && <span className="shrink-0 rounded bg-white/20 px-1 text-[10px] text-white tabular">{clip.speed.toFixed(2).replace(/\.?0+$/, "")}×</span>}
        <span className="truncate font-medium text-white/90">{clip.content.type === "text" ? clip.content.text : clip.name}</span>
      </div>
      {[...new Set([...clip.keyframes.scale, ...clip.keyframes.position_x, ...clip.keyframes.position_y, ...clip.keyframes.volume].map((k) => k.at))].map((at) => (
        <Diamond key={at} size={8} className="pointer-events-none absolute bottom-0.5 -translate-x-1/2 fill-warn text-warn" style={{ left: secs(at) * pps }} />
      ))}
      {clip.fade_in > 0 && <div className="pointer-events-none absolute top-0 left-0 h-full bg-gradient-to-r from-black/50 to-transparent" style={{ width: secs(clip.fade_in) * pps }} />}
      {clip.fade_out > 0 && <div className="pointer-events-none absolute top-0 right-0 h-full bg-gradient-to-l from-black/50 to-transparent" style={{ width: secs(clip.fade_out) * pps }} />}
      {!track.locked && (
        <>
          <div className="absolute inset-y-0 left-0 w-[7px] cursor-ew-resize bg-white/0 hover:bg-white/40" onPointerDown={(e) => onBegin(e, clip, track, "left")} />
          <div className="absolute inset-y-0 right-0 w-[7px] cursor-ew-resize bg-white/0 hover:bg-white/40" onPointerDown={(e) => onBegin(e, clip, track, "right")} />
        </>
      )}
    </div>
  );
});

/** Miniaturas reais da sprite gerada na importação, alinhadas ao tempo de origem. */
function Thumbs({ asset, clip, sourceIn, from, to, height, pps }: { asset: MediaAsset; clip: Clip; sourceIn: number; from: number; to: number; height: number; pps: number }) {
  if (asset.kind === "image" && asset.poster_path) {
    return <div className="absolute inset-x-0 top-0 bg-repeat-x" style={{ height, backgroundImage: `url(${fileUrl(asset.poster_path)})`, backgroundSize: `auto ${height}px` }} />;
  }
  const th = asset.thumbs;
  if (!th) return <div className="absolute inset-x-0 top-0 avs-pulse bg-white/5" style={{ height }} />;
  const tw = Math.round((th.tile_width / th.tile_height) * height);
  const url = fileUrl(th.path);
  const tiles = [];
  // Só as miniaturas da parte visível (antes: no máximo 400 a partir do início,
  // e o resto de um clipe longo ficava vazio).
  for (let i = Math.floor(from / tw); i < Math.ceil(to / tw); i++) {
    const off = fromSecs((i * tw + tw / 2) / pps) * clip.speed;
    const span = clip.duration * clip.speed;
    const t = clip.freeze ? sourceIn : clip.reverse ? sourceIn + span - off : sourceIn + off;
    const idx = Math.max(0, Math.min(th.count - 1, Math.floor(t / th.interval)));
    tiles.push(
      <div
        key={i}
        className="absolute top-0"
        style={{ left: i * tw, width: tw, height, backgroundImage: `url(${url})`, backgroundSize: `${th.count * tw}px ${height}px`, backgroundPosition: `-${idx * tw}px 0` }}
      />,
    );
  }
  return <div className="pointer-events-none absolute inset-x-0 top-0 overflow-hidden opacity-90" style={{ height }}>{tiles}</div>;
}

function Waveform({
  projectId,
  asset,
  from,
  dur,
  reverse,
  left,
  width,
  muted,
  gain,
}: {
  projectId: string;
  asset: MediaAsset;
  from: number;
  dur: number;
  reverse: boolean;
  left: number;
  width: number;
  muted: boolean;
  gain: number;
}) {
  const buckets = Math.max(8, Math.min(2000, Math.floor(width / 2)));
  const { data } = useQuery({
    queryKey: ["wave", asset.id, asset.waveform_path, from, dur, buckets],
    queryFn: () => api.waveform(projectId, asset.id, from, from + dur, buckets),
    staleTime: Infinity,
  });
  const ref = useRef<HTMLCanvasElement>(null);
  useEffect(() => {
    const cv = ref.current;
    if (!cv || !data) return;
    const dpr = window.devicePixelRatio || 1;
    const H = cv.clientHeight;
    cv.width = Math.max(1, Math.floor(width * dpr));
    cv.height = Math.max(1, Math.floor(H * dpr));
    const ctx = cv.getContext("2d")!;
    ctx.scale(dpr, dpr);
    ctx.fillStyle = muted ? "#6e748688" : "#4fc3d9cc";
    const bw = width / data.length;
    const g = Math.min(2, Math.max(0.15, gain));
    (reverse ? [...data].reverse() : data).forEach((v, i) => {
      const hh = Math.max(1, Math.min(1, (v / 255) * g * 1.6) * (H - 2));
      ctx.fillRect(i * bw, (H - hh) / 2, Math.max(1, bw - 0.5), hh);
    });
  }, [data, width, muted, gain, reverse]);
  return <canvas ref={ref} className="pointer-events-none absolute top-0 h-full" style={{ left, width }} />;
}
