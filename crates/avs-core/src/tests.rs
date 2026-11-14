use crate::*;

fn asset(secs: i64) -> MediaAsset {
    MediaAsset {
        id: new_id(),
        kind: AssetKind::Video,
        name: "clip.mp4".into(),
        path: "/tmp/clip.mp4".into(),
        fingerprint: "x".into(),
        size_bytes: 1,
        duration: Time::from_secs(secs),
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
    }
}

fn setup(secs: i64) -> (EditSession, Id, Id) {
    let mut p = Project::new("t", ProjectSettings::default());
    let a = asset(secs);
    let aid = a.id;
    p.assets.push(a);
    let vt = p.timeline.tracks[0].id;
    let mut s = EditSession::new(p);
    s.execute(&EditCommand::InsertClip { track_id: vt, asset_id: aid, start: Time::ZERO, source_in: None, duration: None, ripple: false }).unwrap();
    (s, vt, aid)
}

fn clips(s: &EditSession, track: Id) -> Vec<(i64, i64, i64)> {
    s.project
        .timeline
        .track(track)
        .unwrap()
        .clips
        .iter()
        .map(|c| (c.start.as_secs_f64().round() as i64, c.duration.as_secs_f64().round() as i64, c.source_in().as_secs_f64().round() as i64))
        .collect()
}

#[test]
fn split_then_undo_redo() {
    let (mut s, vt, _) = setup(60);
    let first = s.project.timeline.tracks[0].clips[0].id;
    s.execute(&EditCommand::SplitClip { clip_id: first, at: Time::from_secs(20) }).unwrap();
    assert_eq!(clips(&s, vt), vec![(0, 20, 0), (20, 40, 20)]);
    s.undo();
    assert_eq!(clips(&s, vt), vec![(0, 60, 0)]);
    s.redo();
    assert_eq!(clips(&s, vt), vec![(0, 20, 0), (20, 40, 20)]);
}

#[test]
fn delete_range_ripple_scenario_e() {
    // "Remova o trecho entre 1:20 e 1:30" — cenário E do documento de requisitos.
    let (mut s, vt, _) = setup(180);
    s.execute(&EditCommand::DeleteRange {
        start: Time::parse_timecode("1:20").unwrap(),
        end: Time::parse_timecode("1:30").unwrap(),
        track_ids: None,
        ripple: true,
    })
    .unwrap();
    assert_eq!(clips(&s, vt), vec![(0, 80, 0), (80, 90, 90)]);
    assert_eq!(s.project.timeline.duration(), Time::from_secs(170));
    // e coloca uma transição suave
    let ids: Vec<Id> = s.project.timeline.tracks[0].clips.iter().map(|c| c.id).collect();
    s.execute(&EditCommand::AddTransition { from_clip: ids[0], to_clip: ids[1], kind: TransitionKind::Crossfade, duration: Time::from_secs(1) }).unwrap();
    assert_eq!(clips(&s, vt), vec![(0, 80, 0), (79, 90, 90)]);
    assert_eq!(s.project.timeline.duration(), Time::from_secs(169));
    s.undo();
    s.undo();
    assert_eq!(clips(&s, vt), vec![(0, 180, 0)]);
}

#[test]
fn trim_start_moves_source_in() {
    let (mut s, vt, _) = setup(30);
    let c = s.project.timeline.tracks[0].clips[0].id;
    s.execute(&EditCommand::TrimClip { clip_id: c, new_start: Some(Time::from_secs(5)), new_end: None, ripple: false }).unwrap();
    assert_eq!(clips(&s, vt), vec![(5, 25, 5)]);
}

#[test]
fn rejects_overlap_and_out_of_range() {
    let (mut s, vt, aid) = setup(30);
    let before = s.project.timeline.clone();
    let e = s
        .execute(&EditCommand::InsertClip {
            track_id: vt,
            asset_id: aid,
            start: Time::from_secs(10),
            source_in: None,
            duration: Some(Time::from_secs(5)),
            ripple: false,
        })
        .unwrap_err();
    assert!(matches!(e, CommandError::Validation(_)), "{e}");
    assert_eq!(s.project.timeline, before, "comando inválido não pode alterar o projeto");

    let c = s.project.timeline.tracks[0].clips[0].id;
    let e = s.execute(&EditCommand::TrimClip { clip_id: c, new_start: None, new_end: Some(Time::from_secs(31)), ripple: false }).unwrap_err();
    assert!(e.to_string().contains("ultrapassa"), "{e}");
}

#[test]
fn ripple_insert_and_delete() {
    let (mut s, vt, aid) = setup(10);
    s.execute(&EditCommand::InsertClip { track_id: vt, asset_id: aid, start: Time::ZERO, source_in: None, duration: Some(Time::from_secs(4)), ripple: true })
        .unwrap();
    assert_eq!(clips(&s, vt), vec![(0, 4, 0), (4, 10, 0)]);
    let first = s.project.timeline.tracks[0].clips[0].id;
    s.execute(&EditCommand::DeleteClip { clip_id: first, ripple: true }).unwrap();
    assert_eq!(clips(&s, vt), vec![(0, 10, 0)]);
}

#[test]
fn locked_track_blocks_edits() {
    let (mut s, vt, _) = setup(10);
    s.execute(&EditCommand::UpdateTrack { track_id: vt, patch: TrackPatch { locked: Some(true), ..Default::default() } }).unwrap();
    let c = s.project.timeline.tracks[0].clips[0].id;
    assert!(matches!(s.execute(&EditCommand::DeleteClip { clip_id: c, ripple: false }), Err(CommandError::Locked(_))));
    s.execute(&EditCommand::UpdateTrack { track_id: vt, patch: TrackPatch { locked: Some(false), ..Default::default() } }).unwrap();
}

#[test]
fn json_roundtrip_is_lossless() {
    let (mut s, _, _) = setup(60);
    let tt = s.project.timeline.tracks[1].id;
    s.execute(&EditCommand::AddText { track_id: tt, start: Time::from_secs(1), duration: Time::from_secs(2), text: "Olá".into(), style: None }).unwrap();
    let json = serde_json::to_string_pretty(&s.project).unwrap();
    let back: Project = serde_json::from_str(&json).unwrap();
    assert_eq!(back, s.project);
}

#[test]
fn export_bindings() {
    // `cargo test -p avs-core export_bindings` gera os tipos TS (TS_RS_EXPORT_DIR).
    use ts_rs::TS;
    let cfg = ts_rs::Config::from_env();
    Project::export_all(&cfg).unwrap();
    EditCommand::export_all(&cfg).unwrap();
    HistoryInfo::export_all(&cfg).unwrap();
    CommandError::export_all(&cfg).unwrap();
    ApplyOutcome::export_all(&cfg).unwrap();
    TimeRange::export_all(&cfg).unwrap();
}

#[test]
fn replace_restores_version_and_is_undoable() {
    let (mut s, vt, _) = setup(60);
    let saved = s.project.timeline.clone();
    let c = s.project.timeline.tracks[0].clips[0].id;
    s.execute(&EditCommand::SplitClip { clip_id: c, at: Time::from_secs(10) }).unwrap();
    s.replace("Restaurar versão", saved.clone(), s.project.settings.clone()).unwrap();
    assert_eq!(clips(&s, vt), vec![(0, 60, 0)]);
    s.undo();
    assert_eq!(clips(&s, vt).len(), 2);
    // Versão inválida (sobreposição sem transição) é rejeitada sem alterar nada.
    let mut bad = saved.clone();
    let mut dup = bad.tracks[0].clips[0].clone();
    dup.id = new_id();
    dup.start = Time::from_secs(5);
    dup.duration = Time::from_secs(5);
    bad.tracks[0].clips.push(dup);
    bad.sort_clips();
    let before = s.project.timeline.clone();
    assert!(s.replace("x", bad, s.project.settings.clone()).is_err());
    assert_eq!(s.project.timeline, before);
}

fn first(s: &EditSession) -> Clip {
    s.project.timeline.tracks[0].clips[0].clone()
}

#[test]
fn speed_changes_duration_and_validates_source() {
    let (mut s, vt, _) = setup(60);
    let c = first(&s).id;
    s.execute(&EditCommand::TrimClip { clip_id: c, new_start: None, new_end: Some(Time::from_secs(20)), ripple: false }).unwrap();
    s.execute(&EditCommand::SetClipSpeed { clip_id: c, speed: 2.0, ripple: true }).unwrap();
    assert_eq!(clips(&s, vt), vec![(0, 10, 0)]);
    assert_eq!(first(&s).source_span(), Time::from_secs(20));
    // Split com velocidade: a metade direita começa em 2× o deslocamento na origem.
    s.execute(&EditCommand::SplitClip { clip_id: c, at: Time::from_secs(4) }).unwrap();
    assert_eq!(clips(&s, vt), vec![(0, 4, 0), (4, 6, 8)]);
    // Velocidade que excede a mídia é rejeitada (0,25× de 50 s > 60 s de origem? não: ver slip).
    let right = s.project.timeline.tracks[0].clips[1].id;
    assert!(s.execute(&EditCommand::SlipClip { clip_id: right, delta: Time::from_secs(45) }).is_err(), "slip além do fim da origem");
    s.execute(&EditCommand::SlipClip { clip_id: right, delta: Time::from_secs(10) }).unwrap();
    assert_eq!(clips(&s, vt)[1], (4, 6, 18));
}

#[test]
fn reverse_split_keeps_source_order() {
    let (mut s, vt, _) = setup(30);
    let c = first(&s).id;
    s.execute(&EditCommand::TrimClip { clip_id: c, new_start: None, new_end: Some(Time::from_secs(10)), ripple: false }).unwrap();
    s.execute(&EditCommand::UpdateClip { clip_id: c, patch: ClipPatch { reverse: Some(true), ..Default::default() } }).unwrap();
    s.execute(&EditCommand::SplitClip { clip_id: c, at: Time::from_secs(4) }).unwrap();
    // Invertido: os 4 s iniciais da timeline mostram a origem 6–10 s; o resto, 0–6 s.
    assert_eq!(clips(&s, vt), vec![(0, 4, 6), (4, 6, 0)]);
    // Reverso de trecho muito longo é recusado (memória).
    let (mut s2, _, _) = setup(300);
    let c2 = first(&s2).id;
    assert!(s2.execute(&EditCommand::UpdateClip { clip_id: c2, patch: ClipPatch { reverse: Some(true), ..Default::default() } }).is_err());
}

#[test]
fn roll_slide_duplicate() {
    let (mut s, vt, _) = setup(60);
    let c = first(&s).id;
    s.execute(&EditCommand::SplitClip { clip_id: c, at: Time::from_secs(20) }).unwrap();
    let ids: Vec<Id> = s.project.timeline.tracks[0].clips.iter().map(|c| c.id).collect();
    s.execute(&EditCommand::SplitClip { clip_id: ids[1], at: Time::from_secs(40) }).unwrap();
    let ids: Vec<Id> = s.project.timeline.tracks[0].clips.iter().map(|c| c.id).collect();
    // Roll: corte de 20 s → 23 s.
    s.execute(&EditCommand::RollEdit { left_clip: ids[0], right_clip: ids[1], delta: Time::from_secs(3) }).unwrap();
    assert_eq!(clips(&s, vt), vec![(0, 23, 0), (23, 17, 23), (40, 20, 40)]);
    // Slide do meio −5 s: vizinhos se ajustam, conteúdo do meio igual.
    s.execute(&EditCommand::SlideClip { clip_id: ids[1], delta: -Time::from_secs(5) }).unwrap();
    assert_eq!(clips(&s, vt), vec![(0, 18, 0), (18, 17, 23), (35, 25, 35)]);
    // Duplicar o primeiro no fim.
    let out = s.execute(&EditCommand::DuplicateClip { clip_id: ids[0], track_id: None, start: Some(Time::from_secs(60)) }).unwrap();
    assert_eq!(out.created.len(), 1);
    assert_eq!(clips(&s, vt)[3], (60, 18, 0));
    // Roll além da origem do clipe esquerdo → rejeitado.
    assert!(s.execute(&EditCommand::RollEdit { left_clip: ids[1], right_clip: ids[2], delta: Time::from_secs(30) }).is_err());
}

#[test]
fn freeze_frame_inserts_and_ripples() {
    let (mut s, vt, _) = setup(30);
    let c = first(&s).id;
    s.execute(&EditCommand::FreezeFrame { clip_id: c, at: Time::from_secs(10), duration: Time::from_secs(3) }).unwrap();
    assert_eq!(clips(&s, vt), vec![(0, 10, 0), (10, 3, 10), (13, 20, 10)]);
    let f = &s.project.timeline.tracks[0].clips[1];
    assert!(f.freeze);
    assert_eq!(f.source_span(), Time::ZERO);
}

#[test]
fn keyframes_set_remove_and_survive_trim() {
    let (mut s, _, _) = setup(30);
    let c = first(&s).id;
    let kf = |at: i64, v: f32| EditCommand::SetKeyframe { clip_id: c, property: KeyframeProperty::Scale, at: Time::from_secs(at), value: v, easing: None };
    s.execute(&kf(0, 1.0)).unwrap();
    s.execute(&kf(10, 2.0)).unwrap();
    s.execute(&kf(10, 3.0)).unwrap(); // substitui
    assert_eq!(first(&s).keyframes.scale.len(), 2);
    assert_eq!(interpolate(&first(&s).keyframes.scale, Time::from_secs(5), 1.0), 2.0);
    // Zoom animado abaixo de 100% é recusado (limitação documentada).
    assert!(s.execute(&kf(20, 0.5)).is_err());
    // Aparar o início em 5 s: o valor na nova borda é preservado.
    s.execute(&EditCommand::TrimClip { clip_id: c, new_start: Some(Time::from_secs(5)), new_end: None, ripple: false }).unwrap();
    let k = first(&s).keyframes.scale;
    assert_eq!((k[0].at, k[0].value), (Time::ZERO, 2.0));
    assert_eq!((k[1].at, k[1].value), (Time::from_secs(5), 3.0));
    s.execute(&EditCommand::RemoveKeyframe { clip_id: c, property: KeyframeProperty::Scale, at: Time::from_secs(5) }).unwrap();
    assert_eq!(first(&s).keyframes.scale.len(), 1);
}

#[test]
fn subtitle_track_and_apply_style() {
    let (mut s, _, _) = setup(30);
    let (cues, _) = crate::subtitles::parse(
        "1\n00:00:01,000 --> 00:00:03,000\nOi\n\n2\n00:00:02,500 --> 00:00:04,000\nSobreposta\n\n3\n00:00:05,000 --> 00:00:06,000\n   \n",
    );
    let out = s.execute(&EditCommand::AddSubtitleTrack { name: None, cues, style: None }).unwrap();
    let t = s.project.timeline.track(out.created[0]).unwrap().clone();
    assert_eq!(t.kind, TrackKind::Text);
    assert_eq!(t.clips.len(), 2);
    assert_eq!(t.clips[0].end(), Time::from_millis(2500), "legenda sobreposta encurta a anterior");
    let style = TextStyle { color: "#FFE14D".into(), ..crate::subtitles::caption_style() };
    s.execute(&EditCommand::ApplyTextStyle { track_id: t.id, style: style.clone() }).unwrap();
    for c in &s.project.timeline.track(t.id).unwrap().clips {
        assert!(matches!(&c.content, ClipContent::Text { style: st, .. } if *st == style));
    }
    let back = crate::subtitles::cues_from_track(s.project.timeline.track(t.id).unwrap(), Time::ZERO);
    assert_eq!(back[1].text, "Sobreposta");
}

#[test]
fn old_project_json_still_loads() {
    // Projeto salvo antes de velocidade/efeitos/keyframes existirem.
    let (s, _, _) = setup(10);
    let mut v = serde_json::to_value(&s.project).unwrap();
    for k in ["speed", "reverse", "freeze", "effects", "audio_fx", "keyframes"] {
        v["timeline"]["tracks"][0]["clips"][0].as_object_mut().unwrap().remove(k);
    }
    let p: Project = serde_json::from_value(v).unwrap();
    assert_eq!(p.timeline.tracks[0].clips[0].speed, 1.0);
}

#[test]
fn no_sub_frame_slivers_from_unaligned_media() {
    // Mídia de 21,968 s (não múltipla de 1/30 s): inserida inteira, fica em quadros
    // inteiros; remover até o fim não deixa uma "lasca" de 1 ms.
    let mut p = Project::new("x", ProjectSettings::default());
    let mut a = asset(10);
    a.duration = Time::from_secs_f64(21.968);
    let aid = a.id;
    p.assets.push(a);
    let vt = p.timeline.tracks[0].id;
    let c = apply(&mut p, &EditCommand::InsertClip { track_id: vt, asset_id: aid, start: Time::ZERO, source_in: None, duration: None, ripple: false })
        .unwrap()
        .created[0];
    let clip = p.timeline.clip(c).unwrap();
    assert_eq!(clip.duration, clip.duration.snap_to_frame(p.settings.fps));
    assert!(clip.duration <= Time::from_secs_f64(21.968));
    // Clipe legado (antes da correção), com duração fora da grade.
    let mut p2 = p.clone();
    p2.timeline.tracks[0].clips[0].duration = Time::from_secs_f64(21.968);
    let end = p2.timeline.duration().snap_to_frame(p2.settings.fps);
    apply(&mut p2, &EditCommand::DeleteRange { start: Time::from_secs(20), end, track_ids: None, ripple: true }).unwrap();
    let clips = &p2.timeline.tracks[0].clips;
    assert_eq!(clips.len(), 1, "{:?}", clips.iter().map(|c| c.duration).collect::<Vec<_>>());
    assert_eq!(clips[0].end(), Time::from_secs(20));
}
