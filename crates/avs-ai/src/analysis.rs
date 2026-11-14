//! Pipeline de análise de mídia.
//!
//! Etapas (cada uma em cache próprio, chaveado pela impressão digital do
//! arquivo — a mesma gravação nunca é analisada duas vezes, nem entre projetos):
//! 1. áudio 16 kHz mono (FFmpeg) → volume RMS a cada 100 ms (Rust);
//! 2. fala: Silero VAD (worker) ou, sem Python, limiar de energia (marcado como tal);
//! 3. cenas: PySceneDetect (worker) ou filtro de cena do FFmpeg;
//! 4. imagem: movimento, brilho e rostos (worker, amostrado);
//! 5. transcrição: faster-whisper (worker), quando instalado e solicitado.
//!
//! O método efetivamente usado fica registrado no resultado.

use crate::runtime::{AiPaths, YUNET_FILE};
use crate::{Progress, Result};
use avs_core::{MediaAsset, Time, TimeRange};
use avs_media::ffmpeg::run_with_progress;
use avs_media::{CancelToken, FfmpegTools};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::io::Read;
use std::path::{Path, PathBuf};
use ts_rs::TS;

pub const ANALYSIS_VERSION: u32 = 1;

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SpeechAnalysis {
    /// "silero" | "energy"
    pub method: String,
    pub segments: Vec<TimeRange>,
    pub ratio: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct SceneAnalysis {
    /// "pyscenedetect" | "ffmpeg"
    pub method: String,
    pub cuts: Vec<Time>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct FaceBox {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub score: Option<f32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VisualSample {
    pub t: Time,
    pub motion: f32,
    pub luma: f32,
    #[serde(default)]
    pub faces: Vec<FaceBox>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct VisualAnalysis {
    pub interval: Time,
    pub face_detector: Option<String>,
    pub samples: Vec<VisualSample>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct LoudnessAnalysis {
    pub interval: Time,
    /// RMS em dBFS por intervalo (mínimo −90).
    pub rms_db: Vec<f32>,
    pub peak_db: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Word {
    pub start: Time,
    pub end: Time,
    pub word: String,
    pub prob: f32,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct TranscriptSegment {
    pub start: Time,
    pub end: Time,
    pub text: String,
    pub words: Vec<Word>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct Transcript {
    pub language: String,
    pub language_probability: f32,
    pub model: String,
    pub device: String,
    pub segments: Vec<TranscriptSegment>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AnalysisResult {
    pub version: u32,
    pub fingerprint: String,
    pub duration: Time,
    pub loudness: Option<LoudnessAnalysis>,
    pub speech: Option<SpeechAnalysis>,
    pub scenes: Option<SceneAnalysis>,
    pub visual: Option<VisualAnalysis>,
    pub transcript: Option<Transcript>,
    /// Etapas puladas ou degradadas, com o motivo real.
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, TS)]
#[ts(export)]
pub struct AnalysisOptions {
    pub speech: bool,
    pub scenes: bool,
    pub visual: bool,
    pub transcript: bool,
    /// Código ISO (pt, en, es…) ou `None` para detecção automática.
    pub language: Option<String>,
    pub whisper_model: String,
    /// Intervalo de amostragem visual em segundos.
    pub visual_interval: f32,
    pub force: bool,
}

impl Default for AnalysisOptions {
    fn default() -> Self {
        AnalysisOptions {
            speech: true,
            scenes: true,
            visual: true,
            transcript: false,
            language: None,
            whisper_model: "small".into(),
            visual_interval: 1.0,
            force: false,
        }
    }
}

pub fn cache_dir(paths: &AiPaths, fingerprint: &str) -> PathBuf {
    let safe: String = fingerprint.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    paths.root.join("analysis").join(safe)
}

fn read_json<T: serde::de::DeserializeOwned>(p: &Path) -> Option<T> {
    std::fs::read(p).ok().and_then(|b| serde_json::from_slice(&b).ok())
}

fn write_json<T: Serialize>(p: &Path, v: &T) -> Result<()> {
    if let Some(d) = p.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = p.with_extension("tmp");
    std::fs::write(&tmp, serde_json::to_vec(v)?)?;
    std::fs::rename(tmp, p)?;
    Ok(())
}

/// Resultado consolidado a partir do cache (sem executar nada).
pub fn load_cached(paths: &AiPaths, asset: &MediaAsset) -> Option<AnalysisResult> {
    let dir = cache_dir(paths, &asset.fingerprint);
    let r: AnalysisResult = read_json(&dir.join("result.json"))?;
    (r.version == ANALYSIS_VERSION).then_some(r)
}

fn secs_t(s: f64) -> Time {
    Time::from_secs_f64(s)
}

/// Extrai o áudio em 16 kHz mono (formato de entrada do VAD e do Whisper).
fn extract_wav(tools: &FfmpegTools, asset: &MediaAsset, out: &Path, progress: Progress, cancel: &CancelToken) -> Result<()> {
    if out.is_file() {
        return Ok(());
    }
    let tmp = out.with_extension("part.wav");
    let mut c = tools.ffmpeg_cmd();
    c.args(["-y", "-v", "error", "-progress", "pipe:1", "-nostats", "-i"])
        .arg(&asset.path)
        .args(["-map", "0:a:0", "-ac", "1", "-ar", "16000", "-c:a", "pcm_s16le"])
        .arg(&tmp);
    let total = asset.duration.as_secs_f64().max(0.001);
    let r = run_with_progress(c, cancel, |t| progress("extraindo áudio", (t.as_secs_f64() / total) as f32));
    if let Err(e) = r {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    std::fs::rename(&tmp, out)?;
    Ok(())
}

/// RMS por intervalo de 100 ms, lendo o WAV em blocos (memória constante).
pub fn loudness_from_wav(wav: &Path) -> Result<LoudnessAnalysis> {
    let mut f = std::io::BufReader::new(std::fs::File::open(wav)?);
    let mut header = [0u8; 44];
    f.read_exact(&mut header)?;
    let sr = u32::from_le_bytes([header[24], header[25], header[26], header[27]]) as usize;
    let win = (sr / 10).max(1);
    let mut buf = vec![0u8; win * 2];
    let mut out = vec![];
    let mut peak = 0f32;
    loop {
        let mut filled = 0;
        while filled < buf.len() {
            let n = f.read(&mut buf[filled..])?;
            if n == 0 {
                break;
            }
            filled += n;
        }
        if filled < 2 {
            break;
        }
        let mut acc = 0f64;
        let n = filled / 2;
        for s in buf[..n * 2].chunks_exact(2) {
            let v = i16::from_le_bytes([s[0], s[1]]) as f32 / 32768.0;
            peak = peak.max(v.abs());
            acc += (v as f64) * (v as f64);
        }
        let rms = (acc / n as f64).sqrt();
        out.push(if rms > 0.0 { (20.0 * rms.log10()).max(-90.0) as f32 } else { -90.0 });
        if filled < buf.len() {
            break;
        }
    }
    Ok(LoudnessAnalysis { interval: Time::from_millis(100), rms_db: out, peak_db: if peak > 0.0 { 20.0 * peak.log10() } else { -90.0 } })
}

/// Fala por energia (alternativa sem Python): limiar adaptativo acima do ruído.
pub fn speech_from_energy(l: &LoudnessAnalysis, min_silence: Time) -> SpeechAnalysis {
    let mut sorted = l.rms_db.clone();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let floor = sorted.get(sorted.len() / 10).copied().unwrap_or(-90.0);
    let thr = (floor + 12.0).max(-50.0);
    let step = l.interval;
    let mut segs: Vec<TimeRange> = vec![];
    for (i, &db) in l.rms_db.iter().enumerate() {
        if db < thr {
            continue;
        }
        let s = Time::from_flicks(step.flicks() * i as i64);
        let e = s + step;
        match segs.last_mut() {
            Some(last) if s - last.end <= min_silence => last.end = e,
            _ => segs.push(TimeRange::new(s, e)),
        }
    }
    // Como no Silero: trechos curtos demais (estalos, transientes) não são fala.
    segs.retain(|r| r.duration() >= Time::from_millis(250));
    let total = Time::from_flicks(step.flicks() * l.rms_db.len() as i64);
    let speech: i64 = segs.iter().map(|r| r.duration().flicks()).sum();
    SpeechAnalysis { method: "energy".into(), segments: segs, ratio: if total.is_positive() { speech as f32 / total.flicks() as f32 } else { 0.0 } }
}

/// Cortes de cena com o filtro `select=scene` do FFmpeg (alternativa sem Python).
/// Cortes de cena pelo filtro `select=scene` do FFmpeg (alternativa sem
/// Python), com progresso real: os cortes vão para um arquivo de metadados e
/// o andamento vem de `-progress` (antes, um arquivo de horas ficava em 0%
/// até o fim).
pub fn scenes_ffmpeg(tools: &FfmpegTools, video: &Path, duration: Time, progress: Progress, cancel: &CancelToken) -> Result<SceneAnalysis> {
    let dir = std::env::temp_dir().join(format!("avs-scenes-{}", avs_core::new_id().simple()));
    std::fs::create_dir_all(&dir)?;
    let meta = dir.join("scenes.txt");
    let mut c = tools.ffmpeg_cmd();
    c.args(["-v", "error", "-progress", "pipe:1", "-nostats", "-i"]).arg(video).args([
        "-an",
        "-vf",
        &format!("scale=320:-2,select='gt(scene,0.35)',metadata=print:file={}", avs_media::ffmpeg::escape_filter_value(&meta.to_string_lossy())),
        "-f",
        "null",
        "-",
    ]);
    let total = duration.as_secs_f64().max(0.001);
    let r = run_with_progress(c, cancel, |t| progress("detectando cenas", (t.as_secs_f64() / total).clamp(0.0, 1.0) as f32));
    let text = std::fs::read_to_string(&meta).unwrap_or_default();
    let _ = std::fs::remove_dir_all(&dir);
    r?;
    let cuts = text
        .lines()
        .filter_map(|l| l.split("pts_time:").nth(1))
        .filter_map(|r| r.split_whitespace().next())
        .filter_map(|v| v.parse::<f64>().ok())
        .map(secs_t)
        .collect();
    Ok(SceneAnalysis { method: "ffmpeg".into(), cuts })
}

/// Executa as etapas pedidas, reaproveitando o cache.
pub fn analyze(
    tools: &FfmpegTools,
    paths: &AiPaths,
    asset: &MediaAsset,
    opts: &AnalysisOptions,
    progress: Progress,
    cancel: &CancelToken,
) -> Result<AnalysisResult> {
    let dir = cache_dir(paths, &asset.fingerprint);
    std::fs::create_dir_all(&dir)?;
    let worker = paths.worker();
    let caps = worker.as_ref().and_then(|w| w.capabilities(&paths.models()).ok());
    let has = |k: &str| caps.as_ref().map(|c| !c[k].is_null() && c[k] != json!(false)).unwrap_or(false);
    let mut r: AnalysisResult = if opts.force {
        AnalysisResult {
            version: ANALYSIS_VERSION,
            fingerprint: asset.fingerprint.clone(),
            duration: asset.duration,
            loudness: None,
            speech: None,
            scenes: None,
            visual: None,
            transcript: None,
            warnings: vec![],
        }
    } else {
        load_cached(paths, asset).unwrap_or(AnalysisResult {
            version: ANALYSIS_VERSION,
            fingerprint: asset.fingerprint.clone(),
            duration: asset.duration,
            loudness: None,
            speech: None,
            scenes: None,
            visual: None,
            transcript: None,
            warnings: vec![],
        })
    };
    r.warnings.clear();
    let video_src = asset.proxy_path.clone().filter(|p| p.is_file()).unwrap_or_else(|| asset.path.clone());

    // Pesos aproximados de cada etapa para a fração global (o progresso de cada
    // etapa vem do trabalho real do FFmpeg/worker).
    let stages: Vec<(&str, f32)> = [
        ("audio", if asset.has_audio() { 0.1 } else { 0.0 }),
        ("speech", if opts.speech && asset.has_audio() { 0.15 } else { 0.0 }),
        ("scenes", if opts.scenes && asset.has_video() { 0.2 } else { 0.0 }),
        ("visual", if opts.visual && asset.has_video() { 0.25 } else { 0.0 }),
        ("transcript", if opts.transcript && asset.has_audio() { 0.6 } else { 0.0 }),
    ]
    .into_iter()
    .collect();
    let total: f32 = stages.iter().map(|s| s.1).sum::<f32>().max(0.001);
    let offset = |name: &str| stages.iter().take_while(|s| s.0 != name).map(|s| s.1).sum::<f32>() / total;
    let weight = |name: &str| stages.iter().find(|s| s.0 == name).map(|s| s.1).unwrap_or(0.0) / total;
    let sub = |name: &'static str| {
        let (o, w) = (offset(name), weight(name));
        move |stage: &str, f: f32| progress(stage, o + w * f.clamp(0.0, 1.0))
    };

    let wav = dir.join("audio16k.wav");
    if asset.has_audio() && (opts.speech || opts.transcript) {
        let p = sub("audio");
        extract_wav(tools, asset, &wav, &p, cancel)?;
        if r.loudness.is_none() {
            r.loudness = Some(loudness_from_wav(&wav)?);
        }
    }

    if opts.speech && asset.has_audio() {
        let cached_ok = r.speech.as_ref().map(|s| s.method == "silero" || !has("silero_vad_model")).unwrap_or(false);
        if !cached_ok {
            let p = sub("speech");
            match (&worker, has("silero_vad_model") && has("onnxruntime")) {
                (Some(w), true) => {
                    let v = w.call("vad", json!({"wav": wav}), &p, cancel, None)?;
                    let segs: Vec<TimeRange> = v["speech"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|s| TimeRange::new(secs_t(s[0].as_f64().unwrap_or(0.0)), secs_t(s[1].as_f64().unwrap_or(0.0))))
                        .collect();
                    r.speech = Some(SpeechAnalysis { method: "silero".into(), segments: segs, ratio: v["speech_ratio"].as_f64().unwrap_or(0.0) as f32 });
                }
                _ => {
                    r.warnings.push("Detecção de fala por nível de áudio (instale 'Análise de fala e imagem' para usar o Silero VAD, mais preciso).".into());
                    r.speech = Some(speech_from_energy(r.loudness.as_ref().unwrap(), Time::from_millis(300)));
                }
            }
        }
    }

    if opts.scenes && asset.has_video() {
        let cached_ok = r.scenes.as_ref().map(|s| s.method == "pyscenedetect" || !has("scenedetect")).unwrap_or(false);
        if !cached_ok {
            let p = sub("scenes");
            match (&worker, has("scenedetect") && has("opencv")) {
                (Some(w), true) => {
                    let v = w.call("scenes", json!({"video": video_src}), &p, cancel, None)?;
                    let cuts = v["cuts"].as_array().cloned().unwrap_or_default().iter().filter_map(|x| x.as_f64()).map(secs_t).collect();
                    r.scenes = Some(SceneAnalysis { method: "pyscenedetect".into(), cuts });
                }
                _ => {
                    r.warnings.push("Cortes de cena pelo filtro do FFmpeg (instale 'Análise de fala e imagem' para usar o PySceneDetect).".into());
                    r.scenes = Some(scenes_ffmpeg(tools, &video_src, asset.duration, &p, cancel)?);
                }
            }
        }
    }

    if opts.visual && asset.has_video() {
        let want_faces = paths.vision_models().join(YUNET_FILE).is_file();
        let interval = Time::from_secs_f64(opts.visual_interval.max(0.1) as f64);
        let stale = r.visual.as_ref().map(|v| v.interval != interval || (want_faces && v.face_detector.is_none())).unwrap_or(true);
        if stale {
            match (&worker, has("opencv")) {
                (Some(w), true) => {
                    let p = sub("visual");
                    let v = w.call(
                        "visual",
                        json!({"video": video_src, "interval": opts.visual_interval, "faces": want_faces, "model_dir": paths.vision_models(), "duration": asset.duration.as_secs_f64()}),
                        &p,
                        cancel,
                        None,
                    )?;
                    let samples = v["samples"]
                        .as_array()
                        .cloned()
                        .unwrap_or_default()
                        .iter()
                        .map(|s| VisualSample {
                            t: secs_t(s["t"].as_f64().unwrap_or(0.0)),
                            motion: s["motion"].as_f64().unwrap_or(0.0) as f32,
                            luma: s["luma"].as_f64().unwrap_or(0.0) as f32,
                            faces: serde_json::from_value(s["faces"].clone()).unwrap_or_default(),
                        })
                        .collect();
                    r.visual = Some(VisualAnalysis { interval, face_detector: v["face_detector"].as_str().map(String::from), samples });
                    if !want_faces {
                        r.warnings.push("Rostos não analisados: instale 'Detecção de rostos (YuNet)'.".into());
                    }
                }
                _ => r.warnings.push("Análise de imagem indisponível: instale 'Análise de fala e imagem'.".into()),
            }
        }
    }

    if opts.transcript && asset.has_audio() {
        let lang = opts.language.clone().unwrap_or_else(|| "auto".into());
        let key = dir.join(format!("transcript-{}-{lang}.json", opts.whisper_model));
        let cached: Option<Transcript> = if opts.force { None } else { read_json(&key) };
        match cached {
            Some(t) => r.transcript = Some(t),
            None => {
                let installed = caps.as_ref().and_then(|c| c["whisper_models"].as_array().cloned()).unwrap_or_default();
                if !has("faster_whisper") {
                    r.warnings.push("Transcrição indisponível: instale o componente 'Transcrição (faster-whisper)'.".into());
                } else if !installed.iter().any(|m| m.as_str() == Some(opts.whisper_model.as_str())) {
                    r.warnings.push(format!("Transcrição indisponível: baixe o modelo 'Whisper {}' no Gerenciador de IA.", opts.whisper_model));
                } else if let Some(w) = &worker {
                    let p = sub("transcript");
                    let v = w.call(
                        "transcribe",
                        json!({"wav": wav, "models_dir": paths.models(), "model": opts.whisper_model, "language": opts.language}),
                        &p,
                        cancel,
                        None,
                    )?;
                    let t = parse_transcript(&v);
                    write_json(&key, &t)?;
                    r.transcript = Some(t);
                }
            }
        }
    }

    progress("análise concluída", 1.0);
    write_json(&dir.join("result.json"), &r)?;
    Ok(r)
}

pub fn parse_transcript(v: &serde_json::Value) -> Transcript {
    let segs = v["segments"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .iter()
        .map(|s| TranscriptSegment {
            start: secs_t(s["start"].as_f64().unwrap_or(0.0)),
            end: secs_t(s["end"].as_f64().unwrap_or(0.0)),
            text: s["text"].as_str().unwrap_or("").to_string(),
            words: s["words"]
                .as_array()
                .cloned()
                .unwrap_or_default()
                .iter()
                .map(|w| Word {
                    start: secs_t(w["start"].as_f64().unwrap_or(0.0)),
                    end: secs_t(w["end"].as_f64().unwrap_or(0.0)),
                    word: w["word"].as_str().unwrap_or("").to_string(),
                    prob: w["prob"].as_f64().unwrap_or(0.0) as f32,
                })
                .collect(),
        })
        .collect();
    Transcript {
        language: v["language"].as_str().unwrap_or("").into(),
        language_probability: v["language_probability"].as_f64().unwrap_or(0.0) as f32,
        model: v["model"].as_str().unwrap_or("").into(),
        device: v["device"].as_str().unwrap_or("").into(),
        segments: segs,
    }
}
