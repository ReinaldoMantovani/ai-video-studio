import clsx from "clsx";
import { useState } from "react";
import type { EditorialPreset } from "../../bindings/EditorialPreset";
import { fpsLabel } from "../../lib/time";
import { Dialog } from "../ui";

const PACING: Record<string, string> = { calm: "Calmo", natural: "Natural", dynamic: "Dinâmico", very_dynamic: "Muito dinâmico" };

/** Navegador de presets: mostra exatamente o conteúdo dos arquivos JSON versionados. */
export function PresetsDialog({ open, onClose, presets }: { open: boolean; onClose: () => void; presets: EditorialPreset[] }) {
  const [sel, setSel] = useState<string | null>(null);
  const p = presets.find((x) => x.id === sel) ?? presets[0];
  return (
    <Dialog open={open} onClose={onClose} title="Presets de edição" width={880}>
      <div className="grid h-[60vh] grid-cols-[240px_1fr] gap-4">
        <div className="overflow-y-auto pr-1">
          {presets.map((x) => (
            <button
              key={x.id}
              onClick={() => setSel(x.id)}
              className={clsx("flex w-full items-center justify-between rounded-md px-2.5 py-2 text-left", p?.id === x.id ? "bg-accent-soft text-fg" : "text-fg-2 hover:bg-ink-2")}
            >
              <span className="truncate">{x.name}</span>
              <span className="text-[11px] text-fg-3">{x.aspect}</span>
            </button>
          ))}
        </div>
        {p && (
          <div className="overflow-y-auto pr-2 select-text">
            <h3 className="text-lg font-semibold">{p.name}</h3>
            <p className="mt-1 text-fg-2">{p.description}</p>
            <dl className="mt-4 grid grid-cols-2 gap-x-6 gap-y-3 text-xs">
              <Row k="Quadro" v={`${p.resolution.width}×${p.resolution.height} · ${p.aspect}`} />
              <Row k="FPS" v={p.fps ? fpsLabel(p.fps) : "Preservar o da origem"} />
              <Row k="Ritmo" v={PACING[p.pacing] ?? p.pacing} />
              <Row k="Duração" v={p.duration.max_s ? `alvo ${p.duration.target_s ?? "—"} s, máximo ${p.duration.max_s} s` : "Livre"} />
              <Row k="Áudio" v={`${p.audio.loudness_lufs} LUFS · pico ${p.audio.true_peak_db} dBTP · ducking ${p.audio.music_ducking_db} dB`} />
              <Row k="Legendas" v={p.captions.enabled ? `${p.captions.style} · ${p.captions.position}` : "Desativadas"} />
              <Row k="Efeitos" v={p.effects.allowed.length ? `${p.effects.allowed.join(", ")} (${p.effects.intensity})` : "Nenhum"} />
              <Row k="Transições" v={`${p.transitions.allowed.join(", ")} · ${p.transitions.default_duration_ms} ms · ${p.transitions.frequency}`} />
              <Row k="Reenquadramento" v={p.reframe.mode === "none" ? "Não" : `${p.reframe.mode}${p.reframe.keep_hud ? " · preserva HUD" : ""}`} />
              <Row k="Exportação" v={p.export_profile} />
              <Row k="Silêncio" v={p.cuts.remove_silence ? `remover pausas > ${p.cuts.min_silence_ms} ms` : "manter"} />
              <Row k="Narrativa" v={p.narrative.mode} />
            </dl>
            {[...p.cuts.rules, ...p.narrative.rules, ...p.safety.rules].length > 0 && (
              <ul className="mt-4 list-disc space-y-1 pl-4 text-xs text-fg-2">
                {[...p.cuts.rules, ...p.narrative.rules, ...p.safety.rules].map((r) => (
                  <li key={r}>{r}</li>
                ))}
              </ul>
            )}
            {Object.keys(p.scoring_weights).length > 0 && (
              <div className="mt-4">
                <p className="text-xs text-fg-3">Pesos iniciais de pontuação (heurística calibrável, não previsão de audiência)</p>
                <div className="mt-2 flex flex-col gap-1.5">
                  {Object.entries(p.scoring_weights).map(([k, v]) => (
                    <div key={k} className="flex items-center gap-2 text-xs">
                      <span className="w-40 text-fg-2">{k}</span>
                      <div className="h-1.5 flex-1 rounded-full bg-ink-3">
                        <div className="h-full rounded-full bg-accent" style={{ width: `${v * 100}%` }} />
                      </div>
                      <span className="w-9 text-right tabular">{Math.round(v * 100)}%</span>
                    </div>
                  ))}
                </div>
              </div>
            )}
            <p className="mt-5 text-[11px] text-fg-3">Arquivo: presets/editorial/*{p.id}.json · edições automáticas que usam estas regras chegam na Fase 4.</p>
          </div>
        )}
      </div>
    </Dialog>
  );
}

function Row({ k, v }: { k: string; v: string }) {
  return (
    <div>
      <dt className="text-fg-3">{k}</dt>
      <dd className="mt-0.5 text-fg">{v}</dd>
    </div>
  );
}
