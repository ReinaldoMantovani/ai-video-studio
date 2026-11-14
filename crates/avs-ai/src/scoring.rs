//! Pontuação editorial de trechos (0–100).
//!
//! **Heurística editorial, não previsão de visualizações.** Cada trecho
//! candidato recebe medidas objetivas extraídas da análise (fala, volume,
//! movimento, rostos, cortes de cena, transcrição). Os pesos do preset
//! (`scoring_weights`) combinam essas medidas. Os pesos são parâmetros
//! iniciais de engenharia, a calibrar com uso real.
//! Toda pontuação vem acompanhada de uma explicação legível.

use crate::analysis::{AnalysisResult, VisualSample};
use avs_core::{Time, TimeRange};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ScoredUnit {
    /// Trecho na mídia de origem.
    pub source: TimeRange,
    pub score: f32,
    /// Medida (0..1) por fator.
    pub factors: BTreeMap<String, f32>,
    pub explanation: String,
}

/// Rótulos legíveis dos fatores.
pub fn factor_label(k: &str) -> &'static str {
    match k {
        "semantic_relevance" => "conteúdo relevante",
        "hook_strength" => "início forte",
        "self_containment" => "começo e fim completos",
        "expressiveness" => "expressividade",
        "pacing" => "ritmo de fala",
        "technical_quality" => "boa qualidade técnica",
        "visual_action" => "ação visual intensa",
        "reaction" => "reação marcante",
        "event_sequence" => "sequência de acontecimentos",
        "audio_intensity" => "áudio intenso",
        "humor_signal" => "possível momento de humor",
        "clarity" => "fala clara",
        _ => "outro fator",
    }
}

pub fn default_weights() -> BTreeMap<String, f32> {
    [("semantic_relevance", 0.25), ("hook_strength", 0.2), ("self_containment", 0.2), ("expressiveness", 0.15), ("pacing", 0.1), ("technical_quality", 0.1)]
        .into_iter()
        .map(|(k, v)| (k.to_string(), v))
        .collect()
}

fn overlap(a: &TimeRange, b: &TimeRange) -> f64 {
    a.intersection(b).map(|r| r.duration().as_secs_f64()).unwrap_or(0.0)
}

/// Divide a mídia em unidades candidatas: falas agrupadas (pausas < 1,2 s
/// unem frases) ou, com pouca fala, planos de cena / janelas de 6 s.
pub fn candidate_units(a: &AnalysisResult, window: TimeRange) -> Vec<TimeRange> {
    let speech: Vec<TimeRange> = a.speech.as_ref().map(|s| s.segments.iter().filter_map(|x| x.intersection(&window)).collect()).unwrap_or_default();
    let speech_ratio = speech.iter().map(|s| s.duration().as_secs_f64()).sum::<f64>() / window.duration().as_secs_f64().max(1e-6);
    let mut units: Vec<TimeRange> = vec![];
    if speech_ratio >= 0.15 {
        for s in speech {
            match units.last_mut() {
                Some(u) if s.start - u.end < Time::from_millis(1200) && (s.end - u.start) < Time::from_secs(30) => u.end = s.end,
                _ => units.push(s),
            }
        }
    } else {
        let mut cuts: Vec<Time> = a.scenes.as_ref().map(|s| s.cuts.iter().copied().filter(|t| window.contains(*t)).collect()).unwrap_or_default();
        cuts.insert(0, window.start);
        cuts.push(window.end);
        for w in cuts.windows(2) {
            let (mut s, e) = (w[0], w[1]);
            // Planos longos viram janelas de até 8 s.
            while e - s > Time::from_secs(12) {
                units.push(TimeRange::new(s, s + Time::from_secs(8)));
                s += Time::from_secs(8);
            }
            if e > s {
                units.push(TimeRange::new(s, e));
            }
        }
        // Planos muito curtos se unem ao anterior.
        let mut merged: Vec<TimeRange> = vec![];
        for u in units {
            match merged.last_mut() {
                Some(m) if u.duration() < Time::from_secs(2) => m.end = u.end,
                _ => merged.push(u),
            }
        }
        units = merged;
    }
    units.retain(|u| u.duration() >= Time::from_millis(800));
    units
}

fn rank(v: f32, all: &[f32]) -> f32 {
    if all.len() < 2 {
        return 0.5;
    }
    all.iter().filter(|x| **x < v).count() as f32 / (all.len() - 1) as f32
}

fn mean(v: &[f32]) -> f32 {
    if v.is_empty() {
        0.0
    } else {
        v.iter().sum::<f32>() / v.len() as f32
    }
}

struct Raw {
    speech_density: f32,
    loud_mean: f32,
    loud_var: f32,
    loud_spike: f32,
    start_intensity: f32,
    motion: f32,
    motion_var: f32,
    faces: f32,
    luma: f32,
    scene_events: f32,
    words_per_s: Option<f32>,
    punct: f32,
    ends_sentence: bool,
    boundary_pauses: f32,
    nonspeech_noise: f32,
}

/// Fatia de uma série ordenada por tempo que cai em `[start, end)` (busca binária).
fn slice_by_time<T>(v: &[T], key: impl Fn(&T) -> Time, start: Time, end: Time) -> &[T] {
    let i0 = v.partition_point(|x| key(x) < start);
    let i1 = v.partition_point(|x| key(x) < end);
    &v[i0..i1.max(i0)]
}

fn raw(a: &AnalysisResult, u: &TimeRange) -> Raw {
    let dur = u.duration().as_secs_f64().max(1e-6);
    // Só os trechos de fala perto da unidade (a lista é ordenada): em gravações
    // longas, percorrer tudo para cada unidade era O(unidades × amostras).
    let all_speech: &[TimeRange] = a.speech.as_ref().map(|s| s.segments.as_slice()).unwrap_or(&[]);
    let margin = Time::from_millis(400);
    let first = all_speech.partition_point(|s| s.end <= u.start - margin);
    let last = all_speech.partition_point(|s| s.start < u.end + margin);
    let speech: &[TimeRange] = &all_speech[first..last.max(first)];
    let speech_s: f64 = speech.iter().map(|s| overlap(s, u)).sum();
    let (mut loud, mut nonspeech) = (vec![], vec![]);
    if let Some(l) = &a.loudness {
        let step = l.interval.flicks().max(1);
        // Índices das amostras com t = i·passo dentro de [início, fim).
        let i0 = ((u.start.flicks() + step - 1).div_euclid(step)).max(0) as usize;
        let i1 = ((u.end.flicks() + step - 1).div_euclid(step)).max(0) as usize;
        let mut si = 0usize;
        for i in i0..i1.min(l.rms_db.len()) {
            let t = Time::from_flicks(step * i as i64);
            let db = l.rms_db[i];
            loud.push(db);
            while si < speech.len() && speech[si].end <= t {
                si += 1;
            }
            let in_speech = si < speech.len() && speech[si].contains(t);
            if !in_speech {
                nonspeech.push(db);
            }
        }
    }
    let lm = mean(&loud);
    let lvar = mean(&loud.iter().map(|x| (x - lm).powi(2)).collect::<Vec<_>>()).sqrt();
    let spike = loud.iter().copied().fold(-90.0f32, f32::max) - lm;
    let start_n = loud.len().min(30);
    let start_int = mean(&loud[..start_n]);
    let samples: &[VisualSample] = a.visual.as_ref().map(|v| slice_by_time(&v.samples, |s| s.t, u.start, u.end)).unwrap_or(&[]);
    let motion: Vec<f32> = samples.iter().map(|s| s.motion).collect();
    let mm = mean(&motion);
    let mvar = mean(&motion.iter().map(|x| (x - mm).powi(2)).collect::<Vec<_>>()).sqrt();
    let faces = if samples.is_empty() { 0.0 } else { samples.iter().filter(|s| !s.faces.is_empty()).count() as f32 / samples.len() as f32 };
    let luma = if samples.is_empty() { 0.5 } else { mean(&samples.iter().map(|s| s.luma).collect::<Vec<_>>()) };
    let cuts = a.scenes.as_ref().map(|s| slice_by_time(&s.cuts, |t| *t, u.start, u.end).len()).unwrap_or(0);
    let (mut words, mut punct, mut ends_sentence) = (None, 0.0, false);
    if let Some(t) = &a.transcript {
        let segs = &t.segments[t.segments.partition_point(|s| s.end <= u.start)..];
        let ws: Vec<_> = segs.iter().take_while(|s| s.start < u.end).flat_map(|s| s.words.iter()).filter(|w| u.contains(w.start)).collect();
        if !ws.is_empty() {
            words = Some(ws.len() as f32 / dur as f32);
            let marks = ws.iter().filter(|w| w.word.contains(['?', '!'])).count();
            punct = (marks as f32 / 2.0).min(1.0);
            ends_sentence = ws.last().map(|w| w.word.trim_end().ends_with(['.', '!', '?'])).unwrap_or(false);
        }
    }
    // Começa e termina em pausa (≥ 0,3 s sem fala ao redor das bordas)?
    let near = |t: Time| speech.iter().any(|s| s.start < t + Time::from_millis(300) && s.end > t - Time::from_millis(300) && !(s.start >= t || s.end <= t));
    let boundary = (if near(u.start) { 0.0 } else { 0.5 }) + (if near(u.end) { 0.0 } else { 0.5 });
    Raw {
        speech_density: (speech_s / dur) as f32,
        loud_mean: lm,
        loud_var: lvar,
        loud_spike: spike,
        start_intensity: start_int,
        motion: mm,
        motion_var: mvar,
        faces,
        luma,
        scene_events: cuts as f32 / dur as f32 * 10.0,
        words_per_s: words,
        punct,
        ends_sentence,
        boundary_pauses: boundary,
        nonspeech_noise: mean(&nonspeech),
    }
}

/// Pontua todas as unidades de uma mídia com os pesos do preset.
pub fn score_units(a: &AnalysisResult, units: &[TimeRange], weights: &BTreeMap<String, f32>) -> Vec<ScoredUnit> {
    let weights = if weights.is_empty() { default_weights() } else { weights.clone() };
    let raws: Vec<Raw> = units.iter().map(|u| raw(a, u)).collect();
    let col = |f: &dyn Fn(&Raw) -> f32| raws.iter().map(f).collect::<Vec<f32>>();
    let (lm, lv, ls, si, mo, mv, se) = (
        col(&|r| r.loud_mean),
        col(&|r| r.loud_var),
        col(&|r| r.loud_spike),
        col(&|r| r.start_intensity),
        col(&|r| r.motion),
        col(&|r| r.motion_var),
        col(&|r| r.scene_events),
    );
    let has_visual = a.visual.as_ref().map(|v| !v.samples.is_empty()).unwrap_or(false);
    units
        .iter()
        .zip(raws.iter())
        .map(|(u, r)| {
            let d = u.duration().as_secs_f64() as f32;
            let audio_intensity = rank(r.loud_mean, &lm);
            let visual_action = if has_visual { 0.6 * rank(r.motion, &mo) + 0.4 * rank(r.motion_var, &mv) } else { 0.0 };
            let length_ok = if (5.0..=60.0).contains(&d) {
                1.0
            } else if d < 5.0 {
                d / 5.0
            } else {
                (60.0 / d).max(0.3)
            };
            let pacing = match r.words_per_s {
                Some(w) => (1.0 - ((w - 2.6).abs() / 2.6)).clamp(0.0, 1.0),
                None => r.speech_density.clamp(0.0, 1.0),
            };
            let semantic = match r.words_per_s {
                Some(w) => (0.5 * (w / 3.0).min(1.0) + 0.3 * r.speech_density + 0.2 * r.punct).clamp(0.0, 1.0),
                None => r.speech_density.clamp(0.0, 1.0),
            };
            let tech = {
                let mut q = 1.0f32;
                if has_visual && r.luma < 0.12 {
                    q -= 0.4;
                }
                if r.loud_mean < -45.0 {
                    q -= 0.3;
                }
                q.max(0.0)
            };
            let f: BTreeMap<String, f32> = [
                ("semantic_relevance", semantic),
                ("hook_strength", (0.6 * rank(r.start_intensity, &si) + 0.4 * r.punct.max(if has_visual { rank(r.motion, &mo) } else { 0.0 })).clamp(0.0, 1.0)),
                ("self_containment", ((0.6 * r.boundary_pauses + if r.ends_sentence { 0.2 } else { 0.0 } + 0.2) * length_ok).clamp(0.0, 1.0)),
                ("expressiveness", (0.7 * rank(r.loud_var, &lv) + 0.3 * r.faces).clamp(0.0, 1.0)),
                ("pacing", pacing),
                ("technical_quality", tech),
                ("visual_action", visual_action),
                ("reaction", (0.6 * rank(r.loud_spike, &ls) + 0.4 * r.faces).clamp(0.0, 1.0)),
                ("event_sequence", (0.5 * rank(r.scene_events, &se) + 0.5 * visual_action).clamp(0.0, 1.0)),
                ("audio_intensity", audio_intensity),
                ("humor_signal", (0.5 * rank(r.loud_spike, &ls) + 0.5 * rank(r.loud_var, &lv)).clamp(0.0, 1.0)),
                ("clarity", (r.speech_density * if r.nonspeech_noise > -35.0 { 0.6 } else { 1.0 }).clamp(0.0, 1.0)),
            ]
            .into_iter()
            .map(|(k, v)| (k.to_string(), v))
            .collect();
            let wsum: f32 = weights.values().sum::<f32>().max(1e-6);
            let score = weights.iter().map(|(k, w)| w * f.get(k).copied().unwrap_or(0.0)).sum::<f32>() / wsum * 100.0;
            // Explicação: os três fatores que mais contribuíram.
            let mut contrib: Vec<(&String, f32)> = weights.iter().map(|(k, w)| (k, w * f.get(k).copied().unwrap_or(0.0))).collect();
            contrib.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap());
            let top: Vec<&str> = contrib.iter().filter(|(_, c)| *c > 0.02).take(3).map(|(k, _)| factor_label(k)).collect();
            // Só os fatores (o instante e a nota são mostrados à parte).
            let explanation = if top.is_empty() { "sem destaque claro".to_string() } else { top.join(", ") };
            ScoredUnit { source: *u, score: (score * 10.0).round() / 10.0, factors: f, explanation }
        })
        .collect()
}
