//! Validação estrutural da timeline. Executada após cada comando (o comando é
//! descartado se a validação falhar) e antes de qualquer renderização.

use crate::model::*;
use crate::time::Time;
use serde::Serialize;
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, TS, thiserror::Error)]
#[ts(export)]
#[error("{message}")]
pub struct ValidationIssue {
    pub code: String,
    pub message: String,
    pub clip_id: Option<Id>,
    pub track_id: Option<Id>,
}

fn issue(code: &str, message: String, clip: Option<Id>, track: Option<Id>) -> ValidationIssue {
    ValidationIssue { code: code.into(), message, clip_id: clip, track_id: track }
}

pub fn validate_project(p: &Project) -> Vec<ValidationIssue> {
    let mut out = vec![];
    let s = &p.settings;
    if s.width < 16 || s.height < 16 || !s.width.is_multiple_of(2) || !s.height.is_multiple_of(2) {
        out.push(issue("settings.resolution", format!("Resolução inválida {}x{} (precisa ser par e ≥16)", s.width, s.height), None, None));
    }
    if !s.fps.is_valid() || s.fps.as_f64() > 240.0 {
        out.push(issue("settings.fps", format!("FPS inválido: {}", s.fps.to_ffmpeg()), None, None));
    }
    if !(8000..=192_000).contains(&s.sample_rate) {
        out.push(issue("settings.sample_rate", format!("Taxa de amostragem inválida: {}", s.sample_rate), None, None));
    }
    out.extend(validate_timeline(&p.timeline, &p.assets));
    out
}

pub fn validate_timeline(tl: &Timeline, assets: &[MediaAsset]) -> Vec<ValidationIssue> {
    let mut out = vec![];
    let asset = |id: Id| assets.iter().find(|a| a.id == id);

    for track in &tl.tracks {
        let tid = Some(track.id);
        for c in &track.clips {
            let cid = Some(c.id);
            if c.start < Time::ZERO {
                out.push(issue("clip.negative_start", format!("Clipe '{}' começa antes de 00:00", c.name), cid, tid));
            }
            if !c.duration.is_positive() {
                out.push(issue("clip.non_positive_duration", format!("Clipe '{}' tem duração nula ou negativa", c.name), cid, tid));
            }
            if !(0.0..=4.0).contains(&c.volume) {
                out.push(issue("clip.volume", format!("Volume fora do intervalo 0–4 em '{}'", c.name), cid, tid));
            }
            if !(0.0..=1.0).contains(&c.opacity) {
                out.push(issue("clip.opacity", format!("Opacidade fora do intervalo 0–1 em '{}'", c.name), cid, tid));
            }
            if c.fade_in < Time::ZERO || c.fade_out < Time::ZERO || c.fade_in + c.fade_out > c.duration {
                out.push(issue("clip.fades", format!("Fades de '{}' excedem a duração do clipe", c.name), cid, tid));
            }
            if !(0.05..=20.0).contains(&c.transform.scale) {
                out.push(issue("clip.scale", format!("Escala inválida em '{}'", c.name), cid, tid));
            }
            out.extend(validate_clip_extras(c, track));
            match (&c.content, track.kind) {
                (ClipContent::Text { text, .. }, TrackKind::Text) => {
                    if text.trim().is_empty() {
                        out.push(issue("text.empty", "Texto vazio".into(), cid, tid));
                    }
                    if text.len() > 2000 {
                        out.push(issue("text.too_long", "Texto excede 2000 caracteres".into(), cid, tid));
                    }
                }
                (ClipContent::Text { .. }, _) => out.push(issue("clip.track_kind", "Textos só podem ficar em trilhas de texto".into(), cid, tid)),
                (ClipContent::Media { .. }, TrackKind::Text) => out.push(issue("clip.track_kind", "Trilhas de texto não aceitam mídia".into(), cid, tid)),
                (ClipContent::Media { asset_id, source_in }, kind) => match asset(*asset_id) {
                    None => out.push(issue("clip.missing_asset", format!("Clipe '{}' referencia mídia inexistente", c.name), cid, tid)),
                    Some(a) => {
                        if kind == TrackKind::Video && !a.has_video() {
                            out.push(issue("clip.track_kind", format!("'{}' não possui vídeo", a.name), cid, tid));
                        }
                        if kind == TrackKind::Audio && !a.has_audio() {
                            out.push(issue("clip.track_kind", format!("'{}' não possui áudio", a.name), cid, tid));
                        }
                        if *source_in < Time::ZERO {
                            out.push(issue("clip.source_range", "Ponto de entrada negativo".into(), cid, tid));
                        }
                        if !a.is_unbounded() && *source_in + c.source_span() > a.duration {
                            out.push(issue(
                                "clip.source_range",
                                format!("Clipe '{}' ultrapassa o fim da mídia de origem ({} > {})", c.name, (*source_in + c.source_span()), a.duration),
                                cid,
                                tid,
                            ));
                        }
                    }
                },
            }
        }

        // Sobreposições: só permitidas entre vizinhos ligados por transição
        // com duração idêntica à sobreposição.
        for w in track.clips.windows(2) {
            let (a, b) = (&w[0], &w[1]);
            if b.start < a.start {
                out.push(issue("track.unsorted", "Clipes fora de ordem".into(), Some(b.id), tid));
                continue;
            }
            let overlap = a.end() - b.start;
            if overlap.is_positive() {
                match tl.transition_between(a.id, b.id) {
                    Some(t) if t.duration == overlap => {}
                    Some(_) => out.push(issue(
                        "transition.mismatch",
                        format!("A sobreposição entre '{}' e '{}' não corresponde à transição", a.name, b.name),
                        Some(b.id),
                        tid,
                    )),
                    None => out.push(issue("track.overlap", format!("'{}' e '{}' se sobrepõem sem transição", a.name, b.name), Some(b.id), tid)),
                }
            }
        }
        for w in track.clips.windows(3) {
            if w[2].start < w[0].end() {
                out.push(issue("track.triple_overlap", "Três clipes sobrepostos no mesmo instante".into(), Some(w[2].id), tid));
            }
        }
    }

    for t in &tl.transitions {
        let Some(track) = tl.track(t.track_id) else {
            out.push(issue("transition.track", "Transição em trilha inexistente".into(), None, None));
            continue;
        };
        if track.kind == TrackKind::Text {
            out.push(issue("transition.track", "Transições não se aplicam a trilhas de texto".into(), None, Some(track.id)));
        }
        let ia = track.clips.iter().position(|c| c.id == t.from_clip);
        let ib = track.clips.iter().position(|c| c.id == t.to_clip);
        match (ia, ib) {
            (Some(a), Some(b)) if b == a + 1 => {
                let (ca, cb) = (&track.clips[a], &track.clips[b]);
                if !t.duration.is_positive() || t.duration > ca.duration || t.duration > cb.duration {
                    out.push(issue("transition.duration", "Duração da transição maior que um dos clipes".into(), Some(cb.id), Some(track.id)));
                }
            }
            _ => out.push(issue("transition.clips", "Transição precisa ligar dois clipes consecutivos da mesma trilha".into(), None, Some(track.id))),
        }
    }
    out
}

const MAX_REVERSE: Time = Time::from_secs(120);

/// Velocidade, reverso, congelamento, efeitos e keyframes.
fn validate_clip_extras(c: &Clip, track: &Track) -> Vec<ValidationIssue> {
    let mut out = vec![];
    let (cid, tid) = (Some(c.id), Some(track.id));
    let is_media = matches!(c.content, ClipContent::Media { .. });
    if !(0.1..=8.0).contains(&c.speed) {
        out.push(issue("clip.speed", format!("Velocidade de '{}' fora do intervalo 0,1×–8×", c.name), cid, tid));
    }
    if (c.reverse || c.freeze || c.speed != 1.0) && !is_media {
        out.push(issue("clip.speed", "Velocidade, reverso e congelamento só se aplicam a mídia".into(), cid, tid));
    }
    if c.reverse && c.freeze {
        out.push(issue("clip.reverse", "Um clipe não pode estar invertido e congelado ao mesmo tempo".into(), cid, tid));
    }
    if c.reverse && c.source_span() > MAX_REVERSE {
        out.push(issue(
            "clip.reverse",
            format!("Reverso é limitado a trechos de até {} s de origem (memória); divida o clipe", MAX_REVERSE.as_secs_f64()),
            cid,
            tid,
        ));
    }
    if c.freeze && track.kind != TrackKind::Video {
        out.push(issue("clip.freeze", "Congelar frame só se aplica a trilhas de vídeo".into(), cid, tid));
    }
    let e = &c.effects;
    if !(0.0..=50.0).contains(&e.blur)
        || !(0.0..=2.0).contains(&e.sharpen)
        || !(0.0..=1.0).contains(&e.vignette)
        || !(1000.0..=40000.0).contains(&e.temperature)
    {
        out.push(issue("clip.effects", format!("Valores de efeito fora do intervalo em '{}'", c.name), cid, tid));
    }
    if let Some(l) = &e.lut {
        let ext = l.extension().and_then(|x| x.to_str()).map(|x| x.to_ascii_lowercase());
        if !matches!(ext.as_deref(), Some("cube") | Some("3dl")) {
            out.push(issue("clip.lut", "LUT precisa ser um arquivo .cube ou .3dl".into(), cid, tid));
        } else if !l.is_file() {
            out.push(issue("clip.lut", format!("Arquivo de LUT não encontrado: {}", l.display()), cid, tid));
        }
    }
    let a = &c.audio_fx;
    for g in [a.eq_low_db, a.eq_mid_db, a.eq_high_db] {
        if !(-24.0..=24.0).contains(&g) {
            out.push(issue("clip.eq", "Ganho de equalização fora de ±24 dB".into(), cid, tid));
            break;
        }
    }
    let ranges: [(KeyframeProperty, f32, f32, &str); 4] = [
        (KeyframeProperty::Scale, 1.0, 5.0, "zoom animado precisa ficar entre 100% e 500%"),
        (KeyframeProperty::PositionX, -1.0, 1.0, "posição X animada precisa ficar entre -1 e 1"),
        (KeyframeProperty::PositionY, -1.0, 1.0, "posição Y animada precisa ficar entre -1 e 1"),
        (KeyframeProperty::Volume, 0.0, 4.0, "volume animado precisa ficar entre 0 e 4"),
    ];
    for (p, lo, hi, msg) in ranges {
        let k = c.keyframes.get(p);
        if k.is_empty() {
            continue;
        }
        if p != KeyframeProperty::Volume && track.kind != TrackKind::Video {
            out.push(issue("keyframe.track", "Keyframes de transformação só se aplicam a trilhas de vídeo".into(), cid, tid));
        }
        if k.iter().any(|kf| !(lo..=hi).contains(&kf.value)) {
            out.push(issue("keyframe.range", format!("'{}': {msg}", c.name), cid, tid));
        }
        if k.iter().any(|kf| kf.at < Time::ZERO || kf.at > c.duration) {
            out.push(issue("keyframe.time", format!("'{}': keyframe fora da duração do clipe", c.name), cid, tid));
        }
        if k.windows(2).any(|w| w[1].at <= w[0].at) {
            out.push(issue("keyframe.order", "Keyframes precisam estar em ordem e sem tempos repetidos".into(), cid, tid));
        }
    }
    out
}
