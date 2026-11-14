//! Revisão automática de qualidade (seção 26).
//!
//! Inspeciona a timeline (e as análises das mídias) e aponta problemas com o
//! instante em que ocorrem. Nunca dá "nota perfeita" automática: o resultado
//! é uma lista de verificações com `pass`, `warn` ou `fail`, e o resumo diz
//! apenas que nada foi encontrado nos pontos verificados.

use crate::analysis::AnalysisResult;
use crate::autoedit::ClipMap;
use avs_core::*;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CheckLevel {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QualityIssue {
    pub level: CheckLevel,
    pub message: String,
    /// Instante na timeline (para "ir para").
    pub at: Option<Time>,
    pub clip_id: Option<Id>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QualityCheck {
    /// Identificador estável (ex.: "cut_mid_speech").
    pub id: String,
    pub title: String,
    pub level: CheckLevel,
    pub issues: Vec<QualityIssue>,
    /// O que foi verificado (ou por que não pôde ser).
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct QualityReport {
    pub level: CheckLevel,
    pub duration: Time,
    pub checks: Vec<QualityCheck>,
    pub summary: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ReviewTargets {
    pub target_s: Option<f32>,
    pub max_s: Option<f32>,
    /// Margem inferior reservada pelo preset (fração da altura).
    pub safe_bottom_pct: f32,
}

struct Builder {
    id: &'static str,
    title: &'static str,
    issues: Vec<QualityIssue>,
    note: String,
}

impl Builder {
    fn new(id: &'static str, title: &'static str, note: impl Into<String>) -> Self {
        Builder { id, title, issues: vec![], note: note.into() }
    }
    fn push(&mut self, level: CheckLevel, message: String, at: Option<Time>, clip_id: Option<Id>) {
        if self.issues.len() < 50 {
            self.issues.push(QualityIssue { level, message, at, clip_id });
        }
    }
    fn done(self) -> QualityCheck {
        let level = self.issues.iter().map(|i| i.level).max().unwrap_or(CheckLevel::Pass);
        QualityCheck { id: self.id.into(), title: self.title.into(), level, issues: self.issues, note: self.note }
    }
}

fn fmt_t(t: Time) -> String {
    let s = t.as_secs_f64();
    format!("{}:{:04.1}", (s / 60.0).floor() as i64, s % 60.0)
}

/// Fala em tempo de origem cobrindo `t` (com folga de `inner` para dentro).
fn inside_speech(a: &AnalysisResult, t: Time, inner: Time) -> bool {
    a.speech.as_ref().map(|s| s.segments.iter().any(|r| t > r.start + inner && t < r.end - inner)).unwrap_or(false)
}

pub fn review(p: &Project, analyses: &HashMap<Id, AnalysisResult>, targets: &ReviewTargets) -> QualityReport {
    let fps = p.settings.fps;
    let dur = p.timeline.duration();
    let mut checks = vec![];
    let analysis_of = |c: &Clip| c.asset_id().and_then(|id| analyses.get(&id));
    let media_tracks: Vec<&Track> = p.timeline.tracks.iter().filter(|t| t.kind != TrackKind::Text).collect();
    let video_tracks: Vec<&Track> = p.timeline.tracks.iter().filter(|t| t.kind == TrackKind::Video).collect();

    // 1. Duração
    let mut b = Builder::new("duration", "Duração", format!("{:.1} s", dur.as_secs_f64()));
    if dur.is_zero() {
        b.push(CheckLevel::Fail, "A timeline está vazia.".into(), None, None);
    }
    if let Some(m) = targets.max_s {
        if dur.as_secs_f64() > m as f64 + 0.05 {
            b.push(
                CheckLevel::Fail,
                format!("Duração {:.1} s acima do máximo do preset ({m:.0} s).", dur.as_secs_f64()),
                Some(Time::from_secs_f64(m as f64)),
                None,
            );
        }
    }
    if let Some(t) = targets.target_s {
        let diff = (dur.as_secs_f64() - t as f64).abs() / (t as f64).max(1.0);
        if diff > 0.25 && !dur.is_zero() {
            b.push(CheckLevel::Warn, format!("Duração {:.0} s distante do alvo ({t:.0} s).", dur.as_secs_f64()), None, None);
        }
    }
    checks.push(b.done());

    // 2. Cortes no meio da fala (saltos de conteúdo dentro de um segmento de fala)
    let mut b = Builder::new("cut_mid_speech", "Cortes no meio de falas", "junções entre trechos não contínuos");
    let mut have_speech = false;
    for t in &media_tracks {
        for w in t.clips.windows(2) {
            let (a, c) = (&w[0], &w[1]);
            if a.asset_id().is_none() || c.asset_id().is_none() || (c.start - a.end()).abs() > fps.frame_duration() {
                continue;
            }
            let continuous =
                a.asset_id() == c.asset_id() && !a.reverse && !c.reverse && (c.source_in() - (a.source_in() + a.source_span())).abs() <= fps.frame_duration();
            if continuous {
                continue;
            }
            let inner = Time::from_millis(120);
            let out_t = a.source_in() + a.source_span();
            let mut bad = false;
            if let Some(an) = analysis_of(a) {
                have_speech |= an.speech.is_some();
                bad |= ClipMap::of(a).is_some() && inside_speech(an, out_t, inner);
            }
            if let Some(an) = analysis_of(c) {
                have_speech |= an.speech.is_some();
                bad |= ClipMap::of(c).is_some() && inside_speech(an, c.source_in(), inner);
            }
            if bad {
                b.push(CheckLevel::Warn, format!("Corte em {} interrompe uma fala.", fmt_t(c.start)), Some(c.start), Some(c.id));
            }
        }
    }
    if !have_speech {
        b.note = "sem análise de fala nas junções: não verificado".into();
    }
    checks.push(b.done());

    // 3. Buracos (quadro preto/silêncio por ausência de clipes)
    let mut b = Builder::new("gaps", "Trechos vazios", "intervalos sem vídeo na trilha principal");
    if let Some(vt) = video_tracks.iter().find(|t| !t.clips.is_empty()) {
        let mut cursor = Time::ZERO;
        let mut spans: Vec<TimeRange> = vec![];
        for t in &video_tracks {
            spans.extend(t.clips.iter().map(|c| c.range()));
        }
        spans.sort_by_key(|r| r.start);
        for r in spans {
            if r.start - cursor > fps.frame_duration() {
                b.push(CheckLevel::Warn, format!("{:.1} s sem imagem em {}.", (r.start - cursor).as_secs_f64(), fmt_t(cursor)), Some(cursor), None);
            }
            cursor = cursor.max(r.end);
        }
        if dur - cursor > fps.frame_duration() {
            b.push(CheckLevel::Warn, format!("Final com {:.1} s sem imagem (só áudio ou texto).", (dur - cursor).as_secs_f64()), Some(cursor), None);
        }
        let _ = vt;
    } else {
        b.note = "projeto sem vídeo".into();
    }
    checks.push(b.done());

    // 4. Imagem preta e silêncio longo (pelas análises)
    let mut b = Builder::new("black_silence", "Tela preta e silêncio", "amostras de brilho e volume das mídias usadas");
    for t in &media_tracks {
        for c in &t.clips {
            let (Some(m), Some(a)) = (ClipMap::of(c), analysis_of(c)) else { continue };
            let w = m.window();
            if t.kind == TrackKind::Video {
                if let Some(v) = &a.visual {
                    let mut run: Option<Time> = None;
                    // Amostras ordenadas: só as da janela do clipe (busca binária).
                    let i0 = v.samples.partition_point(|s| s.t < w.start);
                    let i1 = v.samples.partition_point(|s| s.t < w.end);
                    for s in &v.samples[i0..i1.max(i0)] {
                        if s.luma < 0.03 {
                            let st = *run.get_or_insert(s.t);
                            if s.t - st >= Time::from_secs(1) {
                                let at = m.to_timeline(st);
                                b.push(CheckLevel::Warn, format!("Imagem quase preta por ≥ 1 s em {}.", fmt_t(at)), Some(at), Some(c.id));
                                run = Some(Time::from_secs(1_000_000));
                            }
                        } else {
                            run = None;
                        }
                    }
                }
            }
            if let Some(l) = &a.loudness {
                let iv = l.interval;
                if iv.is_positive() {
                    let mut run = 0usize;
                    // Só os índices da janela do clipe (t = i·intervalo).
                    let step = iv.flicks().max(1);
                    let i0 = ((w.start.flicks() + step - 1).div_euclid(step)).max(0) as usize;
                    let i1 = (((w.end.flicks() + step - 1).div_euclid(step)).max(0) as usize).min(l.rms_db.len());
                    for (i, db) in l.rms_db.iter().enumerate().take(i1).skip(i0) {
                        let st = iv * i as i64;
                        run = if *db < -55.0 { run + 1 } else { 0 };
                        if (iv * run as i64) >= Time::from_secs(3) {
                            let at = m.to_timeline(st - iv * (run as i64 - 1));
                            b.push(CheckLevel::Warn, format!("Silêncio de ≥ 3 s em {} ('{}').", fmt_t(at), c.name), Some(at), Some(c.id));
                            run = 0;
                        }
                    }
                }
            }
        }
    }
    checks.push(b.done());

    // 5. Texto: área segura e sobreposição
    let mut b = Builder::new("text", "Textos e legendas", "posição na área segura e sobreposição");
    let bottom = (1.0 - targets.safe_bottom_pct.clamp(0.0, 0.4)).min(0.95);
    let mut texts: Vec<(&Clip, &TextStyle, &str)> = vec![];
    for t in p.timeline.tracks.iter().filter(|t| t.kind == TrackKind::Text && !t.hidden) {
        for c in &t.clips {
            if let ClipContent::Text { style, text } = &c.content {
                texts.push((c, style, text));
            }
        }
    }
    let h = p.settings.height as f32;
    for (c, st, text) in &texts {
        let half = st.size * (h / 1080.0) / h * 0.6 * (text.lines().count().max(1) as f32);
        if st.y + half > bottom || st.y - half < 0.05 || st.x < 0.05 || st.x > 0.95 {
            b.push(
                CheckLevel::Warn,
                format!("Texto \"{}\" fora da área segura em {}.", text.chars().take(30).collect::<String>(), fmt_t(c.start)),
                Some(c.start),
                Some(c.id),
            );
        }
        let chars_per_line = text.lines().map(|l| l.chars().count()).max().unwrap_or(0) as f32;
        if chars_per_line * st.size * 0.55 * (h / 1080.0) > p.settings.width as f32 * 0.92 {
            b.push(CheckLevel::Warn, format!("Linha de texto possivelmente mais larga que o quadro em {}.", fmt_t(c.start)), Some(c.start), Some(c.id));
        }
    }
    for i in 0..texts.len() {
        for j in i + 1..texts.len() {
            let (a, sa, _) = texts[i];
            let (c, sc, _) = texts[j];
            if a.range().intersects(&c.range()) && (sa.y - sc.y).abs() < 0.08 && (sa.x - sc.x).abs() < 0.3 {
                let at = a.start.max(c.start);
                b.push(CheckLevel::Warn, format!("Dois textos sobrepostos em {}.", fmt_t(at)), Some(at), Some(c.id));
            }
        }
    }
    if texts.is_empty() {
        b.note = "sem textos".into();
    }
    checks.push(b.done());

    // 6. Trechos repetidos e clipes curtos
    let mut b = Builder::new("repeats", "Repetições e clipes curtos", "mesmo trecho de mídia usado duas vezes; clipes < 0,4 s");
    let mut uses: Vec<(&Clip, Id, TimeRange)> = vec![];
    for t in &video_tracks {
        for c in &t.clips {
            if let Some(aid) = c.asset_id() {
                let a = p.asset(aid);
                if a.map(|a| a.kind == AssetKind::Image).unwrap_or(false) {
                    continue;
                }
                if c.duration < Time::from_millis(400) && !c.freeze {
                    b.push(
                        CheckLevel::Warn,
                        format!("Clipe muito curto ({:.2} s) em {}.", c.duration.as_secs_f64(), fmt_t(c.start)),
                        Some(c.start),
                        Some(c.id),
                    );
                }
                if !c.freeze {
                    uses.push((c, aid, TimeRange::new(c.source_in(), c.source_in() + c.source_span())));
                }
            }
        }
    }
    for i in 0..uses.len() {
        for j in i + 1..uses.len() {
            let (ca, aa, ra) = uses[i];
            let (cb, ab, rb) = uses[j];
            if aa == ab {
                if let Some(x) = ra.intersection(&rb) {
                    if x.duration() >= Time::from_millis(500) {
                        let at = ca.start.max(cb.start);
                        b.push(
                            CheckLevel::Warn,
                            format!("O mesmo trecho ({:.1} s) aparece duas vezes ('{}').", x.duration().as_secs_f64(), cb.name),
                            Some(at),
                            Some(cb.id),
                        );
                    }
                }
            }
        }
    }
    checks.push(b.done());

    // 7. Áudio: risco de distorção
    let mut b = Builder::new("audio", "Volume e distorção", "pico das mídias × volume dos clipes");
    for t in &media_tracks {
        if t.muted {
            continue;
        }
        for c in &t.clips {
            let Some(a) = analysis_of(c) else { continue };
            let Some(l) = &a.loudness else { continue };
            let vmax = c.keyframes.volume.iter().map(|k| k.value).fold(c.volume, f32::max);
            let peak = l.peak_db + 20.0 * vmax.max(1e-4).log10();
            if peak > -0.1 && !c.audio_muted {
                b.push(CheckLevel::Warn, format!("Risco de distorção em '{}' (pico estimado {peak:+.1} dBFS).", c.name), Some(c.start), Some(c.id));
            }
        }
    }
    let audible = media_tracks.iter().any(|t| {
        !t.muted && t.clips.iter().any(|c| !c.audio_muted && c.volume > 0.0 && c.asset_id().and_then(|id| p.asset(id)).map(|a| a.has_audio()).unwrap_or(false))
    });
    if !audible && !dur.is_zero() {
        b.push(CheckLevel::Warn, "O vídeo não terá som (nenhum clipe com áudio audível).".into(), None, None);
    }
    checks.push(b.done());

    // 8. Proporção
    let mut b = Builder::new("aspect", "Enquadramento", "proporção das mídias × formato do projeto");
    let pa = p.settings.width as f32 / p.settings.height.max(1) as f32;
    for t in &video_tracks {
        for c in &t.clips {
            let Some(v) = c.asset_id().and_then(|id| p.asset(id)).and_then(|a| a.video.as_ref()) else { continue };
            let (mut w, mut hh) = (v.width as f32, v.height as f32);
            if v.rotation.abs() == 90 || v.rotation.abs() == 270 {
                std::mem::swap(&mut w, &mut hh);
            }
            let ma = w / hh.max(1.0);
            if (ma / pa - 1.0).abs() > 0.15 && c.transform.fit == FitMode::Fit && c.transform.scale < (pa / ma).max(ma / pa) * 0.95 {
                b.push(
                    CheckLevel::Warn,
                    format!("'{}' tem proporção diferente do projeto: aparecerão barras (use 'Preencher').", c.name),
                    Some(c.start),
                    Some(c.id),
                );
            }
        }
    }
    checks.push(b.done());

    let level = checks.iter().map(|c| c.level).max().unwrap_or(CheckLevel::Pass);
    let n: usize = checks.iter().map(|c| c.issues.len()).sum();
    let summary = match level {
        CheckLevel::Pass => format!("Nenhum problema encontrado nos {} pontos verificados. Assista ao resultado antes de publicar.", checks.len()),
        CheckLevel::Warn => format!("{n} ponto(s) de atenção."),
        CheckLevel::Fail => format!("{n} ponto(s), incluindo problema(s) que impedem o resultado esperado."),
    };
    QualityReport { level, duration: dur, checks, summary }
}
