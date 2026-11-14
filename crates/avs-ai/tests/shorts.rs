//! Fase 5: sugestão de cortes e reenquadramento vertical.

use avs_ai::analysis::*;
use avs_ai::scoring::default_weights;
use avs_ai::shorts::*;
use avs_core::*;
use std::collections::HashMap;

fn s(x: f64) -> Time {
    Time::from_secs_f64(x)
}

fn analysis(dur: f64, speech: &[(f64, f64)], motion: impl Fn(f64) -> f32, face_x: impl Fn(f64) -> Option<f32>, cuts: &[f64]) -> AnalysisResult {
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
        scenes: Some(SceneAnalysis { method: "ffmpeg".into(), cuts: cuts.iter().map(|c| s(*c)).collect() }),
        visual: Some(VisualAnalysis {
            interval: s(1.0),
            face_detector: Some("yunet".into()),
            samples: (0..n)
                .map(|i| {
                    let t = i as f64;
                    VisualSample {
                        t: s(t),
                        motion: motion(t),
                        luma: 0.5,
                        faces: face_x(t).map(|fx| vec![FaceBox { x: fx - 0.05, y: 0.3, w: 0.1, h: 0.18, score: Some(0.9) }]).unwrap_or_default(),
                    }
                })
                .collect(),
        }),
        transcript: None,
        warnings: vec![],
    }
}

fn project_with(dur: i64, a: AnalysisResult) -> (Project, Id, HashMap<Id, AnalysisResult>) {
    let mut p = Project::new("src", ProjectSettings::default());
    let id = new_id();
    p.assets.push(MediaAsset {
        id,
        kind: AssetKind::Video,
        name: "v.mp4".into(),
        path: "/x/v.mp4".into(),
        fingerprint: "fp".into(),
        size_bytes: 1,
        duration: Time::from_secs(dur),
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
    });
    let vt = p.timeline.tracks[0].id;
    apply(&mut p, &EditCommand::InsertClip { track_id: vt, asset_id: id, start: Time::ZERO, source_in: None, duration: None, ripple: false }).unwrap();
    (p, vt, HashMap::from([(id, a)]))
}

#[test]
fn suggests_non_overlapping_complete_cuts_ranked_by_score() {
    // 10 min de fala em frases de 6 s com pausas de 2 s; dois momentos agitados.
    let speech: Vec<(f64, f64)> = (0..75).map(|i| (i as f64 * 8.0 + 1.0, i as f64 * 8.0 + 7.0)).collect();
    let hot = |t: f64| (120.0..160.0).contains(&t) || (400.0..430.0).contains(&t);
    let a = analysis(600.0, &speech, |t| if hot(t) { 0.5 } else { 0.01 }, |_| None, &[]);
    let (p, vt, an) = project_with(600, a);
    let mut w = default_weights();
    w.insert("visual_action".into(), 0.6);
    let o = ShortsOptions { count: 4, min_s: 20.0, max_s: 45.0, target_s: 30.0 };
    let cs = suggest(&p, vt, &an, &w, &o);
    assert_eq!(cs.len(), 4);
    assert!(cs.windows(2).all(|x| x[0].score >= x[1].score));
    assert_eq!(cs.iter().map(|c| c.rank).collect::<Vec<_>>(), vec![1, 2, 3, 4]);
    for (i, a) in cs.iter().enumerate() {
        let d = a.range.duration().as_secs_f64();
        assert!((20.0..=45.5).contains(&d), "duração {d}");
        // Começa 150 ms antes de uma frase (nunca no meio dela) e termina depois de uma.
        let st = a.range.start.as_secs_f64();
        assert!(speech.iter().any(|(b, _)| (b - 0.15 - st).abs() < 0.05), "início {st}");
        let en = a.range.end.as_secs_f64();
        assert!(speech.iter().any(|(_, e)| (e + 0.3 - en).abs() < 0.05), "fim {en}");
        for b in &cs[i + 1..] {
            assert!(a.range.intersection(&b.range).is_none(), "sobreposição");
        }
        assert!(!a.explanation.is_empty() && a.title.starts_with("Momento em"));
    }
    // Os dois melhores caem nos momentos agitados.
    let top: Vec<f64> = cs[..2].iter().map(|c| c.range.start.as_secs_f64()).collect();
    assert!(top.iter().any(|t| (100.0..160.0).contains(t)) && top.iter().any(|t| (380.0..430.0).contains(t)), "{top:?}");
}

#[test]
fn short_material_yields_honest_short_candidate() {
    let a = analysis(12.0, &[(1.0, 5.0), (6.0, 10.0)], |_| 0.05, |_| None, &[]);
    let (p, vt, an) = project_with(12, a);
    let cs = suggest(&p, vt, &an, &default_weights(), &ShortsOptions { count: 3, min_s: 20.0, max_s: 60.0, target_s: 30.0 });
    assert_eq!(cs.len(), 1, "um único corte cobre todo o material");
    assert!(cs[0].explanation.contains("mais curto que o mínimo"), "{}", cs[0].explanation);
}

#[test]
fn fill_offset_centers_subject_and_clamps_to_image() {
    let (src, out) = ((1920, 1080), (1080, 1920));
    // Geometria do compilador: x0 = (w - sw)/2 + x*w/2; o assunto em fx deve cair no centro.
    let sw = 1920.0 * (1920.0 / 1080.0);
    for fx in [0.3f32, 0.45, 0.5, 0.62] {
        let x = fill_offset(fx, src, out);
        let x0 = (1080.0 - sw) / 2.0 + x * 1080.0 / 2.0;
        let at = x0 + fx * sw;
        assert!((at - 540.0).abs() < 1.0, "fx {fx} → {at}");
    }
    // Nas bordas, limita para não mostrar fora da imagem.
    let x = fill_offset(0.0, src, out);
    let x0 = (1080.0 - sw) / 2.0 + x * 1080.0 / 2.0;
    assert!(x0.abs() < 1.0);
    // Mídia vertical num quadro vertical: nada a fazer.
    assert_eq!(fill_offset(0.2, (1080, 1920), out), 0.0);
}

#[test]
fn reframe_follows_face_by_shot_ignores_jitter() {
    // 0–10 s rosto à esquerda (com tremor), corte de cena em 10 s, rosto à direita;
    // em 24 s o rosto se desloca de forma sustentada sem corte de cena.
    let fx = |t: f64| {
        let j = if (t as i64) % 2 == 0 { 0.02 } else { -0.02 };
        Some(if t < 10.0 {
            0.3 + j
        } else if t < 24.0 {
            0.7 + j
        } else {
            0.45
        })
    };
    let a = analysis(36.0, &[], |_| 0.0, fx, &[10.0]);
    let segs = plan_reframe(Some(&a), TimeRange::new(Time::ZERO, s(36.0)), (1920, 1080), (1080, 1920));
    assert_eq!(segs.len(), 3, "{segs:?}");
    assert_eq!(segs[0].start, Time::ZERO);
    assert_eq!(segs[0].end, s(10.0));
    assert!(segs[0].x > 0.0 && segs[1].x < 0.0, "{segs:?}");
    assert!((segs[2].start.as_secs_f64() - 24.0).abs() < 1.01, "{segs:?}");
    assert!(segs.windows(2).all(|w| w[0].end == w[1].start));
    assert_eq!(segs.last().unwrap().end, s(36.0));
    assert!(segs.iter().all(|s| s.face));

    // Sem rostos: um plano central.
    let a = analysis(20.0, &[], |_| 0.0, |_| None, &[8.0]);
    let segs = plan_reframe(Some(&a), TimeRange::new(Time::ZERO, s(20.0)), (1920, 1080), (1080, 1920));
    assert_eq!(segs.len(), 1);
    assert_eq!(segs[0].x, 0.0);
    assert!(!segs[0].face);
}

#[test]
fn cut_padding_never_spills_into_the_next_clip() {
    // Dois clipes encostados da mesma mídia (pausa já removida): 0–6 s e 10–16 s da origem.
    let a = analysis(20.0, &[(1.0, 6.0), (10.0, 15.0)], |_| 0.05, |_| None, &[]);
    let (mut p, vt, an) = project_with(20, a);
    apply(&mut p, &EditCommand::DeleteRange { start: s(6.0), end: s(10.0), track_ids: Some(vec![vt]), ripple: true }).unwrap();
    apply(&mut p, &EditCommand::DeleteRange { start: s(12.0), end: s(16.0), track_ids: Some(vec![vt]), ripple: true }).unwrap();
    let clips = p.timeline.track(vt).unwrap().clips.clone();
    assert_eq!(clips.len(), 2);
    let cs = suggest(&p, vt, &an, &default_weights(), &ShortsOptions { count: 2, min_s: 1.0, max_s: 5.5, target_s: 5.0 });
    for c in &cs {
        // Cada corte fica inteiro dentro de um clipe: nada de "flash" de poucos quadros.
        assert!(
            clips.iter().any(|k| c.range.start >= k.start && c.range.end <= k.end()),
            "{:?} × {:?}",
            c.range,
            clips.iter().map(|k| k.range()).collect::<Vec<_>>()
        );
    }
}
