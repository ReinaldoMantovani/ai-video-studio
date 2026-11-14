//! Operações estruturadas de edição. São o único caminho para alterar a
//! timeline — usadas pela UI, pela CLI e (Fase 6) pelo assistente de IA, que
//! só pode emitir estes comandos validados, nunca comandos do sistema.

use crate::model::*;
use crate::time::Time;
use crate::validate::{validate_project, ValidationIssue};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[serde(tag = "op", rename_all = "snake_case")]
#[ts(export)]
pub enum EditCommand {
    AddTrack {
        kind: TrackKind,
        name: Option<String>,
    },
    RemoveTrack {
        track_id: Id,
    },
    /// Move a trilha para a posição `index` (0 = mais abaixo no empilhamento de vídeo).
    MoveTrack {
        track_id: Id,
        index: u32,
    },
    UpdateTrack {
        track_id: Id,
        patch: TrackPatch,
    },
    /// Insere um trecho de mídia. `duration` padrão = restante da mídia
    /// (imagens: 5 s). Com `ripple`, empurra clipes posteriores da trilha.
    InsertClip {
        track_id: Id,
        asset_id: Id,
        start: Time,
        source_in: Option<Time>,
        duration: Option<Time>,
        ripple: bool,
    },
    AddText {
        track_id: Id,
        start: Time,
        duration: Time,
        text: String,
        style: Option<TextStyle>,
    },
    SplitClip {
        clip_id: Id,
        at: Time,
    },
    /// Define novas bordas na timeline. Mover o início desloca o ponto de
    /// entrada na mídia de origem (o conteúdo não "escorrega").
    TrimClip {
        clip_id: Id,
        new_start: Option<Time>,
        new_end: Option<Time>,
        ripple: bool,
    },
    MoveClip {
        clip_id: Id,
        track_id: Option<Id>,
        start: Time,
    },
    DeleteClip {
        clip_id: Id,
        ripple: bool,
    },
    /// Remove `[start, end)` das trilhas indicadas (todas se `None`).
    DeleteRange {
        start: Time,
        end: Time,
        track_ids: Option<Vec<Id>>,
        ripple: bool,
    },
    UpdateClip {
        clip_id: Id,
        patch: ClipPatch,
    },
    /// Liga dois clipes adjacentes; o segundo (e os seguintes) recuam `duration`
    /// para criar a sobreposição da transição.
    AddTransition {
        from_clip: Id,
        to_clip: Id,
        kind: TransitionKind,
        duration: Time,
    },
    RemoveTransition {
        transition_id: Id,
    },
    /// Muda a velocidade mantendo o mesmo trecho de origem (a duração na
    /// timeline muda). Com `ripple`, os clipes seguintes acompanham.
    SetClipSpeed {
        clip_id: Id,
        speed: f32,
        ripple: bool,
    },
    /// Desliza o conteúdo dentro do clipe (mesma posição e duração).
    SlipClip {
        clip_id: Id,
        delta: Time,
    },
    /// Move o corte entre dois clipes consecutivos.
    RollEdit {
        left_clip: Id,
        right_clip: Id,
        delta: Time,
    },
    /// Move o clipe entre os vizinhos encostados, ajustando-os.
    SlideClip {
        clip_id: Id,
        delta: Time,
    },
    /// Duplica (padrão: logo após o original, na mesma trilha).
    DuplicateClip {
        clip_id: Id,
        track_id: Option<Id>,
        start: Option<Time>,
    },
    /// Divide em `at` e insere um frame congelado de `duration` (empurra o resto da trilha).
    FreezeFrame {
        clip_id: Id,
        at: Time,
        duration: Time,
    },
    SetKeyframe {
        clip_id: Id,
        property: KeyframeProperty,
        at: Time,
        value: f32,
        easing: Option<Easing>,
    },
    RemoveKeyframe {
        clip_id: Id,
        property: KeyframeProperty,
        at: Time,
    },
    ClearKeyframes {
        clip_id: Id,
        property: KeyframeProperty,
    },
    /// Cria uma trilha de texto com as legendas (ex.: importadas de SRT/VTT).
    AddSubtitleTrack {
        name: Option<String>,
        cues: Vec<crate::subtitles::Cue>,
        style: Option<TextStyle>,
    },
    /// "Aplicar estilo a todas as legendas" da trilha.
    ApplyTextStyle {
        track_id: Id,
        style: TextStyle,
    },
    AddMarker {
        at: Time,
        label: String,
    },
    RemoveMarker {
        marker_id: Id,
    },
    UpdateSettings {
        settings: ProjectSettings,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TrackPatch {
    pub name: Option<String>,
    pub muted: Option<bool>,
    pub hidden: Option<bool>,
    pub locked: Option<bool>,
    pub volume: Option<f32>,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ClipPatch {
    pub name: Option<String>,
    pub volume: Option<f32>,
    pub audio_muted: Option<bool>,
    pub fade_in: Option<Time>,
    pub fade_out: Option<Time>,
    pub opacity: Option<f32>,
    pub color: Option<ColorAdjust>,
    pub transform: Option<Transform>,
    pub reverse: Option<bool>,
    pub freeze: Option<bool>,
    pub effects: Option<VideoEffects>,
    pub audio_fx: Option<AudioEffects>,
    pub text: Option<String>,
    pub text_style: Option<TextStyle>,
}

#[derive(Debug, Clone, PartialEq, Serialize, TS, thiserror::Error)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
#[ts(export)]
pub enum CommandError {
    #[error("não encontrado: {0}")]
    NotFound(String),
    #[error("a trilha '{0}' está bloqueada")]
    Locked(String),
    #[error("{0}")]
    Invalid(String),
    #[error("a edição resultaria em uma timeline inválida: {}", .0.iter().map(|i| i.message.clone()).collect::<Vec<_>>().join("; "))]
    Validation(Vec<ValidationIssue>),
}

type R<T> = Result<T, CommandError>;

impl EditCommand {
    pub fn label(&self) -> &'static str {
        match self {
            EditCommand::AddTrack { .. } => "Adicionar trilha",
            EditCommand::RemoveTrack { .. } => "Remover trilha",
            EditCommand::MoveTrack { .. } => "Mover trilha",
            EditCommand::UpdateTrack { .. } => "Alterar trilha",
            EditCommand::InsertClip { .. } => "Inserir clipe",
            EditCommand::AddText { .. } => "Adicionar texto",
            EditCommand::SplitClip { .. } => "Dividir clipe",
            EditCommand::TrimClip { .. } => "Aparar clipe",
            EditCommand::MoveClip { .. } => "Mover clipe",
            EditCommand::DeleteClip { .. } => "Apagar clipe",
            EditCommand::DeleteRange { .. } => "Remover trecho",
            EditCommand::UpdateClip { .. } => "Alterar propriedades",
            EditCommand::AddTransition { .. } => "Adicionar transição",
            EditCommand::RemoveTransition { .. } => "Remover transição",
            EditCommand::SetClipSpeed { .. } => "Alterar velocidade",
            EditCommand::SlipClip { .. } => "Deslizar conteúdo (slip)",
            EditCommand::RollEdit { .. } => "Mover corte (roll)",
            EditCommand::SlideClip { .. } => "Deslizar clipe (slide)",
            EditCommand::DuplicateClip { .. } => "Duplicar clipe",
            EditCommand::FreezeFrame { .. } => "Congelar frame",
            EditCommand::SetKeyframe { .. } => "Definir keyframe",
            EditCommand::RemoveKeyframe { .. } => "Remover keyframe",
            EditCommand::ClearKeyframes { .. } => "Limpar keyframes",
            EditCommand::AddSubtitleTrack { .. } => "Adicionar legendas",
            EditCommand::ApplyTextStyle { .. } => "Aplicar estilo a todas as legendas",
            EditCommand::AddMarker { .. } => "Adicionar marcador",
            EditCommand::RemoveMarker { .. } => "Remover marcador",
            EditCommand::UpdateSettings { .. } => "Alterar configurações",
        }
    }
}

/// Resultado de um comando aplicado com sucesso.
#[derive(Debug, Clone, PartialEq, Serialize, TS)]
#[ts(export)]
pub struct ApplyOutcome {
    pub label: String,
    /// IDs criados (ex.: novo clipe do split), para a UI selecionar.
    pub created: Vec<Id>,
}

/// Aplica atomicamente: trabalha numa cópia e só confirma se a validação passar.
pub fn apply(project: &mut Project, cmd: &EditCommand) -> R<ApplyOutcome> {
    let mut draft = project.clone();
    let created = apply_inner(&mut draft, cmd)?;
    draft.timeline.sort_clips();
    let issues = validate_project(&draft);
    if !issues.is_empty() {
        return Err(CommandError::Validation(issues));
    }
    draft.updated_at = chrono::Utc::now();
    *project = draft;
    Ok(ApplyOutcome { label: cmd.label().into(), created })
}

fn track_idx(tl: &Timeline, id: Id) -> R<usize> {
    let i = tl.tracks.iter().position(|t| t.id == id).ok_or_else(|| CommandError::NotFound("trilha".into()))?;
    if tl.tracks[i].locked {
        return Err(CommandError::Locked(tl.tracks[i].name.clone()));
    }
    Ok(i)
}

fn clip_loc(tl: &Timeline, id: Id) -> R<(usize, usize)> {
    let (t, c) = tl.locate_clip(id).ok_or_else(|| CommandError::NotFound("clipe".into()))?;
    if tl.tracks[t].locked {
        return Err(CommandError::Locked(tl.tracks[t].name.clone()));
    }
    Ok((t, c))
}

/// Desloca todos os clipes da trilha que começam em `from` ou depois.
fn shift_after(track: &mut Track, from: Time, delta: Time, except: Option<Id>) {
    for c in &mut track.clips {
        if c.start >= from && Some(c.id) != except {
            c.start += delta;
        }
    }
}

/// Reposiciona keyframes após mudar o início do clipe (`shift` = deslocamento
/// do tempo relativo) e mantém o valor nas bordas quando algum sai do clipe.
fn retime_keyframes(c: &mut Clip, shift: Time) {
    let dur = c.duration;
    for k in c.keyframes.all_mut() {
        if k.is_empty() {
            continue;
        }
        let mut moved: Vec<Keyframe> = k.iter().map(|x| Keyframe { at: x.at + shift, ..x.clone() }).collect();
        let cut_lo = moved.iter().any(|x| x.at < Time::ZERO);
        let cut_hi = moved.iter().any(|x| x.at > dur);
        let v0 = interpolate(&moved, Time::ZERO, 0.0);
        let v1 = interpolate(&moved, dur, 0.0);
        let easing0 = moved.iter().rev().find(|x| x.at <= Time::ZERO).map(|x| x.easing).unwrap_or(Easing::Linear);
        moved.retain(|x| x.at >= Time::ZERO && x.at <= dur);
        if cut_lo && moved.first().map(|x| x.at != Time::ZERO).unwrap_or(true) {
            moved.insert(0, Keyframe { at: Time::ZERO, value: v0, easing: easing0 });
        }
        if cut_hi && moved.last().map(|x| x.at != dur).unwrap_or(true) {
            moved.push(Keyframe { at: dur, value: v1, easing: Easing::Linear });
        }
        *k = moved;
    }
}

/// Move a borda esquerda `d` para a direita (d < 0 estende), ajustando a origem.
fn trim_left(c: &mut Clip, d: Time) {
    c.start += d;
    c.duration -= d;
    if !c.freeze && !c.reverse {
        if let ClipContent::Media { source_in, .. } = &mut c.content {
            *source_in += scale_time(d, c.speed);
        }
    }
    retime_keyframes(c, -d);
    if c.fade_in + c.fade_out > c.duration {
        c.fade_in = Time::ZERO;
        c.fade_out = Time::ZERO;
    }
}

/// Move a borda direita `d` para a direita (d < 0 encurta).
fn trim_right(c: &mut Clip, d: Time) {
    c.duration += d;
    if c.reverse && !c.freeze {
        // Invertido: o fim na timeline corresponde ao início na origem.
        if let ClipContent::Media { source_in, .. } = &mut c.content {
            *source_in -= scale_time(d, c.speed);
        }
    }
    retime_keyframes(c, Time::ZERO);
    if c.fade_in + c.fade_out > c.duration {
        c.fade_in = Time::ZERO;
        c.fade_out = Time::ZERO;
    }
}

fn clip_mut(tl: &mut Timeline, id: Id) -> R<&mut Clip> {
    let (t, c) = clip_loc(tl, id)?;
    Ok(&mut tl.tracks[t].clips[c])
}

fn drop_transitions_of(tl: &mut Timeline, clip: Id) {
    tl.transitions.retain(|t| t.from_clip != clip && t.to_clip != clip);
}

fn apply_inner(p: &mut Project, cmd: &EditCommand) -> R<Vec<Id>> {
    let mut created = vec![];
    match cmd.clone() {
        EditCommand::AddTrack { kind, name } => {
            let n = p.timeline.tracks.iter().filter(|t| t.kind == kind).count() + 1;
            let default = match kind {
                TrackKind::Video => format!("Vídeo {n}"),
                TrackKind::Audio => format!("Áudio {n}"),
                TrackKind::Text => format!("Texto {n}"),
            };
            let t = Track::new(kind, name.unwrap_or(default));
            created.push(t.id);
            // Áudio fica abaixo na lista visual mas não afeta o empilhamento de vídeo.
            p.timeline.tracks.push(t);
        }
        EditCommand::RemoveTrack { track_id } => {
            let i = track_idx(&p.timeline, track_id)?;
            p.timeline.tracks.remove(i);
            p.timeline.transitions.retain(|t| t.track_id != track_id);
        }
        EditCommand::MoveTrack { track_id, index } => {
            let i = track_idx(&p.timeline, track_id)?;
            let t = p.timeline.tracks.remove(i);
            let to = (index as usize).min(p.timeline.tracks.len());
            p.timeline.tracks.insert(to, t);
        }
        EditCommand::UpdateTrack { track_id, patch } => {
            // Desbloquear precisa funcionar numa trilha bloqueada.
            let t = p.timeline.track_mut(track_id).ok_or_else(|| CommandError::NotFound("trilha".into()))?;
            if t.locked && patch.locked != Some(false) {
                return Err(CommandError::Locked(t.name.clone()));
            }
            if let Some(v) = patch.name {
                t.name = v;
            }
            if let Some(v) = patch.muted {
                t.muted = v;
            }
            if let Some(v) = patch.hidden {
                t.hidden = v;
            }
            if let Some(v) = patch.locked {
                t.locked = v;
            }
            if let Some(v) = patch.volume {
                if !(0.0..=4.0).contains(&v) {
                    return Err(CommandError::Invalid("volume da trilha fora de 0–4".into()));
                }
                t.volume = v;
            }
        }
        EditCommand::InsertClip { track_id, asset_id, start, source_in, duration, ripple } => {
            let ti = track_idx(&p.timeline, track_id)?;
            let asset = p.asset(asset_id).ok_or_else(|| CommandError::NotFound("mídia".into()))?.clone();
            if asset.status != AssetStatus::Ready {
                return Err(CommandError::Invalid(format!("'{}' ainda não está pronto para edição", asset.name)));
            }
            let source_in = source_in.unwrap_or(Time::ZERO);
            let duration = match duration {
                Some(d) => d,
                None if asset.is_unbounded() => Time::from_secs(5),
                None => {
                    // Quadros inteiros, sem passar do fim da mídia: evita
                    // "lascas" de menos de um quadro em cortes posteriores.
                    let fps = p.settings.fps;
                    let raw = asset.duration - source_in;
                    let d = raw.snap_to_frame(fps);
                    if d > raw {
                        d - fps.frame_duration()
                    } else {
                        d
                    }
                }
            };
            if !duration.is_positive() {
                return Err(CommandError::Invalid("duração do clipe precisa ser positiva".into()));
            }
            let start = start.snap_to_frame(p.settings.fps);
            let clip = Clip::new(start, duration, ClipContent::Media { asset_id, source_in }, asset.name.clone());
            created.push(clip.id);
            let track = &mut p.timeline.tracks[ti];
            if ripple {
                shift_after(track, start, duration, None);
            }
            track.clips.push(clip);
        }
        EditCommand::AddText { track_id, start, duration, text, style } => {
            let ti = track_idx(&p.timeline, track_id)?;
            let name: String = text.chars().take(24).collect();
            let clip = Clip::new(start, duration, ClipContent::Text { text, style: style.unwrap_or_default() }, name);
            created.push(clip.id);
            p.timeline.tracks[ti].clips.push(clip);
        }
        EditCommand::SplitClip { clip_id, at } => {
            let (ti, ci) = clip_loc(&p.timeline, clip_id)?;
            let at = at.snap_to_frame(p.settings.fps);
            let orig = p.timeline.tracks[ti].clips[ci].clone();
            if at <= orig.start || at >= orig.end() {
                return Err(CommandError::Invalid("o ponto de divisão precisa estar dentro do clipe".into()));
            }
            let left_dur = at - orig.start;
            let mut right = orig.clone();
            right.id = new_id();
            trim_left(&mut right, left_dur);
            right.fade_in = Time::ZERO;
            if right.fade_out > right.duration {
                right.fade_out = Time::ZERO;
            }
            let left = &mut p.timeline.tracks[ti].clips[ci];
            trim_right(left, -(orig.end() - at));
            left.fade_out = Time::ZERO;
            if left.fade_in > left.duration {
                left.fade_in = Time::ZERO;
            }
            // Transição de saída passa a sair da metade direita.
            for t in &mut p.timeline.transitions {
                if t.from_clip == clip_id {
                    t.from_clip = right.id;
                }
            }
            created.push(right.id);
            p.timeline.tracks[ti].clips.push(right);
        }
        EditCommand::TrimClip { clip_id, new_start, new_end, ripple } => {
            let (ti, ci) = clip_loc(&p.timeline, clip_id)?;
            let fps = p.settings.fps;
            let old = p.timeline.tracks[ti].clips[ci].clone();
            let s = new_start.map(|t| t.snap_to_frame(fps)).unwrap_or(old.start);
            let e = new_end.map(|t| t.snap_to_frame(fps)).unwrap_or(old.end());
            if e <= s {
                return Err(CommandError::Invalid("o clipe precisa ter duração positiva".into()));
            }
            {
                let c = &mut p.timeline.tracks[ti].clips[ci];
                trim_left(c, s - old.start);
                trim_right(c, e - c.end());
            }
            if ripple && e != old.end() {
                shift_after(&mut p.timeline.tracks[ti], old.end(), e - old.end(), Some(clip_id));
            }
        }
        EditCommand::MoveClip { clip_id, track_id, start } => {
            let (ti, ci) = clip_loc(&p.timeline, clip_id)?;
            let mut clip = p.timeline.tracks[ti].clips.remove(ci);
            drop_transitions_of(&mut p.timeline, clip_id);
            let dest = match track_id {
                Some(id) => track_idx(&p.timeline, id)?,
                None => ti,
            };
            clip.start = start.max(Time::ZERO).snap_to_frame(p.settings.fps);
            p.timeline.tracks[dest].clips.push(clip);
        }
        EditCommand::DeleteClip { clip_id, ripple } => {
            let (ti, ci) = clip_loc(&p.timeline, clip_id)?;
            drop_transitions_of(&mut p.timeline, clip_id);
            let track = &mut p.timeline.tracks[ti];
            let removed = track.clips.remove(ci);
            if ripple {
                let prev_end = if ci > 0 { track.clips[ci - 1].end() } else { Time::ZERO };
                if let Some(next) = track.clips.get(ci) {
                    let target = prev_end.max(removed.start);
                    let delta = target - next.start;
                    if delta < Time::ZERO {
                        let from = next.start;
                        shift_after(track, from, delta, None);
                    }
                }
            }
        }
        EditCommand::DeleteRange { start, end, track_ids, ripple } => {
            let fps = p.settings.fps;
            let (start, end) = (start.snap_to_frame(fps), end.snap_to_frame(fps));
            if end <= start {
                return Err(CommandError::Invalid("intervalo vazio".into()));
            }
            let targets: Vec<Id> = match track_ids {
                Some(ids) => ids,
                None => p.timeline.tracks.iter().filter(|t| !t.locked).map(|t| t.id).collect(),
            };
            // Bordas dentro de uma sobreposição de transição são ambíguas.
            for tr in &p.timeline.transitions {
                if !targets.contains(&tr.track_id) {
                    continue;
                }
                if let Some(b) = p.timeline.clip(tr.to_clip) {
                    let ov = crate::time::TimeRange::new(b.start, b.start + tr.duration);
                    if (ov.start < start && start < ov.end) || (ov.start < end && end < ov.end) {
                        return Err(CommandError::Invalid(format!(
                            "o intervalo começa ou termina dentro de uma transição ({}–{}); ajuste o intervalo ou remova a transição",
                            ov.start, ov.end
                        )));
                    }
                }
            }
            let len = end - start;
            for tid in targets {
                let ti = track_idx(&p.timeline, tid)?;
                // Divide nas bordas.
                for at in [start, end] {
                    let hit = p.timeline.tracks[ti].clips.iter().find(|c| c.start < at && at < c.end()).map(|c| c.id);
                    if let Some(cid) = hit {
                        apply_inner(p, &EditCommand::SplitClip { clip_id: cid, at })?;
                    }
                }
                // Também some uma sobra final de menos de um quadro (mídia cuja
                // duração não é múltipla do quadro).
                let frame = fps.frame_duration();
                let last_start = p.timeline.tracks[ti].clips.iter().map(|c| c.start).max().unwrap_or(Time::ZERO);
                let doomed: Vec<Id> = p.timeline.tracks[ti]
                    .clips
                    .iter()
                    .filter(|c| (c.start >= start && c.end() <= end) || (c.start == end && c.duration < frame && c.start == last_start))
                    .map(|c| c.id)
                    .collect();
                for id in &doomed {
                    drop_transitions_of(&mut p.timeline, *id);
                }
                let track = &mut p.timeline.tracks[ti];
                track.clips.retain(|c| !doomed.contains(&c.id));
                if ripple {
                    shift_after(track, end, -len, None);
                }
                track.clips.sort_by_key(|c| c.start);
            }
        }
        EditCommand::UpdateClip { clip_id, patch } => {
            let (ti, ci) = clip_loc(&p.timeline, clip_id)?;
            let c = &mut p.timeline.tracks[ti].clips[ci];
            if let Some(v) = patch.name {
                c.name = v;
            }
            if let Some(v) = patch.volume {
                c.volume = v;
            }
            if let Some(v) = patch.audio_muted {
                c.audio_muted = v;
            }
            if let Some(v) = patch.fade_in {
                c.fade_in = v;
            }
            if let Some(v) = patch.fade_out {
                c.fade_out = v;
            }
            if let Some(v) = patch.opacity {
                c.opacity = v;
            }
            if let Some(v) = patch.color {
                c.color = v;
            }
            if let Some(v) = patch.transform {
                c.transform = v;
            }
            if let Some(v) = patch.reverse {
                c.reverse = v;
            }
            if let Some(v) = patch.freeze {
                c.freeze = v;
            }
            if let Some(v) = patch.effects {
                c.effects = v;
            }
            if let Some(v) = patch.audio_fx {
                c.audio_fx = v;
            }
            if patch.text.is_some() || patch.text_style.is_some() {
                match &mut c.content {
                    ClipContent::Text { text, style } => {
                        if let Some(v) = patch.text {
                            // O nome acompanha o texto, salvo se foi renomeado à mão.
                            let auto: String = text.chars().take(24).collect();
                            if c.name == auto {
                                c.name = v.chars().take(24).collect();
                            }
                            *text = v;
                        }
                        if let Some(v) = patch.text_style {
                            *style = v;
                        }
                    }
                    _ => return Err(CommandError::Invalid("o clipe não é um texto".into())),
                }
            }
        }
        EditCommand::AddTransition { from_clip, to_clip, kind, duration } => {
            let (ta, ca) = clip_loc(&p.timeline, from_clip)?;
            let (tb, cb) = clip_loc(&p.timeline, to_clip)?;
            if ta != tb || cb != ca + 1 {
                return Err(CommandError::Invalid("a transição precisa ligar dois clipes consecutivos da mesma trilha".into()));
            }
            if p.timeline.transition_between(from_clip, to_clip).is_some() {
                return Err(CommandError::Invalid("já existe uma transição entre esses clipes".into()));
            }
            let track = &mut p.timeline.tracks[ta];
            if track.clips[ca].end() != track.clips[cb].start {
                return Err(CommandError::Invalid("os clipes precisam estar encostados (sem lacuna) para receber transição".into()));
            }
            let duration = duration.snap_to_frame(p.settings.fps);
            let from = track.clips[cb].start;
            shift_after(track, from, -duration, None);
            let tr = Transition { id: new_id(), track_id: track.id, from_clip, to_clip, kind, duration };
            created.push(tr.id);
            p.timeline.transitions.push(tr);
        }
        EditCommand::RemoveTransition { transition_id } => {
            let i = p.timeline.transitions.iter().position(|t| t.id == transition_id).ok_or_else(|| CommandError::NotFound("transição".into()))?;
            let tr = p.timeline.transitions.remove(i);
            let ti = track_idx(&p.timeline, tr.track_id)?;
            let track = &mut p.timeline.tracks[ti];
            let from = track.clips.iter().find(|c| c.id == tr.to_clip).map(|c| c.start).ok_or_else(|| CommandError::NotFound("clipe".into()))?;
            shift_after(track, from, tr.duration, None);
        }
        EditCommand::SetClipSpeed { clip_id, speed, ripple } => {
            if !(0.1..=8.0).contains(&speed) {
                return Err(CommandError::Invalid("velocidade precisa ficar entre 0,1× e 8×".into()));
            }
            let fps = p.settings.fps;
            let (ti, ci) = clip_loc(&p.timeline, clip_id)?;
            let c = &mut p.timeline.tracks[ti].clips[ci];
            if c.freeze {
                return Err(CommandError::Invalid("um frame congelado não tem velocidade".into()));
            }
            let old_end = c.end();
            let span = c.source_span();
            let new_dur = scale_time(span, 1.0 / speed).snap_to_frame(fps).max(fps.frame_duration());
            let ratio = new_dur.flicks() as f64 / c.duration.flicks().max(1) as f64;
            for k in c.keyframes.all_mut() {
                for kf in k.iter_mut() {
                    kf.at = Time::from_flicks((kf.at.flicks() as f64 * ratio).round() as i64).min(new_dur);
                }
                k.dedup_by_key(|x| x.at);
            }
            c.speed = speed;
            c.duration = new_dur;
            // Reajusta para não ultrapassar a origem por arredondamento de frame.
            if c.source_span() > span {
                c.duration = (new_dur - fps.frame_duration()).max(fps.frame_duration());
            }
            if c.fade_in + c.fade_out > c.duration {
                c.fade_in = Time::ZERO;
                c.fade_out = Time::ZERO;
            }
            let delta = c.end() - old_end;
            if ripple && delta != Time::ZERO {
                shift_after(&mut p.timeline.tracks[ti], old_end, delta, Some(clip_id));
            }
        }
        EditCommand::SlipClip { clip_id, delta } => {
            let fps = p.settings.fps;
            let c = clip_mut(&mut p.timeline, clip_id)?;
            match &mut c.content {
                ClipContent::Media { source_in, .. } => *source_in = (*source_in + delta).snap_to_frame(fps),
                _ => return Err(CommandError::Invalid("slip só se aplica a mídia".into())),
            }
        }
        EditCommand::RollEdit { left_clip, right_clip, delta } => {
            let (ta, ca) = clip_loc(&p.timeline, left_clip)?;
            let (tb, cb) = clip_loc(&p.timeline, right_clip)?;
            if ta != tb || cb != ca + 1 {
                return Err(CommandError::Invalid("roll precisa de dois clipes consecutivos da mesma trilha".into()));
            }
            let d = delta.snap_to_frame(p.settings.fps);
            let track = &mut p.timeline.tracks[ta];
            trim_right(&mut track.clips[ca], d);
            trim_left(&mut track.clips[cb], d);
        }
        EditCommand::SlideClip { clip_id, delta } => {
            let (ti, ci) = clip_loc(&p.timeline, clip_id)?;
            if p.timeline.transitions.iter().any(|t| t.from_clip == clip_id || t.to_clip == clip_id) {
                return Err(CommandError::Invalid("remova as transições do clipe antes de usar slide".into()));
            }
            let d = delta.snap_to_frame(p.settings.fps);
            let track = &mut p.timeline.tracks[ti];
            let has_prev = ci > 0 && track.clips[ci - 1].end() == track.clips[ci].start;
            let has_next = ci + 1 < track.clips.len() && track.clips[ci].end() == track.clips[ci + 1].start;
            if !has_prev && !has_next {
                return Err(CommandError::Invalid("slide precisa de pelo menos um vizinho encostado; use mover".into()));
            }
            if has_prev {
                trim_right(&mut track.clips[ci - 1], d);
            }
            track.clips[ci].start += d;
            if has_next {
                trim_left(&mut track.clips[ci + 1], d);
            }
        }
        EditCommand::DuplicateClip { clip_id, track_id, start } => {
            let (ti, ci) = clip_loc(&p.timeline, clip_id)?;
            let mut c = p.timeline.tracks[ti].clips[ci].clone();
            c.id = new_id();
            c.start = start.unwrap_or(c.end()).max(Time::ZERO).snap_to_frame(p.settings.fps);
            let dest = match track_id {
                Some(id) => track_idx(&p.timeline, id)?,
                None => ti,
            };
            created.push(c.id);
            p.timeline.tracks[dest].clips.push(c);
        }
        EditCommand::FreezeFrame { clip_id, at, duration } => {
            let fps = p.settings.fps;
            let (ti, ci) = clip_loc(&p.timeline, clip_id)?;
            let orig = p.timeline.tracks[ti].clips[ci].clone();
            if p.timeline.tracks[ti].kind != TrackKind::Video || orig.freeze {
                return Err(CommandError::Invalid("congelar frame precisa de um clipe de vídeo".into()));
            }
            let at = at.snap_to_frame(fps);
            if at < orig.start || at >= orig.end() {
                return Err(CommandError::Invalid("o cursor precisa estar sobre o clipe".into()));
            }
            let duration = duration.snap_to_frame(fps).max(fps.frame_duration());
            let src_at = {
                let off = scale_time(at - orig.start, orig.speed);
                if orig.reverse {
                    orig.source_in() + orig.source_span() - off - fps.frame_duration()
                } else {
                    orig.source_in() + off
                }
            }
            .max(Time::ZERO);
            if at > orig.start {
                apply_inner(p, &EditCommand::SplitClip { clip_id, at })?;
            }
            let track = &mut p.timeline.tracks[ti];
            shift_after(track, at, duration, None);
            let mut f = orig.clone();
            f.id = new_id();
            f.name = format!("{} (congelado)", orig.name);
            f.start = at;
            f.duration = duration;
            f.freeze = true;
            f.reverse = false;
            f.speed = 1.0;
            f.fade_in = Time::ZERO;
            f.fade_out = Time::ZERO;
            f.keyframes = Keyframes::default();
            if let ClipContent::Media { source_in, .. } = &mut f.content {
                *source_in = src_at;
            }
            created.push(f.id);
            track.clips.push(f);
            // A transição que entrava no clipe original (se dividido no início) continua válida;
            // a que saía dele continua na metade direita (já deslocada).
        }
        EditCommand::SetKeyframe { clip_id, property, at, value, easing } => {
            let fps = p.settings.fps;
            let c = clip_mut(&mut p.timeline, clip_id)?;
            let at = at.snap_to_frame(fps).max(Time::ZERO).min(c.duration);
            let k = c.keyframes.get_mut(property);
            let kf = Keyframe { at, value, easing: easing.unwrap_or(Easing::Linear) };
            match k.iter_mut().find(|x| x.at == at) {
                Some(x) => *x = kf,
                None => {
                    k.push(kf);
                    k.sort_by_key(|x| x.at);
                }
            }
        }
        EditCommand::RemoveKeyframe { clip_id, property, at } => {
            let c = clip_mut(&mut p.timeline, clip_id)?;
            let k = c.keyframes.get_mut(property);
            let n = k.len();
            k.retain(|x| x.at != at);
            if k.len() == n {
                return Err(CommandError::NotFound("keyframe".into()));
            }
        }
        EditCommand::ClearKeyframes { clip_id, property } => {
            clip_mut(&mut p.timeline, clip_id)?.keyframes.get_mut(property).clear();
        }
        EditCommand::AddSubtitleTrack { name, cues, style } => {
            let fps = p.settings.fps;
            let style = style.unwrap_or_else(crate::subtitles::caption_style);
            let mut track = Track::new(TrackKind::Text, name.unwrap_or_else(|| "Legendas".into()));
            let mut cues: Vec<_> = cues.into_iter().filter(|c| !c.text.trim().is_empty()).collect();
            cues.sort_by_key(|c| c.start);
            for cue in cues {
                let s = cue.start.snap_to_frame(fps).max(Time::ZERO);
                let e = cue.end.snap_to_frame(fps);
                // Legendas que se sobrepõem: a anterior termina onde a próxima começa.
                if let Some(prev) = track.clips.last_mut() {
                    if prev.end() > s {
                        prev.duration = s - prev.start;
                    }
                }
                track.clips.retain(|c| c.duration.is_positive());
                if e > s {
                    let text = cue.text.trim().to_string();
                    let name: String = text.chars().take(24).collect();
                    track.clips.push(Clip::new(s, e - s, ClipContent::Text { text, style: style.clone() }, name));
                }
            }
            if track.clips.is_empty() {
                return Err(CommandError::Invalid("nenhuma legenda válida encontrada".into()));
            }
            created.push(track.id);
            p.timeline.tracks.push(track);
        }
        EditCommand::ApplyTextStyle { track_id, style } => {
            let ti = track_idx(&p.timeline, track_id)?;
            let mut n = 0;
            for c in &mut p.timeline.tracks[ti].clips {
                if let ClipContent::Text { style: s, .. } = &mut c.content {
                    *s = style.clone();
                    n += 1;
                }
            }
            if n == 0 {
                return Err(CommandError::Invalid("a trilha não tem textos".into()));
            }
        }
        EditCommand::AddMarker { at, label } => {
            let m = Marker { id: new_id(), at, label, color: "#8b7cf6".into() };
            created.push(m.id);
            p.timeline.markers.push(m);
            p.timeline.markers.sort_by_key(|m| m.at);
        }
        EditCommand::RemoveMarker { marker_id } => {
            p.timeline.markers.retain(|m| m.id != marker_id);
        }
        EditCommand::UpdateSettings { settings } => {
            p.settings = settings;
        }
    }
    Ok(created)
}
