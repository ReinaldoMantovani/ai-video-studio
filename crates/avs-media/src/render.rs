//! Renderização incremental por blocos.
//!
//! A timeline é dividida em blocos de N frames. Cada bloco é compilado de
//! forma independente; a chave de cache é o hash do grafo + argumentos +
//! identidade dos arquivos de entrada. Depois de uma edição pequena, apenas
//! os blocos cujo grafo mudou são renderizados de novo; os demais são
//! reaproveitados. Os blocos são unidos sem recodificar o vídeo (demuxer
//! concat) e o áudio — mantido em PCM nos blocos, sem "priming" de AAC — é
//! codificado uma única vez no arquivo final.
//!
//! O mesmo caminho atende preview (baixa resolução, encoder rápido) e
//! exportação final (qualidade configurável, encoder por hardware).

use crate::compile::{compile, write_text_files, CompileOptions, CompiledGraph, AUDIO_OUT, VIDEO_OUT};
use crate::ffmpeg::{escape_concat_path, run_capture, run_with_progress, CancelToken, FfmpegTools, MediaError, Result};
use crate::fonts::FontResolver;
use avs_core::*;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::{Duration, Instant};
use ts_rs::TS;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum Container {
    Mp4,
    Mov,
    Webm,
}

impl Container {
    pub fn ext(self) -> &'static str {
        match self {
            Container::Mp4 => "mp4",
            Container::Mov => "mov",
            Container::Webm => "webm",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VideoEncode {
    /// Encoder FFmpeg (libx264, h264_nvenc, libx265, libvpx-vp9…).
    pub encoder: String,
    /// Qualidade estilo CRF (0 = sem perdas … 51 = pior). Mapeada para cada encoder.
    pub quality: u8,
    /// Velocidade (preset do libx264: ultrafast…veryslow).
    pub speed: String,
    /// Se definido, usa taxa de bits alvo em vez de qualidade constante.
    pub bitrate_kbps: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AudioEncode {
    pub bitrate_kbps: u32,
    /// Normalização de loudness (EBU R128) no estágio final, alvo em LUFS.
    pub loudness_lufs: Option<f32>,
}

/// Estimativa de memória de um processo FFmpeg que renderiza um bloco (MB),
/// a partir de medições reais (1080p, `getrusage`, FFmpeg 6.1): decodificação
/// e grafo ≈ 210 MB; libx264 `veryfast` ≈ +210 MB, `medium` ≈ +420 MB. Escala
/// com a área do quadro.
pub fn chunk_memory_estimate_mb(width: u32, height: u32, encoder: &str, speed: &str) -> u32 {
    let area = (width as f64 * height as f64 / (1920.0 * 1080.0)).max(0.25);
    let enc = match encoder {
        "libx264" => match speed {
            "ultrafast" | "superfast" => 120.0,
            "veryfast" | "faster" => 220.0,
            "fast" => 320.0,
            "medium" => 420.0,
            _ => 700.0,
        },
        "libx265" => match speed {
            "ultrafast" | "superfast" | "veryfast" => 350.0,
            "faster" | "fast" => 500.0,
            _ => 800.0,
        },
        "libvpx-vp9" => 350.0,
        // Encoders por hardware guardam os quadros na GPU.
        _ => 80.0,
    };
    ((210.0 + enc) * area).round() as u32
}

/// Quantos blocos renderizar ao mesmo tempo: limitado pelos núcleos (o x264
/// já usa vários threads por processo), pela memória disponível (até 60%
/// dela) e a 1 em encoders por hardware (sessões limitadas pela GPU).
pub fn plan_parallelism(width: u32, height: u32, encoder: &str, speed: &str, cores: usize, available_mb: u64) -> (usize, u32) {
    let est = chunk_memory_estimate_mb(width, height, encoder, speed);
    if !encoder.starts_with("lib") {
        return (1, est);
    }
    let by_cpu = (cores / 2).clamp(2, 4);
    let by_mem = ((available_mb as f64 * 0.6) / est.max(1) as f64).floor() as usize;
    (by_cpu.min(by_mem).max(1), est)
}

#[derive(Debug, Clone)]
pub struct RenderRequest {
    pub project: Project,
    /// `None` = timeline inteira.
    pub range: Option<TimeRange>,
    pub compile: CompileOptions,
    pub video: VideoEncode,
    pub audio: AudioEncode,
    pub container: Container,
    pub chunk_len: Time,
    pub parallel: usize,
    /// Cache persistente de blocos (por projeto).
    pub chunk_dir: PathBuf,
    /// Arquivos temporários (scripts, listas, textos).
    pub work_dir: PathBuf,
    pub output: PathBuf,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum RenderStage {
    Compiling,
    Rendering,
    Muxing,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RenderProgress {
    pub stage: RenderStage,
    /// 0..1 calculado a partir do tempo realmente processado pelo FFmpeg.
    pub fraction: f32,
    pub chunks_total: u32,
    pub chunks_done: u32,
    pub chunks_reused: u32,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct RenderOutcome {
    pub output: PathBuf,
    pub duration: Time,
    #[ts(type = "number")]
    pub frames: i64,
    pub chunks_total: u32,
    pub chunks_reused: u32,
    #[ts(type = "number")]
    pub elapsed_ms: u64,
}

/// Argumentos do encoder de vídeo, por família.
pub fn video_codec_args(v: &VideoEncode, fps: Rational) -> Vec<String> {
    let gop = ((fps.as_f64() * 2.0).round() as u32).max(1).to_string();
    let q = v.quality.min(51).to_string();
    let mut a: Vec<String> = vec!["-c:v".into(), v.encoder.clone()];
    let e = v.encoder.as_str();
    match (e, v.bitrate_kbps) {
        (_, Some(kbps)) if !e.starts_with("libvpx") => {
            a.extend(["-b:v".into(), format!("{kbps}k"), "-maxrate".into(), format!("{}k", kbps * 3 / 2), "-bufsize".into(), format!("{}k", kbps * 2)]);
        }
        ("libx264" | "libx265", None) => a.extend(["-crf".into(), q]),
        (x, None) if x.ends_with("_nvenc") => a.extend(["-rc".into(), "vbr".into(), "-cq".into(), q, "-b:v".into(), "0".into()]),
        (x, None) if x.ends_with("_qsv") => a.extend(["-global_quality".into(), q]),
        (x, None) if x.ends_with("_amf") => a.extend(["-rc".into(), "cqp".into(), "-qp_i".into(), q.clone(), "-qp_p".into(), q]),
        (x, None) if x.ends_with("_videotoolbox") => {
            // VideoToolbox usa qualidade 1–100 (maior = melhor).
            let vt = (100 - (v.quality.min(51) as u32 * 100 / 51)).clamp(1, 100);
            a.extend(["-q:v".into(), vt.to_string()]);
        }
        (x, b) if x.starts_with("libvpx") => {
            a.extend([
                "-crf".into(),
                q,
                "-b:v".into(),
                b.map(|k| format!("{k}k")).unwrap_or("0".into()),
                "-row-mt".into(),
                "1".into(),
                "-deadline".into(),
                "good".into(),
                "-cpu-used".into(),
                "4".into(),
            ]);
        }
        _ => {}
    }
    match e {
        "libx264" | "libx265" => a.extend(["-preset".into(), v.speed.clone()]),
        x if x.ends_with("_nvenc") => a.extend(["-preset".into(), "p5".into()]),
        _ => {}
    }
    if e == "libx265" || e.starts_with("hevc_") {
        a.extend(["-tag:v".into(), "hvc1".into()]);
    }
    if e == "libx265" {
        a.extend(["-x265-params".into(), "log-level=error".into()]);
    }
    a.extend(["-g".into(), gop, "-pix_fmt".into(), "yuv420p".into()]);
    a
}

fn timescale(fps: Rational) -> u32 {
    let k = 10_000u32.div_ceil(fps.num).max(1);
    fps.num * k
}

fn input_identity(path: &Path) -> String {
    match std::fs::metadata(path) {
        Ok(m) => {
            let mt = m.modified().ok().and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok()).map(|d| d.as_millis()).unwrap_or(0);
            format!("{}|{}|{}", path.display(), m.len(), mt)
        }
        Err(_) => format!("{}|missing", path.display()),
    }
}

struct ChunkPlan {
    graph: CompiledGraph,
    args: Vec<String>,
    script: PathBuf,
    file: PathBuf,
    cached: bool,
}

fn chunk_ranges(range: TimeRange, fps: Rational, chunk_len: Time) -> Vec<TimeRange> {
    let f0 = range.start.frame_floor(fps);
    let f1 = range.end.snap_to_frame(fps).frame_floor(fps);
    let step = chunk_len.frame_floor(fps).max(1);
    let mut out = vec![];
    let mut f = f0;
    while f < f1 {
        let e = (f + step).min(f1);
        out.push(TimeRange::new(Time::from_frames(f, fps), Time::from_frames(e, fps)));
        f = e;
    }
    out
}

/// Renderiza com cache de blocos. Escreve em `<saida>.part` e só renomeia
/// para o nome final ao concluir; a validação é responsabilidade do chamador
/// (ver `validate::validate_output`) antes de anunciar sucesso.
pub fn render(
    tools: &FfmpegTools,
    fonts: &FontResolver,
    req: &RenderRequest,
    cancel: &CancelToken,
    on_progress: &(dyn Fn(RenderProgress) + Sync),
) -> Result<RenderOutcome> {
    let t0 = Instant::now();
    let fps = req.compile.fps;
    let duration = req.project.timeline.duration();
    if !duration.is_positive() {
        return Err(MediaError::InvalidTimeline("a timeline está vazia".into()));
    }
    let range = req.range.unwrap_or(TimeRange::new(Time::ZERO, duration));
    std::fs::create_dir_all(&req.chunk_dir)?;
    std::fs::create_dir_all(&req.work_dir)?;
    // Diretório estável e endereçado por conteúdo: o grafo (e a chave de
    // cache) não muda entre execuções.
    let text_dir = req.chunk_dir.join("text");

    on_progress(RenderProgress { stage: RenderStage::Compiling, fraction: 0.0, chunks_total: 0, chunks_done: 0, chunks_reused: 0 });
    let chunk_ext = if req.container == Container::Webm { "mkv" } else { "mov" };
    let mut codec = video_codec_args(&req.video, fps);
    if chunk_ext == "mov" {
        codec.extend(["-video_track_timescale".into(), timescale(fps).to_string()]);
    }
    let mut plans = vec![];
    for cr in chunk_ranges(range, fps, req.chunk_len) {
        let graph = compile(&req.project, cr, &req.compile, fonts, &text_dir)?;
        let mut h = Sha256::new();
        h.update(tools.version.as_bytes());
        h.update(graph.filter.as_bytes());
        for i in &graph.inputs {
            h.update(input_identity(&i.path).as_bytes());
            h.update(format!("{}|{}|{}|{}", i.seek.flicks(), i.duration.flicks(), i.image, i.audio_only).as_bytes());
        }
        for d in &graph.deps {
            h.update(input_identity(d).as_bytes());
        }
        for (_, txt) in &graph.text_files {
            h.update(txt.as_bytes());
        }
        h.update(codec.join(" ").as_bytes());
        h.update(format!("{}x{}@{}|{}", req.compile.width, req.compile.height, fps.to_ffmpeg(), req.compile.sample_rate).as_bytes());
        let key = hex::encode(h.finalize());
        let file = req.chunk_dir.join(format!("{}.{chunk_ext}", &key[..40]));
        let script = req.work_dir.join(format!("graph-{}.txt", &key[..16]));
        let cached = file.is_file();
        let mut args: Vec<String> = vec!["-y".into(), "-v".into(), "error".into(), "-progress".into(), "pipe:1".into(), "-nostats".into()];
        args.extend(graph.input_args(fps));
        args.extend(tools.filter_script_args(&script));
        args.extend(["-map".into(), VIDEO_OUT.into(), "-map".into(), AUDIO_OUT.into()]);
        args.extend(codec.iter().cloned());
        args.extend([
            "-r".into(),
            fps.to_ffmpeg(),
            "-c:a".into(),
            "pcm_s16le".into(),
            "-ar".into(),
            req.compile.sample_rate.to_string(),
            "-ac".into(),
            "2".into(),
        ]);
        plans.push(ChunkPlan { graph, args, script, file, cached });
    }

    let total_frames: i64 = plans.iter().map(|p| p.graph.frames).sum();
    let total = plans.len() as u32;
    let reused = plans.iter().filter(|p| p.cached).count() as u32;
    let done = AtomicUsize::new(reused as usize);
    // frames concluídos por bloco (para fração real)
    let progress_frames: Mutex<Vec<i64>> = Mutex::new(plans.iter().map(|p| if p.cached { p.graph.frames } else { 0 }).collect());
    let report = |stage: RenderStage, frac: f32| {
        on_progress(RenderProgress { stage, fraction: frac, chunks_total: total, chunks_done: done.load(Ordering::SeqCst) as u32, chunks_reused: reused })
    };
    let render_share = 0.92f32; // o restante é o mux final

    let todo: Vec<usize> = (0..plans.len()).filter(|&i| !plans[i].cached).collect();
    let next = AtomicUsize::new(0);
    let first_err: Mutex<Option<MediaError>> = Mutex::new(None);
    std::thread::scope(|s| {
        for _ in 0..req.parallel.clamp(1, 8) {
            s.spawn(|| loop {
                if first_err.lock().unwrap().is_some() || cancel.is_cancelled() {
                    break;
                }
                let k = next.fetch_add(1, Ordering::SeqCst);
                let Some(&i) = todo.get(k) else { break };
                let plan = &plans[i];
                let res = (|| -> Result<()> {
                    write_text_files(&plan.graph)?;
                    std::fs::write(&plan.script, &plan.graph.filter)?;
                    let tmp = plan.file.with_extension(format!("part.{chunk_ext}"));
                    let mut cmd = tools.ffmpeg_cmd();
                    cmd.args(&plan.args).arg(&tmp);
                    let fd = fps.frame_duration().flicks().max(1);
                    let r = run_with_progress(cmd, cancel, |t| {
                        let f = (t.flicks() / fd).min(plan.graph.frames);
                        let mut pf = progress_frames.lock().unwrap();
                        pf[i] = f;
                        let sum: i64 = pf.iter().sum();
                        drop(pf);
                        report(RenderStage::Rendering, render_share * (sum as f32 / total_frames.max(1) as f32));
                    });
                    if let Err(e) = r {
                        // Bloco parcial nunca entra no cache.
                        let _ = std::fs::remove_file(&tmp);
                        let _ = std::fs::remove_file(&plan.script);
                        return Err(e);
                    }
                    std::fs::rename(&tmp, &plan.file)?;
                    let _ = std::fs::remove_file(&plan.script);
                    progress_frames.lock().unwrap()[i] = plan.graph.frames;
                    done.fetch_add(1, Ordering::SeqCst);
                    Ok(())
                })();
                if let Err(e) = res {
                    first_err.lock().unwrap().get_or_insert(e);
                    break;
                }
            });
        }
    });
    if cancel.is_cancelled() {
        return Err(MediaError::Cancelled);
    }
    if let Some(e) = first_err.into_inner().unwrap() {
        return Err(e);
    }
    report(RenderStage::Muxing, render_share);

    // Lista do concat com durações exatas de cada bloco.
    let list = req.work_dir.join("concat.txt");
    let mut body = String::from("ffconcat version 1.0\n");
    for p in &plans {
        let d = Time::from_frames(p.graph.frames, fps);
        body.push_str(&format!("file {}\nduration {}\n", escape_concat_path(&p.file), d.to_ffmpeg()));
    }
    std::fs::write(&list, body)?;

    if let Some(dir) = req.output.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let ext = req.container.ext();
    let tmp_out = req.output.with_extension(format!("part.{ext}"));
    let mut cmd = tools.ffmpeg_cmd();
    cmd.args(["-y", "-v", "error", "-progress", "pipe:1", "-nostats", "-f", "concat", "-safe", "0", "-i"]).arg(&list);
    cmd.args(["-map", "0:v:0", "-map", "0:a:0", "-c:v", "copy"]);
    if let Some(lufs) = req.audio.loudness_lufs {
        cmd.args(["-af", &format!("loudnorm=I={lufs:.1}:TP=-1.5:LRA=11")]);
    }
    match req.container {
        Container::Webm => cmd.args(["-c:a", "libopus"]),
        _ => cmd.args(["-c:a", "aac"]),
    };
    cmd.args(["-b:a", &format!("{}k", req.audio.bitrate_kbps), "-ar", &req.compile.sample_rate.to_string()]);
    if req.container != Container::Webm {
        cmd.args(["-movflags", "+faststart", "-video_track_timescale", &timescale(fps).to_string()]);
    }
    cmd.arg(&tmp_out);
    let dur_s = Time::from_frames(total_frames, fps).as_secs_f64().max(0.001);
    let r = run_with_progress(cmd, cancel, |t| report(RenderStage::Muxing, render_share + (1.0 - render_share) * (t.as_secs_f64() / dur_s).min(1.0) as f32));
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp_out);
        return Err(e);
    }
    std::fs::rename(&tmp_out, &req.output)?;
    let _ = std::fs::remove_file(&list);

    Ok(RenderOutcome {
        output: req.output.clone(),
        duration: Time::from_frames(total_frames, fps),
        frames: total_frames,
        chunks_total: total,
        chunks_reused: reused,
        elapsed_ms: t0.elapsed().as_millis() as u64,
    })
}

/// Renderiza um único frame (scrubbing) pelo mesmo compilador — WYSIWYG.
pub fn render_frame(tools: &FfmpegTools, fonts: &FontResolver, project: &Project, at: Time, opts: &CompileOptions, work_dir: &Path, out: &Path) -> Result<()> {
    let fps = opts.fps;
    let at = Time::from_frames(at.frame_floor(fps).max(0), fps);
    let dur = project.timeline.duration();
    if at >= dur {
        return Err(MediaError::InvalidTimeline("posição além do fim da timeline".into()));
    }
    let mut o = opts.clone();
    o.include_audio = false;
    let graph = compile(project, TimeRange::new(at, at + fps.frame_duration()), &o, fonts, &work_dir.join("text"))?;
    write_text_files(&graph)?;
    std::fs::create_dir_all(work_dir)?;
    let script = work_dir.join(format!("frame-{}.txt", uuid::Uuid::new_v4()));
    std::fs::write(&script, &graph.filter)?;
    let mut cmd = tools.ffmpeg_cmd();
    cmd.args(["-y", "-v", "error"]).args(graph.input_args(fps)).args(tools.filter_script_args(&script));
    cmd.args(["-map", VIDEO_OUT, "-frames:v", "1", "-update", "1", "-q:v", "3"]).arg(out);
    let r = run_capture(cmd, Some(Duration::from_secs(60)));
    let _ = std::fs::remove_file(&script);
    r.map(|_| ())
}

/// Remove blocos de cache não usados há mais de `max_age` (limpeza controlada).
pub fn prune_chunk_cache(dir: &Path, max_age: Duration) -> Result<u64> {
    let mut freed = 0;
    let now = std::time::SystemTime::now();
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            let Ok(m) = e.metadata() else { continue };
            let old = m.accessed().or_else(|_| m.modified()).ok().and_then(|t| now.duration_since(t).ok()).map(|a| a > max_age).unwrap_or(false);
            if m.is_file() && old {
                freed += m.len();
                let _ = std::fs::remove_file(e.path());
            }
        }
    }
    Ok(freed)
}

#[cfg(test)]
mod parallel_tests {
    use super::*;

    #[test]
    fn parallelism_respects_memory_cores_and_hardware() {
        // 1080p medium ≈ 630 MB por processo (medido: 627 MB).
        assert_eq!(chunk_memory_estimate_mb(1920, 1080, "libx264", "medium"), 630);
        // Máquina com bastante RAM: limitado pelos núcleos.
        assert_eq!(plan_parallelism(1920, 1080, "libx264", "medium", 2, 16000).0, 2);
        assert_eq!(plan_parallelism(1920, 1080, "libx264", "medium", 16, 64000).0, 4);
        // Pouca RAM livre (1,5 GB): um bloco por vez em 1080p medium.
        assert_eq!(plan_parallelism(1920, 1080, "libx264", "medium", 8, 1500).0, 1);
        // 4K usa 4× mais memória.
        assert!(chunk_memory_estimate_mb(3840, 2160, "libx264", "medium") > 2400);
        assert_eq!(plan_parallelism(3840, 2160, "libx264", "medium", 8, 6000).0, 1);
        // Preview pequeno: o mínimo de área evita estimativas irreais.
        assert!(chunk_memory_estimate_mb(960, 540, "libx264", "ultrafast") >= 80);
        // Hardware: sempre 1.
        assert_eq!(plan_parallelism(1920, 1080, "h264_nvenc", "p5", 16, 64000).0, 1);
    }
}
