//! Planejadores de edição automática.
//!
//! Transformam a análise em `EditCommand`s — as mesmas operações validadas e
//! desfazíveis da edição manual. Nada é aplicado sem que o plano seja
//! mostrado antes (trechos, tempo removido e motivos).

use crate::analysis::{AnalysisResult, Transcript};
use avs_core::subtitles::Cue;
use avs_core::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;

/// Mapeamento origem → timeline de um clipe de mídia (velocidade, sem reverso).
#[derive(Debug, Clone, Copy)]
pub struct ClipMap {
    pub start: Time,
    pub source_in: Time,
    pub span: Time,
    pub speed: f32,
}

impl ClipMap {
    pub fn of(c: &Clip) -> Option<ClipMap> {
        if c.reverse || c.freeze {
            return None;
        }
        match c.content {
            ClipContent::Media { source_in, .. } => Some(ClipMap { start: c.start, source_in, span: c.source_span(), speed: c.speed }),
            _ => None,
        }
    }
    pub fn window(&self) -> TimeRange {
        TimeRange::new(self.source_in, self.source_in + self.span)
    }
    pub fn to_timeline(&self, src: Time) -> Time {
        self.start + scale_time(src - self.source_in, 1.0 / self.speed)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PauseRules {
    /// Pausas menores que isso são mantidas.
    pub min_silence: Time,
    /// Silêncio mantido antes e depois de cada fala.
    pub padding: Time,
    /// Não cortar pausas com ação visual (gameplay).
    pub preserve_action: bool,
    /// Movimento médio acima do qual a pausa é considerada "com ação".
    pub motion_threshold: f32,
}

impl Default for PauseRules {
    fn default() -> Self {
        PauseRules { min_silence: Time::from_millis(700), padding: Time::from_millis(150), preserve_action: false, motion_threshold: 0.02 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlannedCut {
    pub clip_id: Id,
    pub track_id: Id,
    /// Trecho na timeline (antes de aplicar).
    pub range: TimeRange,
    /// Trecho na mídia de origem.
    pub source: TimeRange,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PausePlan {
    pub cuts: Vec<PlannedCut>,
    pub removed: Time,
    pub kept_action: u32,
    pub skipped: Vec<String>,
}

fn mean_motion(a: &AnalysisResult, r: TimeRange) -> Option<f32> {
    let v = a.visual.as_ref()?;
    let s: Vec<f32> = v.samples.iter().filter(|x| r.contains(x.t)).map(|x| x.motion).collect();
    (!s.is_empty()).then(|| s.iter().sum::<f32>() / s.len() as f32)
}

/// Planeja a remoção de pausas nos clipes indicados (ou em todos os clipes de
/// mídia de trilhas desbloqueadas com análise de fala).
pub fn plan_pauses(p: &Project, analyses: &HashMap<Id, AnalysisResult>, clips: Option<&[Id]>, rules: &PauseRules) -> PausePlan {
    let fps = p.settings.fps;
    let mut plan = PausePlan { cuts: vec![], removed: Time::ZERO, kept_action: 0, skipped: vec![] };
    for t in &p.timeline.tracks {
        if t.kind == TrackKind::Text || t.locked {
            continue;
        }
        // Regiões de transição: cortes que as tocam são evitados.
        let guard: Vec<TimeRange> = p
            .timeline
            .transitions
            .iter()
            .filter(|tr| tr.track_id == t.id)
            .filter_map(|tr| t.clips.iter().find(|c| c.id == tr.to_clip).map(|b| TimeRange::new(b.start, b.start + tr.duration)))
            .collect();
        for c in &t.clips {
            if let Some(ids) = clips {
                if !ids.contains(&c.id) {
                    continue;
                }
            }
            let Some(asset_id) = c.asset_id() else { continue };
            let Some(m) = ClipMap::of(c) else {
                if clips.is_some() {
                    plan.skipped.push(format!("'{}': clipe invertido ou congelado", c.name));
                }
                continue;
            };
            let Some(a) = analyses.get(&asset_id) else {
                if clips.is_some() {
                    plan.skipped.push(format!("'{}': mídia ainda não analisada", c.name));
                }
                continue;
            };
            let Some(speech) = &a.speech else { continue };
            let w = m.window();
            // Pausas = complemento da fala dentro da janela do clipe.
            let mut gaps: Vec<(TimeRange, bool, bool)> = vec![]; // (gap, toca início, toca fim)
            let mut cursor = w.start;
            for s in speech.segments.iter().filter(|s| s.intersects(&w)) {
                if s.start > cursor {
                    gaps.push((TimeRange::new(cursor, s.start), cursor == w.start, false));
                }
                cursor = cursor.max(s.end);
            }
            if cursor < w.end {
                gaps.push((TimeRange::new(cursor, w.end), cursor == w.start, true));
            }
            for (g, at_start, at_end) in gaps {
                if g.duration() < rules.min_silence {
                    continue;
                }
                if rules.preserve_action {
                    if let Some(mm) = mean_motion(a, g) {
                        if mm >= rules.motion_threshold {
                            plan.kept_action += 1;
                            continue;
                        }
                    }
                }
                let s = if at_start { g.start } else { g.start + rules.padding };
                let e = if at_end { g.end } else { g.end - rules.padding };
                if e <= s {
                    continue;
                }
                let tr = TimeRange::new(m.to_timeline(s).snap_to_frame(fps), m.to_timeline(e).snap_to_frame(fps));
                if tr.duration() < fps.frame_duration() || guard.iter().any(|gr| gr.intersects(&tr)) {
                    continue;
                }
                // Não remover o clipe inteiro.
                if tr.start <= c.start && tr.end >= c.end() {
                    continue;
                }
                plan.removed += tr.duration();
                plan.cuts.push(PlannedCut { clip_id: c.id, track_id: t.id, range: tr, source: TimeRange::new(s, e) });
            }
        }
    }
    plan.cuts.sort_by_key(|c| c.range.start);
    plan
}

/// Comandos para aplicar o plano: do fim para o início (os trechos anteriores
/// não se deslocam), com ripple na trilha do clipe e nas `sync_tracks`
/// (ex.: legendas, que precisam acompanhar a fala).
pub fn pause_commands(plan: &PausePlan, sync_tracks: &[Id]) -> Vec<EditCommand> {
    let mut cuts = plan.cuts.clone();
    cuts.sort_by_key(|c| std::cmp::Reverse(c.range.start));
    cuts.into_iter()
        .map(|c| {
            let mut tracks = vec![c.track_id];
            tracks.extend(sync_tracks.iter().copied().filter(|t| *t != c.track_id));
            EditCommand::DeleteRange { start: c.range.start, end: c.range.end, track_ids: Some(tracks), ripple: true }
        })
        .collect()
}

/// Cortes de cena dentro do clipe, em tempo de timeline.
pub fn scene_times(c: &Clip, a: &AnalysisResult, fps: Rational) -> Vec<Time> {
    let (Some(m), Some(sc)) = (ClipMap::of(c), &a.scenes) else { return vec![] };
    let w = m.window();
    sc.cuts.iter().filter(|t| **t > w.start && **t < w.end).map(|t| m.to_timeline(*t).snap_to_frame(fps)).filter(|t| *t > c.start && *t < c.end()).collect()
}

/// Marcadores nos cortes de cena.
pub fn scene_marker_commands(c: &Clip, a: &AnalysisResult, fps: Rational) -> Vec<EditCommand> {
    scene_times(c, a, fps).into_iter().enumerate().map(|(i, at)| EditCommand::AddMarker { at, label: format!("Cena {}", i + 2) }).collect()
}

/// Divide o clipe em cada corte de cena (do fim para o início: o id original
/// continua sendo a parte esquerda).
pub fn scene_split_commands(c: &Clip, a: &AnalysisResult, fps: Rational) -> Vec<EditCommand> {
    let mut ts = scene_times(c, a, fps);
    ts.sort_by_key(|t| std::cmp::Reverse(*t));
    ts.into_iter().map(|at| EditCommand::SplitClip { clip_id: c.id, at }).collect()
}

/// Agrupa palavras em legendas legíveis (limite de caracteres e de duração,
/// quebra em pontuação ou pausas), mapeadas para a timeline do clipe.
pub fn caption_cues(t: &Transcript, c: &Clip, max_chars: usize, max_dur: Time) -> Vec<Cue> {
    let Some(m) = ClipMap::of(c) else { return vec![] };
    let w = m.window();
    let mut cues = vec![];
    let mut text = String::new();
    let mut start: Option<Time> = None;
    let mut last_end = Time::ZERO;
    let flush = |cues: &mut Vec<Cue>, text: &mut String, start: &mut Option<Time>, end: Time| {
        if let Some(s) = start.take() {
            let tx = text.trim().to_string();
            if !tx.is_empty() && end > s {
                cues.push(Cue { start: m.to_timeline(s), end: m.to_timeline(end), text: tx });
            }
        }
        text.clear();
    };
    let words: Vec<_> = t
        .segments
        .iter()
        .flat_map(|s| {
            if s.words.is_empty() {
                // Sem timestamps por palavra: usa o segmento inteiro.
                vec![(s.start, s.end, s.text.clone())]
            } else {
                s.words.iter().map(|w| (w.start, w.end, w.word.clone())).collect()
            }
        })
        .collect();
    for (ws, we, word) in words {
        if we <= w.start || ws >= w.end {
            continue;
        }
        let (ws, we) = (ws.max(w.start), we.min(w.end));
        let gap = start.is_some() && ws - last_end > Time::from_millis(600);
        let too_long = start.map(|s| we - s > max_dur).unwrap_or(false);
        let too_wide = text.chars().count() + word.chars().count() > max_chars;
        if gap || too_long || too_wide {
            flush(&mut cues, &mut text, &mut start, last_end);
        }
        if start.is_none() {
            start = Some(ws);
        }
        // faster-whisper já traz o espaço no início de cada palavra; segmentos inteiros não.
        if !text.is_empty() && !word.starts_with(' ') {
            text.push(' ');
        }
        text.push_str(&word);
        last_end = we;
        let ends_sentence = word.trim_end().ends_with(['.', '!', '?']);
        if ends_sentence && text.chars().count() > max_chars / 2 {
            flush(&mut cues, &mut text, &mut start, last_end);
        }
    }
    flush(&mut cues, &mut text, &mut start, last_end);
    cues
}
