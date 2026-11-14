//! Leitura de metadados com FFprobe e impressão digital de arquivos.

use crate::ffmpeg::{run_capture, FfmpegTools, MediaError, Result};
use avs_core::*;
use serde::Deserialize;
use sha2::{Digest, Sha256};
use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::path::Path;
use std::time::Duration;

#[derive(Debug, Deserialize)]
struct ProbeJson {
    #[serde(default)]
    streams: Vec<StreamJson>,
    format: Option<FormatJson>,
}

#[derive(Debug, Deserialize)]
struct FormatJson {
    format_name: Option<String>,
    duration: Option<String>,
    size: Option<String>,
}

#[derive(Debug, Deserialize)]
struct StreamJson {
    codec_type: Option<String>,
    codec_name: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    r_frame_rate: Option<String>,
    avg_frame_rate: Option<String>,
    time_base: Option<String>,
    pix_fmt: Option<String>,
    bits_per_raw_sample: Option<String>,
    sample_rate: Option<String>,
    channels: Option<u32>,
    duration: Option<String>,
    nb_frames: Option<String>,
    #[serde(default)]
    disposition: Option<serde_json::Value>,
    #[serde(default)]
    tags: Option<serde_json::Value>,
    #[serde(default)]
    side_data_list: Option<Vec<serde_json::Value>>,
}

/// Resultado bruto do probe, já convertido para o modelo.
#[derive(Debug, Clone)]
pub struct ProbeInfo {
    pub kind: AssetKind,
    pub container: String,
    pub duration: Time,
    pub size_bytes: u64,
    pub video: Option<VideoInfo>,
    pub audio: Option<AudioInfo>,
}

const IMAGE_CODECS: &[&str] = &["png", "mjpeg", "webp", "bmp", "tiff", "gif", "jpegls"];

pub fn probe(tools: &FfmpegTools, path: &Path) -> Result<ProbeInfo> {
    if !path.is_file() {
        return Err(MediaError::InvalidMedia(format!("arquivo não encontrado: {}", path.display())));
    }
    let mut cmd = tools.ffprobe_cmd();
    cmd.args(["-v", "error", "-print_format", "json", "-show_format", "-show_streams"]).arg(path);
    let out = run_capture(cmd, Some(Duration::from_secs(60))).map_err(|e| match e {
        MediaError::Failed { stderr, .. } => MediaError::InvalidMedia(format!("{}: {}", path.display(), stderr)),
        other => other,
    })?;
    let j: ProbeJson = serde_json::from_slice(&out)?;
    parse_probe(j, path)
}

fn parse_probe(j: ProbeJson, path: &Path) -> Result<ProbeInfo> {
    let fmt = j.format.ok_or_else(|| MediaError::InvalidMedia("sem informações de formato".into()))?;
    let container = fmt.format_name.unwrap_or_default();
    let size_bytes = fmt.size.and_then(|s| s.parse().ok()).unwrap_or_else(|| std::fs::metadata(path).map(|m| m.len()).unwrap_or(0));

    // Ignora capas embutidas (attached_pic) ao escolher o stream de vídeo.
    let is_cover = |s: &StreamJson| s.disposition.as_ref().and_then(|d| d.get("attached_pic")).and_then(|v| v.as_i64()).unwrap_or(0) == 1;
    let vs = j.streams.iter().find(|s| s.codec_type.as_deref() == Some("video") && !is_cover(s));
    let audio_streams: Vec<&StreamJson> = j.streams.iter().filter(|s| s.codec_type.as_deref() == Some("audio")).collect();

    let video = vs.map(|s| {
        let fps = s.r_frame_rate.as_deref().and_then(Rational::parse).filter(|r| r.as_f64() <= 1000.0).unwrap_or(Rational::new(30, 1));
        let avg = s.avg_frame_rate.as_deref().and_then(Rational::parse);
        // VFR: taxa média difere da nominal em mais de 0,5 %.
        let vfr = avg.map(|a| ((a.as_f64() - fps.as_f64()).abs() / fps.as_f64()) > 0.005).unwrap_or(false);
        let rotation = rotation_of(s);
        let (mut w, mut h) = (s.width.unwrap_or(0), s.height.unwrap_or(0));
        if rotation.abs() % 180 == 90 {
            std::mem::swap(&mut w, &mut h); // dimensões de exibição
        }
        VideoInfo {
            codec: s.codec_name.clone().unwrap_or_default(),
            width: w,
            height: h,
            fps,
            avg_fps: avg,
            time_base: s.time_base.as_deref().and_then(Rational::parse),
            vfr,
            pix_fmt: s.pix_fmt.clone(),
            bit_depth: s.bits_per_raw_sample.as_deref().and_then(|b| b.parse().ok()).or_else(|| {
                s.pix_fmt.as_deref().map(|p| {
                    if p.contains("10") {
                        10
                    } else if p.contains("12") {
                        12
                    } else {
                        8
                    }
                })
            }),
            rotation,
        }
    });
    let audio = audio_streams.first().map(|s| AudioInfo {
        codec: s.codec_name.clone().unwrap_or_default(),
        sample_rate: s.sample_rate.as_deref().and_then(|r| r.parse().ok()).unwrap_or(48_000),
        channels: s.channels.unwrap_or(2),
        stream_count: audio_streams.len() as u32,
    });

    let is_image = vs.map(|s| {
        let codec = s.codec_name.as_deref().unwrap_or("");
        let single = s.nb_frames.as_deref().map(|n| n == "1").unwrap_or(false) || s.duration.is_none();
        container.contains("image2") || container.ends_with("_pipe") || (IMAGE_CODECS.contains(&codec) && single && audio.is_none())
    }) == Some(true);

    let kind = if is_image {
        AssetKind::Image
    } else if video.is_some() {
        AssetKind::Video
    } else if audio.is_some() {
        AssetKind::Audio
    } else {
        return Err(MediaError::InvalidMedia(format!("{} não contém vídeo, áudio ou imagem", path.display())));
    };

    let dur_s = fmt
        .duration
        .as_deref()
        .and_then(|d| d.parse::<f64>().ok())
        .or_else(|| vs.and_then(|s| s.duration.as_deref()).and_then(|d| d.parse().ok()))
        .unwrap_or(0.0);
    let duration = if kind == AssetKind::Image { Time::ZERO } else { Time::from_secs_f64(dur_s) };
    if kind != AssetKind::Image && !duration.is_positive() {
        return Err(MediaError::InvalidMedia(format!("{} tem duração desconhecida ou zero (arquivo corrompido?)", path.display())));
    }
    if let Some(v) = &video {
        if v.width == 0 || v.height == 0 {
            return Err(MediaError::InvalidMedia("stream de vídeo sem dimensões".into()));
        }
    }
    Ok(ProbeInfo { kind, container, duration, size_bytes, video, audio })
}

fn rotation_of(s: &StreamJson) -> i32 {
    if let Some(list) = &s.side_data_list {
        for sd in list {
            if let Some(r) = sd.get("rotation").and_then(|v| v.as_f64()) {
                return r.round() as i32;
            }
        }
    }
    s.tags.as_ref().and_then(|t| t.get("rotate")).and_then(|v| v.as_str()).and_then(|v| v.parse().ok()).unwrap_or(0)
}

/// Impressão digital rápida (tamanho + SHA-256 do primeiro e do último MiB).
/// Lê no máximo 2 MiB, mesmo para arquivos de várias horas; permite
/// relocalizar mídia movida sem confundir arquivos diferentes.
pub fn fingerprint(path: &Path) -> Result<String> {
    const CHUNK: u64 = 1 << 20;
    let mut f = File::open(path)?;
    let len = f.metadata()?.len();
    let mut h = Sha256::new();
    h.update(len.to_le_bytes());
    let mut buf = vec![0u8; CHUNK.min(len) as usize];
    f.read_exact(&mut buf)?;
    h.update(&buf);
    if len > CHUNK {
        let tail = CHUNK.min(len - CHUNK);
        f.seek(SeekFrom::End(-(tail as i64)))?;
        let mut b2 = vec![0u8; tail as usize];
        f.read_exact(&mut b2)?;
        h.update(&b2);
    }
    Ok(format!("{len:x}-{}", &hex::encode(h.finalize())[..32]))
}

/// Procura recursivamente em `root` um arquivo com a mesma impressão digital
/// (para relocalizar mídias movidas). Considera apenas arquivos de mesmo tamanho.
pub fn find_by_fingerprint(root: &Path, fp: &str, max_depth: usize) -> Option<std::path::PathBuf> {
    let size = u64::from_str_radix(fp.split('-').next()?, 16).ok()?;
    let mut stack = vec![(root.to_path_buf(), 0usize)];
    while let Some((dir, depth)) = stack.pop() {
        let Ok(rd) = std::fs::read_dir(&dir) else { continue };
        for e in rd.flatten() {
            let p = e.path();
            let Ok(md) = e.metadata() else { continue };
            if md.is_dir() && depth < max_depth {
                stack.push((p, depth + 1));
            } else if md.is_file() && md.len() == size && fingerprint(&p).ok().as_deref() == Some(fp) {
                return Some(p);
            }
        }
    }
    None
}

pub fn build_asset(info: &ProbeInfo, path: &Path, fp: String) -> MediaAsset {
    MediaAsset {
        id: new_id(),
        kind: info.kind,
        name: path.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        path: path.to_path_buf(),
        fingerprint: fp,
        size_bytes: info.size_bytes,
        duration: info.duration,
        container: info.container.clone(),
        video: info.video.clone(),
        audio: info.audio.clone(),
        proxy_path: None,
        thumbs: None,
        waveform_path: None,
        poster_path: None,
        status: AssetStatus::Processing,
    }
}
