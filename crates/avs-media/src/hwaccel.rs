//! Detecção de encoders por hardware.
//!
//! Não confia no nome da GPU nem na lista de `-encoders`: cada candidato é
//! testado codificando alguns frames reais. Só encoders aprovados no teste
//! são oferecidos na exportação.

use crate::ffmpeg::{run_capture, FfmpegTools};
use serde::Serialize;
use std::time::Duration;
use ts_rs::TS;

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct EncoderInfo {
    /// Nome do encoder FFmpeg (ex.: `h264_nvenc`).
    pub name: String,
    pub codec: String,
    pub hardware: bool,
    pub vendor: String,
    /// Compilado na build do FFmpeg.
    pub compiled: bool,
    /// Passou no teste de codificação real.
    pub working: bool,
    pub error: Option<String>,
}

const CANDIDATES: &[(&str, &str, bool, &str)] = &[
    ("libx264", "h264", false, "software"),
    ("libx265", "hevc", false, "software"),
    ("libvpx-vp9", "vp9", false, "software"),
    ("h264_nvenc", "h264", true, "NVIDIA NVENC"),
    ("hevc_nvenc", "hevc", true, "NVIDIA NVENC"),
    ("h264_qsv", "h264", true, "Intel Quick Sync"),
    ("hevc_qsv", "hevc", true, "Intel Quick Sync"),
    ("h264_amf", "h264", true, "AMD AMF"),
    ("hevc_amf", "hevc", true, "AMD AMF"),
    ("h264_videotoolbox", "h264", true, "Apple VideoToolbox"),
    ("hevc_videotoolbox", "hevc", true, "Apple VideoToolbox"),
];

pub fn detect_encoders(tools: &FfmpegTools) -> Vec<EncoderInfo> {
    let listed = {
        let mut c = tools.ffmpeg_cmd();
        c.args(["-v", "error", "-encoders"]);
        run_capture(c, Some(Duration::from_secs(20))).map(|b| String::from_utf8_lossy(&b).into_owned()).unwrap_or_default()
    };
    let compiled = |name: &str| listed.lines().any(|l| l.split_whitespace().nth(1) == Some(name));

    std::thread::scope(|s| {
        let handles: Vec<_> = CANDIDATES
            .iter()
            .map(|&(name, codec, hw, vendor)| {
                let is_compiled = compiled(name);
                s.spawn(move || {
                    let (working, error) = if is_compiled { test_encoder(tools, name) } else { (false, None) };
                    EncoderInfo { name: name.into(), codec: codec.into(), hardware: hw, vendor: vendor.into(), compiled: is_compiled, working, error }
                })
            })
            .collect();
        handles.into_iter().filter_map(|h| h.join().ok()).collect()
    })
}

fn test_encoder(tools: &FfmpegTools, name: &str) -> (bool, Option<String>) {
    let mut c = tools.ffmpeg_cmd();
    c.args(["-v", "error", "-f", "lavfi", "-i", "testsrc2=s=320x240:r=30:d=0.2", "-frames:v", "5", "-pix_fmt", "yuv420p", "-c:v", name, "-f", "null", "-"]);
    match run_capture(c, Some(Duration::from_secs(20))) {
        Ok(_) => (true, None),
        Err(e) => (false, Some(crate::ffmpeg::tail(&e.to_string(), 3))),
    }
}

/// Escolhe o melhor encoder funcional para o codec: hardware primeiro.
pub fn best_encoder<'a>(encoders: &'a [EncoderInfo], codec: &str, prefer_hw: bool) -> Option<&'a EncoderInfo> {
    let mut list: Vec<&EncoderInfo> = encoders.iter().filter(|e| e.codec == codec && e.working).collect();
    list.sort_by_key(|e| if e.hardware == prefer_hw { 0 } else { 1 });
    list.first().copied()
}
