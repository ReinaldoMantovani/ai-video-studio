//! Testes da IA local.
//! - Planejadores: determinísticos, aplicados num projeto real via `EditSession`.
//! - Análise por energia e cenas via FFmpeg: mídia real gerada no teste.
//! - Pipeline completo com o worker Python: instala o componente "analysis"
//!   num venv vazio com pip e analisa fala sintética (espeak-ng) e cortes
//!   reais. Habilite com `AVS_AI_INTEGRATION=1` (precisa de rede para o pip).
//! - Ollama: servidor HTTP local de teste que fala o protocolo do Ollama
//!   (o Ollama real não está disponível no ambiente de CI).

use avs_ai::analysis::*;
use avs_ai::autoedit::*;
use avs_ai::ollama::Ollama;
use avs_ai::runtime::{self, AiPaths};
use avs_core::*;
use avs_media::ffmpeg::run_capture;
use avs_media::{CancelToken, FfmpegTools};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::time::Duration;

fn s(x: f64) -> Time {
    Time::from_secs_f64(x)
}

fn video_asset(secs: i64) -> MediaAsset {
    MediaAsset {
        id: new_id(),
        kind: AssetKind::Video,
        name: "fala.mp4".into(),
        path: "/x/fala.mp4".into(),
        fingerprint: "fp".into(),
        size_bytes: 1,
        duration: Time::from_secs(secs),
        container: "mp4".into(),
        video: Some(VideoInfo {
            codec: "h264".into(),
            width: 1280,
            height: 720,
            fps: Rational::new(30, 1),
            avg_fps: None,
            time_base: None,
            vfr: false,
            pix_fmt: None,
            bit_depth: None,
            rotation: 0,
        }),
        audio: Some(AudioInfo { codec: "aac".into(), sample_rate: 48000, channels: 2, stream_count: 1 }),
        proxy_path: None,
        thumbs: None,
        waveform_path: None,
        poster_path: None,
        status: AssetStatus::Ready,
    }
}

fn analysis(speech: &[(f64, f64)], motion: &[(f64, f32)]) -> AnalysisResult {
    AnalysisResult {
        version: ANALYSIS_VERSION,
        fingerprint: "fp".into(),
        duration: s(60.0),
        loudness: None,
        speech: Some(SpeechAnalysis { method: "silero".into(), segments: speech.iter().map(|(a, b)| TimeRange::new(s(*a), s(*b))).collect(), ratio: 0.5 }),
        scenes: Some(SceneAnalysis { method: "pyscenedetect".into(), cuts: vec![s(10.0), s(25.0)] }),
        visual: Some(VisualAnalysis {
            interval: s(1.0),
            face_detector: None,
            samples: motion.iter().map(|(t, m)| VisualSample { t: s(*t), motion: *m, luma: 0.5, faces: vec![] }).collect(),
        }),
        transcript: None,
        warnings: vec![],
    }
}

fn session_with_clip(secs: i64) -> (EditSession, Id, Id, Id) {
    let mut p = Project::new("ai", ProjectSettings::default());
    let a = video_asset(secs);
    let aid = a.id;
    p.assets.push(a);
    let vt = p.timeline.tracks[0].id;
    let mut s = EditSession::new(p);
    let c =
        s.execute(&EditCommand::InsertClip { track_id: vt, asset_id: aid, start: Time::ZERO, source_in: None, duration: None, ripple: false }).unwrap().created
            [0];
    (s, aid, vt, c)
}

#[test]
fn pause_plan_removes_silences_with_padding_and_applies_cleanly() {
    let (mut sess, aid, vt, clip) = session_with_clip(30);
    // fala: 1–5, 5.3–9 (pausa curta, mantida), 14–20, 28–29.5
    let a = analysis(&[(1.0, 5.0), (5.3, 9.0), (14.0, 20.0), (28.0, 29.5)], &[]);
    let analyses = HashMap::from([(aid, a)]);
    let rules = PauseRules { min_silence: Time::from_millis(700), padding: Time::from_millis(200), preserve_action: false, motion_threshold: 0.02 };
    let plan = plan_pauses(&sess.project, &analyses, None, &rules);
    let r: Vec<(f64, f64)> =
        plan.cuts.iter().map(|c| ((c.range.start.as_secs_f64() * 100.0).round() / 100.0, (c.range.end.as_secs_f64() * 100.0).round() / 100.0)).collect();
    // início sem folga, pausas internas com 0,2 s de cada lado; o silêncio final
    // (0,5 s) é menor que o mínimo e fica.
    assert_eq!(r, vec![(0.0, 0.8), (9.2, 13.8), (20.2, 27.8)]);
    let removed = plan.removed.as_secs_f64();
    assert!((removed - (0.8 + 4.6 + 7.6)).abs() < 0.05, "{removed}");

    // Legendas na trilha de texto precisam acompanhar.
    let tt = sess.project.timeline.tracks[1].id;
    sess.execute(&EditCommand::AddText { track_id: tt, start: s(14.0), duration: s(6.0), text: "fala 3".into(), style: None }).unwrap();
    let cmds = pause_commands(&plan, &[tt]);
    sess.execute_batch("Remover pausas", &cmds).unwrap();
    let total = sess.project.timeline.duration().as_secs_f64();
    assert!((total - (30.0 - removed)).abs() < 0.05, "duração final {total}");
    // A fala 3 (origem 14–20) começa em 14 − 0,8 − 4,6 = 8,6 s, e a legenda também.
    let v: Vec<&Clip> = sess.project.timeline.track(vt).unwrap().clips.iter().collect();
    let third = v.iter().find(|c| (c.source_in().as_secs_f64() - 13.8).abs() < 0.05).expect("clipe da fala 3");
    assert!((third.start.as_secs_f64() - 8.4).abs() < 0.05, "{}", third.start);
    let cap = &sess.project.timeline.track(tt).unwrap().clips[0];
    assert!((cap.start.as_secs_f64() - 8.6).abs() < 0.05, "legenda em {}", cap.start);
    // Desfazer tudo de uma vez.
    sess.undo();
    assert_eq!(sess.project.timeline.duration(), Time::from_secs(30));
    let _ = clip;
}

#[test]
fn pause_plan_preserves_silent_action_and_respects_speed() {
    let (mut sess, aid, _vt, clip) = session_with_clip(30);
    let a = analysis(&[(0.0, 5.0), (15.0, 30.0)], &[(6.0, 0.08), (8.0, 0.09), (10.0, 0.07), (12.0, 0.1)]);
    let analyses = HashMap::from([(aid, a)]);
    let mut rules = PauseRules { preserve_action: true, ..Default::default() };
    let plan = plan_pauses(&sess.project, &analyses, Some(&[clip]), &rules);
    assert!(plan.cuts.is_empty(), "pausa com ação visual deve ser mantida");
    assert_eq!(plan.kept_action, 1);
    rules.preserve_action = false;
    // A 2×, a pausa de origem 5–15 vira 2,5–7,5 na timeline (menos a folga).
    sess.execute(&EditCommand::SetClipSpeed { clip_id: clip, speed: 2.0, ripple: true }).unwrap();
    let plan = plan_pauses(&sess.project, &analyses, Some(&[clip]), &rules);
    assert_eq!(plan.cuts.len(), 1);
    let c = &plan.cuts[0];
    assert!((c.range.start.as_secs_f64() - 2.575).abs() < 0.04 && (c.range.end.as_secs_f64() - 7.425).abs() < 0.04, "{:?}", c.range);
}

#[test]
fn scenes_and_captions() {
    let (mut sess, aid, _vt, clip) = session_with_clip(30);
    let a = analysis(&[], &[]);
    let fps = sess.project.settings.fps;
    let c = sess.project.timeline.clip(clip).unwrap().clone();
    assert_eq!(scene_marker_commands(&c, &a, fps).len(), 2);
    sess.execute_batch("Dividir nas cenas", &scene_split_commands(&c, &a, fps)).unwrap();
    let starts: Vec<i64> = sess.project.timeline.tracks[0].clips.iter().map(|c| c.start.as_secs_f64().round() as i64).collect();
    assert_eq!(starts, vec![0, 10, 25]);

    let w = |a: f64, b: f64, t: &str| Word { start: s(a), end: s(b), word: t.into(), prob: 0.9 };
    let t = Transcript {
        language: "pt".into(),
        language_probability: 0.99,
        model: "small".into(),
        device: "cpu".into(),
        segments: vec![TranscriptSegment {
            start: s(1.0),
            end: s(9.0),
            text: String::new(),
            words: vec![
                w(1.0, 1.4, " Olá"),
                w(1.4, 1.8, " pessoal,"),
                w(1.9, 2.3, " hoje"),
                w(2.3, 2.8, " vamos"),
                w(2.8, 3.3, " editar."),
                w(5.0, 5.5, " Depois"),
                w(5.5, 6.0, " da"),
                w(6.0, 6.6, " pausa."),
            ],
        }],
    };
    let first = sess.project.timeline.tracks[0].clips[0].clone();
    let cues = caption_cues(&t, &first, 42, Time::from_secs(4));
    let texts: Vec<&str> = cues.iter().map(|c| c.text.as_str()).collect();
    assert_eq!(texts, vec!["Olá pessoal, hoje vamos editar.", "Depois da pausa."]);
    assert_eq!(cues[1].start, s(5.0));
    let _ = aid;
}

fn tools() -> FfmpegTools {
    FfmpegTools::locate(&[]).unwrap()
}

fn gen(args: &[&str], out: &Path) {
    let mut c = tools().ffmpeg_cmd();
    c.args(["-y", "-v", "error"]).args(args).arg(out);
    run_capture(c, Some(Duration::from_secs(60))).unwrap();
}

/// 2 s de silêncio + fala sintética (espeak-ng) + 2 s de silêncio.
fn speech_media(dir: &Path) -> Option<PathBuf> {
    let speech = dir.join("speech.wav");
    let ok = std::process::Command::new("espeak-ng")
        .args(["-v", "pt-br", "-s", "150", "-w"])
        .arg(&speech)
        .arg("Olá, este é um teste do detector de fala do estúdio.")
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !ok {
        eprintln!("espeak-ng indisponível: teste de fala pulado");
        return None;
    }
    let out = dir.join("fala.mp4");
    gen(
        &[
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=22050:cl=mono:d=2",
            "-i",
            speech.to_str().unwrap(),
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=22050:cl=mono:d=2",
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=320x240:r=25:d=4[a];color=c=blue:s=320x240:r=25:d=6[b];[a][b]concat",
            "-filter_complex",
            "[0:a][1:a][2:a]concat=n=3:v=0:a=1[aud]",
            "-map",
            "3:v",
            "-map",
            "[aud]",
            "-shortest",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-c:a",
            "aac",
        ],
        &out,
    );
    Some(out)
}

fn real_asset(path: &Path) -> MediaAsset {
    let t = tools();
    let mut a = avs_media::probe::build_asset(&avs_media::probe::probe(&t, path).unwrap(), path, avs_media::probe::fingerprint(path).unwrap());
    a.status = AssetStatus::Ready;
    a
}

#[test]
fn energy_speech_and_ffmpeg_scenes_without_python() {
    let d = tempfile::tempdir().unwrap();
    let Some(media) = speech_media(d.path()) else { return };
    let asset = real_asset(&media);
    // Sem venv: o pipeline usa as alternativas e diz isso.
    let paths = AiPaths::new(d.path(), PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workers/python"));
    let seen = std::sync::Mutex::new(vec![]);
    let r = analyze(
        &tools(),
        &paths,
        &asset,
        &AnalysisOptions { visual: false, ..Default::default() },
        &|st, f| {
            if st == "detectando cenas" {
                seen.lock().unwrap().push(f)
            }
        },
        &CancelToken::new(),
    )
    .unwrap();
    // Progresso real durante a detecção de cenas (não fica parado em 0%).
    let v = seen.lock().unwrap().clone();
    assert!(v.len() >= 2 && v.windows(2).all(|w| w[1] >= w[0]), "{v:?}");
    assert!(v.iter().any(|f| *f > 0.0), "{v:?}");
    let sp = r.speech.as_ref().unwrap();
    assert_eq!(sp.method, "energy");
    assert_eq!(sp.segments.len(), 1, "{:?}", sp.segments);
    assert!((sp.segments[0].start.as_secs_f64() - 2.0).abs() < 0.3, "{:?}", sp.segments[0]);
    let sc = r.scenes.as_ref().unwrap();
    assert_eq!(sc.method, "ffmpeg");
    assert_eq!(sc.cuts.len(), 1);
    assert!((sc.cuts[0].as_secs_f64() - 4.0).abs() < 0.1, "{:?}", sc.cuts);
    assert!(r.warnings.iter().any(|w| w.contains("nível de áudio")));
    // Cache: segunda chamada não refaz (resultado idêntico, instantâneo).
    let t0 = std::time::Instant::now();
    let r2 = analyze(&tools(), &paths, &asset, &AnalysisOptions { visual: false, ..Default::default() }, &|_, _| {}, &CancelToken::new()).unwrap();
    assert!(t0.elapsed() < Duration::from_millis(500));
    assert_eq!(r2.speech.unwrap().segments, sp.segments);
}

#[test]
fn full_pipeline_installs_runtime_and_runs_worker() {
    if std::env::var("AVS_AI_INTEGRATION").is_err() {
        eprintln!("defina AVS_AI_INTEGRATION=1 para instalar o runtime e testar o worker real");
        return;
    }
    let d = tempfile::tempdir().unwrap();
    let Some(media) = speech_media(d.path()) else { return };
    let asset = real_asset(&media);
    let lib = std::env::var("AVS_AI_TEST_LIB").map(PathBuf::from).unwrap_or_else(|_| d.path().join("lib"));
    let paths = AiPaths::new(&lib, PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../workers/python"));
    let stages = std::sync::Mutex::new(vec![]);
    runtime::install(&paths, "analysis", &|s, _| stages.lock().unwrap().push(s.to_string()), &CancelToken::new()).unwrap();
    runtime::install(&paths, "faces", &|_, _| {}, &CancelToken::new()).unwrap();
    let (comps, caps) = runtime::components(&paths);
    assert!(comps.iter().find(|c| c.id == "faces").unwrap().installed, "YuNet baixado e verificado (SHA-256)");
    assert!(caps.is_some(), "runtime instalado");
    assert!(comps.iter().find(|c| c.id == "analysis").unwrap().installed, "{comps:?}");
    assert!(!comps.iter().find(|c| c.id == "transcription").unwrap().installed);

    let progress = std::sync::Mutex::new(vec![]);
    let r = analyze(
        &tools(),
        &paths,
        &asset,
        &AnalysisOptions { visual_interval: 0.5, transcript: true, ..Default::default() },
        &|s, f| progress.lock().unwrap().push((s.to_string(), f)),
        &CancelToken::new(),
    )
    .unwrap();
    let sp = r.speech.as_ref().unwrap();
    assert_eq!(sp.method, "silero");
    assert_eq!(sp.segments.len(), 1, "{:?}", sp.segments);
    assert!((sp.segments[0].start.as_secs_f64() - 2.0).abs() < 0.2, "{:?}", sp.segments);
    let sc = r.scenes.as_ref().unwrap();
    assert_eq!(sc.method, "pyscenedetect");
    assert_eq!(sc.cuts.len(), 1);
    assert!((sc.cuts[0].as_secs_f64() - 4.0).abs() < 0.1);
    let v = r.visual.as_ref().unwrap();
    assert_eq!(v.face_detector.as_deref(), Some("yunet"));
    // vídeo cortado pelo `-shortest` à duração do áudio (~7,5 s), amostrado a cada 0,5 s
    assert!(v.samples.len() >= 14, "{}", v.samples.len());
    let red = v.samples.iter().find(|x| x.t < s(3.0)).unwrap().luma;
    let blue = v.samples.iter().find(|x| x.t > s(5.0)).unwrap().luma;
    assert!(red > blue, "vermelho é mais claro que azul em luma: {red} × {blue}");
    assert!(r.transcript.is_none());
    assert!(r.warnings.iter().any(|w| w.contains("Transcrição indisponível")), "{:?}", r.warnings);
    let p = progress.into_inner().unwrap();
    assert!(p.windows(2).all(|w| w[1].1 + 1e-4 >= w[0].1), "progresso monotônico");
    assert_eq!(p.last().unwrap().1, 1.0);

    // Cancelamento de uma chamada ao worker.
    let w = paths.worker().unwrap();
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(300));
        c2.cancel();
    });
    let long = d.path().join("long.mp4");
    gen(&["-f", "lavfi", "-i", "testsrc2=s=320x240:r=25:d=120", "-c:v", "libx264", "-preset", "ultrafast"], &long);
    let e = w.call("visual", serde_json::json!({"video": long, "interval": 0.1}), &|_, _| {}, &cancel, None).unwrap_err();
    assert!(matches!(e, avs_ai::AiError::Cancelled), "{e}");
}

/// Servidor HTTP mínimo que imita a API do Ollama (somente para o teste).
fn fake_ollama() -> String {
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in l.incoming().flatten() {
            let mut r = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            r.read_line(&mut first).unwrap();
            let mut len = 0usize;
            loop {
                let mut h = String::new();
                r.read_line(&mut h).unwrap();
                if h == "\r\n" || h.is_empty() {
                    break;
                }
                if let Some(v) = h.to_ascii_lowercase().strip_prefix("content-length:") {
                    len = v.trim().parse().unwrap();
                }
            }
            let mut body = vec![0u8; len];
            r.read_exact(&mut body).unwrap();
            let path = first.split_whitespace().nth(1).unwrap_or("").to_string();
            let resp = match path.as_str() {
                "/api/version" => r#"{"version":"0.12.0"}"#.to_string(),
                "/api/tags" => r#"{"models":[{"name":"qwen3:4b","size":2500000000,"details":{"family":"qwen3","parameter_size":"4.0B","quantization_level":"Q4_K_M"}}]}"#.into(),
                "/api/pull" => "{\"status\":\"pulling manifest\"}\n{\"status\":\"downloading\",\"total\":100,\"completed\":40}\n{\"status\":\"downloading\",\"total\":100,\"completed\":100}\n{\"status\":\"success\"}\n".into(),
                "/api/generate" => {
                    let req: serde_json::Value = serde_json::from_slice(&body).unwrap();
                    assert!(req["format"].is_object(), "schema JSON enviado");
                    r#"{"response":"{\"ok\":true,\"language\":\"pt\"}","done":true}"#.into()
                }
                _ => "{}".into(),
            };
            let mut s = stream;
            write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", resp.len(), resp).unwrap();
        }
    });
    format!("http://{addr}")
}

#[test]
fn ollama_client_protocol() {
    let o = Ollama::new(&fake_ollama());
    let st = o.status();
    assert!(st.running, "{:?}", st.error);
    assert_eq!(st.version.as_deref(), Some("0.12.0"));
    assert_eq!(st.models[0].name, "qwen3:4b");
    assert_eq!(st.models[0].parameter_size.as_deref(), Some("4.0B"));
    let prog = std::sync::Mutex::new(vec![]);
    o.pull("qwen3:4b", &|_, f| prog.lock().unwrap().push(f), &CancelToken::new()).unwrap();
    assert_eq!(*prog.lock().unwrap().last().unwrap(), 1.0);
    assert!(prog.lock().unwrap().contains(&0.4));
    assert!(o.test_model("qwen3:4b").unwrap().contains("\"ok\":true"));
    // Ollama parado: estado claro, sem pânico.
    let off = Ollama::new("http://127.0.0.1:9").status();
    assert!(!off.running && off.error.is_some());
}
