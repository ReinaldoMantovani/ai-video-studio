import clsx from "clsx";
import { ChevronLeft, ChevronRight, Grid2x2, Loader2, Pause, Play, RefreshCw, SkipBack } from "lucide-react";
import { useQuery } from "@tanstack/react-query";
import { useCallback, useEffect, useRef, useState } from "react";
import type { SafeArea } from "../../bindings/SafeArea";
import { api, errorMessage, fileUrl } from "../../lib/api";
import { frameDuration, fromSecs, secs, timecode } from "../../lib/time";
import { useApp } from "../../store/app";
import { IconButton, Progress } from "../ui";

/**
 * Player WYSIWYG.
 * - Parado/scrubbing: mostra o frame renderizado pelo mesmo compilador da exportação.
 * - Reprodução: usa o preview renderizado (com cache por blocos). Se a
 *   timeline mudou, o preview é regenerado automaticamente em segundo plano;
 *   enquanto isso, o frame exato continua disponível.
 * - Timelines longas: o preview é feito por janelas de ~2 min em volta do
 *   cursor; durante a reprodução a próxima janela é preparada antes e a troca
 *   acontece no fim da atual (sem renderizar horas para assistir segundos).
 */
/** Mensagem útil quando a WebView não consegue reproduzir o preview. */
function playbackError(e: unknown): string {
  const name = (e as { name?: string })?.name;
  if (name === "NotSupportedError") {
    return "A interface não conseguiu reproduzir o preview. No Linux, confira se os codecs do GStreamer estão instalados (gstreamer1.0-libav / gst-libav). O frame exato, a edição e a exportação continuam funcionando.";
  }
  if (name === "NotAllowedError") return "A reprodução foi bloqueada pelo sistema. Clique em reproduzir novamente.";
  return errorMessage(e);
}

export function Player({ safeArea }: { safeArea: SafeArea | null }) {
  const ps = useApp((s) => s.ps)!;
  const playhead = useApp((s) => s.playhead);
  const setPlayhead = useApp((s) => s.setPlayhead);
  const preview = useApp((s) => s.preview);
  const nextPreview = useApp((s) => s.nextPreview);
  const jobs = useApp((s) => s.jobs);
  const p = ps.project;
  const fps = p.settings.fps;
  const duration = p.timeline.tracks.reduce((m, t) => t.clips.reduce((mm, c) => Math.max(mm, c.start + c.duration), m), 0);
  const revision = ps.history.revision;
  const range = preview?.range ?? null;
  const base = range?.start ?? 0;
  // Atual = mesma revisão e (timeline inteira ou janela que contém o cursor).
  const fresh = !!preview && preview.revision === revision && (!range || (playhead >= range.start && playhead < range.end - frameDuration(fps)));
  const resume = useRef(false);
  const prefetched = useRef<string | null>(null);
  const kicking = useRef(false);
  const [playing, setPlaying] = useState(false);
  const [frameUrl, setFrameUrl] = useState<string | null>(null);
  const [frameErr, setFrameErr] = useState<string | null>(null);
  const [quality, setQuality] = useState(540);
  const [guides, setGuides] = useState(false);
  const video = useRef<HTMLVideoElement>(null);
  const pending = useRef(0);
  // Linux: o <video> toca pelo servidor local de mídia (o WebKitGTK não carrega `asset://`).
  const mediaBase = useQuery({ queryKey: ["media-base"], queryFn: api.mediaBaseUrl, staleTime: Infinity });
  const videoSrc = (p: string) => (mediaBase.data ? mediaBase.data + encodeURIComponent(p) : fileUrl(p));
  const previewJob = Object.values(jobs).find((j) => j.kind === "preview" && j.project_id === p.id && (j.state === "running" || j.state === "queued"));

  // Frame exato no playhead (debounce + só a última requisição vale).
  useEffect(() => {
    if (playing || duration === 0 || playhead >= duration) {
      if (duration === 0) setFrameUrl(null);
      return;
    }
    const ticket = ++pending.current;
    const t = setTimeout(async () => {
      try {
        const path = await api.renderFrame(p.id, playhead, quality);
        if (ticket === pending.current) {
          setFrameUrl(fileUrl(path) ?? null);
          setFrameErr(null);
        }
      } catch (e) {
        if (ticket === pending.current) setFrameErr(errorMessage(e));
      }
    }, 70);
    return () => clearTimeout(t);
  }, [playhead, revision, quality, playing, duration, p.id]);

  // Preview automático após edições (debounce): blocos inalterados são reaproveitados.
  useEffect(() => {
    if (duration === 0 || fresh || playing) return;
    const t = setTimeout(() => api.requestPreview(p.id, useApp.getState().playhead).catch(() => {}), 1200);
    return () => clearTimeout(t);
  }, [revision, duration, fresh, playing, p.id]);

  // Fim da janela atual durante a reprodução. O laço para (o vídeo novo começa em 0
  // e moveria o cursor para o início da janela); `onLoadedData` retoma do ponto certo,
  // já na próxima janela, ou o efeito abaixo troca quando ela ficar pronta.
  const windowEnd = useCallback(() => {
    const st = useApp.getState();
    resume.current = true;
    video.current?.pause();
    setPlaying(false);
    const nx = st.nextPreview;
    if (nx?.range && st.playhead >= nx.range.start && st.playhead < nx.range.end) {
      st.setPreview(nx);
      st.setNextPreview(null);
    }
  }, []);

  // Sincroniza playhead durante a reprodução.
  useEffect(() => {
    if (!playing) return;
    let raf = 0;
    let last = -1;
    let lastAt = performance.now();
    const tick = () => {
      const v = video.current;
      const st = useApp.getState();
      const r = st.preview?.range;
      if (v) {
        // Vigia: o WebKitGTK às vezes fica "tocando" sem avançar logo após trocar de
        // arquivo (paused=false, readyState 4, currentTime parado). Um pause/play
        // reinicia o pipeline; sem isso a reprodução travaria na troca de janela.
        const now = performance.now();
        if (v.currentTime !== last || v.paused || v.seeking || v.readyState < 3) {
          last = v.currentTime;
          lastAt = now;
        } else if (now - lastAt > 800) {
          lastAt = now;
          kicking.current = true;
          v.pause();
          v.play()
            .catch(() => {})
            .finally(() => setTimeout(() => (kicking.current = false), 0));
        }
        const t = fromSecs(v.currentTime) + (r?.start ?? 0);
        setPlayhead(t);
        if (r && r.end < duration) {
          // Prepara a próxima janela 40 s antes do fim da atual.
          const key = `${st.preview!.path}`;
          if (t > r.end - fromSecs(40) && !st.nextPreview && prefetched.current !== key) {
            prefetched.current = key;
            api.requestPreview(p.id, r.end - fromSecs(10)).catch(() => {});
          }
          // Fim da janela: troca para a próxima (ou espera ela ficar pronta).
          if (v.ended || t >= r.end - frameDuration(fps)) {
            cancelAnimationFrame(raf);
            windowEnd();
            return;
          }
        } else if (v.ended) setPlaying(false);
      }
      raf = requestAnimationFrame(tick);
    };
    raf = requestAnimationFrame(tick);
    return () => cancelAnimationFrame(raf);
  }, [playing, setPlayhead, duration, fps, p.id, windowEnd]);

  // A próxima janela ficou pronta enquanto a reprodução esperava: continua.
  useEffect(() => {
    const st = useApp.getState();
    const nx = nextPreview;
    if (resume.current && !playing && nx?.range && playhead >= nx.range.start && playhead < nx.range.end) {
      st.setPreview(nx);
      st.setNextPreview(null);
    }
  }, [nextPreview, playing, playhead]);

  // Seek externo (timeline) com o vídeo parado.
  useEffect(() => {
    const v = video.current;
    if (v && !playing && fresh && Math.abs(v.currentTime - secs(playhead - base)) > 0.02) v.currentTime = secs(playhead - base);
  }, [playhead, playing, fresh, base]);

  const toggle = useCallback(() => {
    const v = video.current;
    if (!fresh || !v) return;
    if (playing) {
      v.pause();
      setPlaying(false);
    } else {
      if (playhead >= duration - frameDuration(fps)) setPlayhead(0);
      resume.current = false;
      v.currentTime = playhead >= duration - frameDuration(fps) ? 0 : secs(playhead - base);
      v.play().then(() => setPlaying(true)).catch((e) => useApp.getState().toast("error", playbackError(e)));
    }
  }, [fresh, playing, playhead, duration, fps, setPlayhead, base]);

  // Pausa se o preview ficar desatualizado durante a reprodução.
  useEffect(() => {
    if (!fresh && playing) {
      video.current?.pause();
      setPlaying(false);
    }
  }, [fresh, playing]);

  useEffect(() => {
    const k = (e: KeyboardEvent) => {
      if ((e.target as HTMLElement).closest("input, textarea, select, button")) return;
      if (e.code === "Space") {
        e.preventDefault();
        toggle();
      }
    };
    window.addEventListener("keydown", k);
    return () => window.removeEventListener("keydown", k);
  }, [toggle]);

  const step = (n: number) => setPlayhead(Math.min(Math.max(0, playhead + n * frameDuration(fps)), Math.max(0, duration - frameDuration(fps))));
  const aspect = p.settings.width / p.settings.height;

  return (
    <div className="flex h-full flex-col bg-ink-0">
      <div className="relative flex min-h-0 flex-1 items-center justify-center p-4">
        <div className="relative max-h-full max-w-full overflow-hidden rounded-md bg-black shadow-xl shadow-black/40" style={{ aspectRatio: `${aspect}`, height: "100%" }}>
          {duration === 0 ? (
            <div className="flex size-full flex-col items-center justify-center gap-2 p-6 text-center text-fg-3">
              <p className="text-fg-2">A timeline está vazia</p>
              <p className="text-xs">Arraste mídias do painel ou do seu computador para a timeline.</p>
            </div>
          ) : (
            <>
              {fresh && preview && mediaBase.isFetched && (
                <video
                  ref={video}
                  src={videoSrc(preview.path)}
                  className={clsx("absolute inset-0 size-full object-contain", !playing && "opacity-0")}
                  preload="auto"
                  onPause={(e) => {
                    // O fim natural do arquivo também dispara `pause`; quem decide é `onEnded`.
                    if (!resume.current && !kicking.current && !e.currentTarget.ended) setPlaying(false);
                  }}
                  onEnded={() => {
                    if (resume.current) return;
                    const r = useApp.getState().preview?.range;
                    if (playing && r && r.end < duration) windowEnd();
                    else setPlaying(false);
                  }}
                  onLoadedData={(e) => {
                    // Troca de janela durante a reprodução: continua do mesmo ponto.
                    if (resume.current) {
                      const v = e.currentTarget;
                      resume.current = false;
                      // Só dá play depois do seek concluir: no WebKitGTK, play() durante
                      // o seek inicial deixa o pipeline parado (paused=false sem avançar).
                      v.addEventListener("seeked", () => v.play().then(() => setPlaying(true)).catch(() => setPlaying(false)), { once: true });
                      v.currentTime = secs(useApp.getState().playhead - base);
                    }
                  }}
                />
              )}
              {!playing && frameUrl && <img src={frameUrl} alt="Frame atual" className="absolute inset-0 size-full object-contain" draggable={false} />}
              {frameErr && !playing && <p className="absolute inset-x-0 bottom-3 mx-auto w-fit max-w-[90%] rounded bg-black/70 px-2 py-1 text-xs text-bad select-text">{frameErr}</p>}
            </>
          )}
          {guides && <Guides safe={safeArea} />}
        </div>
      </div>
      <div className="flex h-11 shrink-0 items-center gap-2 border-t border-line bg-ink-1 px-3">
        <IconButton label="Início (Home)" onClick={() => setPlayhead(0)}>
          <SkipBack size={15} />
        </IconButton>
        <IconButton label="Frame anterior (←)" onClick={() => step(-1)}>
          <ChevronLeft size={16} />
        </IconButton>
        <button
          onClick={toggle}
          disabled={!fresh}
          aria-label={playing ? "Pausar (Espaço)" : "Reproduzir (Espaço)"}
          title={fresh ? "Reproduzir/pausar (Espaço)" : "Aguarde o preview ficar pronto"}
          className="flex size-8 items-center justify-center rounded-full bg-fg text-ink-0 transition-opacity disabled:opacity-30"
        >
          {playing ? <Pause size={15} fill="currentColor" /> : <Play size={15} fill="currentColor" className="ml-0.5" />}
        </button>
        <IconButton label="Próximo frame (→)" onClick={() => step(1)}>
          <ChevronRight size={16} />
        </IconButton>
        <span className="ml-1 text-[13px] tabular">
          {timecode(playhead, fps)} <span className="text-fg-3">/ {timecode(duration, fps)}</span>
        </span>
        <div className="flex-1" />
        {duration > 0 && (
          <div className="flex w-56 items-center gap-2 text-xs">
            {fresh ? (
              <span className="text-fg-3">Preview atualizado</span>
            ) : previewJob ? (
              <>
                <Loader2 size={13} className="shrink-0 animate-spin text-accent" />
                <span className="shrink-0 text-fg-2">Preview</span>
                <Progress value={previewJob.progress} />
              </>
            ) : (
              <button className="flex items-center gap-1.5 text-fg-2 hover:text-fg" onClick={() => api.requestPreview(p.id, playhead)}>
                <RefreshCw size={13} /> Atualizar preview
              </button>
            )}
          </div>
        )}
        <select aria-label="Resolução do frame" value={quality} onChange={(e) => setQuality(Number(e.target.value))} className="h-7 rounded-md border border-line bg-ink-2 px-1.5 text-xs">
          <option value={360}>360p</option>
          <option value={540}>540p</option>
          <option value={720}>720p</option>
          <option value={1080}>1080p</option>
        </select>
        <IconButton label="Guias e áreas seguras" active={guides} onClick={() => setGuides(!guides)}>
          <Grid2x2 size={15} />
        </IconButton>
      </div>
    </div>
  );
}

function Guides({ safe }: { safe: SafeArea | null }) {
  const s = safe ?? { top_pct: 5, bottom_pct: 5, left_pct: 5, right_pct: 5 };
  return (
    <div className="pointer-events-none absolute inset-0">
      {/* regra dos terços */}
      <div className="absolute inset-y-0 left-1/3 border-l border-white/15" />
      <div className="absolute inset-y-0 left-2/3 border-l border-white/15" />
      <div className="absolute inset-x-0 top-1/3 border-t border-white/15" />
      <div className="absolute inset-x-0 top-2/3 border-t border-white/15" />
      {/* área segura da plataforma (do preset) */}
      <div
        className="absolute border border-dashed border-accent/80"
        style={{ top: `${s.top_pct}%`, bottom: `${s.bottom_pct}%`, left: `${s.left_pct}%`, right: `${s.right_pct}%` }}
      />
    </div>
  );
}
