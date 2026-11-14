//! Legendas SRT e WebVTT: leitura tolerante e escrita padrão.

use crate::model::{ClipContent, TextStyle, Track};
use crate::time::Time;
use serde::{Deserialize, Serialize};
use ts_rs::TS;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Cue {
    pub start: Time,
    pub end: Time,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum SubtitleFormat {
    Srt,
    Vtt,
}

impl SubtitleFormat {
    pub fn from_path(p: &std::path::Path) -> Option<Self> {
        match p.extension()?.to_str()?.to_ascii_lowercase().as_str() {
            "srt" => Some(SubtitleFormat::Srt),
            "vtt" => Some(SubtitleFormat::Vtt),
            _ => None,
        }
    }
}

/// Estilo padrão de legenda (legível, com contorno, acima da área de controles).
pub fn caption_style() -> TextStyle {
    TextStyle {
        font: "Inter".into(),
        size: 58.0,
        color: "#FFFFFF".into(),
        outline_color: "#000000".into(),
        outline_width: 4.0,
        background: None,
        x: 0.5,
        y: 0.84,
    }
}

/// `00:01:02,345` | `00:01:02.345` | `01:02.345` (VTT curto).
fn parse_ts(s: &str) -> Option<Time> {
    let s = s.trim().replace(',', ".");
    let parts: Vec<&str> = s.split(':').collect();
    let (h, m, sec) = match parts.as_slice() {
        [h, m, s] => (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?, *s),
        [m, s] => (0, m.parse::<i64>().ok()?, *s),
        _ => return None,
    };
    let (whole, frac) = sec.split_once('.').unwrap_or((sec, "0"));
    let whole: i64 = whole.parse().ok()?;
    let ms: i64 = format!("{:0<3}", &frac[..frac.len().min(3)]).parse().ok()?;
    if m >= 60 || whole >= 60 {
        return None;
    }
    Some(Time::from_millis(((h * 60 + m) * 60 + whole) * 1000 + ms))
}

fn strip_tags(s: &str) -> String {
    // Remove tags simples (<i>, <b>, <c.classe>, {\an8}) mantendo o texto.
    let mut out = String::with_capacity(s.len());
    let mut depth_angle = false;
    let mut depth_brace = false;
    for ch in s.chars() {
        match ch {
            '<' => depth_angle = true,
            '>' if depth_angle => depth_angle = false,
            '{' => depth_brace = true,
            '}' if depth_brace => depth_brace = false,
            _ if depth_angle || depth_brace => {}
            _ => out.push(ch),
        }
    }
    out.replace("&amp;", "&").replace("&lt;", "<").replace("&gt;", ">").replace("&nbsp;", " ")
}

/// Lê SRT ou VTT (detecta pelo conteúdo). Blocos malformados são ignorados e
/// contados em `skipped`.
pub fn parse(text: &str) -> (Vec<Cue>, usize) {
    let text = text.trim_start_matches('\u{feff}').replace("\r\n", "\n").replace('\r', "\n");
    let mut cues = vec![];
    let mut skipped = 0;
    for block in text.split("\n\n") {
        let lines: Vec<&str> = block.lines().filter(|l| !l.trim().is_empty()).collect();
        let Some(ti) = lines.iter().position(|l| l.contains("-->")) else {
            // Cabeçalho WEBVTT, NOTE, STYLE ou índice solto.
            let first = lines.first().map(|l| l.trim()).unwrap_or("");
            if !(first.is_empty() || first.starts_with("WEBVTT") || first.starts_with("NOTE") || first.starts_with("STYLE") || first.starts_with("REGION")) {
                skipped += 1;
            }
            continue;
        };
        let (a, b) = lines[ti].split_once("-->").unwrap();
        // VTT permite configurações após o tempo final ("00:01.000 align:start").
        let b = b.split_whitespace().next().unwrap_or("");
        match (parse_ts(a), parse_ts(b)) {
            (Some(start), Some(end)) if end > start => {
                let body = lines[ti + 1..].iter().map(|l| strip_tags(l.trim())).collect::<Vec<_>>().join("\n");
                if body.trim().is_empty() {
                    skipped += 1;
                } else {
                    cues.push(Cue { start, end, text: body });
                }
            }
            _ => skipped += 1,
        }
    }
    cues.sort_by_key(|c| c.start);
    (cues, skipped)
}

fn fmt_ts(t: Time, sep: char) -> String {
    let ms = (t.flicks() as i128 * 1000 / crate::time::FLICKS_PER_SECOND as i128).max(0) as i64;
    format!("{:02}:{:02}:{:02}{sep}{:03}", ms / 3_600_000, (ms / 60_000) % 60, (ms / 1000) % 60, ms % 1000)
}

pub fn write(cues: &[Cue], fmt: SubtitleFormat) -> String {
    let mut out = String::new();
    if fmt == SubtitleFormat::Vtt {
        out.push_str("WEBVTT\n\n");
    }
    for (i, c) in cues.iter().enumerate() {
        match fmt {
            SubtitleFormat::Srt => out.push_str(&format!("{}\n{} --> {}\n{}\n\n", i + 1, fmt_ts(c.start, ','), fmt_ts(c.end, ','), c.text.trim())),
            SubtitleFormat::Vtt => out.push_str(&format!("{} --> {}\n{}\n\n", fmt_ts(c.start, '.'), fmt_ts(c.end, '.'), c.text.trim())),
        }
    }
    out
}

/// Legendas a partir dos textos de uma trilha, com deslocamento opcional
/// (ex.: exportar só um intervalo).
pub fn cues_from_track(track: &Track, offset: Time) -> Vec<Cue> {
    track
        .clips
        .iter()
        .filter_map(|c| match &c.content {
            ClipContent::Text { text, .. } => Some(Cue { start: (c.start - offset).max(Time::ZERO), end: c.end() - offset, text: text.clone() }),
            _ => None,
        })
        .filter(|c| c.end > c.start)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    const SRT: &str = "\u{feff}1\r\n00:00:01,000 --> 00:00:02,500\r\nOlá, <i>mundo</i>!\r\n\r\n2\r\n00:00:03,000 --> 00:00:04,000\r\nSegunda\r\nlinha dupla\r\n\r\n3\r\nlixo --> 00:00:05,000\r\nquebrado\r\n";
    const VTT: &str =
        "WEBVTT\n\nNOTE comentário\n\n00:01.000 --> 00:02.500 align:start\nOlá &amp; tchau\n\nid-2\n01:00:03.000 --> 01:00:04.000\n<c.yellow>Cor</c>\n";

    #[test]
    fn parse_srt_and_vtt() {
        let (c, skipped) = parse(SRT);
        assert_eq!(c.len(), 2);
        assert_eq!(skipped, 1);
        assert_eq!(c[0].start, Time::from_millis(1000));
        assert_eq!(c[0].end, Time::from_millis(2500));
        assert_eq!(c[0].text, "Olá, mundo!");
        assert_eq!(c[1].text, "Segunda\nlinha dupla");
        let (v, sk) = parse(VTT);
        assert_eq!(sk, 0);
        assert_eq!(v[0].text, "Olá & tchau");
        assert_eq!(v[1].start, Time::from_secs(3603));
        assert_eq!(v[1].text, "Cor");
    }

    #[test]
    fn roundtrip() {
        let (c, _) = parse(SRT);
        let srt = write(&c, SubtitleFormat::Srt);
        assert!(srt.starts_with("1\n00:00:01,000 --> 00:00:02,500\nOlá, mundo!\n\n2\n"));
        assert_eq!(parse(&srt).0, c);
        let vtt = write(&c, SubtitleFormat::Vtt);
        assert!(vtt.starts_with("WEBVTT\n\n00:00:01.000 --> 00:00:02.500\n"));
        assert_eq!(parse(&vtt).0, c);
    }
}
