// Ações de edição compartilhadas entre atalhos, barra da timeline e painéis.
import type { MediaAsset } from "../../bindings/MediaAsset";
import type { Project } from "../../bindings/Project";
import type { Track } from "../../bindings/Track";
import { useApp } from "../../store/app";

export function clipAt(p: Project, t: number, trackFilter?: (tr: Track) => boolean) {
  for (const tr of p.timeline.tracks) {
    if (tr.locked || (trackFilter && !trackFilter(tr))) continue;
    const c = tr.clips.find((c) => c.start < t && t < c.start + c.duration);
    if (c) return c;
  }
  return null;
}

/** Divide o clipe selecionado (ou todos sob o cursor) na posição do playhead. */
export async function splitAtPlayhead() {
  const s = useApp.getState();
  const p = s.ps?.project;
  if (!p) return;
  const t = s.playhead;
  const sel = s.selectedClip ? p.timeline.tracks.flatMap((tr) => tr.clips).find((c) => c.id === s.selectedClip) : null;
  if (sel && sel.start < t && t < sel.start + sel.duration) {
    await s.exec({ op: "split_clip", clip_id: sel.id, at: t });
    return;
  }
  const hits = p.timeline.tracks.filter((tr) => !tr.locked).flatMap((tr) => tr.clips.filter((c) => c.start < t && t < c.start + c.duration));
  if (hits.length === 0) {
    s.toast("info", "Posicione o cursor sobre um clipe para dividir.");
    return;
  }
  await s.batch(
    "Dividir clipes",
    hits.map((c) => ({ op: "split_clip", clip_id: c.id, at: t })),
  );
}

export async function deleteSelection(ripple: boolean) {
  const s = useApp.getState();
  if (s.selectedTransition) {
    await s.exec({ op: "remove_transition", transition_id: s.selectedTransition });
    return;
  }
  if (s.selectedClip) await s.exec({ op: "delete_clip", clip_id: s.selectedClip, ripple });
}

export function compatibleTrack(p: Project, a: MediaAsset): Track | undefined {
  const want = a.kind === "audio" ? "audio" : "video";
  return p.timeline.tracks.find((t) => t.kind === want && !t.locked);
}

/** Acrescenta a mídia ao fim da primeira trilha compatível. */
export async function appendAsset(a: MediaAsset) {
  const s = useApp.getState();
  const p = s.ps?.project;
  if (!p) return;
  const tr = compatibleTrack(p, a);
  if (!tr) {
    s.toast("error", "Nenhuma trilha compatível desbloqueada. Adicione uma trilha na timeline.");
    return;
  }
  const end = tr.clips.reduce((m, c) => Math.max(m, c.start + c.duration), 0);
  const created = await s.exec({ op: "insert_clip", track_id: tr.id, asset_id: a.id, start: end, source_in: null, duration: null, ripple: false });
  if (created?.[0]) s.select(created[0]);
}

/** Ctrl+D: duplica o clipe selecionado no fim da sua trilha. */
export async function duplicateSelection() {
  const s = useApp.getState();
  const p = s.ps?.project;
  if (!p || !s.selectedClip) return;
  const tr = p.timeline.tracks.find((t) => t.clips.some((c) => c.id === s.selectedClip));
  if (!tr) return;
  const end = tr.clips.reduce((m, c) => Math.max(m, c.start + c.duration), 0);
  const created = await s.exec({ op: "duplicate_clip", clip_id: s.selectedClip, track_id: null, start: end });
  if (created?.[0]) s.select(created[0]);
}

/** F: congela 2 s do frame sob o cursor no clipe de vídeo selecionado (ou sob o cursor). */
export async function freezeAtPlayhead() {
  const s = useApp.getState();
  const p = s.ps?.project;
  if (!p) return;
  const t = s.playhead;
  const sel = p.timeline.tracks.flatMap((tr) => (tr.kind === "video" ? tr.clips : [])).find((c) => c.id === s.selectedClip && c.start <= t && t < c.start + c.duration);
  const target = sel ?? clipAt(p, t, (tr) => tr.kind === "video");
  if (!target) {
    s.toast("info", "Posicione o cursor sobre um clipe de vídeo para congelar o frame.");
    return;
  }
  await s.exec({ op: "freeze_frame", clip_id: target.id, at: t, duration: 2 * 705_600_000 });
}
