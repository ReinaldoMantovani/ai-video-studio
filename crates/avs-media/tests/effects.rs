//! Velocidade, reverso, congelamento, keyframes, efeitos e áudio — verificados
//! nos pixels e no áudio dos arquivos exportados (FFmpeg real).

use avs_core::*;
use avs_media::compile::CompileOptions;
use avs_media::ffmpeg::{run_capture, run_stderr};
use avs_media::probe::{build_asset, fingerprint, probe};
use avs_media::render::*;
use avs_media::validate::*;
use avs_media::*;
use std::path::{Path, PathBuf};
use std::time::Duration;

struct Fx {
    _d: tempfile::TempDir,
    root: PathBuf,
    t: FfmpegTools,
    fonts: FontResolver,
}

fn ff(t: &FfmpegTools, args: &[&str], out: &Path) {
    let mut c = t.ffmpeg_cmd();
    c.args(["-y", "-v", "error"]).args(args).arg(out);
    run_capture(c, Some(Duration::from_secs(60))).unwrap();
}

fn fixture() -> Fx {
    let d = tempfile::tempdir().unwrap();
    let root = d.path().to_path_buf();
    let t = FfmpegTools::locate(&[]).unwrap();
    // 0–2 s vermelho, 2–4 s azul, com tom contínuo.
    ff(
        &t,
        &[
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=640x360:r=30:d=2",
            "-f",
            "lavfi",
            "-i",
            "color=c=blue:s=640x360:r=30:d=2",
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:d=4:r=48000",
            "-filter_complex",
            "[0:v][1:v]concat=n=2:v=1:a=0[v]",
            "-map",
            "[v]",
            "-map",
            "2:a",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-g",
            "10",
            "-pix_fmt",
            "yuv420p",
            "-c:a",
            "aac",
            "-shortest",
        ],
        &root.join("seq.mp4"),
    );
    // metade esquerda vermelha, direita azul
    ff(&t, &["-f", "lavfi", "-i", "color=c=red:s=320x360[l];color=c=blue:s=320x360[r];[l][r]hstack", "-frames:v", "1"], &root.join("lr.png"));
    ff(&t, &["-f", "lavfi", "-i", "color=c=0x808080:s=640x360", "-frames:v", "1"], &root.join("gray.png"));
    ff(&t, &["-f", "lavfi", "-i", "color=c=white:s=640x360", "-frames:v", "1"], &root.join("white.png"));
    Fx { _d: d, root, t, fonts: FontResolver::new(&[]) }
}

fn asset(fx: &Fx, name: &str) -> MediaAsset {
    let p = fx.root.join(name);
    let mut a = build_asset(&probe(&fx.t, &p).unwrap(), &p, fingerprint(&p).unwrap());
    a.status = AssetStatus::Ready;
    a
}

fn session(fx: &Fx, names: &[&str]) -> (EditSession, Vec<Id>) {
    let mut p = Project::new("fx", ProjectSettings { width: 640, height: 360, fps: Rational::new(30, 1), ..Default::default() });
    let mut ids = vec![];
    for n in names {
        let a = asset(fx, n);
        ids.push(a.id);
        p.assets.push(a);
    }
    (EditSession::new(p), ids)
}

fn vt(s: &EditSession) -> Id {
    s.project.timeline.tracks[0].id
}

fn export(fx: &Fx, s: &EditSession, name: &str, chunk: Time) -> PathBuf {
    let out = fx.root.join(name);
    let req = RenderRequest {
        project: s.project.clone(),
        range: None,
        compile: CompileOptions::from_project(&s.project),
        video: VideoEncode { encoder: "libx264".into(), quality: 12, speed: "veryfast".into(), bitrate_kbps: None },
        audio: AudioEncode { bitrate_kbps: 192, loudness_lufs: None },
        container: Container::Mp4,
        chunk_len: chunk,
        parallel: 2,
        chunk_dir: fx.root.join("chunks"),
        work_dir: fx.root.join("work"),
        output: out.clone(),
    };
    render(&fx.t, &fx.fonts, &req, &CancelToken::new(), &|_| {}).unwrap();
    let exp = Expectation {
        duration: s.project.timeline.duration(),
        width: 640,
        height: 360,
        fps: Rational::new(30, 1),
        video_codec: "h264".into(),
        audio: true,
        covered_video: vec![],
        covered_audio: vec![],
    };
    let rep = validate_output(&fx.t, &out, &exp, true, &CancelToken::new()).unwrap();
    assert!(rep.ok, "{name}: {:?}", rep.checks);
    out
}

/// Cor média de uma região (x, y, w, h) do frame em `t`.
fn rgb(fx: &Fx, file: &Path, t: f64, region: (u32, u32, u32, u32)) -> [u8; 3] {
    let (x, y, w, h) = region;
    let mut c = fx.t.ffmpeg_cmd();
    c.args(["-v", "error", "-ss", &format!("{t}"), "-i"]).arg(file).args([
        "-frames:v",
        "1",
        "-vf",
        &format!("crop={w}:{h}:{x}:{y},scale=1:1:flags=area"),
        "-f",
        "rawvideo",
        "-pix_fmt",
        "rgb24",
        "pipe:1",
    ]);
    let b = run_capture(c, None).unwrap();
    [b[0], b[1], b[2]]
}
const CENTER: (u32, u32, u32, u32) = (300, 160, 40, 40);

fn mean_volume(fx: &Fx, file: &Path, from: f64, to: f64) -> f64 {
    let mut c = fx.t.ffmpeg_cmd();
    c.args(["-v", "info", "-nostats", "-i"]).arg(file).args(["-af", &format!("atrim={from}:{to},volumedetect"), "-f", "null", "-"]);
    let log = run_stderr(c, &CancelToken::new()).unwrap();
    let l = log.lines().find(|l| l.contains("mean_volume")).unwrap();
    l.split("mean_volume:").nth(1).unwrap().trim().trim_end_matches(" dB").parse().unwrap()
}

fn insert(s: &mut EditSession, asset: Id, start: i64, dur_ms: Option<i64>) -> Id {
    let t = vt(s);
    s.execute(&EditCommand::InsertClip {
        track_id: t,
        asset_id: asset,
        start: Time::from_secs(start),
        source_in: None,
        duration: dur_ms.map(Time::from_millis),
        ripple: false,
    })
    .unwrap()
    .created[0]
}

#[test]
fn speed_reverse_and_freeze_show_the_right_frames() {
    let fx = fixture();
    // 2×: 4 s de origem em 2 s.
    let (mut s, ids) = session(&fx, &["seq.mp4"]);
    let c = insert(&mut s, ids[0], 0, Some(4000));
    s.execute(&EditCommand::SetClipSpeed { clip_id: c, speed: 2.0, ripple: true }).unwrap();
    assert_eq!(s.project.timeline.duration(), Time::from_secs(2));
    let out = export(&fx, &s, "fast.mp4", Time::from_secs(30));
    assert!(rgb(&fx, &out, 0.5, CENTER)[0] > 200, "0,5 s → origem 1 s (vermelho)");
    assert!(rgb(&fx, &out, 1.5, CENTER)[2] > 200, "1,5 s → origem 3 s (azul)");

    // Reverso: começa azul, termina vermelho.
    let (mut s, ids) = session(&fx, &["seq.mp4"]);
    let c = insert(&mut s, ids[0], 0, Some(4000));
    s.execute(&EditCommand::UpdateClip { clip_id: c, patch: ClipPatch { reverse: Some(true), ..Default::default() } }).unwrap();
    // blocos de 1,5 s cortam o clipe invertido no meio: precisa continuar correto
    let out = export(&fx, &s, "rev.mp4", Time::from_millis(1500));
    assert!(rgb(&fx, &out, 0.5, CENTER)[2] > 200, "início invertido = azul");
    assert!(rgb(&fx, &out, 3.5, CENTER)[0] > 200, "fim invertido = vermelho");

    // Câmera lenta 0,5×: 2 s de origem (vermelho) em 4 s, com áudio esticado.
    let (mut s, ids) = session(&fx, &["seq.mp4"]);
    let c = insert(&mut s, ids[0], 0, Some(2000));
    s.execute(&EditCommand::SetClipSpeed { clip_id: c, speed: 0.5, ripple: true }).unwrap();
    let out = export(&fx, &s, "slow.mp4", Time::from_secs(30));
    assert!(rgb(&fx, &out, 3.6, CENTER)[0] > 200);
    assert!(mean_volume(&fx, &out, 1.0, 3.0) > -30.0, "áudio presente na câmera lenta");

    // Congelar em 3 s por 2 s: tudo azul e silencioso durante o congelamento.
    let (mut s, ids) = session(&fx, &["seq.mp4"]);
    let c = insert(&mut s, ids[0], 0, Some(4000));
    s.execute(&EditCommand::FreezeFrame { clip_id: c, at: Time::from_secs(3), duration: Time::from_secs(2) }).unwrap();
    assert_eq!(s.project.timeline.duration(), Time::from_secs(6));
    let out = export(&fx, &s, "freeze.mp4", Time::from_secs(30));
    for t in [3.1, 4.0, 4.9] {
        assert!(rgb(&fx, &out, t, CENTER)[2] > 200, "congelado em {t}s");
    }
    assert!(mean_volume(&fx, &out, 3.2, 4.8) < -80.0, "frame congelado não tem áudio");
    assert!(mean_volume(&fx, &out, 0.5, 2.5) > -30.0);
}

#[test]
fn keyframed_zoom_and_pan_is_frame_accurate_across_chunks() {
    let fx = fixture();
    let (mut s, ids) = session(&fx, &["lr.png"]);
    let c = insert(&mut s, ids[0], 0, Some(2000));
    for (prop, at, v) in [
        (KeyframeProperty::Scale, 0, 1.0),
        (KeyframeProperty::Scale, 1500, 2.0),
        (KeyframeProperty::PositionX, 0, 0.0),
        (KeyframeProperty::PositionX, 1500, -1.0),
    ] {
        s.execute(&EditCommand::SetKeyframe { clip_id: c, property: prop, at: Time::from_millis(at), value: v, easing: None }).unwrap();
    }
    let single = export(&fx, &s, "zoom1.mp4", Time::from_secs(30));
    let left = (60, 160, 40, 40);
    assert!(rgb(&fx, &single, 0.0, left)[0] > 200, "início: lado esquerdo vermelho");
    assert!(rgb(&fx, &single, 1.8, left)[2] > 200, "fim: zoom 2× deslocado mostra só o azul");
    let mid = rgb(&fx, &single, 0.75, (300, 160, 40, 40));
    let _ = mid;
    let chunked = export(&fx, &s, "zoom2.mp4", Time::from_millis(700));
    let mut c = fx.t.ffmpeg_cmd();
    c.args(["-v", "info", "-nostats", "-i"]).arg(&single).arg("-i").arg(&chunked).args(["-lavfi", "psnr", "-f", "null", "-"]);
    let log = run_stderr(c, &CancelToken::new()).unwrap();
    let avg = log.lines().find(|l| l.contains("PSNR")).unwrap().split("average:").nth(1).unwrap().split_whitespace().next().unwrap().to_string();
    let db: f64 = if avg == "inf" { 99.0 } else { avg.parse().unwrap() };
    assert!(db > 35.0, "keyframes em blocos × passada única: {db:.1} dB");
}

#[test]
fn effects_and_lut_change_pixels() {
    let fx = fixture();
    // LUT 3D 2×2×2 que inverte as cores.
    let lut = fx.root.join("invert.cube");
    let mut body = String::from("LUT_3D_SIZE 2\n");
    for b in 0..2 {
        for g in 0..2 {
            for r in 0..2 {
                body.push_str(&format!("{} {} {}\n", 1 - r, 1 - g, 1 - b));
            }
        }
    }
    std::fs::write(&lut, body).unwrap();

    let (mut s, ids) = session(&fx, &["seq.mp4", "white.png", "gray.png"]);
    let a = insert(&mut s, ids[0], 0, Some(1000));
    let b = insert(&mut s, ids[1], 1, Some(1000));
    let c = insert(&mut s, ids[2], 2, Some(1000));
    let fxp = |e: VideoEffects| ClipPatch { effects: Some(e), ..Default::default() };
    s.execute(&EditCommand::UpdateClip { clip_id: a, patch: fxp(VideoEffects { lut: Some(lut.clone()), ..Default::default() }) }).unwrap();
    s.execute(&EditCommand::UpdateClip { clip_id: b, patch: fxp(VideoEffects { vignette: 1.0, blur: 2.0, sharpen: 0.5, ..Default::default() }) }).unwrap();
    s.execute(&EditCommand::UpdateClip { clip_id: c, patch: fxp(VideoEffects { temperature: 3000.0, ..Default::default() }) }).unwrap();
    let out = export(&fx, &s, "effects.mp4", Time::from_secs(30));
    let inv = rgb(&fx, &out, 0.5, CENTER);
    assert!(inv[0] < 40 && inv[1] > 200 && inv[2] > 200, "LUT invertida: vermelho → ciano, obtido {inv:?}");
    let center = rgb(&fx, &out, 1.5, CENTER);
    let corner = rgb(&fx, &out, 1.5, (0, 0, 24, 24));
    assert!((corner[0] as i32) < center[0] as i32 - 60, "vinheta escurece os cantos: {corner:?} × {center:?}");
    let warm = rgb(&fx, &out, 2.5, CENTER);
    assert!(warm[0] as i32 > warm[2] as i32 + 15, "3000 K aquece o cinza: {warm:?}");
    // LUT inexistente é rejeitada na validação.
    let e = s.execute(&EditCommand::UpdateClip { clip_id: a, patch: fxp(VideoEffects { lut: Some(fx.root.join("nao.cube")), ..Default::default() }) });
    assert!(e.is_err());
}

#[test]
fn volume_keyframes_and_audio_processing() {
    let fx = fixture();
    let (mut s, ids) = session(&fx, &["seq.mp4"]);
    let c = insert(&mut s, ids[0], 0, Some(4000));
    for (at, v) in [(0, 0.0), (3000, 1.0)] {
        s.execute(&EditCommand::SetKeyframe { clip_id: c, property: KeyframeProperty::Volume, at: Time::from_millis(at), value: v, easing: None }).unwrap();
    }
    s.execute(&EditCommand::UpdateClip {
        clip_id: c,
        patch: ClipPatch {
            audio_fx: Some(AudioEffects { highpass: true, denoise: true, compressor: true, eq_low_db: -3.0, eq_mid_db: 2.0, eq_high_db: 1.0 }),
            ..Default::default()
        },
    })
    .unwrap();
    let out = export(&fx, &s, "vol.mp4", Time::from_millis(1700));
    let start = mean_volume(&fx, &out, 0.0, 0.3);
    let end = mean_volume(&fx, &out, 3.2, 3.9);
    assert!(end > -30.0, "fim audível: {end}");
    assert!(start < end - 20.0, "rampa de volume: início {start} dB × fim {end} dB");
}

/// PSNR (dB) entre dois arquivos de imagem/vídeo no primeiro frame.
fn psnr_frames(fx: &Fx, a: &Path, b: &Path) -> f64 {
    let mut c = fx.t.ffmpeg_cmd();
    c.args(["-v", "info", "-nostats", "-i"]).arg(a).arg("-i").arg(b).args([
        "-frames:v",
        "1",
        "-lavfi",
        "[0:v]format=yuv420p[x];[1:v]format=yuv420p[y];[x][y]psnr",
        "-f",
        "null",
        "-",
    ]);
    let log = run_stderr(c, &CancelToken::new()).unwrap();
    let v = log.split("average:").nth(1).unwrap().split_whitespace().next().unwrap();
    if v == "inf" {
        99.0
    } else {
        v.parse().unwrap()
    }
}

#[test]
fn strong_blur_is_downscaled_but_looks_the_same() {
    // Imagem detalhada (padrão de teste) com desfoque forte: o grafo usa
    // redução/ampliação, e o resultado precisa bater com o gaussiano em
    // resolução cheia.
    let fx = fixture();
    ff(&fx.t, &["-f", "lavfi", "-i", "testsrc2=s=640x360", "-frames:v", "1"], &fx.root.join("detail.png"));
    let (mut s, ids) = session(&fx, &["detail.png"]);
    let c = insert(&mut s, ids[0], 0, Some(1000));
    s.execute(&EditCommand::UpdateClip {
        clip_id: c,
        patch: ClipPatch { effects: Some(VideoEffects { blur: 40.0, ..Default::default() }), ..Default::default() },
    })
    .unwrap();
    let g = avs_media::compile::compile(
        &s.project,
        TimeRange::new(Time::ZERO, Time::from_secs(1)),
        &CompileOptions::from_project(&s.project),
        &fx.fonts,
        &fx.root.join("txt"),
    )
    .unwrap();
    // σ = 40 × 360/1080 ≈ 13,3 → fator 2.
    assert!(g.filter.contains("flags=area,gblur=sigma=6.667"), "{}", g.filter);
    let out = export(&fx, &s, "blur.mp4", Time::from_secs(5));
    let refimg = fx.root.join("ref.png");
    ff(&fx.t, &["-i", fx.root.join("detail.png").to_str().unwrap(), "-vf", "gblur=sigma=13.333", "-frames:v", "1"], &refimg);
    let p = psnr_frames(&fx, &out, &refimg);
    eprintln!("PSNR do desfoque reduzido: {p:.1} dB");
    assert!(p > 35.0, "PSNR {p:.1} dB contra o gaussiano em resolução cheia");
}

#[test]
fn shared_source_is_opened_once_and_split() {
    // Mesmo trecho da mesma mídia em duas trilhas de vídeo (fundo desfocado +
    // principal): uma entrada só, dividida no grafo; o render continua correto.
    let fx = fixture();
    let (mut s, ids) = session(&fx, &["seq.mp4"]);
    let bg = s.execute(&EditCommand::AddTrack { kind: TrackKind::Video, name: Some("Fundo".into()) }).unwrap().created[0];
    s.execute(&EditCommand::MoveTrack { track_id: bg, index: 0 }).unwrap();
    let main = s.project.timeline.tracks.iter().filter(|t| t.kind == TrackKind::Video).nth(1).unwrap().id;
    let a = s
        .execute(&EditCommand::InsertClip { track_id: bg, asset_id: ids[0], start: Time::ZERO, source_in: None, duration: None, ripple: false })
        .unwrap()
        .created[0];
    let b = s
        .execute(&EditCommand::InsertClip { track_id: main, asset_id: ids[0], start: Time::ZERO, source_in: None, duration: None, ripple: false })
        .unwrap()
        .created[0];
    s.execute(&EditCommand::UpdateClip {
        clip_id: a,
        patch: ClipPatch { audio_muted: Some(true), effects: Some(VideoEffects { blur: 30.0, ..Default::default() }), ..Default::default() },
    })
    .unwrap();
    // O principal ocupa só a metade direita (escala 50%, deslocado).
    s.execute(&EditCommand::UpdateClip {
        clip_id: b,
        patch: ClipPatch { transform: Some(Transform { fit: FitMode::Fit, scale: 0.5, x: 0.5, y: 0.0 }), ..Default::default() },
    })
    .unwrap();
    let g = avs_media::compile::compile(
        &s.project,
        TimeRange::new(Time::ZERO, Time::from_secs(4)),
        &CompileOptions::from_project(&s.project),
        &fx.fonts,
        &fx.root.join("txt"),
    )
    .unwrap();
    assert_eq!(g.inputs.len(), 1, "uma entrada para os dois clipes: {:?}", g.inputs);
    assert!(g.filter.contains("[0:v]split=2"), "{}", g.filter);
    let out = export(&fx, &s, "shared.mp4", Time::from_secs(2));
    // Em 1 s: fundo vermelho desfocado à esquerda, vermelho nítido no centro-direita.
    let left = rgb(&fx, &out, 1.0, (20, 160, 40, 40));
    let right = rgb(&fx, &out, 1.0, (460, 160, 40, 40));
    assert!(left[0] > 150 && right[0] > 150, "{left:?} {right:?}");
    // Em 3 s: azul nas duas camadas.
    let r3 = rgb(&fx, &out, 3.0, (460, 160, 40, 40));
    assert!(r3[2] > 150 && r3[0] < 80, "{r3:?}");
    // Áudio presente (vem do principal).
    assert!(mean_volume(&fx, &out, 0.5, 3.5) > -30.0);
}
