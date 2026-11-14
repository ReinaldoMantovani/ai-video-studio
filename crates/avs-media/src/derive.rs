//! Derivados de mídia gerados na importação: poster, faixa de miniaturas,
//! waveform e proxies. Tudo é escrito no cache do projeto; o original nunca
//! é alterado.

use crate::ffmpeg::{run_capture, run_with_progress, CancelToken, FfmpegTools, MediaError, Result};
use avs_core::*;
use std::io::{BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;

/// Picos por segundo armazenados no arquivo de waveform.
pub const WAVEFORM_PEAKS_PER_SEC: u32 = 50;
const WAVEFORM_RATE: u32 = 8000;

fn seek_args(t: Time) -> [String; 2] {
    ["-ss".into(), t.to_ffmpeg()]
}

/// Extrai um único frame (seek rápido por keyframe + decodificação precisa).
pub fn extract_frame(tools: &FfmpegTools, src: &Path, at: Time, height: u32, out: &Path) -> Result<()> {
    let mut c = tools.ffmpeg_cmd();
    c.args(["-v", "error", "-y"]).args(seek_args(at)).arg("-i").arg(src).args([
        "-frames:v",
        "1",
        "-vf",
        &format!("scale=-2:{height}:flags=bilinear"),
        "-q:v",
        "4",
    ]);
    c.arg(out);
    run_capture(c, Some(Duration::from_secs(60)))?;
    if !out.is_file() {
        return Err(MediaError::InvalidMedia(format!("não foi possível extrair frame em {at}")));
    }
    Ok(())
}

pub fn make_poster(tools: &FfmpegTools, asset: &MediaAsset, out: &Path) -> Result<()> {
    let at = if asset.kind == AssetKind::Image { Time::ZERO } else { Time::from_flicks(asset.duration.flicks() / 10) };
    extract_frame(tools, &asset.path, at, 360, out)
}

/// Gera uma sprite horizontal com até `max_tiles` miniaturas igualmente
/// espaçadas. Cada miniatura usa seek independente (custo constante por
/// miniatura, mesmo em arquivos de horas), com paralelismo limitado.
pub fn make_thumb_strip(tools: &FfmpegTools, asset: &MediaAsset, dir: &Path, max_tiles: u32, parallel: usize) -> Result<ThumbStrip> {
    let v = asset.video.as_ref().ok_or_else(|| MediaError::InvalidMedia("sem vídeo".into()))?;
    std::fs::create_dir_all(dir)?;
    let tile_h = 72u32;
    let tile_w = ((v.width as f64 / v.height as f64) * tile_h as f64).round() as u32 & !1;
    let secs = asset.duration.as_secs_f64();
    let count = if asset.kind == AssetKind::Image { 1 } else { (secs.ceil() as u32).clamp(1, max_tiles) };
    let interval = Time::from_flicks(asset.duration.flicks() / count as i64);
    let times: Vec<(usize, Time)> = (0..count).map(|i| (i as usize, Time::from_flicks(interval.flicks() * i as i64 + interval.flicks() / 2))).collect();

    let tmp = dir.join(".tiles");
    std::fs::create_dir_all(&tmp)?;
    let errors = std::sync::Mutex::new(vec![]);
    let next = std::sync::atomic::AtomicUsize::new(0);
    std::thread::scope(|s| {
        for _ in 0..parallel.max(1) {
            s.spawn(|| loop {
                let i = next.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                let Some((idx, t)) = times.get(i) else { break };
                let out = tmp.join(format!("t{idx:04}.jpg"));
                if let Err(e) = extract_frame(tools, &asset.path, *t, tile_h, &out) {
                    errors.lock().unwrap().push(e.to_string());
                }
            });
        }
    });
    // Tile final: miniaturas que falharam (ex.: final truncado) viram a anterior.
    let mut last_ok: Option<PathBuf> = None;
    for (idx, _) in &times {
        let p = tmp.join(format!("t{idx:04}.jpg"));
        if p.is_file() {
            last_ok = Some(p);
        } else if let Some(prev) = &last_ok {
            std::fs::copy(prev, &p)?;
        }
    }
    if last_ok.is_none() {
        return Err(MediaError::InvalidMedia(format!("falha ao gerar miniaturas: {:?}", errors.into_inner().unwrap())));
    }
    // Primeira posição pode ter falhado: preenche com a primeira válida.
    let first_ok = (0..count).map(|i| tmp.join(format!("t{i:04}.jpg"))).find(|p| p.is_file()).unwrap();
    for i in 0..count {
        let p = tmp.join(format!("t{i:04}.jpg"));
        if !p.is_file() {
            std::fs::copy(&first_ok, &p)?;
        }
    }
    let out = dir.join("thumbs.jpg");
    let mut c = tools.ffmpeg_cmd();
    c.args(["-v", "error", "-y", "-start_number", "0", "-i"])
        .arg(tmp.join("t%04d.jpg"))
        .args(["-vf", &format!("scale={tile_w}:{tile_h},tile={count}x1"), "-frames:v", "1", "-q:v", "5"])
        .arg(&out);
    run_capture(c, Some(Duration::from_secs(120)))?;
    let _ = std::fs::remove_dir_all(&tmp);
    Ok(ThumbStrip { path: out, count, tile_width: tile_w, tile_height: tile_h, interval })
}

/// Waveform: decodifica o primeiro stream de áudio em mono 8 kHz por pipe
/// (memória constante) e grava um pico `u8` a cada 1/50 s.
pub fn make_waveform(tools: &FfmpegTools, asset: &MediaAsset, out: &Path, cancel: &CancelToken) -> Result<u64> {
    let mut c = tools.ffmpeg_cmd();
    c.args(["-v", "error", "-i"])
        .arg(&asset.path)
        .args(["-map", "0:a:0", "-ac", "1", "-ar", &WAVEFORM_RATE.to_string(), "-f", "s16le", "-acodec", "pcm_s16le", "pipe:1"])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    let mut child = c.spawn().map_err(|e| MediaError::Spawn { tool: "ffmpeg".into(), message: e.to_string() })?;
    let mut stderr = child.stderr.take().unwrap();
    let err_t = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = stderr.read_to_string(&mut s);
        s
    });
    let mut reader = BufReader::with_capacity(1 << 16, child.stdout.take().unwrap());
    let bucket = (WAVEFORM_RATE / WAVEFORM_PEAKS_PER_SEC) as usize;
    let mut file = std::io::BufWriter::new(std::fs::File::create(out)?);
    let mut buf = vec![0u8; bucket * 2 * 64];
    let (mut peak, mut n, mut written) = (0i32, 0usize, 0u64);
    // Um pipe pode entregar um número ímpar de bytes: o byte que sobra é o início
    // da próxima amostra e precisa ser guardado, senão as amostras seguintes saem
    // desalinhadas (lixo na waveform).
    let mut carry: Option<u8> = None;
    loop {
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(MediaError::Cancelled);
        }
        let read = reader.read(&mut buf)?;
        if read == 0 {
            break;
        }
        let mut bytes = &buf[..read];
        let mut first = None;
        if let Some(lo) = carry.take() {
            first = Some([lo, bytes[0]]);
            bytes = &bytes[1..];
        }
        if bytes.len() % 2 == 1 {
            carry = Some(bytes[bytes.len() - 1]);
            bytes = &bytes[..bytes.len() - 1];
        }
        let pairs = bytes.iter().step_by(2).zip(bytes.iter().skip(1).step_by(2)).map(|(&a, &b)| [a, b]);
        for s in first.into_iter().chain(pairs) {
            let v = (i16::from_le_bytes(s) as i32).abs();
            peak = peak.max(v);
            n += 1;
            if n == bucket {
                file.write_all(&[((peak * 255) / 32768) as u8])?;
                written += 1;
                peak = 0;
                n = 0;
            }
        }
    }
    if n > 0 {
        file.write_all(&[((peak * 255) / 32768) as u8])?;
        written += 1;
    }
    file.flush()?;
    let status = child.wait()?;
    let err = err_t.join().unwrap_or_default();
    if !status.success() {
        return Err(MediaError::Failed { tool: "ffmpeg".into(), code: status.code(), stderr: crate::ffmpeg::tail(&err, 10) });
    }
    Ok(written)
}

/// Reamostra picos de waveform para `buckets` colunas no intervalo dado.
pub fn waveform_slice(peaks: &[u8], from: Time, to: Time, buckets: usize) -> Vec<u8> {
    if buckets == 0 || to <= from || peaks.is_empty() {
        return vec![];
    }
    let pps = WAVEFORM_PEAKS_PER_SEC as f64;
    let a = (from.as_secs_f64() * pps).max(0.0);
    let b = (to.as_secs_f64() * pps).max(a);
    let step = (b - a) / buckets as f64;
    (0..buckets)
        .map(|i| {
            let s = (a + step * i as f64).floor() as usize;
            let e = ((a + step * (i + 1) as f64).ceil() as usize).max(s + 1);
            peaks.get(s.min(peaks.len())..e.min(peaks.len())).and_then(|w| w.iter().max().copied()).unwrap_or(0)
        })
        .collect()
}

/// Proxy é recomendado para material pesado de decodificar.
pub fn needs_proxy(asset: &MediaAsset) -> bool {
    let Some(v) = &asset.video else { return false };
    if asset.kind != AssetKind::Video {
        return false;
    }
    let heavy_codec = matches!(v.codec.as_str(), "hevc" | "prores" | "dnxhd" | "av1" | "vp9" | "cfhd" | "h264" if v.height > 1080)
        || matches!(v.codec.as_str(), "prores" | "dnxhd" | "cfhd" | "hevc" | "av1");
    v.height > 1080 || heavy_codec || v.bit_depth.unwrap_or(8) > 8 || v.vfr
}

/// Proxy 540p H.264 com GOP curto (scrubbing rápido), FPS constante
/// igual ao nominal (resolve VFR no preview) e áudio AAC.
pub fn make_proxy(tools: &FfmpegTools, asset: &MediaAsset, out: &Path, cancel: &CancelToken, mut progress: impl FnMut(f32)) -> Result<()> {
    let v = asset.video.as_ref().ok_or_else(|| MediaError::InvalidMedia("sem vídeo".into()))?;
    let tmp = out.with_extension("part.mp4");
    let mut c = tools.ffmpeg_cmd();
    c.args(["-y", "-v", "error", "-progress", "pipe:1", "-nostats", "-i"]).arg(&asset.path).args([
        "-map",
        "0:v:0",
        "-map",
        "0:a:0?",
        "-vf",
        &format!("scale=-2:'min(540,ih)':flags=bilinear,fps={},format=yuv420p", v.fps.to_ffmpeg()),
        "-c:v",
        "libx264",
        "-preset",
        "veryfast",
        "-crf",
        "24",
        "-g",
        "15",
        "-c:a",
        "aac",
        "-b:a",
        "128k",
        "-movflags",
        "+faststart",
    ]);
    c.arg(&tmp);
    let total = asset.duration.as_secs_f64().max(0.001);
    run_with_progress(c, cancel, |t| progress((t.as_secs_f64() / total).clamp(0.0, 1.0) as f32))?;
    std::fs::rename(&tmp, out)?;
    Ok(())
}
