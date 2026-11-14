//! Fase 6: assistente por linguagem natural.

use avs_ai::analysis::*;
use avs_ai::assistant::*;
use avs_ai::autoplan::PresetRules;
use avs_ai::ollama::Ollama;
use avs_ai::scoring::default_weights;
use avs_core::*;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};

fn s(x: f64) -> Time {
    Time::from_secs_f64(x)
}

fn ctx(sel: Option<Id>, at: f64) -> AssistantContext {
    AssistantContext { selected_clip: sel, playhead: s(at) }
}

fn parse(t: &str) -> Vec<Intent> {
    parse_rules(t, &ctx(None, 7.0), s(60.0)).unwrap_or_else(|e| panic!("{t}: {e}"))
}

#[test]
fn understands_common_portuguese_requests() {
    use Intent::*;
    let cases: Vec<(&str, Vec<Intent>)> = vec![
        ("corte de 0:10 a 0:20", vec![RemoveRange { start: 10.0, end: 20.0 }]),
        ("Remova o trecho entre 1:05 e 1:12,5", vec![RemoveRange { start: 65.0, end: 72.5 }]),
        ("tire os primeiros 5 segundos", vec![RemoveRange { start: 0.0, end: 5.0 }]),
        ("apaga os últimos 3 segundos", vec![RemoveRange { start: 57.0, end: 60.0 }]),
        ("divida em 0:12", vec![Split { at: 12.0 }]),
        ("corte aqui", vec![Split { at: 7.0 }]),
        ("remova as pausas", vec![RemovePauses { min_silence_s: None }]),
        ("tire os silêncios maiores que 1,5 segundos", vec![RemovePauses { min_silence_s: Some(1.5) }]),
        ("gere legendas", vec![Captions]),
        ("deixe o vídeo com 30 segundos", vec![TargetDuration { seconds: 30.0 }]),
        ("reduza para 1 minuto e 15 segundos", vec![TargetDuration { seconds: 75.0 }]),
        ("aumente o volume da música em 3 dB", vec![Volume { target: Target::Music, db: 3.0 }]),
        ("abaixe a música", vec![Volume { target: Target::Music, db: -6.0 }]),
        ("silencie a música", vec![Mute { target: Target::Music, mute: true }]),
        ("acelere o clipe selecionado 2x", vec![Speed { target: Target::Selected, factor: 2.0 }]),
        ("coloque em câmera lenta", vec![Speed { target: Target::All, factor: 0.5 }]),
        ("coloque um zoom de 20% em 0:15", vec![Zoom { at: 15.0, duration: 2.0, amount: 20.0 }]),
        ("aplique fade de saída de 2 segundos no final", vec![Fade { target: Target::All, kind: FadeKind::Out, seconds: 2.0 }]),
        ("coloque transição entre todos os clipes", vec![Transition { kind: TransitionKind::Crossfade, seconds: 0.5, at: None }]),
        ("transição pelo preto em 0:30", vec![Transition { kind: TransitionKind::FadeBlack, seconds: 0.5, at: Some(30.0) }]),
        ("congele a imagem em 0:08 por 2 segundos", vec![Freeze { at: 8.0, seconds: 2.0 }]),
        ("deixe o vídeo em preto e branco", vec![Effect { target: Target::All, effect: EffectName::BlackWhite, amount: 1.0 }]),
        ("deixe mais claro", vec![Color { target: Target::All, brightness: 0.08, contrast: 1.0, saturation: 1.0 }]),
        ("apague o clipe selecionado", vec![DeleteClip { target: Target::Selected }]),
        (
            "adicione o texto \"Inscreva-se!\" em 0:05 por 4 segundos no topo",
            vec![AddText { text: "Inscreva-se!".into(), at: 5.0, duration: 4.0, position: TextPosition::Top }],
        ),
        ("marque este ponto como \"gancho\"", vec![Marker { at: 7.0, label: "gancho".into() }]),
        // Vários pedidos numa frase.
        (
            "remova as pausas, depois gere legendas e aumente a voz em 2 dB",
            vec![RemovePauses { min_silence_s: None }, Captions, Volume { target: Target::Voice, db: 2.0 }],
        ),
    ];
    for (text, want) in cases {
        assert_eq!(parse(text), want, "{text}");
    }
}

#[test]
fn explains_what_it_did_not_understand() {
    let e = parse_rules("faça um bolo de chocolate", &ctx(None, 0.0), s(10.0)).unwrap_err();
    assert!(e.contains("Não entendi") && e.contains("Exemplos"), "{e}");
    let e = parse_rules("adicione um texto em 0:05", &ctx(None, 0.0), s(10.0)).unwrap_err();
    assert!(e.contains("aspas"), "{e}");
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
        noise_reduction: false,
        voice_priority: false,
        scoring_weights: default_weights(),
        safe_bottom_pct: 0.05,
    }
}

fn asset(secs: i64, video: bool) -> MediaAsset {
    MediaAsset {
        id: new_id(),
        kind: if video { AssetKind::Video } else { AssetKind::Audio },
        name: if video { "fala.mp4".into() } else { "musica.m4a".into() },
        path: "/x".into(),
        fingerprint: "fp".into(),
        size_bytes: 1,
        duration: Time::from_secs(secs),
        container: "mp4".into(),
        video: video.then(|| VideoInfo {
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
    }
}

/// 40 s de vídeo com fala em 1–9, 13–21, 25–33 e música de 40 s.
fn project() -> (EditSession, HashMap<Id, AnalysisResult>, Id, Id) {
    let mut p = Project::new("assist", ProjectSettings::default());
    let (v, m) = (asset(40, true), asset(40, false));
    let speech = [(1.0, 9.0), (13.0, 21.0), (25.0, 33.0)];
    let a = AnalysisResult {
        version: ANALYSIS_VERSION,
        fingerprint: "fp".into(),
        duration: s(40.0),
        loudness: None,
        speech: Some(SpeechAnalysis { method: "silero".into(), segments: speech.iter().map(|(a, b)| TimeRange::new(s(*a), s(*b))).collect(), ratio: 0.6 }),
        scenes: None,
        visual: None,
        transcript: None,
        warnings: vec![],
    };
    let an = HashMap::from([(v.id, a)]);
    let (vid, mid) = (v.id, m.id);
    p.assets.extend([v, m]);
    let (vt, at) = (p.timeline.tracks[0].id, p.timeline.tracks[2].id);
    let mut sess = EditSession::new(p);
    let vc = sess
        .execute(&EditCommand::InsertClip { track_id: vt, asset_id: vid, start: Time::ZERO, source_in: None, duration: None, ripple: false })
        .unwrap()
        .created[0];
    let mc = sess
        .execute(&EditCommand::InsertClip { track_id: at, asset_id: mid, start: Time::ZERO, source_in: None, duration: None, ripple: false })
        .unwrap()
        .created[0];
    (sess, an, vc, mc)
}

#[test]
fn executes_as_one_undoable_step_with_real_effects() {
    let (mut sess, an, vc, mc) = project();
    let c = ctx(Some(vc), 0.0);
    let text = "remova as pausas, aumente a música em 6 dB, adicione o texto \"Oi\" em 0:02 por 2 segundos e aplique fade de saída de 1 segundo no final";
    let intents = parse_rules(text, &c, sess.project.timeline.duration()).unwrap();
    assert_eq!(intents.len(), 4);
    let rev = sess.info().revision;
    let done = sess.transaction("Assistente", |p| execute(p, &intents, &c, &an, &rules()).map_err(CommandError::Invalid)).unwrap();
    assert_eq!(sess.info().revision, rev + 1, "um único passo no histórico");
    assert!(done[0].summary.contains("pausa"), "{}", done[0].summary);
    assert!(done[2].summary.contains("2,0 s") && done[2].summary.contains("\"Oi\""), "{}", done[2].summary);
    let p = &sess.project;
    assert!(validate_project(p).is_empty());
    // Pausas: 0–0,85, 9,15–12,85, 21,15–24,85, 33,15–40 → vídeo com 40 − 0,85 − 3,7·2 − 6,85 = 24,9 s.
    let vt = p.timeline.tracks[0].id;
    let vend = p.timeline.track(vt).unwrap().clips.iter().map(|c| c.end()).max().unwrap().as_secs_f64();
    assert!((vend - 24.9).abs() < 0.1, "{vend}");
    let m = p.timeline.clip(mc).unwrap();
    assert!((m.volume - 10f32.powf(6.0 / 20.0)).abs() < 1e-3);
    assert!(p
        .timeline
        .tracks
        .iter()
        .any(|t| t.kind == TrackKind::Text && t.clips.iter().any(|c| matches!(&c.content, ClipContent::Text { text, .. } if text == "Oi"))));
    // Fade de saída no último clipe de cada trilha de mídia.
    let last_v = p.timeline.track(vt).unwrap().clips.last().unwrap();
    assert_eq!(last_v.fade_out, Time::from_secs(1));
    sess.undo();
    assert_eq!(sess.project.timeline.duration(), Time::from_secs(40));
    assert_eq!(sess.project.timeline.clip(mc).unwrap().volume, 1.0);
}

#[test]
fn errors_are_specific_and_nothing_changes() {
    let (mut sess, an, _, _) = project();
    let before = serde_json::to_value(&sess.project.timeline).unwrap();
    // Instante fora do vídeo.
    let c = ctx(None, 0.0);
    let e = sess.transaction("x", |p| execute(p, &[Intent::Split { at: 99.0 }], &c, &an, &rules()).map_err(CommandError::Invalid)).unwrap_err();
    assert!(e.to_string().contains("fora do vídeo"), "{e}");
    // Clipe selecionado ausente.
    let e = sess
        .transaction("x", |p| execute(p, &[Intent::Speed { target: Target::Selected, factor: 2.0 }], &c, &an, &rules()).map_err(CommandError::Invalid))
        .unwrap_err();
    assert!(e.to_string().contains("Selecione um clipe"), "{e}");
    // Primeiro passo válido, segundo inválido: nada é aplicado (transação).
    let e = sess
        .transaction("x", |p| {
            execute(p, &[Intent::RemoveRange { start: 1.0, end: 2.0 }, Intent::Speed { target: Target::All, factor: 9.0 }], &c, &an, &rules())
                .map_err(CommandError::Invalid)
        })
        .unwrap_err();
    assert!(e.to_string().contains("Passo 2"), "{e}");
    // Sem transcrição: legendas explicam o que fazer.
    let e = sess.transaction("x", |p| execute(p, &[Intent::Captions], &c, &an, &rules()).map_err(CommandError::Invalid)).unwrap_err();
    assert!(e.to_string().contains("transcrição"), "{e}");
    // Duração-alvo maior que o vídeo: não estica.
    let e = sess.transaction("x", |p| execute(p, &[Intent::TargetDuration { seconds: 90.0 }], &c, &an, &rules()).map_err(CommandError::Invalid)).unwrap_err();
    assert!(e.to_string().contains("nada é esticado"), "{e}");
    assert_eq!(serde_json::to_value(&sess.project.timeline).unwrap(), before);
}

#[test]
fn target_duration_and_zoom_and_transition() {
    let (mut sess, an, _, _) = project();
    let c = ctx(None, 0.0);
    let i = parse_rules("deixe com 18 segundos", &c, sess.project.timeline.duration()).unwrap();
    sess.transaction("x", |p| execute(p, &i, &c, &an, &rules()).map_err(CommandError::Invalid)).unwrap();
    let vt = sess.project.timeline.tracks[0].id;
    let d = sess.project.timeline.track(vt).unwrap().clips.iter().map(|c| c.end()).max().unwrap().as_secs_f64();
    assert!(d <= 18.5, "{d}");
    let i = parse_rules("coloque transição entre todos os clipes e coloque um zoom em 0:03", &c, sess.project.timeline.duration()).unwrap();
    sess.transaction("x", |p| execute(p, &i, &c, &an, &rules()).map_err(CommandError::Invalid)).unwrap();
    assert!(!sess.project.timeline.transitions.is_empty());
    assert!(sess.project.timeline.tracks[0].clips.iter().any(|c| c.keyframes.scale.iter().any(|k| k.value > 1.0)));
}

/// Ollama falso que devolve a resposta dada (só para o teste).
fn fake_ollama(response: &'static str) -> (String, std::sync::Arc<std::sync::Mutex<Vec<serde_json::Value>>>) {
    let seen = std::sync::Arc::new(std::sync::Mutex::new(vec![]));
    let seen2 = seen.clone();
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
            let req: serde_json::Value = serde_json::from_slice(&body).unwrap_or_default();
            seen2.lock().unwrap().push(req);
            let resp = serde_json::json!({ "response": response, "done": true }).to_string();
            let mut s = stream;
            write!(s, "HTTP/1.1 200 OK\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}", resp.len(), resp).unwrap();
        }
    });
    (format!("http://{addr}"), seen)
}

#[test]
fn llm_output_is_schema_constrained_and_validated() {
    let (sess, _, vc, _) = project();
    let c = ctx(Some(vc), 4.0);
    let (system, prompt, schema) = llm_request("tire a parte chata do começo e coloque um título", &sess.project, &c);
    assert!(system.contains("Nunca invente"));
    assert!(prompt.contains("[SELECIONADO]") && prompt.contains("cursor em 4.00 s"));
    assert!(!prompt.contains("/x"), "caminhos de arquivo não vão para o modelo");

    let (url, seen) = fake_ollama(
        r#"{"actions":[{"action":"remove_range","start":0,"end":1.2},{"action":"add_text","text":"Bem-vindo","at":0,"duration":3,"position":"top"},{"action":"captions"}],"note":"ok"}"#,
    );
    let r = Ollama::new(&url).generate("qwen3:4b", &system, &prompt, Some(&schema), 0.1).unwrap();
    let (intents, note) = parse_llm(&r).unwrap();
    assert_eq!(note, "ok");
    assert_eq!(intents[0], Intent::RemoveRange { start: 0.0, end: 1.2 });
    assert_eq!(intents[2], Intent::Captions);
    let req = seen.lock().unwrap()[0].clone();
    assert!(req["format"]["properties"]["actions"].is_object(), "o schema restringe a resposta");

    // Ação fora do vocabulário (ex.: tentativa de executar algo): recusada, não "consertada".
    let e = parse_llm(r#"{"actions":[{"action":"run_shell","cmd":"rm -rf /"}],"note":""}"#).unwrap_err();
    assert!(e.contains("ação inválida"), "{e}");
    let e = parse_llm("isto não é JSON").unwrap_err();
    assert!(e.contains("JSON"), "{e}");
    let e = parse_llm(r#"{"actions":[{"action":"remove_range","start":"0"}],"note":""}"#).unwrap_err();
    assert!(e.contains("ação inválida"), "{e}");
}
