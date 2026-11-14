//! Fluxo completo da camada de serviço com FFmpeg real (cenários D, E e G).

use avs_ai::shorts::{FramingMode, ShortsOptions};
use avs_app::shorts::{ShortsExport, ShortsRequest};
use avs_app::*;
use avs_core::*;
use avs_media::ffmpeg::run_capture;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

#[derive(Default)]
struct Rec(Mutex<Vec<AppEvent>>);
impl EventSink for Rec {
    fn emit(&self, e: AppEvent) {
        self.0.lock().unwrap().push(e);
    }
}

fn presets_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../../presets")
}

fn gen(st: &Studio, args: &[&str], out: &Path) {
    let mut c = st.ffmpeg().ffmpeg_cmd();
    c.args(["-y", "-v", "error"]).args(args).arg(out);
    run_capture(c, Some(Duration::from_secs(60))).unwrap();
}

fn start(lib: &Path, ev: Arc<Rec>) -> Studio {
    let mut cfg = StudioConfig::new(lib.to_path_buf(), presets_dir());
    cfg.deep_validation = true;
    Studio::start(cfg, ev).unwrap()
}

#[test]
fn full_flow_import_edit_preview_export_reopen() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    let media = tmp.path().join("media");
    std::fs::create_dir_all(&media).unwrap();
    let ev = Arc::new(Rec::default());
    let st = start(&lib, ev.clone());

    assert_eq!(st.presets().len(), 21, "erros: {:?}", st.preset_errors());
    assert!(st.export_profiles().iter().any(|p| p.id == "youtube_shorts"));

    gen(
        &st,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=1280x720:r=30:d=8",
            "-f",
            "lavfi",
            "-i",
            "sine=f=440:d=8",
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
        &media.join("cam.mp4"),
    );
    gen(&st, &["-f", "lavfi", "-i", "sine=f=330:d=12", "-c:a", "libmp3lame"], &media.join("trilha.mp3"));

    // Projeto a partir do preset Gameplay YouTube (16:9, 1080p).
    let ps = st.create_project("Cenário D", Some("gameplay_youtube"), None).unwrap();
    let pid = ps.project.id;
    assert_eq!((ps.project.settings.width, ps.project.settings.height), (1920, 1080));
    assert_eq!(ps.project.preset_id.as_deref(), Some("gameplay_youtube"));

    // Importação por pasta.
    let imp = st.import_media(pid, std::slice::from_ref(&media)).unwrap();
    assert_eq!(imp.assets.len(), 2, "{:?}", imp.errors);
    // Reimportar o mesmo arquivo não duplica.
    let again = st.import_media(pid, &[media.join("cam.mp4")]).unwrap();
    assert_eq!(again.skipped.len(), 1);
    st.wait_idle(Duration::from_secs(120)).unwrap();
    let p = st.project_state(pid).unwrap().project;
    let cam = p.assets.iter().find(|a| a.name == "cam.mp4").unwrap().clone();
    let song = p.assets.iter().find(|a| a.name == "trilha.mp3").unwrap().clone();
    assert!(cam.thumbs.is_some() && cam.poster_path.is_some() && cam.waveform_path.is_some(), "derivados: {cam:?}");
    assert!(song.waveform_path.is_some());
    assert!(!st.waveform(pid, cam.id, Time::ZERO, Time::from_secs(8), 100).unwrap().is_empty());

    // Edição manual: clipe, divisão, texto, música, transição.
    let vt = p.timeline.tracks[0].id;
    let tt = p.timeline.tracks[1].id;
    let at = p.timeline.tracks[2].id;
    st.execute(pid, &EditCommand::InsertClip { track_id: vt, asset_id: cam.id, start: Time::ZERO, source_in: None, duration: None, ripple: false }).unwrap();
    let c0 = st.project_state(pid).unwrap().project.timeline.tracks[0].clips[0].id;
    let (out, _) = st.execute(pid, &EditCommand::SplitClip { clip_id: c0, at: Time::from_secs(4) }).unwrap();
    let c1 = out.created[0];
    st.execute(pid, &EditCommand::DeleteClip { clip_id: c0, ripple: false }).unwrap();
    st.undo(pid).unwrap();
    st.execute(pid, &EditCommand::AddTransition { from_clip: c0, to_clip: c1, kind: TransitionKind::Crossfade, duration: Time::from_millis(500) }).unwrap();
    st.execute(pid, &EditCommand::AddText { track_id: tt, start: Time::from_secs(1), duration: Time::from_secs(2), text: "Título".into(), style: None })
        .unwrap();
    st.execute(
        pid,
        &EditCommand::InsertClip { track_id: at, asset_id: song.id, start: Time::ZERO, source_in: None, duration: Some(Time::from_secs(6)), ripple: false },
    )
    .unwrap();
    let state = st.project_state(pid).unwrap();
    assert!(state.dirty);
    let dur = state.project.timeline.duration();
    assert_eq!(dur, Time::from_millis(7_500));

    // Frame de preview e preview completo.
    let f = st.render_frame(pid, Time::from_secs(2), 360).unwrap();
    assert!(f.is_file());
    let job = st.request_preview(pid).unwrap();
    let r = st.wait_job(job, Duration::from_secs(180)).unwrap();
    assert_eq!(r.state, "completed", "{:?}", r.error);
    assert!(ev.0.lock().unwrap().iter().any(|e| matches!(e, AppEvent::PreviewReady { .. })));

    // Exportação por perfil (720p para o teste ser rápido), validada.
    let prof = st.export_profiles().into_iter().find(|p| p.id == "youtube_1080p").unwrap();
    let mut req = ExportRequest::from_profile(&prof, tmp.path().join("exports"), "Meu vídeo: final?".into());
    req.width = Some(1280);
    req.height = Some(720);
    req.speed = "veryfast".into();
    let job = st.export(pid, req.clone()).unwrap();
    let r = st.wait_job(job, Duration::from_secs(300)).unwrap();
    assert_eq!(r.state, "completed", "{:?}", r.error);
    let res: serde_json::Value = r.result.unwrap();
    let path = PathBuf::from(res["path"].as_str().unwrap());
    assert!(path.is_file());
    assert!(path.file_name().unwrap().to_string_lossy().starts_with("Meu_vídeo__final_"), "{path:?}");
    assert_eq!(res["report"]["ok"], true);
    assert_eq!(st.list_exports(Some(pid)).unwrap().len(), 1);

    // Mesmo nome de novo → não sobrescreve; blocos reaproveitados.
    let r2 = st.wait_job(st.export(pid, req).unwrap(), Duration::from_secs(300)).unwrap();
    let res2 = r2.result.unwrap();
    assert_ne!(res2["path"], res["path"]);
    assert_eq!(res2["chunks_reused"], res2["chunks_total"]);

    // Fechar sem salvar → reabrir recupera do autosave (cenário G).
    let rev_project = st.project_state(pid).unwrap().project;
    st.shutdown();
    drop(st);
    let st = start(&lib, Arc::new(Rec::default()));
    let reopened = st.open_project(pid).unwrap();
    assert!(reopened.recovered, "deve recuperar do autosave");
    assert_eq!(reopened.project.timeline, rev_project.timeline);
    st.save_project(pid).unwrap();
    st.close_project(pid);
    let again = st.open_project(pid).unwrap();
    assert!(!again.recovered && !again.dirty);
    assert_eq!(st.list_projects().unwrap()[0].name, "Cenário D");
    assert!(st.list_projects().unwrap()[0].thumbnail.is_some());
    st.shutdown();
}

#[test]
fn pause_resume_and_cancel_export() {
    let tmp = tempfile::tempdir().unwrap();
    let st = start(&tmp.path().join("lib"), Arc::new(Rec::default()));
    let media = tmp.path().join("m.mp4");
    gen(
        &st,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=1920x1080:r=30:d=40",
            "-f",
            "lavfi",
            "-i",
            "sine=d=40",
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
        &media,
    );
    let pid = st.create_project("Pausa", None, None).unwrap().project.id;
    let a = st.import_media(pid, &[media]).unwrap().assets[0].clone();
    let vt = st.project_state(pid).unwrap().project.timeline.tracks[0].id;
    st.execute(pid, &EditCommand::InsertClip { track_id: vt, asset_id: a.id, start: Time::ZERO, source_in: None, duration: None, ripple: false }).unwrap();
    st.wait_idle(Duration::from_secs(120)).unwrap();

    let req = ExportRequest {
        output_dir: tmp.path().join("out"),
        file_name: "pausa".into(),
        container: avs_media::render::Container::Mp4,
        codec: "h264".into(),
        encoder: Some("libx264".into()),
        prefer_hardware: false,
        width: None,
        height: None,
        fps: None,
        quality: 23,
        speed: "medium".into(),
        bitrate_kbps: None,
        audio_bitrate_kbps: 128,
        loudness_lufs: None,
        range: None,
    };
    let job = st.export(pid, req.clone()).unwrap();
    // Espera o primeiro bloco (30 s) terminar e pausa durante o segundo.
    let t0 = std::time::Instant::now();
    loop {
        let j = st.jobs().unwrap().into_iter().find(|j| j.id == job).unwrap();
        if j.progress > 0.70 || t0.elapsed() > Duration::from_secs(120) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    st.pause_job(job).unwrap();
    let r = st.wait_job(job, Duration::from_secs(30)).unwrap();
    assert_eq!(r.state, "paused");
    st.resume_job(job).unwrap();
    let r = st.wait_job(job, Duration::from_secs(300)).unwrap();
    assert_eq!(r.state, "completed", "{:?}", r.error);
    assert!(r.result.unwrap()["chunks_reused"].as_u64().unwrap() >= 1, "bloco concluído antes da pausa deve ser reaproveitado");

    // Cancelamento de job em fila e em execução.
    let mut r2 = req.clone();
    r2.file_name = "cancelado".into();
    r2.quality = 10;
    r2.encoder = Some("libx264".into());
    let j2 = st.export(pid, r2).unwrap();
    std::thread::sleep(Duration::from_millis(500));
    st.cancel_job(j2).unwrap();
    assert_eq!(st.wait_job(j2, Duration::from_secs(30)).unwrap().state, "cancelled");
    assert!(!tmp.path().join("out/cancelado.mp4").exists());
    st.shutdown();
}

#[test]
fn subtitles_import_export_roundtrip() {
    let tmp = tempfile::tempdir().unwrap();
    let st = start(&tmp.path().join("lib"), Arc::new(Rec::default()));
    let pid = st.create_project("Legendas", Some("youtube_shorts"), None).unwrap().project.id;
    let srt = tmp.path().join("falas.srt");
    std::fs::write(&srt, "1\n00:00:01,000 --> 00:00:02,000\nPrimeira\n\n2\n00:00:02,500 --> 00:00:04,000\nSegunda <b>fala</b>\n").unwrap();
    // Importar por "importar mídia" também reconhece legendas.
    let r = st.import_media(pid, std::slice::from_ref(&srt)).unwrap();
    assert_eq!(r.subtitle_tracks.len(), 1, "{:?}", r.errors);
    let tid = r.subtitle_tracks[0];
    let p = st.project_state(pid).unwrap().project;
    assert_eq!(p.timeline.track(tid).unwrap().clips.len(), 2);
    let out = tmp.path().join("saida.vtt");
    assert_eq!(st.export_subtitles(pid, tid, &out).unwrap(), 2);
    let vtt = std::fs::read_to_string(&out).unwrap();
    assert!(vtt.starts_with("WEBVTT\n\n00:00:01.000 --> 00:00:02.000\nPrimeira"), "{vtt}");
    assert!(vtt.contains("Segunda fala"));
    assert!(st.export_subtitles(pid, tid, &tmp.path().join("x.txt")).is_err());
    // Desfazer remove a trilha importada.
    st.undo(pid).unwrap();
    assert!(st.project_state(pid).unwrap().project.timeline.track(tid).is_none());
    st.shutdown();
}

#[test]
fn ai_analysis_and_auto_edit_actions() {
    let tmp = tempfile::tempdir().unwrap();
    let st = start(&tmp.path().join("lib"), Arc::new(Rec::default()));
    // Fala sintética entre silêncios + corte de cena em 5 s.
    let speech = tmp.path().join("s.wav");
    if !std::process::Command::new("espeak-ng")
        .args(["-v", "pt-br", "-w"])
        .arg(&speech)
        .arg("Primeira frase do teste.")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        eprintln!("espeak-ng indisponível; teste pulado");
        return;
    }
    let media = tmp.path().join("fala.mp4");
    gen(
        &st,
        &[
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=22050:cl=mono:d=3",
            "-i",
            speech.to_str().unwrap(),
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=22050:cl=mono:d=4",
            "-i",
            speech.to_str().unwrap(),
            "-f",
            "lavfi",
            "-i",
            "color=c=red:s=320x240:r=25:d=5[a];color=c=blue:s=320x240:r=25:d=10[b];[a][b]concat",
            "-filter_complex",
            "[0:a][1:a][2:a][3:a]concat=n=4:v=0:a=1[aud]",
            "-map",
            "4:v",
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
        &media,
    );
    let pid = st.create_project("IA", Some("entrevistas"), None).unwrap().project.id;
    let a = st.import_media(pid, &[media]).unwrap().assets[0].clone();
    let vt = st.project_state(pid).unwrap().project.timeline.tracks[0].id;
    let clip = st
        .execute(pid, &EditCommand::InsertClip { track_id: vt, asset_id: a.id, start: Time::ZERO, source_in: None, duration: None, ripple: false })
        .unwrap()
        .0
        .created[0];
    // Sem análise: erro claro.
    assert_eq!(st.plan_pauses(pid, None, st.pause_rules(pid).unwrap()).unwrap_err().code, "not_analyzed");

    let jobs = st.analyze_media(pid, &[], false, false).unwrap();
    for j in jobs {
        let r = st.wait_job(j, Duration::from_secs(120)).unwrap();
        assert_eq!(r.state, "completed", "{:?}", r.error);
    }
    let an = st.analyses(pid).unwrap();
    let res = an.iter().find(|x| x.asset_id == a.id).unwrap().result.clone().unwrap();
    assert_eq!(res.speech.as_ref().unwrap().segments.len(), 2, "{:?}", res.speech);

    let rules = st.pause_rules(pid).unwrap();
    assert_eq!(rules.min_silence, Time::from_millis(900), "regra do preset 'entrevistas'");
    let plan = st.plan_pauses(pid, None, rules).unwrap();
    assert!(plan.cuts.len() >= 2, "silêncio inicial e entre as frases: {:?}", plan.cuts);
    let before = st.project_state(pid).unwrap().project.timeline.duration();
    let after = st.apply_pauses(pid, &plan, true).unwrap().project.timeline.duration();
    assert!((before - after - plan.removed).as_secs_f64().abs() < 0.05);
    assert!(st.project_state(pid).unwrap().history.undo_label.unwrap().starts_with("Remover pausas"));
    st.undo(pid).unwrap();

    let ps = st.apply_scenes(pid, clip, false).unwrap();
    assert_eq!(ps.project.timeline.markers.len(), 1);
    assert!((ps.project.timeline.markers[0].at.as_secs_f64() - 5.0).abs() < 0.1);
    assert_eq!(st.captions_from_transcript(pid, clip, 42).unwrap_err().code, "no_transcript");

    let status = st.ai_status();
    assert!(status.components.iter().any(|c| c.id == "analysis"));
    assert!(st.set_ai_settings(AiSettingsExt::remote()).is_err(), "Ollama remoto é recusado");
    st.shutdown();
}

#[test]
fn dashboard_auto_edit_flow_job_snapshots_review_and_export() {
    let tmp = tempfile::tempdir().unwrap();
    let st = start(&tmp.path().join("lib"), Arc::new(Rec::default()));
    let speech = tmp.path().join("s.wav");
    if !std::process::Command::new("espeak-ng")
        .args(["-v", "pt-br", "-w"])
        .arg(&speech)
        .arg("Esta é uma frase de teste para a edição automática.")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        eprintln!("espeak-ng indisponível; teste pulado");
        return;
    }
    // Vídeo horizontal: silêncio 3 s, fala, silêncio 4 s, fala; ação (cores) a partir de 5 s.
    let media = tmp.path().join("fala.mp4");
    let sp = speech.to_str().unwrap();
    gen(
        &st,
        &[
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=22050:cl=mono:d=3",
            "-i",
            sp,
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=22050:cl=mono:d=4",
            "-i",
            sp,
            "-f",
            "lavfi",
            "-i",
            "color=c=gray:s=320x180:r=25:d=5[a];testsrc2=s=320x180:r=25:d=15[b];[a][b]concat",
            "-filter_complex",
            "[0:a][1:a][2:a][3:a]concat=n=4:v=0:a=1[aud]",
            "-map",
            "4:v",
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
        &media,
    );
    let music = tmp.path().join("musica.m4a");
    gen(&st, &["-f", "lavfi", "-i", "sine=f=220:d=20", "-c:a", "aac"], &music);

    // Fluxo do dashboard: projeto com preset vertical → importar → colocar → job.
    let pid = st.create_project("Auto", Some("youtube_shorts"), None).unwrap().project.id;
    let imp = st.import_media(pid, &[media, music]).unwrap();
    assert_eq!(imp.assets.len(), 2);
    let ids: Vec<Id> = imp.assets.iter().map(|a| a.id).collect();
    let ps = st.place_assets(pid, &ids).unwrap();
    let vt = ps.project.timeline.tracks.iter().find(|t| t.kind == TrackKind::Video).unwrap();
    assert_eq!(vt.clips.len(), 1);
    assert_eq!(vt.clips[0].transform.fit, FitMode::Fill, "preset vertical reenquadra mídia horizontal");
    let before = ps.project.timeline.duration();

    // Pré-visualização sem análise: o plano recusa com mensagem clara.
    // Sem análise: sem plano, mas a lista do que falta (a interface oferece "Analisar e aplicar").
    let pv = st.auto_edit_preview(pid, &Default::default()).unwrap();
    assert!(pv.report.steps.is_empty());
    assert_eq!(pv.missing_analysis.len(), 2, "{:?}", pv.missing_analysis);

    let job = st.auto_edit_start(pid, Default::default()).unwrap();
    let r = st.wait_job(job, Duration::from_secs(240)).unwrap();
    assert_eq!(r.state, "completed", "{:?}", r.error);
    let res = r.result.unwrap();
    let steps: Vec<String> = res["report"]["steps"].as_array().unwrap().iter().map(|s| s["kind"].as_str().unwrap().to_string()).collect();
    assert!(steps.contains(&"pauses".to_string()), "{steps:?}");
    assert!(steps.contains(&"ducking".to_string()), "{steps:?}");
    // Sem transcrição instalada: o relatório explica o que foi pulado.
    assert!(res["report"]["warnings"].as_array().unwrap().iter().any(|w| w.as_str().unwrap().contains("Transcrição")), "{}", res["report"]["warnings"]);
    assert!(res["review"]["checks"].as_array().unwrap().len() >= 6);

    let ps = st.project_state(pid).unwrap();
    let after = ps.project.timeline.duration();
    assert!(after < before, "{after} < {before}");
    assert!(ps.history.undo_label.as_deref().unwrap().starts_with("Edição automática"));
    let snaps = st.list_snapshots(pid).unwrap();
    assert!(snaps.iter().any(|s| s.label.starts_with("Original")), "{snaps:?}");
    assert!(snaps.iter().any(|s| s.label.starts_with("Edição IA v1")), "{snaps:?}");

    // Pré-visualização agora funciona (análise em cache) e não altera nada.
    let rev = ps.history.revision;
    let pv = st.auto_edit_preview(pid, &Default::default()).unwrap();
    assert!(pv.missing_analysis.is_empty());
    assert_eq!(st.project_state(pid).unwrap().history.revision, rev);

    // O resultado exporta e passa na validação (keyframes, ducking, efeitos de áudio).
    let prof = st.export_profiles().into_iter().find(|p| p.id == "youtube_shorts").or_else(|| st.export_profiles().into_iter().next()).unwrap();
    let mut req = ExportRequest::from_profile(&prof, tmp.path().join("exports"), "auto".into());
    req.width = Some(360);
    req.height = Some(640);
    req.speed = "veryfast".into();
    req.encoder = Some("libx264".into());
    req.codec = "h264".into();
    req.prefer_hardware = false;
    let r = st.wait_job(st.export(pid, req).unwrap(), Duration::from_secs(300)).unwrap();
    assert_eq!(r.state, "completed", "{:?}", r.error);
    let out = r.result.unwrap();
    assert_eq!(out["report"]["ok"], true, "{}", out["report"]);
    let d = out["report"]["duration"].as_f64().unwrap_or(after.as_secs_f64());
    assert!((d - after.as_secs_f64()).abs() < 0.15, "exportado {d} × timeline {after}");

    // Desfazer volta ao original em um passo.
    st.undo(pid).unwrap();
    assert_eq!(st.project_state(pid).unwrap().project.timeline.duration(), before);
    st.shutdown();
}

/// Centróide horizontal (0..1) dos pixels claros de um frame.
fn bright_centroid_x(st: &Studio, file: &Path, t: Option<f64>) -> (f64, usize) {
    let mut c = st.ffmpeg().ffmpeg_cmd();
    c.args(["-v", "error"]);
    if let Some(t) = t {
        c.args(["-ss", &format!("{t}")]);
    }
    c.arg("-i").arg(file).args(["-frames:v", "1", "-vf", "scale=108:192", "-f", "rawvideo", "-pix_fmt", "gray", "pipe:1"]);
    let b = run_capture(c, Some(Duration::from_secs(30))).unwrap();
    let (mut sx, mut n) = (0usize, 0usize);
    for (i, v) in b.iter().enumerate() {
        if *v > 180 {
            sx += i % 108;
            n += 1;
        }
    }
    ((sx as f64 / n.max(1) as f64 + 0.5) / 108.0, n)
}

#[test]
fn shorts_suggest_create_reframe_on_face_and_batch_export() {
    let tmp = tempfile::tempdir().unwrap();
    let lib = tmp.path().join("lib");
    let st = start(&lib, Arc::new(Rec::default()));
    let speech = tmp.path().join("s.wav");
    if !std::process::Command::new("espeak-ng")
        .args(["-v", "pt-br", "-w"])
        .arg(&speech)
        .arg("Esta é uma frase completa para testar os cortes verticais.")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        eprintln!("espeak-ng indisponível; teste pulado");
        return;
    }
    // 16:9 com o "rosto" (quadrado branco) a 25% da largura; 5 frases com pausas.
    let media = tmp.path().join("entrevista.mp4");
    let sp = speech.to_str().unwrap();
    let mut args: Vec<String> = vec![];
    for _ in 0..5 {
        args.extend(["-f", "lavfi", "-i", "anullsrc=r=22050:cl=mono:d=2", "-i", sp].iter().map(|s| s.to_string()));
    }
    args.extend(["-f", "lavfi", "-i", "color=c=black:s=640x360:r=25:d=40"].iter().map(|s| s.to_string()));
    let ins: String = (0..10).map(|i| format!("[{i}:a]")).collect();
    args.extend([
        "-filter_complex".to_string(),
        format!("{ins}concat=n=10:v=0:a=1[aud];[10:v]drawbox=x=130:y=150:w=60:h=60:color=white:t=fill[v]"),
        "-map".into(),
        "[v]".into(),
        "-map".into(),
        "[aud]".into(),
        "-shortest".into(),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "ultrafast".into(),
        "-c:a".into(),
        "aac".into(),
    ]);
    gen(&st, &args.iter().map(|s| s.as_str()).collect::<Vec<_>>(), &media);

    let pid = st.create_project("Entrevista longa", Some("entrevistas"), None).unwrap().project.id;
    let a = st.import_media(pid, &[media]).unwrap().assets[0].clone();
    st.place_assets(pid, &[a.id]).unwrap();
    // Sem análise: erro claro.
    let d = st.shorts_defaults(pid, None).unwrap();
    assert_eq!(d.preset_id, "youtube_shorts");
    assert_eq!(d.framing, FramingMode::Face);
    assert_eq!(st.suggest_shorts(pid, &d.preset_id, &d.options).unwrap_err().code, "not_analyzed");

    for j in st.analyze_media(pid, &[], false, false).unwrap() {
        assert_eq!(st.wait_job(j, Duration::from_secs(120)).unwrap().state, "completed");
    }
    // O detector de rostos (YuNet) roda no worker Python, ausente aqui: o
    // resultado da detecção é injetado no cache para testar o reenquadramento.
    let safe: String = a.fingerprint.chars().filter(|c| c.is_ascii_alphanumeric() || *c == '-').collect();
    let f = lib.join("ai").join("analysis").join(safe).join("result.json");
    let mut r: serde_json::Value = serde_json::from_slice(&std::fs::read(&f).unwrap()).unwrap();
    let flicks = 705_600_000i64;
    r["visual"] = serde_json::json!({
        "interval": flicks,
        "face_detector": "yunet",
        "samples": (0..40).map(|t| serde_json::json!({"t": t as i64 * flicks, "motion": 0.01, "luma": 0.1,
            "faces": [{"x": 0.203, "y": 0.42, "w": 0.094, "h": 0.167, "score": 0.95}]})).collect::<Vec<_>>()
    });
    std::fs::write(&f, serde_json::to_vec(&r).unwrap()).unwrap();

    let before = st.project_state(pid).unwrap();
    let opts = ShortsOptions { count: 3, min_s: 8.0, max_s: 20.0, target_s: 14.0 };
    let cands = st.suggest_shorts(pid, "youtube_shorts", &opts).unwrap();
    assert!(cands.len() >= 2, "{cands:?}");
    for (i, c) in cands.iter().enumerate() {
        assert!(c.range.duration().as_secs_f64() <= 20.5);
        assert!(cands[i + 1..].iter().all(|o| o.range.intersection(&c.range).is_none()));
    }

    let out = tmp.path().join("cortes");
    let req = ShortsRequest {
        preset_id: "youtube_shorts".into(),
        framing: FramingMode::Face,
        candidates: cands[..2].to_vec(),
        captions: false,
        remove_pauses: true,
        export: Some(ShortsExport { output_dir: out.clone(), profile_id: "youtube_shorts".into() }),
    };
    let job = st.create_shorts_start(pid, req).unwrap();
    let r = st.wait_job(job, Duration::from_secs(120)).unwrap();
    assert_eq!(r.state, "completed", "{:?}", r.error);
    let res = r.result.unwrap();
    let made = res["shorts"].as_array().unwrap();
    assert_eq!(made.len(), 2);
    for m in made {
        assert!(m["face_segments"].as_u64().unwrap() >= 1, "{m}");
        let sid: Id = serde_json::from_value(m["project_id"].clone()).unwrap();
        let ps = st.project_state(sid).unwrap();
        assert_eq!((ps.project.settings.width, ps.project.settings.height), (1080, 1920));
        let main = ps.project.timeline.tracks.iter().find(|t| t.kind == TrackKind::Video).unwrap();
        assert!(
            main.clips.iter().all(|c| c.transform.fit == FitMode::Fill && c.transform.x > 0.0),
            "{:?}",
            main.clips.iter().map(|c| &c.transform).collect::<Vec<_>>()
        );
        assert!(ps.project.timeline.duration().as_secs_f64() <= 20.5);
        // Exportação em lote validada; o "rosto" fica no centro do vertical.
        let ej: Id = serde_json::from_value(m["export_job"].clone()).unwrap();
        let er = st.wait_job(ej, Duration::from_secs(300)).unwrap();
        assert_eq!(er.state, "completed", "{:?}", er.error);
        let file = PathBuf::from(er.result.as_ref().unwrap()["path"].as_str().unwrap());
        assert!(file.starts_with(&out));
        let (cx, n) = bright_centroid_x(&st, &file, Some(1.0));
        assert!(n > 20, "quadrado visível");
        assert!((cx - 0.5).abs() < 0.06, "centro do assunto em {cx}");
        assert!(st.list_snapshots(sid).unwrap().iter().any(|s| s.label == "Corte gerado"));
    }
    // A origem não muda.
    let after = st.project_state(pid).unwrap();
    assert_eq!(serde_json::to_value(&after.project.timeline).unwrap(), serde_json::to_value(&before.project.timeline).unwrap());
    assert_eq!(after.history.undo_label, before.history.undo_label);

    // Corte central: o quadrado (a 25%) fica fora do quadro vertical.
    let mut req2 = ShortsRequest {
        preset_id: "youtube_shorts".into(),
        framing: FramingMode::Center,
        candidates: vec![cands[0].clone()],
        captions: false,
        remove_pauses: false,
        export: None,
    };
    let c = st.create_short(pid, &cands[0], &req2).unwrap();
    let fr = st.render_frame(c.project_id, Time::from_secs(1), 640).unwrap();
    assert!(bright_centroid_x(&st, &fr, None).1 < 5, "corte central não mostra o quadrado lateral");

    // Fundo desfocado: trilha de fundo abaixo, vídeo inteiro por cima.
    req2.framing = FramingMode::BlurBackground;
    let c = st.create_short(pid, &cands[0], &req2).unwrap();
    let ps = st.project_state(c.project_id).unwrap();
    let t0 = &ps.project.timeline.tracks[0];
    assert_eq!(t0.name, "Fundo desfocado");
    assert!(t0.clips.iter().all(|c| c.effects.blur > 0.0 && c.transform.fit == FitMode::Fill && c.audio_muted));
    let main = ps.project.timeline.tracks.iter().filter(|t| t.kind == TrackKind::Video).nth(1).unwrap();
    assert!(main.clips.iter().all(|c| c.transform.fit == FitMode::Fit));
    let fr = st.render_frame(c.project_id, Time::from_secs(1), 640).unwrap();
    let (cx, n) = bright_centroid_x(&st, &fr, None);
    assert!(n > 3 && (cx - 0.25).abs() < 0.06, "vídeo inteiro visível: quadrado a ~25% da largura ({cx}, {n})");
    st.shutdown();
}

#[test]
fn short_ending_at_unaligned_timeline_end() {
    // Regressão: mídia de 7,71 s (não múltipla do quadro) inserida inteira; o
    // corte até o fim da timeline gerava "intervalo vazio" após o arredondamento.
    let tmp = tempfile::tempdir().unwrap();
    let st = start(&tmp.path().join("lib"), Arc::new(Rec::default()));
    let media = tmp.path().join("v.mp4");
    gen(
        &st,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=320x180:r=25:d=7.71",
            "-f",
            "lavfi",
            "-i",
            "sine=f=300:d=7.71",
            "-shortest",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-c:a",
            "aac",
        ],
        &media,
    );
    let pid = st.create_project("Origem", Some("entrevistas"), None).unwrap().project.id;
    let a = st.import_media(pid, &[media]).unwrap().assets[0].clone();
    let vt = st.project_state(pid).unwrap().project.timeline.tracks[0].id;
    st.execute(pid, &EditCommand::InsertClip { track_id: vt, asset_id: a.id, start: Time::ZERO, source_in: None, duration: None, ripple: false }).unwrap();
    let end = st.project_state(pid).unwrap().project.timeline.duration();
    let cand = avs_ai::shorts::ShortCandidate {
        rank: 1,
        range: TimeRange::new(Time::from_secs(2), end.snap_to_frame(Rational::new(30, 1))),
        score: 50.0,
        title: "Fim".into(),
        explanation: String::new(),
        units: 1,
        speech_s: 0.0,
    };
    let req = ShortsRequest {
        preset_id: "youtube_shorts".into(),
        framing: FramingMode::BlurBackground,
        candidates: vec![cand.clone()],
        captions: false,
        remove_pauses: false,
        export: None,
    };
    let c = st.create_short(pid, &cand, &req).unwrap();
    let d = c.duration.as_secs_f64();
    assert!((d - (end.as_secs_f64() - 2.0)).abs() < 0.05, "{d}");
    st.shutdown();
}

/// Servidor que fala o protocolo do Ollama e devolve uma resposta fixa (teste).
fn fake_ollama_assistant(answer: &'static str) -> String {
    use std::io::{BufRead, BufReader, Read, Write};
    let l = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let addr = l.local_addr().unwrap();
    std::thread::spawn(move || {
        for stream in l.incoming().flatten() {
            let mut r = BufReader::new(stream.try_clone().unwrap());
            let mut first = String::new();
            if r.read_line(&mut first).is_err() {
                continue;
            }
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
                "/api/tags" => r#"{"models":[{"name":"qwen3:4b","size":2500000000,"details":{}}]}"#.into(),
                "/api/generate" => serde_json::json!({ "response": answer, "done": true }).to_string(),
                _ => "{}".into(),
            };
            let mut s = stream;
            let _ = write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", resp.len(), resp);
        }
    });
    format!("http://{addr}")
}

#[test]
fn assistant_plan_apply_undo_export_and_local_llm() {
    use avs_ai::assistant::AssistantContext;
    let tmp = tempfile::tempdir().unwrap();
    let st = start(&tmp.path().join("lib"), Arc::new(Rec::default()));
    let speech = tmp.path().join("s.wav");
    if !std::process::Command::new("espeak-ng")
        .args(["-v", "pt-br", "-w"])
        .arg(&speech)
        .arg("Uma frase curta para o assistente.")
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
    {
        eprintln!("espeak-ng indisponível; teste pulado");
        return;
    }
    let media = tmp.path().join("v.mp4");
    let sp = speech.to_str().unwrap();
    gen(
        &st,
        &[
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=22050:cl=mono:d=2",
            "-i",
            sp,
            "-f",
            "lavfi",
            "-i",
            "anullsrc=r=22050:cl=mono:d=3",
            "-i",
            sp,
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=320x180:r=25:d=20",
            "-filter_complex",
            "[0:a][1:a][2:a][3:a]concat=n=4:v=0:a=1[aud]",
            "-map",
            "4:v",
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
        &media,
    );
    let pid = st.create_project("Assistente", Some("entrevistas"), None).unwrap().project.id;
    let a = st.import_media(pid, &[media]).unwrap().assets[0].clone();
    st.place_assets(pid, &[a.id]).unwrap();
    for j in st.analyze_media(pid, &[], false, false).unwrap() {
        assert_eq!(st.wait_job(j, Duration::from_secs(120)).unwrap().state, "completed");
    }
    let ctx = AssistantContext { selected_clip: None, playhead: Time::ZERO };

    // Regras: prévia não altera nada; aplicar = um passo; desfazer volta.
    let rev = st.project_state(pid).unwrap().history.revision;
    let plan = st.assistant_plan(pid, "remova as pausas e adicione o texto \"Olá\" em 0:01 por 2 segundos", &ctx, false).unwrap();
    assert_eq!(plan.source, "regras");
    assert_eq!(plan.steps.len(), 2, "{:?}", plan.steps);
    assert!(plan.after < plan.before);
    assert!(plan.data_sent.is_none(), "nada sai do app com as regras");
    assert_eq!(st.project_state(pid).unwrap().history.revision, rev);
    let ps = st.assistant_apply(pid, &plan, &ctx).unwrap();
    assert_eq!(ps.project.timeline.duration(), plan.after);
    assert!(ps.history.undo_label.as_deref().unwrap().starts_with("Assistente: remova as pausas"));

    // O resultado exporta e passa na validação.
    let prof = st.export_profiles().into_iter().find(|p| p.id == "youtube_1080p").unwrap();
    let mut req = ExportRequest::from_profile(&prof, tmp.path().join("out"), "assistente".into());
    req.width = Some(640);
    req.height = Some(360);
    req.speed = "veryfast".into();
    req.encoder = Some("libx264".into());
    req.prefer_hardware = false;
    let r = st.wait_job(st.export(pid, req).unwrap(), Duration::from_secs(300)).unwrap();
    assert_eq!(r.state, "completed", "{:?}", r.error);
    assert_eq!(r.result.unwrap()["report"]["ok"], true);

    st.undo(pid).unwrap();
    assert_eq!(st.project_state(pid).unwrap().project.timeline.duration(), plan.before);

    // Plano antigo não é aplicado sobre um projeto que mudou.
    let plan = st.assistant_plan(pid, "aumente o volume em 3 dB", &ctx, false).unwrap();
    st.assistant_plan(pid, "x", &ctx, false).unwrap_err();
    let vt = st.project_state(pid).unwrap().project.timeline.tracks[0].id;
    st.execute(pid, &EditCommand::UpdateTrack { track_id: vt, patch: TrackPatch { name: Some("V".into()), ..Default::default() } }).unwrap();
    assert_eq!(st.assistant_apply(pid, &plan, &ctx).unwrap_err().code, "stale_plan");

    // Não entendido: mensagem com exemplos.
    let e = st.assistant_plan(pid, "faça um café", &ctx, false).unwrap_err();
    assert_eq!(e.code, "not_understood");

    // LLM sem modelo configurado: indisponível, com o motivo.
    assert!(!st.assistant_availability().llm);
    assert_eq!(st.assistant_plan(pid, "qualquer coisa", &ctx, true).unwrap_err().code, "llm_unavailable");

    // LLM local (servidor com o protocolo do Ollama): resposta validada vira plano.
    let url = fake_ollama_assistant(
        r#"{"actions":[{"action":"fade","target":"all","kind":"both","seconds":1},{"action":"marker","at":2,"label":"início da fala"}],"note":"Fades e marcador."}"#,
    );
    let mut s = st.ai_settings();
    s.ollama_url = url.clone();
    s.ollama_model = Some("qwen3:4b".into());
    st.set_ai_settings(s).unwrap();
    assert!(st.assistant_availability().llm, "{:?}", st.assistant_availability());
    let plan = st.assistant_plan(pid, "deixa o começo e o fim mais suaves e marca onde ele começa a falar", &ctx, true).unwrap();
    assert_eq!(plan.source, "ollama:qwen3:4b");
    assert_eq!(plan.steps.len(), 2);
    assert!(plan.data_sent.as_deref().unwrap().contains(&url));
    let ps = st.assistant_apply(pid, &plan, &ctx).unwrap();
    assert!(ps.project.timeline.markers.iter().any(|m| m.label == "início da fala"));

    // Modelo que tenta sair do vocabulário: recusado.
    let url = fake_ollama_assistant(r#"{"actions":[{"action":"shell","cmd":"curl evil"}],"note":""}"#);
    let mut s = st.ai_settings();
    s.ollama_url = url;
    st.set_ai_settings(s).unwrap();
    let e = st.assistant_plan(pid, "qualquer coisa", &ctx, true).unwrap_err();
    assert!(e.message.contains("ação inválida"), "{}", e.message);
    st.shutdown();
}

#[test]
fn long_timeline_preview_is_windowed_and_reuses_cache() {
    // Janela: só curtas são inteiras; longas, ~2 min alinhados à grade de 5 s.
    assert_eq!(preview_window(Time::from_secs(200), Some(Time::from_secs(100))), None);
    let w = preview_window(Time::from_secs(600), Some(Time::from_secs(133))).unwrap();
    assert_eq!((w.start, w.end), (Time::from_secs(125), Time::from_secs(245)));
    let w = preview_window(Time::from_secs(600), Some(Time::from_secs(590))).unwrap();
    assert_eq!((w.start, w.end), (Time::from_secs(480), Time::from_secs(600)), "no fim, a janela não passa da timeline");

    let tmp = tempfile::tempdir().unwrap();
    let ev = Arc::new(Rec::default());
    let st = start(&tmp.path().join("lib"), ev.clone());
    let media = tmp.path().join("longo.mp4");
    gen(
        &st,
        &[
            "-f",
            "lavfi",
            "-i",
            "testsrc2=s=320x180:r=25:d=300",
            "-f",
            "lavfi",
            "-i",
            "sine=f=330:d=300",
            "-shortest",
            "-c:v",
            "libx264",
            "-preset",
            "ultrafast",
            "-c:a",
            "aac",
        ],
        &media,
    );
    let pid = st.create_project("Longo", Some("entrevistas"), None).unwrap().project.id;
    let a = st.import_media(pid, &[media]).unwrap().assets[0].clone();
    st.place_assets(pid, &[a.id]).unwrap();
    // Miniaturas/waveform terminam antes (atualizar a mídia muda a revisão).
    while st.jobs().unwrap().iter().any(|j| j.kind == "import" && (j.state == "queued" || j.state == "running")) {
        std::thread::sleep(Duration::from_millis(100));
    }

    // Primeira janela em volta de 1:00.
    let t0 = std::time::Instant::now();
    let r = st.wait_job(st.request_preview_at(pid, Some(Time::from_secs(60))).unwrap(), Duration::from_secs(300)).unwrap();
    assert_eq!(r.state, "completed", "{:?}", r.error);
    let first_ms = t0.elapsed().as_millis();
    let res = r.result.unwrap();
    assert_eq!(res["range"]["start"].as_i64().unwrap(), Time::from_secs(55).flicks());
    assert_eq!(res["chunks_total"], 24, "120 s em blocos de 5 s");
    let out = PathBuf::from(res["path"].as_str().unwrap());
    let d = avs_media::probe::probe(st.ffmpeg(), &out).unwrap().duration.as_secs_f64();
    assert!((d - 120.0).abs() < 0.1, "arquivo da janela com {d} s");
    // Cada fluxo também (o player para no fim do vídeo; deriva aqui = dessincronia).
    for s in ["v:0", "a:0"] {
        let o = std::process::Command::new(&st.ffmpeg().ffprobe)
            .args(["-v", "error", "-select_streams", s, "-show_entries", "stream=duration", "-of", "csv=p=0"])
            .arg(&out)
            .output()
            .unwrap();
        let sd: f64 = String::from_utf8_lossy(&o.stdout).trim().parse().unwrap();
        eprintln!("janela {s}: {sd} s");
        assert!((sd - 120.0).abs() < 0.1, "fluxo {s} da janela com {sd} s");
    }
    assert!(ev.0.lock().unwrap().iter().any(|e| matches!(e, AppEvent::PreviewReady { range: Some(r), .. } if r.start == Time::from_secs(55))));

    // Janela seguinte (pedida 10 s antes do fim da atual): a sobreposição vem do cache.
    let r2 = st.wait_job(st.request_preview_at(pid, Some(Time::from_secs(165))).unwrap(), Duration::from_secs(300)).unwrap();
    let res2 = r2.result.unwrap();
    assert_eq!(res2["range"]["start"].as_i64().unwrap(), Time::from_secs(160).flicks());
    assert_eq!(res2["chunks_reused"], 3, "160–175 s já estavam prontos: {res2}");
    // As duas janelas existem (a anterior pode estar tocando).
    assert!(out.exists() && PathBuf::from(res2["path"].as_str().unwrap()).exists());
    eprintln!("primeira janela de 2 min: {first_ms} ms");
    st.shutdown();
}

struct AiSettingsExt;
impl AiSettingsExt {
    fn remote() -> avs_app::ai::AiSettings {
        avs_app::ai::AiSettings { ollama_url: "http://203.0.113.5:11434".into(), ..Default::default() }
    }
}
