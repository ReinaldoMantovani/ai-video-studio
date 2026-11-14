//! Fase 5 — cortes: sugestão de trechos autossuficientes a partir da
//! pontuação editorial e reenquadramento para formatos verticais.
//!
//! - [`suggest`]: candidatos a corte na timeline do projeto de origem, cada um
//!   começando e terminando em limites de trechos (frases completas quando há
//!   fala), com nota, título e justificativa. Nunca se sobrepõem.
//! - [`plan_reframe`]: enquadramento por planos para "Preencher" num quadro
//!   mais estreito que a mídia (ex.: 16:9 → 9:16). Segue o rosto principal
//!   detectado; muda de enquadramento em cortes de cena ou quando o rosto se
//!   desloca de forma sustentada. Cada plano é estático (sem panorâmica), o
//!   que é exato no compilador e evita "câmera nervosa".

use crate::analysis::{AnalysisResult, FaceBox};
use crate::autoedit::ClipMap;
use crate::scoring::{candidate_units, score_units};
use avs_core::*;
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, HashMap};
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShortsOptions {
    pub count: u32,
    pub min_s: f32,
    pub max_s: f32,
    pub target_s: f32,
}

impl Default for ShortsOptions {
    fn default() -> Self {
        ShortsOptions { count: 5, min_s: 15.0, max_s: 60.0, target_s: 35.0 }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ShortCandidate {
    /// Posição no ranking (1 = melhor).
    pub rank: u32,
    /// Trecho na timeline do projeto de origem.
    pub range: TimeRange,
    pub score: f32,
    pub title: String,
    pub explanation: String,
    /// Número de trechos (frases/planos) incluídos.
    pub units: u32,
    /// Fala dentro do trecho (s) — as pausas são removidas ao criar o corte.
    pub speech_s: f32,
}

#[derive(Debug, Clone)]
struct Unit {
    tl: TimeRange,
    /// Clipe que contém o trecho (a folga não atravessa para outro clipe).
    clip: TimeRange,
    score: f32,
    explanation: String,
    speech: f64,
    text: String,
}

fn transcript_text(a: &AnalysisResult, src: TimeRange) -> String {
    a.transcript
        .as_ref()
        .map(|t| {
            t.segments
                .iter()
                .flat_map(|s| s.words.iter())
                .filter(|w| w.start >= src.start - Time::from_millis(100) && w.end <= src.end + Time::from_millis(100))
                .map(|w| w.word.trim())
                .collect::<Vec<_>>()
                .join(" ")
        })
        .unwrap_or_default()
}

fn mmss(t: Time) -> String {
    let s = t.as_secs_f64().max(0.0) as i64;
    format!("{}:{:02}", s / 60, s % 60)
}

fn title_from(text: &str, at: Time) -> String {
    let words: Vec<&str> = text.split_whitespace().collect();
    if words.len() >= 3 {
        let mut t = words.iter().take(8).copied().collect::<Vec<_>>().join(" ");
        t = t.trim_end_matches([',', '.', ';', ':']).to_string();
        if words.len() > 8 {
            t.push('…');
        }
        let mut c = t.chars();
        match c.next() {
            Some(f) => f.to_uppercase().collect::<String>() + c.as_str(),
            None => t,
        }
    } else {
        format!("Momento em {}", mmss(at))
    }
}

/// Unidades pontuadas da trilha principal, em ordem cronológica (timeline).
fn units(p: &Project, main: Id, analyses: &HashMap<Id, AnalysisResult>, weights: &BTreeMap<String, f32>) -> Vec<Unit> {
    let fps = p.settings.fps;
    let mut out = vec![];
    for c in &p.timeline.track(main).map(|t| t.clips.clone()).unwrap_or_default() {
        let (Some(m), Some(a)) = (ClipMap::of(c), c.asset_id().and_then(|id| analyses.get(&id))) else { continue };
        let w = m.window();
        for u in score_units(a, &candidate_units(a, w), weights) {
            let tl = TimeRange::new(m.to_timeline(u.source.start).snap_to_frame(fps), m.to_timeline(u.source.end).snap_to_frame(fps));
            if !tl.duration().is_positive() {
                continue;
            }
            let speech = a
                .speech
                .as_ref()
                .map(|s| s.segments.iter().filter_map(|x| x.intersection(&u.source)).map(|x| x.duration().as_secs_f64()).sum::<f64>())
                .unwrap_or(0.0)
                / c.speed.max(0.01) as f64;
            out.push(Unit { tl, clip: c.range(), score: u.score, explanation: u.explanation, speech, text: transcript_text(a, u.source) });
        }
    }
    out.sort_by_key(|u| u.tl.start);
    out
}

/// Sugere até `count` cortes sem sobreposição, do melhor para o pior.
pub fn suggest(p: &Project, main: Id, analyses: &HashMap<Id, AnalysisResult>, weights: &BTreeMap<String, f32>, o: &ShortsOptions) -> Vec<ShortCandidate> {
    let us = units(p, main, analyses, weights);
    if us.is_empty() {
        return vec![];
    }
    let (min, max, target) = (o.min_s as f64, o.max_s.max(o.min_s + 1.0) as f64, (o.target_s as f64).clamp(o.min_s as f64, o.max_s as f64));
    let max_gap = 6.0;
    let span = |i: usize, j: usize| (us[j].tl.end - us[i].tl.start).as_secs_f64();
    let gap = |a: usize, b: usize| (us[b].tl.start - us[a].tl.end).as_secs_f64();
    let end_all = p.timeline.duration();
    let mut cands: Vec<(usize, usize, f32)> = vec![];
    for seed in 0..us.len() {
        let (mut i, mut j) = (seed, seed);
        // Cresce para a direita até o alvo (a história segue em frente)…
        while j + 1 < us.len() && span(i, j) < target && gap(j, j + 1) <= max_gap && span(i, j + 1) <= max {
            j += 1;
        }
        // …e para a esquerda se ainda estiver curto (contexto antes do pico).
        while i > 0 && span(i, j) < min && gap(i - 1, i) <= max_gap && span(i - 1, j) <= max {
            i -= 1;
        }
        while j + 1 < us.len() && span(i, j) < min && gap(j, j + 1) <= max_gap && span(i, j + 1) <= max {
            j += 1;
        }
        let total: f64 = (i..=j).map(|k| us[k].tl.duration().as_secs_f64()).sum();
        let mean = (i..=j).map(|k| us[k].score as f64 * us[k].tl.duration().as_secs_f64()).sum::<f64>() / total.max(1e-6);
        let s = span(i, j);
        let completeness = if s < min { 100.0 * s / min } else { 100.0 };
        // Gancho: o primeiro trecho conta mais (é o que segura quem rola o feed).
        let score = 0.35 * us[i].score as f64 + 0.45 * mean + 0.2 * completeness;
        cands.push((i, j, score as f32));
    }
    cands.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap());
    let mut chosen: Vec<(usize, usize, f32)> = vec![];
    for c in cands {
        if chosen.len() as u32 >= o.count {
            break;
        }
        if chosen.iter().any(|x| !(c.1 < x.0 || c.0 > x.1)) {
            continue;
        }
        chosen.push(c);
    }
    chosen
        .iter()
        .enumerate()
        .map(|(rank, &(i, j, score))| {
            let prev_end = if i > 0 { us[i - 1].tl.end } else { Time::ZERO };
            let next_start = if j + 1 < us.len() { us[j + 1].tl.start } else { end_all };
            let start = (us[i].tl.start - Time::from_millis(150)).max(prev_end).max(us[i].clip.start).max(Time::ZERO).snap_to_frame(p.settings.fps);
            let end = (us[j].tl.end + Time::from_millis(300)).min(next_start).min(us[j].clip.end).min(end_all).snap_to_frame(p.settings.fps);
            let text: String = (i..=j).map(|k| us[k].text.as_str()).filter(|t| !t.is_empty()).collect::<Vec<_>>().join(" ");
            let n = (j - i + 1) as u32;
            let s = (end - start).as_secs_f64();
            let mut explanation = format!("Abre com {}", us[i].explanation.trim_end_matches('.'));
            if n > 1 {
                explanation.push_str(&format!("; {n} trechos encadeados"));
            }
            if s < min {
                explanation.push_str(&format!("; mais curto que o mínimo ({s:.0} s): não havia material contínuo"));
            }
            ShortCandidate {
                rank: rank as u32 + 1,
                range: TimeRange::new(start, end),
                score,
                title: title_from(&text, start),
                explanation,
                units: n,
                speech_s: (i..=j).map(|k| us[k].speech).sum::<f64>() as f32,
            }
        })
        .collect()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum FramingMode {
    /// Segue o rosto principal (corte central quando não há rosto).
    Face,
    /// Corte central.
    Center,
    /// Vídeo inteiro com o mesmo vídeo desfocado ao fundo.
    BlurBackground,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReframeSegment {
    /// Trecho na mídia de origem.
    pub start: Time,
    pub end: Time,
    /// Deslocamento horizontal do `Transform` (modo Preencher).
    pub x: f32,
    pub face: bool,
}

/// Deslocamento `x` do Transform que centraliza `fx` (0..1 da largura da
/// mídia) num quadro `out` com "Preencher", limitado às bordas da imagem.
pub fn fill_offset(fx: f32, src: (u32, u32), out: (u32, u32)) -> f32 {
    let (sw0, sh0, w, h) = (src.0 as f32, src.1 as f32, out.0 as f32, out.1 as f32);
    let f = (w / sw0).max(h / sh0);
    let sw = sw0 * f;
    if sw <= w + 1.0 {
        return 0.0;
    }
    let x = sw * (1.0 - 2.0 * fx) / w;
    let lim = (sw - w) / w;
    x.clamp(-lim, lim)
}

/// Centro horizontal do assunto: rosto maior; vários rostos cabem juntos no
/// quadro → centro do grupo.
fn subject_x(faces: &[FaceBox], crop_frac: f32) -> Option<f32> {
    let big = faces.iter().filter(|f| f.w > 0.0).max_by(|a, b| (a.w * a.h).partial_cmp(&(b.w * b.h)).unwrap())?;
    let min_area = big.w * big.h * 0.35;
    let group: Vec<&FaceBox> = faces.iter().filter(|f| f.w * f.h >= min_area).collect();
    let (l, r) = group.iter().fold((1.0f32, 0.0f32), |(l, r), f| (l.min(f.x), r.max(f.x + f.w)));
    if group.len() > 1 && r - l <= crop_frac * 0.9 {
        Some((l + r) / 2.0)
    } else {
        Some(big.x + big.w / 2.0)
    }
}

fn median(v: &mut [f32]) -> f32 {
    v.sort_by(|a, b| a.partial_cmp(b).unwrap());
    v[v.len() / 2]
}

/// Planos de enquadramento dentro de `window` (tempo de origem).
pub fn plan_reframe(a: Option<&AnalysisResult>, window: TimeRange, src: (u32, u32), out: (u32, u32)) -> Vec<ReframeSegment> {
    let whole = |x: f32, face: bool| vec![ReframeSegment { start: window.start, end: window.end, x, face }];
    let src_aspect = src.0 as f32 / src.1.max(1) as f32;
    let out_aspect = out.0 as f32 / out.1.max(1) as f32;
    if src_aspect <= out_aspect * 1.02 {
        return whole(0.0, false);
    }
    let crop = out_aspect / src_aspect;
    let Some(a) = a else { return whole(0.0, false) };
    let samples: Vec<(Time, Option<f32>)> =
        a.visual.as_ref().map(|v| v.samples.iter().filter(|s| window.contains(s.t)).map(|s| (s.t, subject_x(&s.faces, crop))).collect()).unwrap_or_default();
    let with_face: Vec<f32> = samples.iter().filter_map(|s| s.1).collect();
    if with_face.is_empty() {
        return whole(0.0, false);
    }
    let fallback = median(&mut with_face.clone());
    // Limites de plano: cortes de cena.
    let mut bounds: Vec<Time> = vec![window.start];
    if let Some(sc) = &a.scenes {
        bounds.extend(sc.cuts.iter().copied().filter(|t| *t > window.start + Time::from_millis(500) && *t < window.end - Time::from_millis(500)));
    }
    bounds.push(window.end);
    let min_seg = Time::from_millis(2000);
    let tol = crop * 0.3;
    let mut segs: Vec<(Time, Time, Vec<f32>)> = vec![];
    for w in bounds.windows(2) {
        let (s, e) = (w[0], w[1]);
        let pts: Vec<(Time, f32)> = samples.iter().filter(|x| x.0 >= s && x.0 < e).filter_map(|x| x.1.map(|f| (x.0, f))).collect();
        let mut cur_start = s;
        let mut cur: Vec<f32> = vec![];
        let mut off = 0usize;
        for (k, &(t, f)) in pts.iter().enumerate() {
            if cur.len() >= 2 {
                let c = median(&mut cur.clone());
                if (f - c).abs() > tol {
                    off += 1;
                    // Deslocamento sustentado (2 amostras) e plano atual longo o bastante.
                    if off >= 2 && t - cur_start >= min_seg {
                        let switch = pts[k + 1 - off].0;
                        segs.push((cur_start, switch, std::mem::take(&mut cur)));
                        cur_start = switch;
                        cur.extend(pts[k + 1 - off..=k].iter().map(|x| x.1));
                        off = 0;
                        continue;
                    }
                } else {
                    off = 0;
                }
            }
            cur.push(f);
        }
        segs.push((cur_start, e, cur));
    }
    let mut out_segs: Vec<ReframeSegment> = vec![];
    for (s, e, mut v) in segs {
        let face = !v.is_empty();
        let fx = if face { median(&mut v) } else { out_segs.last().map(|_| f32::NAN).unwrap_or(fallback) };
        let x = if fx.is_nan() { out_segs.last().unwrap().x } else { fill_offset(fx, src, out) };
        match out_segs.last_mut() {
            // Planos muito curtos ou com o mesmo enquadramento se fundem.
            Some(l) if (l.x - x).abs() < 0.04 || e - s < Time::from_millis(1000) => {
                l.end = e;
                l.face |= face;
            }
            _ => out_segs.push(ReframeSegment { start: s, end: e, x, face }),
        }
    }
    out_segs
}
