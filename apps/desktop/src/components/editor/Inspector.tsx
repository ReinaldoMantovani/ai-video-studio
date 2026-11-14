import { open as openDialog } from "@tauri-apps/plugin-dialog";
import clsx from "clsx";
import { Diamond, Shuffle, Snowflake, SlidersHorizontal, Trash2, X } from "lucide-react";
import { useEffect, useState } from "react";
import type { AudioEffects } from "../../bindings/AudioEffects";
import type { Clip } from "../../bindings/Clip";
import type { ClipPatch } from "../../bindings/ClipPatch";
import type { KeyframeProperty } from "../../bindings/KeyframeProperty";
import type { TextStyle } from "../../bindings/TextStyle";
import type { TransitionKind } from "../../bindings/TransitionKind";
import type { VideoEffects } from "../../bindings/VideoEffects";
import { fromSecs, interpolate, secs, shortDuration, timecode } from "../../lib/time";
import { useApp } from "../../store/app";
import { Button, Empty, Field, inputCls, Slider } from "../ui";

const EMPTY_PATCH: ClipPatch = {
  name: null,
  volume: null,
  audio_muted: null,
  fade_in: null,
  fade_out: null,
  opacity: null,
  color: null,
  transform: null,
  reverse: null,
  freeze: null,
  effects: null,
  audio_fx: null,
  text: null,
  text_style: null,
};

export function Inspector() {
  const ps = useApp((s) => s.ps)!;
  const selClip = useApp((s) => s.selectedClip);
  const selTr = useApp((s) => s.selectedTransition);
  const clip = ps.project.timeline.tracks.flatMap((t) => t.clips).find((c) => c.id === selClip);
  const track = ps.project.timeline.tracks.find((t) => t.clips.some((c) => c.id === selClip));
  const tr = ps.project.timeline.transitions.find((t) => t.id === selTr);
  return (
    <div className="flex h-full flex-col bg-ink-1">
      <div className="flex h-11 shrink-0 items-center border-b border-line px-4 font-semibold">Propriedades</div>
      <div className="min-h-0 flex-1 overflow-y-auto px-4 py-3">
        {clip && track ? (
          <ClipInspector key={clip.id} clip={clip} trackKind={track.kind} trackId={track.id} />
        ) : tr ? (
          <TransitionInspector key={tr.id} id={tr.id} kind={tr.kind} duration={tr.duration} />
        ) : (
          <ProjectInfo />
        )}
      </div>
    </div>
  );
}

function ProjectInfo() {
  const p = useApp((s) => s.ps)!.project;
  return (
    <div>
      <Empty icon={<SlidersHorizontal size={26} />} title="Nada selecionado">
        Selecione um clipe ou transição na timeline para editar.
      </Empty>
      <dl className="mt-2 grid grid-cols-2 gap-3 text-xs">
        <div>
          <dt className="text-fg-3">Resolução</dt>
          <dd>
            {p.settings.width}×{p.settings.height}
          </dd>
        </div>
        <div>
          <dt className="text-fg-3">FPS</dt>
          <dd>{(p.settings.fps.num / p.settings.fps.den).toFixed(p.settings.fps.den === 1 ? 0 : 3)}</dd>
        </div>
        <div>
          <dt className="text-fg-3">Áudio</dt>
          <dd>{p.settings.sample_rate / 1000} kHz estéreo</dd>
        </div>
        <div>
          <dt className="text-fg-3">Mídias</dt>
          <dd>{p.assets.length}</dd>
        </div>
      </dl>
    </div>
  );
}

function Section({ title, children, aside }: { title: string; children: React.ReactNode; aside?: React.ReactNode }) {
  return (
    <section className="border-b border-line py-3.5 first:pt-0 last:border-0">
      <div className="mb-3 flex items-center justify-between">
        <h3 className="text-xs font-semibold text-fg-2">{title}</h3>
        {aside}
      </div>
      <div className="flex flex-col gap-3">{children}</div>
    </section>
  );
}

const KF_FIELD: Record<KeyframeProperty, "scale" | "position_x" | "position_y" | "volume"> = {
  scale: "scale",
  position_x: "position_x",
  position_y: "position_y",
  volume: "volume",
};

/**
 * Slider com suporte a keyframes. Sem keyframes, edita o valor estático.
 * Com keyframes, mostra o valor interpolado no cursor e edita (cria) o keyframe ali.
 */
function AnimSlider({
  clip,
  prop,
  label,
  min,
  max,
  step,
  staticValue,
  format,
  onStatic,
  note,
}: {
  clip: Clip;
  prop: KeyframeProperty;
  label: string;
  min: number;
  max: number;
  step: number;
  staticValue: number;
  format: (v: number) => string;
  onStatic: (v: number) => void;
  note?: string;
}) {
  const exec = useApp((s) => s.exec);
  const toast = useApp((s) => s.toast);
  const playhead = useApp((s) => s.playhead);
  const setPlayhead = useApp((s) => s.setPlayhead);
  const kfs = clip.keyframes[KF_FIELD[prop]];
  const rel = playhead - clip.start;
  const inside = rel >= 0 && rel <= clip.duration;
  const animated = kfs.length > 0;
  const value = animated ? interpolate(kfs, Math.min(Math.max(rel, 0), clip.duration), staticValue) : staticValue;
  const [v, setV] = useState(value);
  useEffect(() => setV(value), [value]);
  const onKf = kfs.find((k) => Math.abs(k.at - rel) < 1000);
  const setKf = (val: number) => {
    if (!inside) {
      toast("info", "Posicione o cursor sobre o clipe para criar keyframes.");
      return;
    }
    exec({ op: "set_keyframe", clip_id: clip.id, property: prop, at: rel, value: val, easing: null });
  };
  return (
    <div className="flex flex-col gap-1.5">
      <div className="flex items-end gap-1.5">
        <div className="flex-1">
          <Slider
            label={animated ? `${label} (animado)` : label}
            min={min}
            max={max}
            step={step}
            value={v}
            format={format}
            onChange={setV}
            onCommit={(x) => (animated ? setKf(x) : onStatic(x))}
          />
        </div>
        <button
          title={onKf ? "Remover keyframe no cursor" : "Adicionar keyframe no cursor"}
          aria-label={onKf ? `Remover keyframe de ${label}` : `Adicionar keyframe de ${label}`}
          onClick={() => (onKf ? exec({ op: "remove_keyframe", clip_id: clip.id, property: prop, at: onKf.at }) : setKf(prop === "scale" ? Math.max(1, v) : v))}
          className={clsx("mb-0.5 rounded p-1 transition-colors", onKf ? "text-accent" : animated ? "text-fg-2 hover:text-accent" : "text-fg-3 hover:text-fg")}
        >
          <Diamond size={13} fill={onKf ? "currentColor" : "none"} />
        </button>
      </div>
      {animated && (
        <div className="flex flex-wrap items-center gap-1">
          {kfs.map((k) => (
            <button
              key={k.at}
              onClick={() => setPlayhead(clip.start + k.at)}
              className="rounded bg-ink-2 px-1.5 py-0.5 text-[10.5px] text-fg-2 tabular hover:bg-ink-3"
              title="Ir para o keyframe"
            >
              {secs(k.at).toFixed(2)}s · {format(k.value)}
            </button>
          ))}
          <button className="ml-auto text-[11px] text-fg-3 hover:text-bad" onClick={() => exec({ op: "clear_keyframes", clip_id: clip.id, property: prop })}>
            Limpar
          </button>
        </div>
      )}
      {animated && note && <p className="text-[11px] text-fg-3">{note}</p>}
    </div>
  );
}

function ClipInspector({ clip, trackKind, trackId }: { clip: Clip; trackKind: string; trackId: string }) {
  const exec = useApp((s) => s.exec);
  const toast = useApp((s) => s.toast);
  const playhead = useApp((s) => s.playhead);
  const fps = useApp((s) => s.ps)!.project.settings.fps;
  const [d, setD] = useState(clip);
  useEffect(() => setD(clip), [clip]);
  const commit = (patch: Partial<ClipPatch>) => exec({ op: "update_clip", clip_id: clip.id, patch: { ...EMPTY_PATCH, ...patch } });
  const isText = clip.content.type === "text";
  const isMedia = clip.content.type === "media";
  const hasVideo = trackKind === "video" && !isText;
  const hasAudio = trackKind !== "text";
  const dStyle = d.content.type === "text" ? d.content.style : null;
  const setStyle = (s: Partial<TextStyle>) => setD({ ...d, content: { ...(d.content as Extract<Clip["content"], { type: "text" }>), style: { ...dStyle!, ...s } } });
  const fxCommit = (e: Partial<VideoEffects>) => commit({ effects: { ...clip.effects, ...e } });
  const setFx = (e: Partial<VideoEffects>) => setD({ ...d, effects: { ...d.effects, ...e } });
  const afxCommit = (a: Partial<AudioEffects>) => commit({ audio_fx: { ...clip.audio_fx, ...a } });
  const setAfx = (a: Partial<AudioEffects>) => setD({ ...d, audio_fx: { ...d.audio_fx, ...a } });
  const [speed, setSpeed] = useState(clip.speed);
  useEffect(() => setSpeed(clip.speed), [clip.speed]);
  const pct = (v: number) => `${Math.round(v * 100)}%`;

  return (
    <div>
      <Section title="Clipe">
        <Field label="Nome">
          <input className={inputCls} value={d.name} onChange={(e) => setD({ ...d, name: e.target.value })} onBlur={() => d.name !== clip.name && commit({ name: d.name })} />
        </Field>
        <div className="grid grid-cols-2 gap-2 text-xs">
          <div>
            <p className="text-fg-3">Início</p>
            <p className="tabular">{timecode(clip.start, fps)}</p>
          </div>
          <div>
            <p className="text-fg-3">Duração</p>
            <p className="tabular">
              {shortDuration(clip.duration)} ({secs(clip.duration).toFixed(2)} s)
            </p>
          </div>
          {clip.content.type === "media" && (
            <div className="col-span-2">
              <p className="text-fg-3">Entrada na origem</p>
              <p className="tabular">{timecode(clip.content.source_in, fps)}</p>
            </div>
          )}
        </div>
      </Section>

      {isText && dStyle && (
        <Section title="Texto">
          <textarea
            className={`${inputCls} h-20 resize-none py-1.5`}
            value={d.content.type === "text" ? d.content.text : ""}
            onChange={(e) => setD({ ...d, content: { ...(d.content as Extract<Clip["content"], { type: "text" }>), text: e.target.value } })}
            onBlur={(e) => clip.content.type === "text" && e.target.value !== clip.content.text && e.target.value.trim() && commit({ text: e.target.value })}
          />
          <Field label="Fonte">
            <input className={inputCls} value={dStyle.font} onChange={(e) => setStyle({ font: e.target.value })} onBlur={() => commit({ text_style: dStyle })} />
          </Field>
          <Slider label="Tamanho" min={16} max={240} step={1} value={dStyle.size} onChange={(v) => setStyle({ size: v })} onCommit={(v) => commit({ text_style: { ...dStyle, size: v } })} />
          <div className="grid grid-cols-2 gap-2">
            <ColorField label="Cor" value={dStyle.color} onChange={(c) => commit({ text_style: { ...dStyle, color: c } })} />
            <ColorField label="Contorno" value={dStyle.outline_color} onChange={(c) => commit({ text_style: { ...dStyle, outline_color: c } })} />
          </div>
          <Slider label="Espessura do contorno" min={0} max={16} step={0.5} value={dStyle.outline_width} onChange={(v) => setStyle({ outline_width: v })} onCommit={(v) => commit({ text_style: { ...dStyle, outline_width: v } })} />
          <label className="flex items-center gap-2 text-xs text-fg-2">
            <input type="checkbox" checked={!!dStyle.background} onChange={(e) => commit({ text_style: { ...dStyle, background: e.target.checked ? "#000000" : null } })} />
            Caixa de fundo
          </label>
          <Slider label="Posição horizontal" min={0} max={1} step={0.01} value={dStyle.x} format={pct} onChange={(v) => setStyle({ x: v })} onCommit={(v) => commit({ text_style: { ...dStyle, x: v } })} />
          <Slider label="Posição vertical" min={0} max={1} step={0.01} value={dStyle.y} format={pct} onChange={(v) => setStyle({ y: v })} onCommit={(v) => commit({ text_style: { ...dStyle, y: v } })} />
          <Button size="sm" onClick={() => exec({ op: "apply_text_style", track_id: trackId, style: dStyle })}>
            Aplicar estilo a todas as legendas da trilha
          </Button>
        </Section>
      )}

      {isMedia && trackKind !== "text" && (
        <Section title="Velocidade">
          {clip.freeze ? (
            <p className="flex items-center gap-1.5 text-xs text-fg-2">
              <Snowflake size={13} className="text-accent" /> Frame congelado ({secs(clip.duration).toFixed(2)} s)
            </p>
          ) : (
            <>
              <Slider
                label="Velocidade"
                min={0.25}
                max={4}
                step={0.05}
                value={speed}
                format={(v) => `${v.toFixed(2)}×`}
                onChange={setSpeed}
                onCommit={(v) => v !== clip.speed && exec({ op: "set_clip_speed", clip_id: clip.id, speed: v, ripple: true })}
              />
              <div className="flex gap-1">
                {[0.5, 1, 1.5, 2].map((v) => (
                  <Button key={v} size="sm" variant={clip.speed === v ? "primary" : "subtle"} className="flex-1" onClick={() => exec({ op: "set_clip_speed", clip_id: clip.id, speed: v, ripple: true })}>
                    {v}×
                  </Button>
                ))}
              </div>
              <p className="text-[11px] text-fg-3">A duração muda e os clipes seguintes da trilha acompanham. O áudio mantém o tom.</p>
              <label className="flex items-center gap-2 text-xs text-fg-2">
                <input type="checkbox" checked={clip.reverse} onChange={(e) => commit({ reverse: e.target.checked })} />
                Reproduzir de trás para frente
              </label>
              {hasVideo && (
                <Button
                  size="sm"
                  onClick={() => {
                    if (playhead < clip.start || playhead >= clip.start + clip.duration) {
                      toast("info", "Posicione o cursor sobre o clipe.");
                      return;
                    }
                    exec({ op: "freeze_frame", clip_id: clip.id, at: playhead, duration: fromSecs(2) });
                  }}
                >
                  <Snowflake size={13} /> Congelar frame no cursor (2 s)
                </Button>
              )}
            </>
          )}
        </Section>
      )}

      {hasVideo && (
        <Section title="Transformação">
          <Field label="Enquadramento">
            <select className={inputCls} value={d.transform.fit} onChange={(e) => commit({ transform: { ...d.transform, fit: e.target.value as "fit" | "fill" } })}>
              <option value="fit">Encaixar (com barras)</option>
              <option value="fill">Preencher (recorta excesso)</option>
            </select>
          </Field>
          <AnimSlider clip={clip} prop="scale" label="Escala" min={0.1} max={4} step={0.01} staticValue={clip.transform.scale} format={pct} onStatic={(v) => commit({ transform: { ...clip.transform, scale: v } })} note="Zoom animado vai de 100% a 500%." />
          <AnimSlider clip={clip} prop="position_x" label="Posição X" min={-1} max={1} step={0.01} staticValue={clip.transform.x} format={(v) => v.toFixed(2)} onStatic={(v) => commit({ transform: { ...clip.transform, x: v } })} />
          <AnimSlider clip={clip} prop="position_y" label="Posição Y" min={-1} max={1} step={0.01} staticValue={clip.transform.y} format={(v) => v.toFixed(2)} onStatic={(v) => commit({ transform: { ...clip.transform, y: v } })} />
          <Slider label="Opacidade" min={0} max={1} step={0.01} value={d.opacity} format={pct} onChange={(v) => setD({ ...d, opacity: v })} onCommit={(v) => commit({ opacity: v })} />
        </Section>
      )}

      {hasVideo && (
        <Section title="Cor">
          <Slider label="Brilho" min={-0.5} max={0.5} step={0.01} value={d.color.brightness} format={(v) => v.toFixed(2)} onChange={(v) => setD({ ...d, color: { ...d.color, brightness: v } })} onCommit={(v) => commit({ color: { ...d.color, brightness: v } })} />
          <Slider label="Contraste" min={0.5} max={2} step={0.01} value={d.color.contrast} format={(v) => v.toFixed(2)} onChange={(v) => setD({ ...d, color: { ...d.color, contrast: v } })} onCommit={(v) => commit({ color: { ...d.color, contrast: v } })} />
          <Slider label="Saturação" min={0} max={2.5} step={0.01} value={d.color.saturation} format={(v) => v.toFixed(2)} onChange={(v) => setD({ ...d, color: { ...d.color, saturation: v } })} onCommit={(v) => commit({ color: { ...d.color, saturation: v } })} />
          <Slider label="Temperatura" min={2000} max={12000} step={100} value={d.effects.temperature} format={(v) => `${v} K`} onChange={(v) => setFx({ temperature: v })} onCommit={(v) => fxCommit({ temperature: v })} />
          <div className="flex items-center gap-2">
            <Button
              size="sm"
              className="flex-1"
              onClick={async () => {
                const f = await openDialog({ multiple: false, title: "Escolher LUT", filters: [{ name: "LUT 3D", extensions: ["cube", "3dl"] }] });
                if (typeof f === "string") fxCommit({ lut: f });
              }}
            >
              {clip.effects.lut ? `LUT: ${clip.effects.lut.split(/[\\/]/).pop()}` : "Aplicar LUT (.cube)"}
            </Button>
            {clip.effects.lut && (
              <button aria-label="Remover LUT" className="text-fg-3 hover:text-bad" onClick={() => fxCommit({ lut: null })}>
                <X size={14} />
              </button>
            )}
          </div>
          <Button size="sm" variant="ghost" onClick={() => commit({ color: { brightness: 0, contrast: 1, saturation: 1 }, effects: { ...clip.effects, temperature: 6500, lut: null } })}>
            Restaurar cor original
          </Button>
        </Section>
      )}

      {hasVideo && (
        <Section title="Efeitos">
          <Slider label="Desfoque" min={0} max={20} step={0.5} value={d.effects.blur} format={(v) => (v ? v.toFixed(1) : "desligado")} onChange={(v) => setFx({ blur: v })} onCommit={(v) => fxCommit({ blur: v })} />
          <Slider label="Nitidez" min={0} max={2} step={0.05} value={d.effects.sharpen} format={(v) => (v ? v.toFixed(2) : "desligada")} onChange={(v) => setFx({ sharpen: v })} onCommit={(v) => fxCommit({ sharpen: v })} />
          <Slider label="Vinheta" min={0} max={1} step={0.05} value={d.effects.vignette} format={(v) => (v ? pct(v) : "desligada")} onChange={(v) => setFx({ vignette: v })} onCommit={(v) => fxCommit({ vignette: v })} />
        </Section>
      )}

      {hasAudio && !isText && !clip.freeze && (
        <Section title="Áudio">
          <AnimSlider
            clip={clip}
            prop="volume"
            label="Volume"
            min={0}
            max={2}
            step={0.01}
            staticValue={clip.volume}
            format={(v) => (v === 0 ? "mudo" : `${(20 * Math.log10(v)).toFixed(1)} dB`)}
            onStatic={(v) => commit({ volume: v })}
            note="Com keyframes, o volume animado multiplica o volume do clipe."
          />
          <label className="flex items-center gap-2 text-xs text-fg-2">
            <input type="checkbox" checked={d.audio_muted} onChange={(e) => commit({ audio_muted: e.target.checked })} />
            Silenciar áudio deste clipe
          </label>
        </Section>
      )}

      {hasAudio && !isText && !clip.freeze && (
        <Section title="Tratamento de áudio">
          {(
            [
              ["highpass", "Cortar graves (ronco, vento)"],
              ["denoise", "Reduzir ruído de fundo"],
              ["compressor", "Compressor de voz"],
            ] as const
          ).map(([k, label]) => (
            <label key={k} className="flex items-center gap-2 text-xs text-fg-2">
              <input type="checkbox" checked={clip.audio_fx[k]} onChange={(e) => afxCommit({ [k]: e.target.checked })} />
              {label}
            </label>
          ))}
          {(
            [
              ["eq_low_db", "Graves (100 Hz)"],
              ["eq_mid_db", "Médios (1 kHz)"],
              ["eq_high_db", "Agudos (8 kHz)"],
            ] as const
          ).map(([k, label]) => (
            <Slider
              key={k}
              label={label}
              min={-12}
              max={12}
              step={0.5}
              value={d.audio_fx[k]}
              format={(v) => `${v > 0 ? "+" : ""}${v.toFixed(1)} dB`}
              onChange={(v) => setAfx({ [k]: v })}
              onCommit={(v) => afxCommit({ [k]: v })}
            />
          ))}
        </Section>
      )}

      <Section title="Fades">
        <Slider label="Entrada" min={0} max={Math.min(5, secs(clip.duration) / 2)} step={0.05} value={secs(d.fade_in)} format={(v) => `${v.toFixed(2)} s`} onChange={(v) => setD({ ...d, fade_in: fromSecs(v) })} onCommit={(v) => commit({ fade_in: fromSecs(v) })} />
        <Slider label="Saída" min={0} max={Math.min(5, secs(clip.duration) / 2)} step={0.05} value={secs(d.fade_out)} format={(v) => `${v.toFixed(2)} s`} onChange={(v) => setD({ ...d, fade_out: fromSecs(v) })} onCommit={(v) => commit({ fade_out: fromSecs(v) })} />
      </Section>

      <Button variant="danger" size="sm" className="mt-2 w-full" onClick={() => exec({ op: "delete_clip", clip_id: clip.id, ripple: false })}>
        <Trash2 size={13} /> Apagar clipe
      </Button>
    </div>
  );
}

function ColorField({ label, value, onChange }: { label: string; value: string; onChange: (v: string) => void }) {
  const [v, setV] = useState(value);
  useEffect(() => setV(value), [value]);
  return (
    <Field label={label}>
      <div className="flex h-8 items-center gap-2 rounded-md border border-line bg-ink-2 px-1.5">
        <input type="color" aria-label={label} value={v.slice(0, 7)} onChange={(e) => setV(e.target.value.toUpperCase())} onBlur={() => v !== value && onChange(v)} className="size-5 cursor-pointer border-0 bg-transparent p-0" />
        <span className="text-xs tabular">{v}</span>
      </div>
    </Field>
  );
}

const KINDS: { kind: TransitionKind; name: string }[] = [
  { kind: "crossfade", name: "Dissolver cruzado" },
  { kind: "dissolve", name: "Dissolver granulado" },
  { kind: "fade_black", name: "Passar pelo preto" },
  { kind: "wipe_left", name: "Cortina para a esquerda" },
  { kind: "slide_left", name: "Deslizar para a esquerda" },
];

function TransitionInspector({ id, kind, duration }: { id: string; kind: TransitionKind; duration: number }) {
  const exec = useApp((s) => s.exec);
  return (
    <div className="flex flex-col gap-3">
      <div className="flex items-center gap-2 font-medium">
        <Shuffle size={15} className="text-accent" /> Transição
      </div>
      <p className="text-xs text-fg-2">Duração: {secs(duration).toFixed(2)} s</p>
      <p className="text-xs">{KINDS.find((k) => k.kind === kind)?.name}</p>
      <p className="text-xs text-fg-3">Para mudar o tipo ou a duração, remova e aplique novamente pelo painel Transições.</p>
      <Button variant="danger" size="sm" onClick={() => exec({ op: "remove_transition", transition_id: id })}>
        <Trash2 size={13} /> Remover transição
      </Button>
    </div>
  );
}
