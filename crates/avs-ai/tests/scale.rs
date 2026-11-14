//! Fase 7: escala. Gravação de 3 h (análise sintética com ~1800 trechos de
//! fala) — o planejamento, a aplicação de centenas de cortes numa transação,
//! a revisão, as sugestões de cortes e o desfazer precisam continuar rápidos
//! (os limites são folgados para build de debug em CI).

use avs_ai::analysis::*;
use avs_ai::autoplan::{self, AutoEditOptions, PresetRules};
use avs_ai::review::{review, ReviewTargets};
use avs_ai::scoring::default_weights;
use avs_ai::shorts::{suggest, ShortsOptions};
use avs_core::*;
use std::collections::HashMap;
use std::time::Instant;

fn s(x: f64) -> Time {
    Time::from_secs_f64(x)
}

#[test]
fn three_hour_recording_plans_and_applies_fast() {
    let dur = 3.0 * 3600.0;
    // Fala de 4,6 s alternando pausas de 0,4 / 2,2 / 0,3 / 1,6 s (como o `avs bench`).
    let gaps = [0.4, 2.2, 0.3, 1.6];
    let mut speech = vec![];
    let mut t = 0.4;
    let mut k = 0;
    while t + 4.6 < dur {
        speech.push(TimeRange::new(s(t), s(t + 4.6)));
        t += 4.6 + gaps[k % 4];
        k += 1;
    }
    let n = (dur / 0.1) as usize;
    let a = AnalysisResult {
        version: ANALYSIS_VERSION,
        fingerprint: "fp".into(),
        duration: s(dur),
        loudness: Some(LoudnessAnalysis {
            interval: s(0.1),
            rms_db: (0..n).map(|i| if speech.iter().any(|r| r.contains(s(i as f64 * 0.1))) { -20.0 } else { -70.0 }).collect(),
            peak_db: -3.0,
        }),
        speech: Some(SpeechAnalysis { method: "silero".into(), segments: speech.clone(), ratio: 0.8 }),
        scenes: Some(SceneAnalysis { method: "pyscenedetect".into(), cuts: (1..400).map(|i| s(i as f64 * 27.0)).collect() }),
        visual: Some(VisualAnalysis {
            interval: s(2.0),
            face_detector: Some("yunet".into()),
            samples: (0..(dur / 2.0) as usize)
                .map(|i| VisualSample { t: s(i as f64 * 2.0), motion: ((i * 7919) % 100) as f32 / 1000.0, luma: 0.4, faces: vec![] })
                .collect(),
        }),
        transcript: None,
        warnings: vec![],
    };
    eprintln!("{} trechos de fala", speech.len());
    let mut p = Project::new("3h", ProjectSettings::default());
    let mut asset = MediaAsset {
        id: new_id(),
        kind: AssetKind::Video,
        name: "live.mp4".into(),
        path: "/x".into(),
        fingerprint: "fp".into(),
        size_bytes: 1,
        duration: s(dur),
        container: "mp4".into(),
        video: Some(VideoInfo {
            codec: "h264".into(),
            width: 1920,
            height: 1080,
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
    };
    asset.status = AssetStatus::Ready;
    let aid = asset.id;
    p.assets.push(asset);
    let vt = p.timeline.tracks[0].id;
    let mut sess = EditSession::new(p);
    sess.execute(&EditCommand::InsertClip { track_id: vt, asset_id: aid, start: Time::ZERO, source_in: None, duration: None, ripple: false }).unwrap();
    let an = HashMap::from([(aid, a)]);
    let rules = PresetRules {
        name: "Lives".into(),
        remove_silence: true,
        min_silence_ms: 900,
        padding_ms: 150,
        preserve_silent_action: false,
        remove_fillers: false,
        captions: false,
        caption_y: 0.85,
        caption_size: 52.0,
        transitions: vec![],
        transition_ms: 300,
        transition_frequency: "rare".into(),
        effects: vec![],
        effect_intensity: "off".into(),
        narrative: "preserve".into(),
        target_s: None,
        max_s: None,
        music_ducking_db: -12.0,
        noise_reduction: true,
        voice_priority: true,
        scoring_weights: default_weights(),
        safe_bottom_pct: 0.05,
    };

    let t0 = Instant::now();
    let rep = sess.transaction("Auto", |p| autoplan::run(p, &rules, &an, &AutoEditOptions { zoom: false, transitions: false, ..Default::default() })).unwrap();
    let t_plan = t0.elapsed();
    let clips = sess.project.timeline.tracks[0].clips.len();
    eprintln!("edição automática: {:?}, {} → {} s, {clips} clipes", t_plan, rep.before.as_secs_f64() as i64, rep.after.as_secs_f64() as i64);
    assert!(clips > 900, "{clips}");
    assert!(t_plan.as_secs_f64() < 10.0, "edição automática de 3 h levou {t_plan:?}");

    let t0 = Instant::now();
    let r = review(&sess.project, &an, &ReviewTargets::default());
    let t_rev = t0.elapsed();
    eprintln!("revisão: {t_rev:?} ({} pontos)", r.checks.iter().map(|c| c.issues.len()).sum::<usize>());
    assert!(t_rev.as_secs_f64() < 2.0, "revisão levou {t_rev:?} (antes da indexação: 1,9 s; depois: ~0,07 s em debug)");

    let t0 = Instant::now();
    let c = suggest(&sess.project, vt, &an, &default_weights(), &ShortsOptions { count: 10, min_s: 20.0, max_s: 60.0, target_s: 40.0 });
    let t_sug = t0.elapsed();
    eprintln!("sugestões: {t_sug:?} ({} cortes)", c.len());
    assert_eq!(c.len(), 10);
    assert!(t_sug.as_secs_f64() < 2.0, "sugestões levaram {t_sug:?} (antes: 3,4 s; depois: ~0,13 s em debug)");

    let t0 = Instant::now();
    sess.undo();
    sess.redo();
    let t_ur = t0.elapsed();
    eprintln!("desfazer + refazer: {t_ur:?}");
    assert!(t_ur.as_secs_f64() < 2.0);

    let t0 = Instant::now();
    let json = serde_json::to_vec(&sess.project).unwrap();
    eprintln!("serializar: {:?} ({} KB)", t0.elapsed(), json.len() / 1024);
    assert!(validate_project(&sess.project).is_empty());
}
