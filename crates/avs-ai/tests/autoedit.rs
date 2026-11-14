//! Fase 4: pontuação, "Editar automaticamente" e revisão de qualidade.
//! Análises sintéticas e determinísticas aplicadas num projeto real
//! (`EditSession::transaction`), validando a timeline resultante.

use avs_ai::analysis::*;
use avs_ai::autoplan::{self, AutoEditOptions, CutIntensity, PresetRules};
use avs_ai::review::{review, CheckLevel, ReviewTargets};
use avs_ai::scoring::*;
use avs_core::*;
use std::collections::{BTreeMap, HashMap};

fn s(x: f64) -> Time {
    Time::from_secs_f64(x)
}

fn asset(secs: i64, kind: AssetKind, w: u32, h: u32) -> MediaAsset {
    MediaAsset {
        id: new_id(),
        kind,
        name: "m.mp4".into(),
        path: "/x/m.mp4".into(),
        fingerprint: "fp".into(),
        size_bytes: 1,
        duration: Time::from_secs(secs),
        container: "mp4".into(),
        video: (kind == AssetKind::Video).then(|| VideoInfo {
            codec: "h264".into(),
            width: w,
            height: h,
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

/// Fala nos trechos dados; movimento por segundo; volume −20 dB na fala, −70 fora.
fn analysis(dur: f64, speech: &[(f64, f64)], motion: impl Fn(f64) -> f32, luma: f32) -> AnalysisResult {
    let n = dur as usize;
    let in_speech = |t: f64| speech.iter().any(|(a, b)| t >= *a && t < *b);
    AnalysisResult {
        version: ANALYSIS_VERSION,
        fingerprint: "fp".into(),
        duration: s(dur),
        loudness: Some(LoudnessAnalysis {
            interval: s(0.5),
            rms_db: (0..n * 2).map(|i| if in_speech(i as f64 * 0.5) { -20.0 } else { -70.0 }).collect(),
            peak_db: -3.0,
        }),
        speech: Some(SpeechAnalysis {
            method: "silero".into(),
            segments: speech.iter().map(|(a, b)| TimeRange::new(s(*a), s(*b))).collect(),
            ratio: speech.iter().map(|(a, b)| b - a).sum::<f64>() as f32 / dur as f32,
        }),
        scenes: Some(SceneAnalysis { method: "ffmpeg".into(), cuts: vec![] }),
        visual: Some(VisualAnalysis {
            interval: s(1.0),
            face_detector: None,
            samples: (0..n).map(|i| VisualSample { t: s(i as f64), motion: motion(i as f64), luma, faces: vec![] }).collect(),
        }),
        transcript: None,
        warnings: vec![],
    }
}

fn rules() -> PresetRules {
    PresetRules {
        name: "Teste".into(),
        remove_silence: true,
        min_silence_ms: 700,
        padding_ms: 150,
        preserve_silent_action: false,
        remove_fillers: false,
        captions: false,
        caption_y: 0.8,
        caption_size: 64.0,
        transitions: vec!["crossfade".into()],
        transition_ms: 300,
        transition_frequency: "moderate".into(),
        effects: vec!["zoom".into(), "color_correction".into()],
        effect_intensity: "subtle".into(),
        narrative: "preserve".into(),
        target_s: None,
        max_s: None,
        music_ducking_db: -12.0,
        noise_reduction: true,
        voice_priority: true,
        scoring_weights: default_weights(),
        safe_bottom_pct: 0.1,
    }
}

struct Fixture {
    sess: EditSession,
    analyses: HashMap<Id, AnalysisResult>,
    video: Id,
    music_clip: Option<Id>,
}

fn fixture(dur: i64, a: AnalysisResult, with_music: bool) -> Fixture {
    let mut p = Project::new("auto", ProjectSettings::default());
    let v = asset(dur, AssetKind::Video, 1920, 1080);
    let vid = v.id;
    p.assets.push(v);
    let mut analyses = HashMap::from([(vid, a)]);
    let mut music_aid = None;
    if with_music {
        let m = asset(dur, AssetKind::Audio, 0, 0);
        music_aid = Some(m.id);
        // Música: sem fala, volume constante.
        analyses.insert(m.id, analysis(dur as f64, &[], |_| 0.0, 0.0));
        analyses.get_mut(&m.id).unwrap().visual = None;
        p.assets.push(m);
    }
    let vt = p.timeline.tracks[0].id;
    let at = p.timeline.tracks[2].id;
    let mut sess = EditSession::new(p);
    sess.execute(&EditCommand::InsertClip { track_id: vt, asset_id: vid, start: Time::ZERO, source_in: None, duration: None, ripple: false }).unwrap();
    let music_clip = music_aid.map(|m| {
        sess.execute(&EditCommand::InsertClip { track_id: at, asset_id: m, start: Time::ZERO, source_in: None, duration: None, ripple: false }).unwrap().created
            [0]
    });
    Fixture { sess, analyses, video: vid, music_clip }
}

#[test]
fn scoring_prefers_expressive_complete_speech_and_explains() {
    // Fala 1: 2–8 s com muito movimento; fala 2: 12–18 s parada; fala 3: 22–23 s (curta).
    let a = analysis(30.0, &[(2.0, 8.0), (12.0, 18.0), (22.0, 23.0)], |t| if (2.0..8.0).contains(&t) { 0.2 } else { 0.01 }, 0.5);
    let units = candidate_units(&a, TimeRange::new(Time::ZERO, s(30.0)));
    assert_eq!(units.len(), 3, "{units:?}");
    let mut w = default_weights();
    w.insert("visual_action".into(), 0.5);
    let sc = score_units(&a, &units, &w);
    assert!(sc.iter().all(|u| (0.0..=100.0).contains(&u.score)));
    assert!(sc[0].score > sc[1].score, "{} vs {}", sc[0].score, sc[1].score);
    assert!(sc.iter().all(|u| !u.explanation.is_empty()));
    assert!(sc[0].explanation.contains(factor_label("visual_action")), "{}", sc[0].explanation);
}

#[test]
fn auto_edit_is_one_undoable_step_and_report_matches_timeline() {
    // 60 s: fala em blocos com pausas longas; ação no meio de uma fala.
    let a = analysis(60.0, &[(1.0, 10.0), (14.0, 24.0), (30.0, 40.0), (47.0, 58.0)], |t| if (35.0..37.0).contains(&t) { 0.5 } else { 0.01 }, 0.2);
    let mut f = fixture(60, a, true);
    let before_rev = f.sess.info().revision;
    let opts = AutoEditOptions::default();
    let r = rules();
    let analyses = f.analyses.clone();
    let rep = f.sess.transaction("Edição automática", |p| autoplan::run(p, &r, &analyses, &opts)).unwrap();
    let p = &f.sess.project;
    assert!(validate_project(p).is_empty(), "{:?}", validate_project(p));
    // Pausas: início 0–1 e final 58–60 (folga só do lado da fala) + 3 internas
    // (0,15 s de folga de cada lado).
    let removed: f64 = rep.removed.iter().map(|x| x.duration().as_secs_f64()).sum();
    let expect = (1.0 - 0.15) + (4.0 - 0.3) + (6.0 - 0.3) + (7.0 - 0.3) + (2.0 - 0.15);
    assert!((removed - expect).abs() < 0.1, "removido {removed} esperado {expect}");
    let vt = p.timeline.tracks[0].id;
    let vdur = p.timeline.track(vt).unwrap().clips.iter().map(|c| c.end()).max().unwrap();
    // Transições (moderate: só em saltos ≥ 2 s) encurtam a trilha pela sobreposição.
    let ntr = p.timeline.transitions.len() as f64;
    assert!(ntr >= 1.0, "esperava transições nos saltos");
    assert!((vdur.as_secs_f64() - (60.0 - removed - ntr * 0.3)).abs() < 0.1, "trilha {vdur} removido {removed} trans {ntr}");
    assert_eq!(rep.after, p.timeline.duration());
    let kinds: Vec<&str> = rep.steps.iter().map(|s| s.kind.as_str()).collect();
    for k in ["pauses", "transitions", "zoom", "audio", "ducking", "color"] {
        assert!(kinds.contains(&k), "etapa {k} ausente: {kinds:?}");
    }
    // Áudio da fala tratado, cor corrigida (luma 0.2), zoom ≥ 1 em algum clipe.
    let clips = &p.timeline.track(vt).unwrap().clips;
    assert!(clips.iter().all(|c| c.audio_fx.denoise && c.audio_fx.compressor && c.audio_fx.highpass));
    assert!(clips.iter().all(|c| c.color.brightness > 0.0));
    let zoomed: Vec<&Clip> = clips.iter().filter(|c| !c.keyframes.scale.is_empty()).collect();
    assert_eq!(zoomed.len(), 1, "um único pico de ação");
    assert!(zoomed[0].keyframes.scale.iter().all(|k| k.value >= 1.0));
    // O zoom cai no pico (origem 35–37 s).
    let z = zoomed[0];
    let peak_src = z.source_in() + z.keyframes.scale.iter().find(|k| k.value > 1.0).unwrap().at;
    assert!((35.0..37.5).contains(&peak_src.as_secs_f64()), "zoom em {peak_src}");
    // Ducking: a música abaixa durante a fala.
    let mc = p.timeline.clip(f.music_clip.unwrap()).unwrap();
    assert!(mc.keyframes.volume.iter().any(|k| (k.value - 10f32.powf(-12.0 / 20.0)).abs() < 1e-3));
    // Uma única entrada no histórico; desfazer restaura tudo.
    assert_eq!(f.sess.info().revision, before_rev + 1);
    f.sess.undo();
    assert_eq!(f.sess.project.timeline.duration(), Time::from_secs(60));
    assert!(f.sess.project.timeline.transitions.is_empty());
    let _ = f.video;
}

#[test]
fn auto_edit_selects_best_moments_to_target_and_never_stretches() {
    // 120 s com 6 falas de 8 s; as falas 2 e 5 têm muita ação.
    let sp: Vec<(f64, f64)> = (0..6).map(|i| (i as f64 * 20.0 + 2.0, i as f64 * 20.0 + 10.0)).collect();
    let a = analysis(120.0, &sp, |t| if (22.0..30.0).contains(&t) || (82.0..90.0).contains(&t) { 0.4 } else { 0.01 }, 0.5);
    let mut f = fixture(120, a.clone(), false);
    let mut r = rules();
    r.narrative = "best_moments".into();
    r.transitions.clear();
    r.effects.clear();
    r.scoring_weights.insert("visual_action".into(), 0.6);
    let opts = AutoEditOptions { target_duration: Some(s(20.0)), ..Default::default() };
    let analyses = f.analyses.clone();
    let rep = f.sess.transaction("Auto", |p| autoplan::run(p, &r, &analyses, &opts)).unwrap();
    let d = f.sess.project.timeline.duration().as_secs_f64();
    assert!((12.0..=20.5).contains(&d), "duração {d}");
    let kept: Vec<&autoplan::SelectedMoment> = rep.moments.iter().filter(|m| m.kept).collect();
    assert_eq!(kept.len(), 2, "{:?}", rep.moments.iter().map(|m| (m.range.start.as_secs_f64(), m.score, m.kept)).collect::<Vec<_>>());
    // Os escolhidos são os de ação, em ordem cronológica.
    let vt = f.sess.project.timeline.tracks[0].id;
    let srcs: Vec<f64> = f.sess.project.timeline.track(vt).unwrap().clips.iter().map(|c| c.source_in().as_secs_f64()).collect();
    assert!(srcs.windows(2).all(|w| w[0] < w[1]), "{srcs:?}");
    assert!(srcs.iter().all(|x| (20.0..32.0).contains(x) || (80.0..92.0).contains(x)), "{srcs:?}");

    // Alvo maior que o material: avisa, não estica.
    let mut g = fixture(120, a, false);
    let opts = AutoEditOptions { target_duration: Some(s(300.0)), ..Default::default() };
    let analyses = g.analyses.clone();
    let rep = g.sess.transaction("Auto", |p| autoplan::run(p, &r, &analyses, &opts)).unwrap();
    assert!(g.sess.project.timeline.duration() <= s(120.0));
    let _ = rep;
}

#[test]
fn auto_edit_conservative_cuts_less_than_aggressive() {
    let a = analysis(40.0, &[(1.0, 5.0), (6.0, 10.0), (11.5, 15.0), (17.0, 25.0), (26.0, 39.0)], |_| 0.01, 0.5);
    let mut out = vec![];
    for cuts in [CutIntensity::Conservative, CutIntensity::Balanced, CutIntensity::Aggressive] {
        let mut f = fixture(40, a.clone(), false);
        let r = rules();
        let opts = AutoEditOptions { cuts, transitions: false, zoom: false, ..Default::default() };
        let analyses = f.analyses.clone();
        f.sess.transaction("Auto", |p| autoplan::run(p, &r, &analyses, &opts)).unwrap();
        out.push(f.sess.project.timeline.duration());
    }
    assert!(out[0] > out[1] && out[1] > out[2], "{out:?}");
}

#[test]
fn auto_edit_refuses_without_analysis() {
    let a = analysis(10.0, &[], |_| 0.0, 0.5);
    let mut f = fixture(10, a, false);
    let r = rules();
    let empty = HashMap::new();
    let err = f.sess.transaction("Auto", |p| autoplan::run(p, &r, &empty, &AutoEditOptions::default())).unwrap_err();
    assert!(err.to_string().contains("analisad"), "{err}");
    assert_eq!(f.sess.project.timeline.duration(), Time::from_secs(10), "nada mudou");
}

#[test]
fn review_finds_real_problems_and_never_claims_perfection() {
    let a = analysis(30.0, &[(2.0, 12.0), (15.0, 25.0)], |_| 0.05, 0.5);
    let mut f = fixture(30, a, false);
    let analyses = f.analyses.clone();
    let vt = f.sess.project.timeline.tracks[0].id;
    let tt = f.sess.project.timeline.tracks[1].id;
    // Remove 6–18 s: o corte junta o meio de duas falas.
    f.sess.execute(&EditCommand::DeleteRange { start: s(6.0), end: s(18.0), track_ids: Some(vec![vt]), ripple: true }).unwrap();
    // Texto fora da área segura e dois textos sobrepostos.
    let low = TextStyle { y: 0.97, ..Default::default() };
    f.sess.execute(&EditCommand::AddText { track_id: tt, start: s(1.0), duration: s(2.0), text: "rodapé".into(), style: Some(low) }).unwrap();
    f.sess.execute(&EditCommand::AddTrack { kind: TrackKind::Text, name: Some("T2".into()) }).ok();
    let t2 = f.sess.project.timeline.tracks.iter().filter(|t| t.kind == TrackKind::Text).nth(1).map(|t| t.id).unwrap();
    f.sess.execute(&EditCommand::AddText { track_id: tt, start: s(8.0), duration: s(2.0), text: "a".into(), style: None }).unwrap();
    f.sess.execute(&EditCommand::AddText { track_id: t2, start: s(9.0), duration: s(2.0), text: "b".into(), style: None }).unwrap();
    let rep = review(&f.sess.project, &analyses, &ReviewTargets { target_s: Some(60.0), max_s: Some(15.0), safe_bottom_pct: 0.1 });
    let lvl = |id: &str| rep.checks.iter().find(|c| c.id == id).unwrap().level;
    assert_eq!(lvl("duration"), CheckLevel::Fail);
    assert_eq!(lvl("cut_mid_speech"), CheckLevel::Warn);
    let cut = &rep.checks.iter().find(|c| c.id == "cut_mid_speech").unwrap().issues[0];
    assert!((cut.at.unwrap().as_secs_f64() - 6.0).abs() < 0.05);
    assert_eq!(lvl("text"), CheckLevel::Warn);
    assert!(rep.checks.iter().find(|c| c.id == "text").unwrap().issues.len() >= 2);
    assert_eq!(rep.level, CheckLevel::Fail);

    // Projeto limpo: "pass", mas o texto não promete perfeição.
    let a = analysis(10.0, &[(1.0, 9.0)], |_| 0.05, 0.5);
    let g = fixture(10, a, false);
    let rep = review(&g.sess.project, &g.analyses, &ReviewTargets::default());
    assert_eq!(rep.level, CheckLevel::Pass, "{:#?}", rep.checks.iter().filter(|c| c.level != CheckLevel::Pass).collect::<Vec<_>>());
    assert!(rep.summary.contains("Assista"), "{}", rep.summary);
    let _: BTreeMap<String, f32> = default_weights();
}
