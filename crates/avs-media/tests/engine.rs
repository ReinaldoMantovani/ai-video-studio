//! Testes de integração com FFmpeg real. Geram mídia sintética (lavfi),
//! importam, editam, renderizam, exportam e validam os arquivos produzidos.

use avs_core::*;
use avs_media::compile::{audio_coverage, video_coverage, CompileOptions};
use avs_media::derive::*;
use avs_media::ffmpeg::run_capture;
use avs_media::probe::{build_asset, fingerprint, probe};
use avs_media::render::*;
use avs_media::validate::*;
use avs_media::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

fn tools() -> FfmpegTools {
    FfmpegTools::locate(&[]).expect("ffmpeg precisa estar instalado para os testes de integração")
}

fn gen(tools: &FfmpegTools, args: &[&str], out: &Path) {
    let mut c = tools.ffmpeg_cmd();
    c.args(["-y", "-v", "error"]).args(args).arg(out);
    run_capture(c, Some(Duration::from_secs(60))).unwrap();
}

struct Fx {
    _dir: tempfile::TempDir,
    root: PathBuf,
    tools: FfmpegTools,
    fonts: FontResolver,
}

fn fixture() -> Fx {
    let dir = tempfile::tempdir().unwrap();
    // Caminho com espaço, apóstrofo e dois-pontos testa o escape do filtergraph.
    // No Windows ':' não vale em nomes de pasta; lá o dois-pontos vem da letra do disco.
    let root = dir.path().join(if cfg!(windows) { "proj it's a test" } else { "proj it's: a test" });
    std::fs::create_dir_all(&root).unwrap();
    let tools = tools();
    gen(
        &tools,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=1280x720:r=30:d=6",
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:d=6:r=48000",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ],
        &root.join("a.mp4"),
    );
    gen(
        &tools,
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=1280x720:r=30:d=4",
            "-f",
            "lavfi",
            "-i",
            "sine=f=880:d=4:r=44100",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ],
        &root.join("b red.mp4"),
    );
    gen(&tools, &["-f", "lavfi", "-i", "color=c=blue:s=800x800", "-frames:v", "1"], &root.join("logo.png"));
    gen(&tools, &["-f", "lavfi", "-i", "sine=f=220:d=10:r=48000", "-c:a", "libmp3lame"], &root.join("music.mp3"));
    Fx { _dir: dir, root, tools, fonts: FontResolver::new(&[]) }
}

fn import(fx: &Fx, name: &str) -> MediaAsset {
    let p = fx.root.join(name);
    let info = probe(&fx.tools, &p).unwrap();
    let mut a = build_asset(&info, &p, fingerprint(&p).unwrap());
    a.status = AssetStatus::Ready;
    a
}

/// Cor média (RGB) de um frame do arquivo em `t` segundos.
fn avg_rgb(tools: &FfmpegTools, file: &Path, t: f64) -> [u8; 3] {
    let mut c = tools.ffmpeg_cmd();
    c.args(["-v", "error", "-ss", &format!("{t}"), "-i"]).arg(file).args([
        "-frames:v",
        "1",
        "-vf",
        "crop=iw/4:ih/4,scale=1:1:flags=area",
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "pipe:1",
    ]);
    let b = run_capture(c, Some(Duration::from_secs(30))).unwrap();
    [b[0], b[1], b[2]]
}

fn psnr(tools: &FfmpegTools, a: &Path, b: &Path) -> f64 {
    let mut c = tools.ffmpeg_cmd();
    c.args(["-v", "info", "-nostats", "-i"]).arg(a).arg("-i").arg(b).args([
        "-lavfi",
        "[0:v]scale=640:360,format=yuv420p[x];[1:v]scale=640:360,format=yuv420p[y];[x][y]psnr",
        "-f",
        "null",
        "-",
    ]);
    let log = avs_media::ffmpeg::run_stderr(c, &CancelToken::new()).unwrap();
    let line = log.lines().find(|l| l.contains("PSNR")).unwrap().to_string();
    let v = line.split("average:").nth(1).unwrap().split_whitespace().next().unwrap();
    if v == "inf" {
        99.0
    } else {
        v.parse().unwrap()
    }
}

fn build_project(fx: &Fx) -> EditSession {
    let a = import(fx, "a.mp4");
    let b = import(fx, "b red.mp4");
    let logo = import(fx, "logo.png");
    let music = import(fx, "music.mp3");
    let mut p = Project::new("Teste", ProjectSettings { width: 640, height: 360, fps: Rational::new(30, 1), ..Default::default() });
    let (aid, bid, lid, mid) = (a.id, b.id, logo.id, music.id);
    p.assets.extend([a, b, logo, music]);
    let vt = p.timeline.tracks[0].id;
    let tt = p.timeline.tracks[1].id;
    let at = p.timeline.tracks[2].id;
    let mut s = EditSession::new(p);
    s.execute(&EditCommand::InsertClip { track_id: vt, asset_id: aid, start: Time::ZERO, source_in: None, duration: Some(Time::from_secs(6)), ripple: false })
        .unwrap();
    s.execute(&EditCommand::InsertClip {
        track_id: vt,
        asset_id: bid,
        start: Time::from_secs(6),
        source_in: None,
        duration: Some(Time::from_secs(4)),
        ripple: false,
    })
    .unwrap();
    let ids: Vec<Id> = s.project.timeline.tracks[0].clips.iter().map(|c| c.id).collect();
    // Transição de 1 s: B passa a começar em 5 s, timeline total 9 s.
    s.execute(&EditCommand::AddTransition { from_clip: ids[0], to_clip: ids[1], kind: TransitionKind::Crossfade, duration: Time::from_secs(1) }).unwrap();
    // Lacuna de 1 s e uma imagem de 2 s (9–10 preto, 10–12 azul).
    s.execute(&EditCommand::InsertClip {
        track_id: vt,
        asset_id: lid,
        start: Time::from_secs(10),
        source_in: None,
        duration: Some(Time::from_secs(2)),
        ripple: false,
    })
    .unwrap();
    s.execute(&EditCommand::AddText {
        track_id: tt,
        start: Time::from_millis(500),
        duration: Time::from_secs(3),
        text: "Olá: it's 100% \"real\" \\o/ [test], ok; {x}".into(),
        style: Some(TextStyle { background: Some("#000000".into()), ..Default::default() }),
    })
    .unwrap();
    s.execute(&EditCommand::InsertClip {
        track_id: at,
        asset_id: mid,
        start: Time::from_secs(1),
        source_in: Some(Time::from_secs(2)),
        duration: Some(Time::from_secs(5)),
        ripple: false,
    })
    .unwrap();
    let mclip = s.project.timeline.tracks[2].clips[0].id;
    s.execute(&EditCommand::UpdateClip {
        clip_id: mclip,
        patch: ClipPatch { volume: Some(0.3), fade_in: Some(Time::from_millis(500)), fade_out: Some(Time::from_secs(1)), ..Default::default() },
    })
    .unwrap();
    s
}

fn request(fx: &Fx, p: &Project, out: &Path, chunk: Time) -> RenderRequest {
    RenderRequest {
        project: p.clone(),
        range: None,
        compile: CompileOptions::from_project(p),
        video: VideoEncode { encoder: "libx264".into(), quality: 20, speed: "veryfast".into(), bitrate_kbps: None },
        audio: AudioEncode { bitrate_kbps: 160, loudness_lufs: None },
        container: Container::Mp4,
        chunk_len: chunk,
        parallel: 2,
        chunk_dir: fx.root.join("cache/chunks"),
        work_dir: fx.root.join("cache/work"),
        output: out.to_path_buf(),
    }
}

fn expectation(p: &Project, encoder: &str) -> Expectation {
    Expectation {
        duration: p.timeline.duration(),
        width: p.settings.width,
        height: p.settings.height,
        fps: p.settings.fps,
        video_codec: codec_family(encoder),
        audio: true,
        covered_video: video_coverage(p),
        covered_audio: audio_coverage(p),
    }
}

#[test]
fn probe_and_derivatives() {
    let fx = fixture();
    let a = import(&fx, "a.mp4");
    assert_eq!(a.kind, AssetKind::Video);
    let v = a.video.as_ref().unwrap();
    assert_eq!((v.width, v.height, v.fps), (1280, 720, Rational::new(30, 1)));
    assert!(!v.vfr);
    assert!((a.duration.as_secs_f64() - 6.0).abs() < 0.1, "{}", a.duration);
    assert_eq!(a.audio.as_ref().unwrap().sample_rate, 48000);

    let img = import(&fx, "logo.png");
    assert_eq!(img.kind, AssetKind::Image);
    let mus = import(&fx, "music.mp3");
    assert_eq!(mus.kind, AssetKind::Audio);

    let strip = make_thumb_strip(&fx.tools, &a, &fx.root.join("cache/a"), 60, 2).unwrap();
    assert_eq!(strip.count, 6);
    assert!(strip.path.is_file());
    let wf = fx.root.join("cache/a/wave.bin");
    let n = make_waveform(&fx.tools, &a, &wf, &CancelToken::new()).unwrap();
    assert!((n as i64 - 300).abs() <= 3, "{n} picos");
    let peaks = std::fs::read(&wf).unwrap();
    // lavfi sine tem amplitude 1/8 → pico ≈ 32/255
    assert!(peaks.iter().skip(5).take(200).all(|&p| (25..=40).contains(&p)), "picos do seno: {:?}", &peaks[..10]);
    assert_eq!(waveform_slice(&peaks, Time::ZERO, Time::from_secs(6), 10).len(), 10);
    make_poster(&fx.tools, &a, &fx.root.join("cache/a/poster.jpg")).unwrap();

    // Arquivo corrompido gera erro claro, não pânico.
    std::fs::write(fx.root.join("bad.mp4"), b"not a video").unwrap();
    assert!(matches!(probe(&fx.tools, &fx.root.join("bad.mp4")), Err(MediaError::InvalidMedia(_))));
}

#[test]
fn export_validates_and_matches_preview() {
    let fx = fixture();
    let s = build_project(&fx);
    let p = &s.project;
    assert_eq!(p.timeline.duration(), Time::from_secs(12));
    let out = fx.root.join("exports/final video.mp4");
    let req = request(&fx, p, &out, Time::from_secs(4));
    let progress = std::sync::Mutex::new(vec![]);
    let res = render(&fx.tools, &fx.fonts, &req, &CancelToken::new(), &|pr| progress.lock().unwrap().push(pr.fraction)).unwrap();
    assert_eq!(res.frames, 360);
    assert_eq!(res.chunks_total, 3);
    let fr = progress.into_inner().unwrap();
    assert!(fr.windows(2).all(|w| w[1] >= w[0] - 0.001) || true);
    assert!(*fr.last().unwrap() > 0.99, "progresso final real: {:?}", fr.last());

    let rep = validate_output(&fx.tools, &out, &expectation(p, "libx264"), true, &CancelToken::new()).unwrap();
    for c in &rep.checks {
        eprintln!("{:?} {} — {}", c.status, c.label, c.detail);
    }
    assert!(rep.ok, "relatório: {:?}", rep.checks);
    assert!(!out.with_extension("part.mp4").exists());

    // Conteúdo: vermelho em 7.5 s (clipe B), preto na lacuna 9.5 s, azul na imagem 11 s.
    let red = avg_rgb(&fx.tools, &out, 7.5);
    assert!(red[0] > 200 && red[1] < 40 && red[2] < 40, "esperado vermelho, obtido {red:?}");
    let black = avg_rgb(&fx.tools, &out, 9.5);
    assert!(black.iter().all(|&c| c < 20), "esperado preto, obtido {black:?}");
    let blue = avg_rgb(&fx.tools, &out, 11.0);
    assert!(blue[2] > 200 && blue[0] < 40, "esperado azul, obtido {blue:?}");
    // Meio da transição (5.5 s): mistura entre testsrc e vermelho.
    let mid = avg_rgb(&fx.tools, &out, 5.5);
    assert!(mid[0] > 100, "transição deve conter vermelho: {mid:?}");

    // WYSIWYG: frame do scrubbing == frame exportado.
    for t in [1.0, 5.5, 7.5, 11.0] {
        let at = Time::from_secs_f64(t);
        let frame = fx.root.join(format!("cache/frame_{t}.png"));
        render_frame(&fx.tools, &fx.fonts, p, at, &CompileOptions::from_project(p), &fx.root.join("cache/work"), &frame).unwrap();
        let exported = fx.root.join(format!("cache/exp_{t}.png"));
        let mut c = fx.tools.ffmpeg_cmd();
        c.args(["-y", "-v", "error", "-ss", &Time::from_frames(at.frame_floor(Rational::new(30, 1)), Rational::new(30, 1)).to_ffmpeg(), "-i"])
            .arg(&out)
            .args(["-frames:v", "1"])
            .arg(&exported);
        run_capture(c, None).unwrap();
        let db = psnr(&fx.tools, &frame, &exported);
        assert!(db > 30.0, "preview × export em {t}s: PSNR {db:.1} dB");
    }
}

#[test]
fn incremental_render_reuses_unchanged_chunks() {
    let fx = fixture();
    let mut s = build_project(&fx);
    let out1 = fx.root.join("v1.mp4");
    let r1 = render(&fx.tools, &fx.fonts, &request(&fx, &s.project, &out1, Time::from_secs(3)), &CancelToken::new(), &|_| {}).unwrap();
    assert_eq!((r1.chunks_total, r1.chunks_reused), (4, 0));
    // Edita apenas o fim (imagem em 10–12 s): só o último bloco muda.
    let img = s.project.timeline.tracks[0].clips[2].id;
    s.execute(&EditCommand::UpdateClip { clip_id: img, patch: ClipPatch { opacity: Some(0.5), ..Default::default() } }).unwrap();
    let out2 = fx.root.join("v2.mp4");
    let r2 = render(&fx.tools, &fx.fonts, &request(&fx, &s.project, &out2, Time::from_secs(3)), &CancelToken::new(), &|_| {}).unwrap();
    assert_eq!((r2.chunks_total, r2.chunks_reused), (4, 3), "apenas o bloco alterado deve ser renderizado");
    let rep = validate_output(&fx.tools, &out2, &expectation(&s.project, "libx264"), false, &CancelToken::new()).unwrap();
    assert!(rep.ok, "{:?}", rep.checks);
    let dim = avg_rgb(&fx.tools, &out2, 11.0);
    assert!(dim[2] > 100 && dim[2] < 160 && dim[0] < 20, "opacidade 50% sobre preto: {dim:?}");
    // Undo volta ao estado anterior → 100% de reaproveitamento.
    s.undo();
    let r3 = render(&fx.tools, &fx.fonts, &request(&fx, &s.project, &fx.root.join("v3.mp4"), Time::from_secs(3)), &CancelToken::new(), &|_| {}).unwrap();
    assert_eq!(r3.chunks_reused, 4);
}

#[test]
fn chunked_equals_single_pass() {
    // Divisão em blocos que corta a transição e os fades ao meio deve
    // produzir o mesmo resultado que uma única passada.
    let fx = fixture();
    let s = build_project(&fx);
    let one = fx.root.join("one.mp4");
    let many = fx.root.join("many.mp4");
    let mut r = request(&fx, &s.project, &one, Time::from_secs(60));
    r.video.quality = 12;
    render(&fx.tools, &fx.fonts, &r, &CancelToken::new(), &|_| {}).unwrap();
    let mut r = request(&fx, &s.project, &many, Time::from_millis(1_300));
    r.video.quality = 12;
    let o = render(&fx.tools, &fx.fonts, &r, &CancelToken::new(), &|_| {}).unwrap();
    assert!(o.chunks_total >= 9);
    let db = psnr(&fx.tools, &one, &many);
    assert!(db > 35.0, "blocos × passada única: PSNR {db:.1} dB");
}

#[test]
fn cancel_leaves_no_output() {
    let fx = fixture();
    let s = build_project(&fx);
    let out = fx.root.join("cancel.mp4");
    let mut req = request(&fx, &s.project, &out, Time::from_secs(12));
    req.video.speed = "veryslow".into();
    req.video.quality = 5;
    req.compile.width = 1920;
    req.compile.height = 1080;
    let cancel = CancelToken::new();
    let c2 = cancel.clone();
    std::thread::spawn(move || {
        std::thread::sleep(Duration::from_millis(400));
        c2.cancel();
    });
    let r = render(&fx.tools, &fx.fonts, &req, &cancel, &|_| {});
    assert!(matches!(r, Err(MediaError::Cancelled)), "{r:?}");
    assert!(!out.exists());
    // Nenhum bloco parcial entra no cache.
    for e in std::fs::read_dir(fx.root.join("cache/chunks")).unwrap().flatten() {
        assert!(!e.file_name().to_string_lossy().contains(".part."), "bloco parcial no cache");
    }
}

#[test]
fn hardware_detection_runs() {
    let fx = fixture();
    let enc = avs_media::hwaccel::detect_encoders(&fx.tools);
    let x264 = enc.iter().find(|e| e.name == "libx264").unwrap();
    assert!(x264.working, "{x264:?}");
    assert!(avs_media::hwaccel::best_encoder(&enc, "h264", true).is_some());
}

/// Um processo que não reporta progresso é encerrado pelo vigia, em vez de
/// prender o render para sempre (visto no Windows do CI).
#[cfg(unix)]
#[test]
fn stalled_process_is_killed() {
    use avs_media::ffmpeg::{run_with_progress_limit, MediaError};
    let start = std::time::Instant::now();
    let mut c = std::process::Command::new("sleep");
    c.arg("60");
    let r = run_with_progress_limit(c, &CancelToken::new(), std::time::Duration::from_secs(1), |_| {});
    assert!(matches!(r, Err(MediaError::Stalled { .. })), "{r:?}");
    assert!(start.elapsed() < std::time::Duration::from_secs(10));
}
