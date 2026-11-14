//! Validação do arquivo exportado com FFprobe (+ passada de decodificação).
//! Uma exportação só é considerada concluída se nenhuma verificação falhar.

use crate::ffmpeg::{run_capture, run_stderr_timeout, CancelToken, FfmpegTools, Result};
use avs_core::*;
use serde::{Deserialize, Serialize};
use std::path::Path;
use std::time::Duration;
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum CheckStatus {
    Pass,
    Warn,
    Fail,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Check {
    pub id: String,
    pub label: String,
    pub status: CheckStatus,
    pub detail: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct ValidationReport {
    pub checks: Vec<Check>,
    pub ok: bool,
    pub warnings: u32,
}

#[derive(Debug, Clone)]
pub struct Expectation {
    pub duration: Time,
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
    /// Família do codec esperado (h264, hevc, vp9).
    pub video_codec: String,
    pub audio: bool,
    /// Trechos (relativos ao início do arquivo) com vídeo visível.
    pub covered_video: Vec<TimeRange>,
    /// Trechos (relativos) com áudio audível.
    pub covered_audio: Vec<TimeRange>,
}

pub fn codec_family(encoder: &str) -> String {
    if encoder.contains("264") {
        "h264".into()
    } else if encoder.contains("265") || encoder.starts_with("hevc") {
        "hevc".into()
    } else if encoder.contains("vp9") {
        "vp9".into()
    } else if encoder.contains("av1") {
        "av1".into()
    } else {
        encoder.into()
    }
}

#[derive(Deserialize)]
struct Pj {
    #[serde(default)]
    streams: Vec<Ps>,
    format: Option<Pf>,
}
#[derive(Deserialize)]
struct Ps {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
    duration: Option<String>,
    nb_frames: Option<String>,
}
#[derive(Deserialize)]
struct Pf {
    duration: Option<String>,
}

fn push(v: &mut Vec<Check>, id: &str, label: &str, status: CheckStatus, detail: impl Into<String>) {
    v.push(Check { id: id.into(), label: label.into(), status, detail: detail.into() });
}

pub fn validate_output(tools: &FfmpegTools, path: &Path, exp: &Expectation, deep: bool, cancel: &CancelToken) -> Result<ValidationReport> {
    use CheckStatus::*;
    let mut c = vec![];
    let size = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        push(&mut c, "exists", "Arquivo gerado", Fail, format!("{} não existe ou está vazio", path.display()));
        return Ok(finish(c));
    }
    push(&mut c, "exists", "Arquivo gerado", Pass, format!("{:.1} MB", size as f64 / 1e6));

    let mut cmd = tools.ffprobe_cmd();
    cmd.args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"]).arg(path);
    let j: Pj = match run_capture(cmd, Some(Duration::from_secs(60))).and_then(|b| Ok(serde_json::from_slice(&b)?)) {
        Ok(j) => j,
        Err(e) => {
            push(&mut c, "probe", "Leitura do contêiner", Fail, e.to_string());
            return Ok(finish(c));
        }
    };
    let v = j.streams.iter().find(|s| s.codec_type.as_deref() == Some("video"));
    let a = j.streams.iter().find(|s| s.codec_type.as_deref() == Some("audio"));
    let Some(v) = v else {
        push(&mut c, "video", "Stream de vídeo", Fail, "nenhum stream de vídeo no arquivo");
        return Ok(finish(c));
    };
    let (w, h) = (v.width.unwrap_or(0), v.height.unwrap_or(0));
    push(
        &mut c,
        "resolution",
        "Resolução",
        if (w, h) == (exp.width, exp.height) { Pass } else { Fail },
        format!("{w}x{h} (esperado {}x{})", exp.width, exp.height),
    );

    let fps = v.r_frame_rate.as_deref().and_then(Rational::parse);
    let fps_ok = fps.map(|f| (f.as_f64() - exp.fps.as_f64()).abs() < 0.01).unwrap_or(false);
    push(
        &mut c,
        "fps",
        "Taxa de quadros",
        if fps_ok { Pass } else { Fail },
        format!("{} (esperado {})", fps.map(|f| f.to_string()).unwrap_or("?".into()), exp.fps),
    );

    let codec = v.codec_name.clone().unwrap_or_default();
    push(&mut c, "codec", "Codec de vídeo", if codec == exp.video_codec { Pass } else { Fail }, format!("{codec} (esperado {})", exp.video_codec));

    let vdur = v
        .duration
        .as_deref()
        .and_then(|d| d.parse::<f64>().ok())
        .or_else(|| v.nb_frames.as_deref().and_then(|n| n.parse::<f64>().ok()).zip(fps).map(|(n, f)| n / f.as_f64()))
        .or_else(|| j.format.as_ref().and_then(|f| f.duration.as_deref()).and_then(|d| d.parse().ok()))
        .unwrap_or(0.0);
    let expd = exp.duration.as_secs_f64();
    let tol = (2.0 / exp.fps.as_f64()).max(0.05);
    push(
        &mut c,
        "duration",
        "Duração",
        if (vdur - expd).abs() <= tol { Pass } else { Fail },
        format!("{vdur:.3}s (esperado {expd:.3}s, tolerância {tol:.3}s)"),
    );
    if let Some(n) = v.nb_frames.as_deref().and_then(|n| n.parse::<i64>().ok()) {
        let expf = exp.duration.frame_floor(exp.fps);
        push(&mut c, "frames", "Contagem de frames", if (n - expf).abs() <= 1 { Pass } else { Warn }, format!("{n} (esperado {expf})"));
    }

    match (a, exp.audio) {
        (Some(a), true) => {
            let ad = a.duration.as_deref().and_then(|d| d.parse::<f64>().ok()).unwrap_or(vdur);
            let drift = (ad - vdur).abs();
            push(&mut c, "audio", "Stream de áudio", Pass, a.codec_name.clone().unwrap_or_default());
            push(
                &mut c,
                "sync",
                "Sincronismo A/V (duração)",
                if drift <= 0.1 { Pass } else { Warn },
                format!("diferença entre streams: {:.0} ms", drift * 1000.0),
            );
        }
        (None, true) => push(&mut c, "audio", "Stream de áudio", Fail, "áudio esperado mas ausente"),
        (_, false) => {}
    }

    if deep {
        let mut cmd = tools.ffmpeg_cmd();
        cmd.args(["-v", "info", "-nostats", "-i"]).arg(path);
        cmd.args(["-vf", "blackdetect=d=0.5:pix_th=0.08"]);
        if exp.audio {
            cmd.args(["-af", "silencedetect=n=-60dB:d=1.5"]);
        }
        cmd.args(["-f", "null", "-"]);
        // Decodificar é bem mais rápido que tempo real; 4× a duração (mín. 2 min) só
        // estoura se o FFmpeg travar.
        let limit = Duration::from_secs_f64((exp.duration.as_secs_f64() * 4.0).max(120.0));
        let log = run_stderr_timeout(cmd, cancel, Some(limit))?;
        let decode_errors: Vec<&str> = log
            .lines()
            .filter(|l| {
                let l = l.to_ascii_lowercase();
                (l.contains("error") || l.contains("invalid data") || l.contains("corrupt")) && !l.contains("blackdetect") && !l.contains("silencedetect")
            })
            .take(5)
            .collect();
        push(
            &mut c,
            "decode",
            "Integridade (decodificação completa)",
            if decode_errors.is_empty() { Pass } else { Fail },
            if decode_errors.is_empty() { "sem erros de decodificação".to_string() } else { decode_errors.join(" | ") },
        );

        let blacks = parse_pairs(&log, "black_start:", "black_end:");
        let unexpected_black: Vec<(f64, f64)> = blacks.into_iter().filter(|(s, e)| overlaps_secs(&exp.covered_video, *s, *e) > 0.5).collect();
        push(
            &mut c,
            "black",
            "Frames pretos inesperados",
            if unexpected_black.is_empty() { Pass } else { Warn },
            if unexpected_black.is_empty() {
                "nenhum".into()
            } else {
                format!(
                    "{} trecho(s), ex.: {:.2}s–{:.2}s (pode ser do material original)",
                    unexpected_black.len(),
                    unexpected_black[0].0,
                    unexpected_black[0].1
                )
            },
        );
        if exp.audio {
            let sil = parse_pairs(&log, "silence_start:", "silence_end:");
            let unexpected: Vec<(f64, f64)> = sil.into_iter().filter(|(s, e)| overlaps_secs(&exp.covered_audio, *s, *e) > 1.0).collect();
            push(
                &mut c,
                "silence",
                "Silêncio acidental",
                if unexpected.is_empty() { Pass } else { Warn },
                if unexpected.is_empty() {
                    "nenhum".into()
                } else {
                    format!("{} trecho(s) silencioso(s) onde há clipes com áudio, ex.: {:.2}s–{:.2}s", unexpected.len(), unexpected[0].0, unexpected[0].1)
                },
            );
        }
    }
    Ok(finish(c))
}

fn finish(checks: Vec<Check>) -> ValidationReport {
    let ok = !checks.iter().any(|c| c.status == CheckStatus::Fail);
    let warnings = checks.iter().filter(|c| c.status == CheckStatus::Warn).count() as u32;
    ValidationReport { checks, ok, warnings }
}

fn parse_pairs(log: &str, start_key: &str, end_key: &str) -> Vec<(f64, f64)> {
    let num_after = |l: &str, k: &str| -> Option<f64> {
        let i = l.find(k)? + k.len();
        l[i..].trim_start().split(|c: char| c.is_whitespace() || c == '|').next()?.parse().ok()
    };
    let mut out = vec![];
    let mut pending: Option<f64> = None;
    for l in log.lines() {
        if let Some(s) = num_after(l, start_key) {
            pending = Some(s);
        }
        if let Some(e) = num_after(l, end_key) {
            out.push((pending.take().unwrap_or(0.0), e));
        }
    }
    if let Some(s) = pending {
        out.push((s, f64::MAX));
    }
    out
}

fn overlaps_secs(ranges: &[TimeRange], s: f64, e: f64) -> f64 {
    ranges.iter().map(|r| (r.end.as_secs_f64().min(e) - r.start.as_secs_f64().max(s)).max(0.0)).sum()
}
