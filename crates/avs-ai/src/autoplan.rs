//! "Editar automaticamente": plano completo a partir da análise e do preset.
//!
//! Executa sobre um `Project` (dentro de uma transação ou de uma cópia para
//! pré-visualização) usando somente `apply(EditCommand)`. Cada etapa é
//! registrada com sua justificativa. Regras:
//! - não inventa nada: só corta, reorganiza (cronologicamente) e trata o
//!   material existente;
//! - prioriza uma história coerente: une trechos próximos e mantém a ordem;
//! - não estica o material para atingir uma duração; avisa quando não há
//!   conteúdo suficiente.

use crate::analysis::AnalysisResult;
use crate::autoedit::{caption_cues, plan_pauses, ClipMap, PauseRules};
use crate::scoring::{candidate_units, score_units, ScoredUnit};
use avs_core::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use ts_rs::TS;

/// Regras do preset relevantes para a edição automática (montadas a partir
/// do `EditorialPreset` pela camada de serviço).
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PresetRules {
    pub name: String,
    pub remove_silence: bool,
    pub min_silence_ms: u32,
    pub padding_ms: u32,
    pub preserve_silent_action: bool,
    pub remove_fillers: bool,
    pub captions: bool,
    pub caption_y: f32,
    pub caption_size: f32,
    pub transitions: Vec<String>,
    pub transition_ms: u32,
    /// rare | moderate | frequent
    pub transition_frequency: String,
    pub effects: Vec<String>,
    /// off | subtle | moderate | intense
    pub effect_intensity: String,
    /// preserve | summarize | highlight_events | short_story | best_moments
    pub narrative: String,
    pub target_s: Option<f32>,
    pub max_s: Option<f32>,
    pub music_ducking_db: f32,
    pub noise_reduction: bool,
    pub voice_priority: bool,
    pub scoring_weights: BTreeMap<String, f32>,
    pub safe_bottom_pct: f32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CutIntensity {
    Conservative,
    Balanced,
    Aggressive,
}

/// Escolhas de estilo do usuário (seção 8); `None` = usar o preset.
#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AutoEditOptions {
    pub cuts: CutIntensity,
    /// off | subtle | moderate | intense
    pub effects: Option<String>,
    pub narrative: Option<String>,
    pub target_duration: Option<Time>,
    pub remove_pauses: Option<bool>,
    pub remove_fillers: Option<bool>,
    pub captions: Option<bool>,
    pub transitions: bool,
    pub zoom: bool,
    pub audio_treatment: bool,
    pub ducking: bool,
    pub color_correction: bool,
}

impl Default for AutoEditOptions {
    fn default() -> Self {
        AutoEditOptions {
            cuts: CutIntensity::Balanced,
            effects: None,
            narrative: None,
            target_duration: None,
            remove_pauses: None,
            remove_fillers: None,
            captions: None,
            transitions: true,
            zoom: true,
            audio_treatment: true,
            ducking: true,
            color_correction: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct PlanStep {
    /// captions | fillers | pauses | selection | transitions | zoom | audio | ducking | color
    pub kind: String,
    pub title: String,
    pub detail: String,
    pub count: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SelectedMoment {
    /// Trecho na timeline original.
    pub range: TimeRange,
    pub score: f32,
    pub explanation: String,
    pub kept: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AutoEditReport {
    pub preset: String,
    pub before: Time,
    pub after: Time,
    pub steps: Vec<PlanStep>,
    /// Trechos removidos, na timeline original (para pré-visualizar).
    pub removed: Vec<TimeRange>,
    pub moments: Vec<SelectedMoment>,
    pub warnings: Vec<String>,
}

const FILLERS: &[&str] = &["hã", "ãh", "hum", "humm", "hmm", "éé", "ééé", "ahn", "ah", "eh", "uh", "um", "uhm", "erm", "eee", "mmm"];

fn is_filler(w: &str) -> bool {
    let t: String = w.trim().trim_matches(|c: char| !c.is_alphanumeric()).to_lowercase();
    !t.is_empty() && FILLERS.contains(&t.as_str())
}

fn union(mut v: Vec<TimeRange>) -> Vec<TimeRange> {
    v.sort_by_key(|r| r.start);
    let mut out: Vec<TimeRange> = vec![];
    for r in v {
        match out.last_mut() {
            Some(l) if r.start <= l.end => l.end = l.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

fn ap(p: &mut Project, c: EditCommand) -> Result<Vec<Id>, CommandError> {
    apply(p, &c).map(|o| o.created)
}

/// Clipes de mídia "principais": a primeira trilha de vídeo com clipes (ou a
/// primeira de áudio, para projetos só de áudio).
fn main_track(p: &Project) -> Option<Id> {
    p.timeline
        .tracks
        .iter()
        .find(|t| t.kind == TrackKind::Video && !t.locked && t.clips.iter().any(|c| c.asset_id().is_some()))
        .or_else(|| p.timeline.tracks.iter().find(|t| t.kind == TrackKind::Audio && !t.locked && !t.clips.is_empty()))
        .map(|t| t.id)
}

fn is_dialogue(a: Option<&AnalysisResult>) -> bool {
    a.and_then(|a| a.speech.as_ref()).map(|s| s.ratio >= 0.2).unwrap_or(false)
}

pub fn run(p: &mut Project, rules: &PresetRules, analyses: &HashMap<Id, AnalysisResult>, opts: &AutoEditOptions) -> Result<AutoEditReport, CommandError> {
    let fps = p.settings.fps;
    let before = p.timeline.duration();
    let mut rep = AutoEditReport { preset: rules.name.clone(), before, after: before, steps: vec![], removed: vec![], moments: vec![], warnings: vec![] };
    let Some(main) = main_track(p) else {
        return Err(CommandError::Invalid("a timeline não tem clipes de mídia para editar".into()));
    };
    let analysis_of = |c: &Clip| c.asset_id().and_then(|id| analyses.get(&id));
    let main_clips: Vec<Clip> = p.timeline.track(main).unwrap().clips.iter().filter(|c| c.asset_id().is_some()).cloned().collect();
    if main_clips.iter().all(|c| analysis_of(c).is_none()) {
        return Err(CommandError::Invalid("as mídias da timeline ainda não foram analisadas".into()));
    }
    let effect_level = opts.effects.clone().unwrap_or_else(|| rules.effect_intensity.clone());
    let narrative = opts.narrative.clone().unwrap_or_else(|| rules.narrative.clone());

    // ── 1. Legendas (antes dos cortes: as trilhas de texto acompanham) ──
    if opts.captions.unwrap_or(rules.captions) {
        let mut n = 0u32;
        let mut cues_all = vec![];
        for c in &main_clips {
            if let Some(t) = analysis_of(c).and_then(|a| a.transcript.as_ref()) {
                cues_all.extend(caption_cues(t, c, 42, Time::from_secs(4)));
            }
        }
        if cues_all.is_empty() {
            rep.warnings.push("Legendas não geradas: não há transcrição (instale a transcrição e analise com 'Transcrever fala').".into());
        } else {
            n = cues_all.len() as u32;
            let mut style = subtitles::caption_style();
            style.y = rules.caption_y;
            style.size = rules.caption_size;
            ap(p, EditCommand::AddSubtitleTrack { name: Some("Legendas automáticas".into()), cues: cues_all, style: Some(style) })?;
        }
        if n > 0 {
            rep.steps.push(PlanStep {
                kind: "captions".into(),
                title: "Legendas".into(),
                detail: format!("{n} legendas sincronizadas pela transcrição, dentro da área segura"),
                count: n,
            });
        }
    }

    // ── 2–4. Trechos a remover (na timeline original) ──
    let mut remove: Vec<(Id, TimeRange)> = vec![];
    // guarda de transições
    let guard: Vec<TimeRange> = p
        .timeline
        .transitions
        .iter()
        .filter(|tr| tr.track_id == main)
        .filter_map(|tr| p.timeline.clip(tr.to_clip).map(|b| TimeRange::new(b.start, b.start + tr.duration)))
        .collect();

    // Palavras de preenchimento (só com transcrição)
    if opts.remove_fillers.unwrap_or(rules.remove_fillers) {
        let mut n = 0u32;
        for c in &main_clips {
            let (Some(m), Some(t)) = (ClipMap::of(c), analysis_of(c).and_then(|a| a.transcript.as_ref())) else { continue };
            for w in t.segments.iter().flat_map(|s| s.words.iter()).filter(|w| is_filler(&w.word)) {
                if !m.window().contains(w.start) {
                    continue;
                }
                let r = TimeRange::new(m.to_timeline(w.start).snap_to_frame(fps), m.to_timeline(w.end.min(m.window().end)).snap_to_frame(fps));
                if r.duration() >= fps.frame_duration() && !guard.iter().any(|g| g.intersects(&r)) {
                    remove.push((c.id, r));
                    n += 1;
                }
            }
        }
        if n > 0 {
            rep.steps.push(PlanStep {
                kind: "fillers".into(),
                title: "Palavras de preenchimento".into(),
                detail: format!("{n} hesitações (\"hã\", \"hum\"…) removidas"),
                count: n,
            });
        }
    }

    // Pausas
    if opts.remove_pauses.unwrap_or(rules.remove_silence) {
        let k = match opts.cuts {
            CutIntensity::Conservative => 1.6,
            CutIntensity::Balanced => 1.0,
            CutIntensity::Aggressive => 0.6,
        };
        let pr = PauseRules {
            min_silence: Time::from_millis((rules.min_silence_ms as f64 * k) as i64),
            padding: Time::from_millis((rules.padding_ms as f64 * k.min(1.0)) as i64),
            preserve_action: rules.preserve_silent_action,
            ..Default::default()
        };
        let ids: Vec<Id> = main_clips.iter().map(|c| c.id).collect();
        let plan = plan_pauses(p, analyses, Some(&ids), &pr);
        let n = plan.cuts.len() as u32;
        for c in &plan.cuts {
            remove.push((c.clip_id, c.range));
        }
        let mut detail =
            format!("{n} pausas maiores que {:.1} s removidas, mantendo {:.2} s em volta da fala", pr.min_silence.as_secs_f64(), pr.padding.as_secs_f64());
        if plan.kept_action > 0 {
            detail.push_str(&format!("; {} pausa(s) mantida(s) por terem ação na imagem", plan.kept_action));
        }
        if n > 0 || plan.kept_action > 0 {
            rep.steps.push(PlanStep { kind: "pauses".into(), title: "Pausas".into(), detail, count: n });
        }
    }

    // Seleção narrativa
    let target = opts.target_duration.or(rules.target_s.map(|s| Time::from_secs_f64(s as f64)));
    let max = rules.max_s.map(|s| Time::from_secs_f64(s as f64));
    let removed_so_far: Time = union(remove.iter().map(|(_, r)| *r).collect()).iter().fold(Time::ZERO, |a, r| a + r.duration());
    let remaining = before - removed_so_far;
    let goal = match (target, max) {
        (Some(t), Some(m)) => Some(t.min(m)),
        (Some(t), None) => Some(t),
        (None, Some(m)) => Some(m),
        (None, None) => None,
    };
    let wants_selection = narrative != "preserve" && goal.map(|g| remaining > g + Time::from_secs(2)).unwrap_or(false);
    if wants_selection || narrative != "preserve" {
        // Pontuação de todas as unidades (também usada pelo zoom e no relatório).
        let mut scored: Vec<(Clip, ScoredUnit)> = vec![];
        for c in &main_clips {
            let (Some(m), Some(a)) = (ClipMap::of(c), analysis_of(c)) else { continue };
            for u in score_units(a, &candidate_units(a, m.window()), &rules.scoring_weights) {
                scored.push((c.clone(), u));
            }
        }
        if wants_selection && !scored.is_empty() {
            let goal = goal.unwrap();
            let tl = |c: &Clip, r: &TimeRange| {
                let m = ClipMap::of(c).unwrap();
                TimeRange::new(m.to_timeline(r.start).snap_to_frame(fps), m.to_timeline(r.end).snap_to_frame(fps))
            };
            let already = union(remove.iter().map(|(_, r)| *r).collect());
            let net = |r: &TimeRange| r.duration() - already.iter().filter_map(|x| x.intersection(r)).fold(Time::ZERO, |a, x| a + x.duration());
            let mut order: Vec<usize> = (0..scored.len()).collect();
            if narrative == "summarize" {
                // Espalha pela gravação: melhor trecho de cada parte, depois o resto.
                let parts = 4usize.min(scored.len()).max(1);
                let span = before.flicks() / parts as i64 + 1;
                let mut firsts = vec![];
                for part in 0..parts {
                    if let Some(i) = (0..scored.len())
                        .filter(|&i| tl(&scored[i].0, &scored[i].1.source).start.flicks() / span == part as i64)
                        .max_by(|&a, &b| scored[a].1.score.partial_cmp(&scored[b].1.score).unwrap())
                    {
                        firsts.push(i);
                    }
                }
                order.sort_by(|&a, &b| scored[b].1.score.partial_cmp(&scored[a].1.score).unwrap());
                order.retain(|i| !firsts.contains(i));
                firsts.extend(order);
                order = firsts;
            } else {
                order.sort_by(|&a, &b| scored[b].1.score.partial_cmp(&scored[a].1.score).unwrap());
            }
            let mut keep: Vec<usize> = vec![];
            let mut total = Time::ZERO;
            for i in order {
                let r = tl(&scored[i].0, &scored[i].1.source);
                let d = net(&r);
                if total + d > goal && !keep.is_empty() {
                    continue;
                }
                keep.push(i);
                total += d;
                if total >= goal - Time::from_secs(3) {
                    break;
                }
            }
            if total < goal - Time::from_secs(5) && keep.len() == scored.len() {
                rep.warnings.push(format!(
                    "Material insuficiente para {:.0} s: o resultado terá {:.0} s (nada é esticado artificialmente).",
                    goal.as_secs_f64(),
                    total.as_secs_f64()
                ));
            }
            // Coerência: trechos mantidos separados por < 1,5 s se unem.
            let mut kept_tl: Vec<(Id, TimeRange)> = keep.iter().map(|&i| (scored[i].0.id, tl(&scored[i].0, &scored[i].1.source))).collect();
            kept_tl.sort_by_key(|(_, r)| r.start);
            let mut merged: Vec<(Id, TimeRange)> = vec![];
            for (cid, r) in kept_tl {
                match merged.last_mut() {
                    Some((lc, l)) if *lc == cid && r.start - l.end < Time::from_millis(1500) => l.end = l.end.max(r.end),
                    _ => merged.push((cid, r)),
                }
            }
            // Remove o complemento dentro de cada clipe principal.
            let mut n = 0u32;
            for c in &main_clips {
                let window = c.range();
                let mut cursor = window.start;
                for (_, r) in merged.iter().filter(|(cid, _)| *cid == c.id) {
                    if r.start > cursor {
                        remove.push((c.id, TimeRange::new(cursor, r.start)));
                        n += 1;
                    }
                    cursor = cursor.max(r.end);
                }
                if cursor < window.end && ClipMap::of(c).is_some() && analysis_of(c).is_some() {
                    if cursor == window.start {
                        // Nada mantido deste clipe: remove inteiro (pela faixa).
                        remove.push((c.id, window));
                    } else {
                        remove.push((c.id, TimeRange::new(cursor, window.end)));
                    }
                    n += 1;
                }
            }
            let mode_label = match narrative.as_str() {
                "summarize" => "resumo distribuído pela gravação",
                "highlight_events" => "acontecimentos de destaque",
                "short_story" => "história curta",
                "best_moments" => "melhores momentos",
                _ => "seleção",
            };
            rep.steps.push(PlanStep {
                kind: "selection".into(),
                title: "Seleção de trechos".into(),
                detail: format!("{} de {} trechos mantidos ({mode_label}, alvo {:.0} s), em ordem cronológica", keep.len(), scored.len(), goal.as_secs_f64()),
                count: n,
            });
            for (i, (c, u)) in scored.iter().enumerate() {
                rep.moments.push(SelectedMoment { range: tl(c, &u.source), score: u.score, explanation: u.explanation.clone(), kept: keep.contains(&i) });
            }
        } else {
            for (c, u) in &scored {
                if let Some(m) = ClipMap::of(c) {
                    rep.moments.push(SelectedMoment {
                        range: TimeRange::new(m.to_timeline(u.source.start), m.to_timeline(u.source.end)),
                        score: u.score,
                        explanation: u.explanation.clone(),
                        kept: true,
                    });
                }
            }
        }
        rep.moments.sort_by_key(|m| m.range.start);
    }

    // Aplica as remoções: união, do fim para o início, com ripple na trilha
    // principal e nas trilhas de texto (legendas acompanham).
    let ranges = union(remove.iter().map(|(_, r)| *r).filter(|r| !guard.iter().any(|g| g.intersects(r))).collect());
    let text_tracks: Vec<Id> = p.timeline.tracks.iter().filter(|t| t.kind == TrackKind::Text && !t.locked).map(|t| t.id).collect();
    let mut tracks = vec![main];
    tracks.extend(text_tracks.iter().copied());
    for r in ranges.iter().rev() {
        let r = TimeRange::new(r.start.snap_to_frame(fps), r.end.snap_to_frame(fps));
        if r.duration().is_positive() {
            ap(p, EditCommand::DeleteRange { start: r.start, end: r.end, track_ids: Some(tracks.clone()), ripple: true })?;
        }
    }
    rep.removed = ranges;

    // Trilha sonora (áudio sem fala em outras trilhas) termina com o vídeo, com fade.
    if !rep.removed.is_empty() {
        let main_end = p.timeline.track(main).unwrap().clips.iter().map(|c| c.end()).max().unwrap_or(Time::ZERO).snap_to_frame(fps);
        let music_tracks: Vec<Id> = p
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio && t.id != main && !t.locked && t.clips.iter().any(|c| c.end() > main_end && !is_dialogue(analysis_of(c))))
            .map(|t| t.id)
            .collect();
        let end = p.timeline.duration().snap_to_frame(fps);
        if !music_tracks.is_empty() && main_end.is_positive() && end > main_end {
            ap(p, EditCommand::DeleteRange { start: main_end, end, track_ids: Some(music_tracks.clone()), ripple: false })?;
            let lasts: Vec<(Id, Time)> = music_tracks
                .iter()
                .filter_map(|t| p.timeline.track(*t).and_then(|t| t.clips.iter().filter(|c| c.end() == main_end).map(|c| (c.id, c.duration)).next()))
                .collect();
            for (id, d) in lasts {
                let f = Time::from_millis(1500).min(d / 2).snap_to_frame(fps);
                ap(p, EditCommand::UpdateClip { clip_id: id, patch: ClipPatch { fade_out: Some(f), ..Default::default() } })?;
            }
            rep.steps.push(PlanStep {
                kind: "music_end".into(),
                title: "Final da trilha sonora".into(),
                detail: "música cortada no fim do vídeo, com fade de saída".into(),
                count: music_tracks.len() as u32,
            });
        }
    }

    // ── 5. Transições entre os clipes resultantes ──
    let kind = rules.transitions.iter().find_map(|t| match t.as_str() {
        "crossfade" => Some(TransitionKind::Crossfade),
        "dissolve" => Some(TransitionKind::Dissolve),
        "fadeblack" | "fade_black" => Some(TransitionKind::FadeBlack),
        "wipeleft" | "wipe_left" => Some(TransitionKind::WipeLeft),
        "slideleft" | "slide_left" => Some(TransitionKind::SlideLeft),
        _ => None,
    });
    if opts.transitions && rules.transition_frequency != "rare" {
        if let Some(kind) = kind {
            let dur = Time::from_millis(rules.transition_ms.max(100) as i64).snap_to_frame(fps);
            let clips: Vec<Clip> = p.timeline.track(main).unwrap().clips.clone();
            let mut n = 0u32;
            for w in clips.windows(2) {
                let (a, b) = (&w[0], &w[1]);
                if a.end() != b.start || a.duration < dur * 3 || b.duration < dur * 3 || p.timeline.transition_between(a.id, b.id).is_some() {
                    continue;
                }
                // "moderate": só onde houve salto de conteúdo (≥ 2 s de origem) ou troca de mídia.
                let jump = a.asset_id() != b.asset_id() || (b.source_in() - (a.source_in() + a.source_span())).as_secs_f64().abs() >= 2.0;
                if rules.transition_frequency == "moderate" && !jump {
                    continue;
                }
                if ap(p, EditCommand::AddTransition { from_clip: a.id, to_clip: b.id, kind, duration: dur }).is_ok() {
                    n += 1;
                }
            }
            if n > 0 {
                rep.steps.push(PlanStep {
                    kind: "transitions".into(),
                    title: "Transições".into(),
                    detail: format!("{n} transições discretas ({} ms) nos saltos de conteúdo", rules.transition_ms),
                    count: n,
                });
            }
        }
    }

    // ── 6. Zoom pontual nos picos de ação/reação ──
    let zoom_level = match effect_level.as_str() {
        "subtle" => Some(1.08),
        "moderate" => Some(1.15),
        "intense" => Some(1.25),
        _ => None,
    };
    if opts.zoom && rules.effects.iter().any(|e| e == "zoom") && p.timeline.track(main).map(|t| t.kind == TrackKind::Video).unwrap_or(false) {
        if let Some(z) = zoom_level {
            let clips: Vec<Clip> = p.timeline.track(main).unwrap().clips.clone();
            let mut peaks: Vec<(f32, Id, Time)> = vec![];
            for c in &clips {
                let (Some(m), Some(a)) = (ClipMap::of(c), analysis_of(c)) else { continue };
                let Some(v) = &a.visual else { continue };
                // Pico de movimento (com reforço de pico de volume) dentro do clipe.
                for s in v.samples.iter().filter(|s| m.window().contains(s.t)) {
                    let rel = m.to_timeline(s.t) - c.start;
                    if rel > Time::from_millis(700) && rel + Time::from_millis(1900) < c.duration && c.keyframes.scale.is_empty() && c.transform.scale == 1.0 {
                        peaks.push((s.motion, c.id, rel));
                    }
                }
            }
            peaks.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap());
            let per_min = match effect_level.as_str() {
                "subtle" => 1.0,
                "moderate" => 2.0,
                _ => 3.0,
            };
            let max_n = ((p.timeline.duration().as_secs_f64() / 60.0 * per_min).ceil() as usize).clamp(1, 12);
            let median = peaks.get(peaks.len() / 2).map(|x| x.0).unwrap_or(0.0);
            let mut used: Vec<(Id, Time)> = vec![];
            let mut n = 0u32;
            for (mo, cid, rel) in peaks {
                if n as usize >= max_n || mo <= median * 1.5 || mo < 0.01 {
                    break;
                }
                if used.iter().any(|(c, t)| *c == cid && (*t - rel).as_secs_f64().abs() < 4.0) {
                    continue;
                }
                let at = |x: i64| rel + Time::from_millis(x);
                for (t, v, e) in
                    [(at(-600), 1.0, Easing::EaseInOut), (at(0), z, Easing::Hold), (at(1200), z, Easing::EaseInOut), (at(1800), 1.0, Easing::Linear)]
                {
                    ap(p, EditCommand::SetKeyframe { clip_id: cid, property: KeyframeProperty::Scale, at: t, value: v, easing: Some(e) })?;
                }
                used.push((cid, rel));
                n += 1;
            }
            if n > 0 {
                rep.steps.push(PlanStep {
                    kind: "zoom".into(),
                    title: "Zoom pontual".into(),
                    detail: format!("{n} zoom(s) de {:.0}% nos picos de movimento", (z - 1.0) * 100.0),
                    count: n,
                });
            }
        }
    }

    // ── 7. Tratamento de áudio da fala ──
    if opts.audio_treatment && (rules.noise_reduction || rules.voice_priority) {
        let ids: Vec<Id> = p.timeline.track(main).unwrap().clips.iter().filter(|c| is_dialogue(analysis_of(c))).map(|c| c.id).collect();
        for id in &ids {
            let mut fx = p.timeline.clip(*id).unwrap().audio_fx.clone();
            fx.highpass |= rules.noise_reduction;
            fx.denoise |= rules.noise_reduction;
            fx.compressor |= rules.voice_priority;
            ap(p, EditCommand::UpdateClip { clip_id: *id, patch: ClipPatch { audio_fx: Some(fx), ..Default::default() } })?;
        }
        if !ids.is_empty() {
            let mut parts = vec![];
            if rules.noise_reduction {
                parts.push("corte de graves e redução de ruído");
            }
            if rules.voice_priority {
                parts.push("compressor de voz");
            }
            rep.steps.push(PlanStep {
                kind: "audio".into(),
                title: "Áudio da fala".into(),
                detail: format!("{} em {} clipe(s)", parts.join(" + "), ids.len()),
                count: ids.len() as u32,
            });
        }
    }

    // ── 8. Ducking da música sob a fala ──
    if opts.ducking && rules.music_ducking_db < 0.0 {
        // Fala na timeline atual, a partir dos clipes de diálogo.
        let mut speech_tl: Vec<TimeRange> = vec![];
        for c in &p.timeline.track(main).unwrap().clips {
            let (Some(m), Some(a)) = (ClipMap::of(c), analysis_of(c)) else { continue };
            if let Some(sp) = &a.speech {
                for s in sp.segments.iter().filter_map(|s| s.intersection(&m.window())) {
                    speech_tl.push(TimeRange::new(m.to_timeline(s.start), m.to_timeline(s.end)));
                }
            }
        }
        let mut merged: Vec<TimeRange> = vec![];
        speech_tl.sort_by_key(|r| r.start);
        for r in speech_tl {
            match merged.last_mut() {
                Some(l) if r.start - l.end < Time::from_millis(800) => l.end = l.end.max(r.end),
                _ => merged.push(r),
            }
        }
        let duck = 10f32.powf(rules.music_ducking_db / 20.0);
        let music: Vec<Clip> = p
            .timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Audio && t.id != main && !t.locked)
            .flat_map(|t| t.clips.iter())
            .filter(|c| c.keyframes.volume.is_empty() && !is_dialogue(analysis_of(c)))
            .cloned()
            .collect();
        let mut n = 0u32;
        if !merged.is_empty() {
            for mc in &music {
                let mut kfs: Vec<(Time, f32)> = vec![];
                for r in merged.iter().filter_map(|r| r.intersection(&TimeRange::new(mc.start - Time::from_secs(1), mc.end() + Time::from_secs(1)))) {
                    let rel = |t: Time| (t - mc.start).max(Time::ZERO).min(mc.duration);
                    kfs.push((rel(r.start - Time::from_millis(250)), 1.0));
                    kfs.push((rel(r.start), duck));
                    kfs.push((rel(r.end), duck));
                    kfs.push((rel(r.end + Time::from_millis(350)), 1.0));
                }
                kfs.dedup_by_key(|k| k.0.snap_to_frame(fps));
                for (t, v) in kfs.into_iter().take(400) {
                    ap(p, EditCommand::SetKeyframe { clip_id: mc.id, property: KeyframeProperty::Volume, at: t, value: v, easing: Some(Easing::Linear) })?;
                }
                n += 1;
            }
        }
        if n > 0 {
            rep.steps.push(PlanStep {
                kind: "ducking".into(),
                title: "Música sob a fala".into(),
                detail: format!("volume da música reduzido {:.0} dB durante a fala em {n} clipe(s)", -rules.music_ducking_db),
                count: n,
            });
        }
    }

    // ── 9. Correção básica de exposição ──
    if opts.color_correction && rules.effects.iter().any(|e| e == "color_correction" || e == "color_grade") {
        let mut n = 0u32;
        let clips: Vec<Clip> = p.timeline.track(main).unwrap().clips.clone();
        for c in &clips {
            let Some(v) = analysis_of(c).and_then(|a| a.visual.as_ref()) else { continue };
            if v.samples.is_empty() || !c.color.is_neutral() {
                continue;
            }
            let luma = v.samples.iter().map(|s| s.luma).sum::<f32>() / v.samples.len() as f32;
            if luma < 0.3 {
                let b = ((0.4 - luma) * 0.5).min(0.12);
                ap(
                    p,
                    EditCommand::UpdateClip {
                        clip_id: c.id,
                        patch: ClipPatch { color: Some(ColorAdjust { brightness: b, contrast: 1.05, saturation: 1.0 }), ..Default::default() },
                    },
                )?;
                n += 1;
            }
        }
        if n > 0 {
            rep.steps.push(PlanStep {
                kind: "color".into(),
                title: "Exposição".into(),
                detail: format!("{n} clipe(s) escuro(s) clareado(s) levemente"),
                count: n,
            });
        }
    }

    rep.after = p.timeline.duration();
    if let Some(m) = max {
        if rep.after > m {
            rep.warnings.push(format!(
                "O resultado ({:.0} s) excede o máximo do preset ({:.0} s); use a duração-alvo ou o estilo 'Agressivo'.",
                rep.after.as_secs_f64(),
                m.as_secs_f64()
            ));
        }
    }
    if rep.steps.is_empty() {
        rep.warnings.push("Nenhuma alteração necessária com este preset e estas opções.".into());
    }
    Ok(rep)
}
