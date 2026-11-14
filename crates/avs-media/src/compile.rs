//! Compilador de renderização: `Project` + intervalo → filtergraph FFmpeg.
//!
//! É o **único** caminho de renderização. Preview completo, frame único do
//! scrubbing, renderização de trecho e exportação final usam este mesmo
//! compilador; por isso o que o player mostra é o que será exportado.
//!
//! Princípios:
//! - Posições em **frames** (vídeo) e **amostras** (áudio) inteiras, derivadas
//!   do tempo exato em flicks; nada de somar segundos em ponto flutuante.
//! - Cada clipe vira uma entrada `-ss/-t` própria (seek rápido): o custo não
//!   cresce com a posição do trecho em arquivos de horas.
//! - Cada clipe é preenchido (tpad/apad) e aparado no número exato de
//!   frames/amostras: arquivos curtos ou VFR não dessincronizam a timeline.
//! - Renderizar um intervalo `[a, b)` produz exatamente os mesmos pixels que o
//!   mesmo trecho da renderização completa (a janela é expandida para conter
//!   transições/fades inteiros e depois aparada) — base da renderização
//!   incremental por blocos.

use crate::ffmpeg::{escape_filter_value, MediaError, Result};
use crate::fonts::FontResolver;
use avs_core::*;
use sha2::{Digest, Sha256};
use std::fmt::Write as _;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct CompileOptions {
    pub width: u32,
    pub height: u32,
    pub fps: Rational,
    pub sample_rate: u32,
    pub use_proxies: bool,
    pub include_audio: bool,
}

impl CompileOptions {
    pub fn from_project(p: &Project) -> Self {
        CompileOptions {
            width: p.settings.width,
            height: p.settings.height,
            fps: p.settings.fps,
            sample_rate: p.settings.sample_rate,
            use_proxies: false,
            include_audio: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct InputSpec {
    pub path: PathBuf,
    pub seek: Time,
    pub duration: Time,
    pub image: bool,
    /// Entrada só de áudio (`-vn`).
    pub audio_only: bool,
}

#[derive(Debug, Clone)]
pub struct CompiledGraph {
    pub inputs: Vec<InputSpec>,
    pub filter: String,
    pub has_audio: bool,
    pub range: TimeRange,
    pub frames: i64,
    pub samples: i64,
    /// Arquivos de texto (drawtext `textfile`) a gravar antes de executar.
    pub text_files: Vec<(PathBuf, String)>,
    /// Arquivos auxiliares lidos pelo grafo (ex.: LUTs) — entram na chave de cache.
    pub deps: Vec<PathBuf>,
}

pub const VIDEO_OUT: &str = "[vout]";
pub const AUDIO_OUT: &str = "[aout]";

impl CompiledGraph {
    /// Argumentos de entrada do FFmpeg, na ordem dos índices usados no grafo.
    pub fn input_args(&self, fps: Rational) -> Vec<String> {
        let mut a = vec![];
        for i in &self.inputs {
            if i.image {
                a.extend(["-loop".into(), "1".into(), "-framerate".into(), fps.to_ffmpeg()]);
                a.extend(["-t".into(), (i.duration + Time::from_millis(250)).to_ffmpeg()]);
            } else {
                a.extend(["-ss".into(), i.seek.to_ffmpeg(), "-t".into(), (i.duration + Time::from_millis(250)).to_ffmpeg()]);
                if i.audio_only {
                    a.push("-vn".into());
                }
            }
            a.push("-i".into());
            a.push(i.path.to_string_lossy().into_owned());
        }
        a
    }
}

fn frame_of(t: Time, fps: Rational) -> i64 {
    // Arredonda ao frame mais próximo (tempos já costumam estar alinhados).
    let num = t.flicks() as i128 * fps.num as i128;
    let den = fps.den as i128 * FLICKS_PER_SECOND as i128;
    ((num * 2 + den).div_euclid(den * 2)) as i64
}

fn sample_of(t: Time, sr: u32) -> i64 {
    ((t.flicks() as i128 * sr as i128 * 2 + FLICKS_PER_SECOND as i128).div_euclid(FLICKS_PER_SECOND as i128 * 2)) as i64
}

fn secs(x: f64) -> String {
    format!("{x:.6}")
}

/// `#RRGGBB` → `0xRRGGBB` (valores inválidos viram o padrão — nunca são
/// inseridos crus no filtergraph).
pub fn ff_color(hex: &str, default: &str) -> String {
    let h = hex.trim().trim_start_matches('#');
    if (h.len() == 6 || h.len() == 8) && h.chars().all(|c| c.is_ascii_hexdigit()) {
        format!("0x{h}")
    } else {
        default.to_string()
    }
}

fn even(x: f64) -> i64 {
    ((x / 2.0).round() as i64) * 2
}

struct Geometry {
    sw: i64,
    sh: i64,
    crop: Option<(i64, i64, i64, i64)>,
    pad: Option<(i64, i64)>,
}

fn geometry(srcw: u32, srch: u32, w: u32, h: u32, t: &Transform) -> Option<Geometry> {
    let (sw0, sh0, w, h) = (srcw as f64, srch as f64, w as i64, h as i64);
    let f = match t.fit {
        FitMode::Fit => (w as f64 / sw0).min(h as f64 / sh0),
        FitMode::Fill => (w as f64 / sw0).max(h as f64 / sh0),
    } * t.scale as f64;
    let sw = even(sw0 * f).max(2);
    let sh = even(sh0 * f).max(2);
    let x0 = even((w - sw) as f64 / 2.0 + t.x as f64 * w as f64 / 2.0);
    let y0 = even((h - sh) as f64 / 2.0 + t.y as f64 * h as f64 / 2.0);
    let (vx0, vy0) = (x0.max(0), y0.max(0));
    let (vx1, vy1) = ((x0 + sw).min(w), (y0 + sh).min(h));
    if vx1 <= vx0 || vy1 <= vy0 {
        return None;
    }
    let (cw, ch) = (vx1 - vx0, vy1 - vy0);
    let crop = (cw != sw || ch != sh).then_some((cw, ch, vx0 - x0, vy0 - y0));
    let pad = (cw != w || ch != h).then_some((vx0, vy0));
    Some(Geometry { sw, sh, crop, pad })
}

/// Expande a janela até não cortar nenhuma transição ou fade ao meio.
fn expand_window(p: &Project, mut w: TimeRange) -> TimeRange {
    let tl = &p.timeline;
    let mut regions: Vec<TimeRange> = vec![];
    for t in &tl.tracks {
        for c in &t.clips {
            if c.fade_in.is_positive() {
                regions.push(TimeRange::new(c.start, c.start + c.fade_in));
            }
            if c.fade_out.is_positive() {
                regions.push(TimeRange::new(c.end() - c.fade_out, c.end()));
            }
        }
    }
    for tr in &tl.transitions {
        if let Some(b) = tl.clip(tr.to_clip) {
            regions.push(TimeRange::new(b.start, b.start + tr.duration));
        }
    }
    for _ in 0..8 {
        let before = w;
        for r in &regions {
            if r.start < w.start && w.start < r.end {
                w.start = r.start;
            }
            if r.start < w.end && w.end < r.end {
                w.end = r.end;
            }
        }
        if before == w {
            break;
        }
    }
    w
}

/// Intervalos onde há vídeo visível (para distinguir frames pretos esperados).
pub fn video_coverage(p: &Project) -> Vec<TimeRange> {
    merge(
        p.timeline
            .tracks
            .iter()
            .filter(|t| t.kind == TrackKind::Video && !t.hidden)
            .flat_map(|t| t.clips.iter().filter(|c| c.opacity > 0.05).map(|c| c.range()))
            .collect(),
    )
}

/// Intervalos onde há áudio audível.
pub fn audio_coverage(p: &Project) -> Vec<TimeRange> {
    merge(
        p.timeline
            .tracks
            .iter()
            .filter(|t| t.kind != TrackKind::Text && !t.muted && t.volume > 0.0)
            .flat_map(|t| {
                t.clips
                    .iter()
                    .filter(|c| !c.audio_muted && c.volume > 0.0)
                    .filter(|c| c.asset_id().and_then(|id| p.asset(id)).map(|a| a.has_audio()).unwrap_or(false))
                    .map(|c| c.range())
            })
            .collect(),
    )
}

fn merge(mut v: Vec<TimeRange>) -> Vec<TimeRange> {
    v.sort_by_key(|r| r.start);
    let mut out: Vec<TimeRange> = vec![];
    for r in v {
        match out.last_mut() {
            Some(l) if r.start <= l.end => l.end = l.end.max(r.end),
            _ => out.push(r),
        }
    }
    out
}

/// Compila o intervalo `range` (alinhado a frames) do projeto.
pub fn compile(p: &Project, range: TimeRange, opts: &CompileOptions, fonts: &FontResolver, text_dir: &Path) -> Result<CompiledGraph> {
    let issues = validate_project(p);
    if !issues.is_empty() {
        return Err(MediaError::InvalidTimeline(issues.iter().map(|i| i.message.clone()).collect::<Vec<_>>().join("; ")));
    }
    let fps = opts.fps;
    let sr = opts.sample_rate;
    let (w, h) = (opts.width, opts.height);
    if w % 2 != 0 || h % 2 != 0 {
        return Err(MediaError::InvalidTimeline("resolução de saída precisa ser par".into()));
    }
    let r = TimeRange::new(range.start.snap_to_frame(fps), range.end.snap_to_frame(fps));
    if r.end <= r.start {
        return Err(MediaError::InvalidTimeline("intervalo de renderização vazio".into()));
    }
    let win = expand_window(p, r);
    let (f0, f1) = (frame_of(win.start, fps), frame_of(win.end, fps));
    let (s0, s1) = (sample_of(win.start, sr), sample_of(win.end, sr));
    let total_frames = f1 - f0;
    let fps_s = fps.to_ffmpeg();
    let size = format!("{w}x{h}");
    let frame_dur = fps.frame_duration();

    let mut g = String::new();
    let mut inputs: Vec<InputSpec> = vec![];
    let mut text_files = vec![];
    let mut deps: Vec<PathBuf> = vec![];
    let mut audio_labels: Vec<String> = vec![];
    let mut n = 0usize; // contador de rótulos

    let bg = ff_color(&p.settings.background, "black");
    writeln!(g, "color=c={bg}:s={size}:r={fps_s},trim=end_frame={total_frames},setpts=PTS-STARTPTS,format=yuv420p[base0];").unwrap();
    let mut base = "base0".to_string();

    let src_path = |a: &MediaAsset| -> PathBuf {
        match (&a.proxy_path, opts.use_proxies) {
            (Some(px), true) if px.is_file() => px.clone(),
            _ => a.path.clone(),
        }
    };

    for track in &p.timeline.tracks {
        match track.kind {
            TrackKind::Video | TrackKind::Audio => {
                let video_on = track.kind == TrackKind::Video && !track.hidden;
                let audio_on = opts.include_audio && !track.muted && track.volume > 0.0;
                // (label, start_frame_rel, len_frames, clip)
                let mut vitems: Vec<(String, i64, i64, &Clip)> = vec![];
                for c in &track.clips {
                    let Some(vis) = c.range().intersection(&win) else { continue };
                    let ClipContent::Media { asset_id, source_in } = &c.content else { continue };
                    let a = p.asset(*asset_id).ok_or_else(|| MediaError::InvalidTimeline("mídia ausente".into()))?;
                    let want_v = video_on && a.has_video() && c.opacity > 0.0;
                    let want_a = audio_on && a.has_audio() && !c.audio_muted && c.volume > 0.0;
                    if !want_v && !want_a {
                        continue;
                    }
                    let fs = frame_of(vis.start, fps);
                    let fe = frame_of(vis.end, fps);
                    let nf = fe - fs;
                    let (ss, se) = (sample_of(vis.start, sr), sample_of(vis.end, sr));
                    let ns = se - ss;
                    if nf <= 0 && ns <= 0 {
                        continue;
                    }
                    // Trecho de origem que corresponde à parte visível (velocidade/reverso/congelado).
                    let o1 = vis.start - c.start;
                    let o2 = vis.end - c.start;
                    let (seek, src_dur) = if c.freeze {
                        (*source_in, frame_dur)
                    } else if c.reverse {
                        let end_src = *source_in + c.source_span();
                        (end_src - scale_time(o2, c.speed), scale_time(o2 - o1, c.speed))
                    } else {
                        (*source_in + scale_time(o1, c.speed), scale_time(o2 - o1, c.speed))
                    };
                    // Entradas idênticas (mesmo arquivo e trecho, ex.: fundo desfocado +
                    // vídeo principal) são abertas uma vez só e divididas no grafo.
                    let spec = InputSpec {
                        path: src_path(a),
                        seek: if a.is_unbounded() { Time::ZERO } else { seek.max(Time::ZERO) },
                        duration: if a.is_unbounded() { vis.duration() } else { src_dur },
                        image: a.kind == AssetKind::Image,
                        audio_only: !want_v || track.kind == TrackKind::Audio,
                    };
                    let idx =
                        match inputs.iter().position(|x| x.path == spec.path && x.seek == spec.seek && x.duration == spec.duration && x.image == spec.image) {
                            Some(i) => {
                                inputs[i].audio_only &= spec.audio_only;
                                i
                            }
                            None => {
                                inputs.push(spec);
                                inputs.len() - 1
                            }
                        };
                    // tempo relativo do início visível em relação ao clipe
                    let rel0 = o1.as_secs_f64();
                    let clip_len = c.duration.as_secs_f64();

                    if want_v && track.kind == TrackKind::Video && nf > 0 {
                        let v = a.video.as_ref().unwrap();
                        let lbl = format!("v{n}");
                        n += 1;
                        let mut f = format!("[{idx}:v]setpts=PTS-STARTPTS");
                        if c.reverse {
                            f.push_str(",reverse");
                        }
                        if !c.freeze && (c.speed - 1.0).abs() > 1e-4 {
                            write!(f, ",setpts=PTS/{:.6}", c.speed).unwrap();
                        }
                        write!(f, ",fps={fps_s}").unwrap();
                        let animated = c.keyframes.has_transform();
                        // Com keyframes, a geometria base é o encaixe a 100%; o zoom/posição
                        // animados são aplicados depois por zoompan (por frame).
                        let base_t = if animated { Transform { scale: 1.0, x: 0.0, y: 0.0, ..c.transform.clone() } } else { c.transform.clone() };
                        match geometry(v.width, v.height, w, h, &base_t) {
                            Some(geo) => {
                                write!(f, ",scale={}:{}:flags=bicubic,setsar=1", geo.sw, geo.sh).unwrap();
                                if !c.color.is_neutral() {
                                    write!(f, ",eq=brightness={:.4}:contrast={:.4}:saturation={:.4}", c.color.brightness, c.color.contrast, c.color.saturation)
                                        .unwrap();
                                }
                                push_effects(&mut f, &c.effects, h, (geo.sw, geo.sh), &mut deps);
                                if c.opacity < 1.0 {
                                    write!(f, ",format=rgba,colorchannelmixer=aa={:.4}", c.opacity).unwrap();
                                }
                                f.push_str(",format=yuva420p");
                                if let Some((cw, ch, cx, cy)) = geo.crop {
                                    write!(f, ",crop={cw}:{ch}:{cx}:{cy}").unwrap();
                                }
                                if let Some((px, py)) = geo.pad {
                                    write!(f, ",pad={w}:{h}:{px}:{py}:color=black@0").unwrap();
                                }
                                if animated {
                                    let tv = format!("(on/{:.6}+{:.6})", fps.as_f64(), rel0);
                                    let z = kf_expr(&c.keyframes.scale, &tv, c.transform.scale.max(1.0));
                                    let x = kf_expr(&c.keyframes.position_x, &tv, c.transform.x);
                                    let y = kf_expr(&c.keyframes.position_y, &tv, c.transform.y);
                                    write!(
                                        f,
                                        ",zoompan=z='{z}':x='(iw-iw/zoom)/2-({x})*iw/(2*zoom)':y='(ih-ih/zoom)/2-({y})*ih/(2*zoom)':d=1:s={w}x{h}:fps={fps_s}"
                                    )
                                    .unwrap();
                                }
                            }
                            None => {
                                // Totalmente fora do quadro: camada transparente.
                                write!(f, ",scale={w}:{h},format=yuva420p,colorchannelmixer=aa=0").unwrap();
                            }
                        }
                        // Fades do clipe (alpha → revela o que está abaixo).
                        if c.fade_in.is_positive() && rel0 < c.fade_in.as_secs_f64() {
                            // A expansão da janela garante rel0 == 0 aqui.
                            write!(f, ",fade=t=in:st=0:d={}:alpha=1", secs(c.fade_in.as_secs_f64())).unwrap();
                        }
                        if c.fade_out.is_positive() {
                            let st = clip_len - c.fade_out.as_secs_f64() - rel0;
                            if st < vis.duration().as_secs_f64() {
                                write!(f, ",fade=t=out:st={}:d={}:alpha=1", secs(st.max(0.0)), secs(c.fade_out.as_secs_f64())).unwrap();
                            }
                        }
                        writeln!(f, ",tpad=stop_mode=clone:stop={nf},trim=end_frame={nf},setpts=PTS-STARTPTS[{lbl}];").unwrap();
                        g.push_str(&f);
                        vitems.push((lbl, fs - f0, nf, c));
                    }

                    if want_a && ns > 0 && !c.freeze {
                        let lbl = format!("a{n}");
                        n += 1;
                        let gain = c.volume * track.volume;
                        let mut f = format!("[{idx}:a]aresample={sr},aformat=sample_fmts=fltp:sample_rates={sr}:channel_layouts=stereo,asetpts=PTS-STARTPTS");
                        if c.reverse {
                            f.push_str(",areverse");
                        }
                        if (c.speed - 1.0).abs() > 1e-4 {
                            for t in atempo_chain(c.speed) {
                                write!(f, ",atempo={t:.6}").unwrap();
                            }
                        }
                        write!(f, ",apad=whole_len={ns},atrim=end_sample={ns},asetpts=PTS-STARTPTS").unwrap();
                        push_audio_fx(&mut f, &c.audio_fx);
                        if (gain - 1.0).abs() > f32::EPSILON {
                            write!(f, ",volume={gain:.5}").unwrap();
                        }
                        if !c.keyframes.volume.is_empty() {
                            let tv = format!("(t+{rel0:.6})");
                            write!(f, ",volume=volume='{}':eval=frame", kf_expr(&c.keyframes.volume, &tv, 1.0)).unwrap();
                        }
                        // Fades do clipe + fades das transições (crossfade de áudio).
                        let mut fin = c.fade_in;
                        let mut fout = c.fade_out;
                        for tr in &p.timeline.transitions {
                            if tr.to_clip == c.id {
                                fin = fin.max(tr.duration);
                            }
                            if tr.from_clip == c.id {
                                fout = fout.max(tr.duration);
                            }
                        }
                        if fin.is_positive() && rel0 < fin.as_secs_f64() {
                            write!(f, ",afade=t=in:st={}:d={}", secs((-rel0).max(0.0)), secs(fin.as_secs_f64())).unwrap();
                        }
                        if fout.is_positive() {
                            let st = clip_len - fout.as_secs_f64() - rel0;
                            if st < vis.duration().as_secs_f64() {
                                write!(f, ",afade=t=out:st={}:d={}", secs(st.max(0.0)), secs(fout.as_secs_f64())).unwrap();
                            }
                        }
                        let off = ss - s0;
                        if off > 0 {
                            write!(f, ",adelay=delays={off}S:all=1").unwrap();
                        }
                        writeln!(f, "[{lbl}];").unwrap();
                        g.push_str(&f);
                        audio_labels.push(lbl);
                    }
                }

                if !vitems.is_empty() {
                    // Sequência da trilha com lacunas transparentes e xfade nas transições.
                    let mut groups: Vec<String> = vec![]; // segmentos pendentes de concat
                    let mut cursor = 0i64;
                    let mut cur: Option<String> = None; // stream acumulado
                    let mut prev_clip: Option<&Clip> = None;
                    let gap = |g: &mut String, n: &mut usize, len: i64| -> String {
                        let l = format!("g{n}");
                        *n += 1;
                        writeln!(g, "color=c=black@0:s={size}:r={fps_s},format=yuva420p,trim=end_frame={len},setpts=PTS-STARTPTS[{l}];").unwrap();
                        l
                    };
                    for (lbl, start, len, clip) in &vitems {
                        let tr = prev_clip.and_then(|pc| p.timeline.transition_between(pc.id, clip.id));
                        match tr {
                            Some(tr) => {
                                // Consolida o acumulado e aplica xfade com o próximo.
                                let acc = flush(&mut g, &mut n, &mut groups, cur.take());
                                let overlap = cursor - start;
                                let o = format!("x{n}");
                                n += 1;
                                writeln!(
                                    g,
                                    "[{acc}][{lbl}]xfade=transition={}:duration={}:offset={}[{o}];",
                                    tr.kind.xfade_name(),
                                    secs(overlap as f64 * frame_dur.as_secs_f64()),
                                    secs(*start as f64 * frame_dur.as_secs_f64())
                                )
                                .unwrap();
                                cur = Some(o);
                            }
                            None => {
                                if *start > cursor {
                                    let gl = gap(&mut g, &mut n, start - cursor);
                                    if let Some(c) = cur.take() {
                                        groups.push(c);
                                    }
                                    groups.push(gl);
                                } else if let Some(c) = cur.take() {
                                    groups.push(c);
                                }
                                cur = Some(lbl.clone());
                            }
                        }
                        cursor = start + len;
                        prev_clip = Some(clip);
                    }
                    if cursor < total_frames {
                        let gl = gap(&mut g, &mut n, total_frames - cursor);
                        if let Some(c) = cur.take() {
                            groups.push(c);
                        }
                        cur = Some(gl);
                    }
                    let track_stream = flush(&mut g, &mut n, &mut groups, cur.take());
                    let nb = format!("base{n}");
                    n += 1;
                    writeln!(g, "[{base}][{track_stream}]overlay=0:0:format=auto:eof_action=pass[{nb}];").unwrap();
                    base = nb;
                }
            }
            TrackKind::Text => {
                if track.hidden {
                    continue;
                }
                for c in &track.clips {
                    let Some(vis) = c.range().intersection(&win) else { continue };
                    let ClipContent::Text { text, style } = &c.content else { continue };
                    let font = fonts.resolve(&style.font).ok_or_else(|| MediaError::InvalidTimeline("nenhuma fonte disponível no sistema".into()))?;
                    let hash = hex::encode(Sha256::digest(text.as_bytes()));
                    let tf = text_dir.join(format!("{}.txt", &hash[..24]));
                    text_files.push((tf.clone(), text.clone()));
                    let a = (c.start - win.start).as_secs_f64();
                    let b = (c.end() - win.start).as_secs_f64();
                    let _ = vis;
                    let size_px = (style.size as f64 * h as f64 / 1080.0).round().max(4.0);
                    let border = (style.outline_width as f64 * h as f64 / 1080.0).round();
                    let fi = c.fade_in.as_secs_f64().max(1e-6);
                    let fo = c.fade_out.as_secs_f64().max(1e-6);
                    let alpha = format!("if(lt(t\\,{a:.6}+{fi:.6})\\,(t-{a:.6})/{fi:.6}\\,if(gt(t\\,{b:.6}-{fo:.6})\\,({b:.6}-t)/{fo:.6}\\,1))");
                    let mut f = format!(
                        "[{base}]drawtext=fontfile={}:textfile={}:expansion=none:fontsize={size_px}:fontcolor={}:borderw={border}:bordercolor={}:x={:.2}*w-text_w/2:y={:.2}*h-text_h/2:line_spacing={}",
                        escape_filter_value(&font.to_string_lossy()),
                        escape_filter_value(&tf.to_string_lossy()),
                        ff_color(&style.color, "white"),
                        ff_color(&style.outline_color, "black"),
                        style.x.clamp(0.0, 1.0),
                        style.y.clamp(0.0, 1.0),
                        (size_px * 0.15).round(),
                    );
                    if let Some(bgc) = &style.background {
                        write!(f, ":box=1:boxcolor={}:boxborderw={}", ff_color(bgc, "black@0.6"), (size_px * 0.3).round()).unwrap();
                    }
                    if c.fade_in.is_positive() || c.fade_out.is_positive() {
                        write!(f, ":alpha='{alpha}'").unwrap();
                    }
                    let nb = format!("base{n}");
                    n += 1;
                    writeln!(f, ":enable='gte(t\\,{a:.6})*lt(t\\,{b:.6})'[{nb}];").unwrap();
                    g.push_str(&f);
                    base = nb;
                }
            }
        }
    }

    // Saída: apara de volta ao intervalo pedido (a janela pode ter sido expandida).
    let (rf0, rf1) = (frame_of(r.start, fps) - f0, frame_of(r.end, fps) - f0);
    let (rs0, rs1) = (sample_of(r.start, sr) - s0, sample_of(r.end, sr) - s0);
    writeln!(g, "[{base}]trim=start_frame={rf0}:end_frame={rf1},setpts=PTS-STARTPTS,format=yuv420p{VIDEO_OUT};").unwrap();

    let has_audio = opts.include_audio;
    if has_audio {
        let total_samples = s1 - s0;
        writeln!(g, "anullsrc=r={sr}:cl=stereo,atrim=end_sample={total_samples},aformat=sample_fmts=fltp[asil];").unwrap();
        if audio_labels.is_empty() {
            writeln!(g, "[asil]atrim=start_sample={rs0}:end_sample={rs1},asetpts=PTS-STARTPTS{AUDIO_OUT}").unwrap();
        } else {
            let mut ins = String::from("[asil]");
            for l in &audio_labels {
                write!(ins, "[{l}]").unwrap();
            }
            writeln!(
                g,
                "{ins}amix=inputs={}:duration=first:dropout_transition=0:normalize=0,atrim=start_sample={rs0}:end_sample={rs1},asetpts=PTS-STARTPTS{AUDIO_OUT}",
                audio_labels.len() + 1
            )
            .unwrap();
        }
    } else {
        // remove o ';' final
        while g.ends_with('\n') || g.ends_with(';') {
            g.pop();
        }
    }

    let g = split_shared_inputs(&g, inputs.len());
    Ok(CompiledGraph { inputs, filter: g, has_audio, range: r, frames: rf1 - rf0, samples: rs1 - rs0, text_files, deps })
}

/// Expressão FFmpeg (por partes) para uma curva de keyframes; `tv` é a
/// expressão do tempo relativo ao clipe, em segundos. Vírgulas são escapadas
/// para uso dentro de aspas simples no filtergraph.
pub fn kf_expr(kfs: &[Keyframe], tv: &str, default: f32) -> String {
    let Some(first) = kfs.first() else { return format!("{default:.6}") };
    if kfs.len() == 1 {
        return format!("{:.6}", first.value);
    }
    let mut expr = format!("{:.6}", kfs.last().unwrap().value);
    for w in kfs.windows(2).rev() {
        let (a, b) = (&w[0], &w[1]);
        let (ta, tb) = (a.at.as_secs_f64(), b.at.as_secs_f64());
        let u = format!("(({tv}-{ta:.6})/{:.6})", (tb - ta).max(1e-6));
        let shaped = match a.easing {
            Easing::Linear => u,
            Easing::EaseInOut => format!("({u}*{u}*(3-2*{u}))"),
            Easing::Hold => "0".into(),
        };
        let seg = format!("({:.6}+({:.6})*{shaped})", a.value, b.value - a.value);
        expr = format!("if(lt({tv}\\,{tb:.6})\\,{seg}\\,{expr})");
    }
    format!("if(lt({tv}\\,{:.6})\\,{:.6}\\,{expr})", first.at.as_secs_f64(), first.value)
}

/// Cadeia de `atempo` (cada estágio aceita 0,5–2,0).
pub fn atempo_chain(speed: f32) -> Vec<f64> {
    let mut s = speed as f64;
    let mut out = vec![];
    while s > 2.0 {
        out.push(2.0);
        s /= 2.0;
    }
    while s < 0.5 {
        out.push(0.5);
        s /= 0.5;
    }
    out.push(s);
    out
}

/// Efeitos de imagem, escalados pela altura de saída (iguais no preview e na exportação).
/// Um rótulo de entrada (`[3:v]`) só pode ser consumido uma vez no grafo:
/// entradas compartilhadas recebem `split`/`asplit` com um ramo por uso.
fn split_shared_inputs(g: &str, n_inputs: usize) -> String {
    let mut g = g.to_string();
    let mut head = String::new();
    for idx in 0..n_inputs {
        for (kind, filt) in [("v", "split"), ("a", "asplit")] {
            let label = format!("[{idx}:{kind}]");
            let uses = g.matches(&label).count();
            if uses > 1 {
                let mut out = String::with_capacity(g.len() + uses * 12);
                let mut rest = g.as_str();
                let mut k = 0;
                while let Some(pos) = rest.find(&label) {
                    out.push_str(&rest[..pos]);
                    write!(out, "[in{idx}{kind}{k}]").unwrap();
                    k += 1;
                    rest = &rest[pos + label.len()..];
                }
                out.push_str(rest);
                g = out;
                write!(head, "{label}{filt}={uses}").unwrap();
                for k in 0..uses {
                    write!(head, "[in{idx}{kind}{k}]").unwrap();
                }
                head.push_str(";\n");
            }
        }
    }
    head + &g
}

/// Fator de redução para desfoque forte: o resultado de um gaussiano grande
/// numa imagem reduzida e ampliada de volta é visualmente equivalente, a uma
/// fração do custo (σ=40 a 1080p num quadro 3413×1920 era a etapa mais lenta
/// dos cortes com fundo desfocado).
pub fn blur_downscale(sigma: f64) -> i64 {
    ((sigma / 6.0).floor() as i64).clamp(1, 8)
}

fn push_effects(f: &mut String, e: &VideoEffects, out_h: u32, frame: (i64, i64), deps: &mut Vec<PathBuf>) {
    let k = out_h as f64 / 1080.0;
    if (e.temperature - 6500.0).abs() > 1.0 {
        write!(f, ",colortemperature=temperature={:.0}", e.temperature).unwrap();
    }
    if let Some(lut) = &e.lut {
        write!(f, ",lut3d=file={}", escape_filter_value(&lut.to_string_lossy())).unwrap();
        deps.push(lut.clone());
    }
    if e.blur > 0.0 {
        let sigma = (e.blur as f64 * k).max(0.1);
        let d = blur_downscale(sigma);
        let (w, h) = frame;
        if d > 1 && w / d >= 16 && h / d >= 16 {
            let (sw, sh) = (((w / d) & !1).max(2), ((h / d) & !1).max(2));
            write!(f, ",scale={sw}:{sh}:flags=area,gblur=sigma={:.3},scale={w}:{h}:flags=bicubic", sigma / d as f64).unwrap();
        } else {
            write!(f, ",gblur=sigma={sigma:.3}").unwrap();
        }
    }
    if e.sharpen > 0.0 {
        write!(f, ",unsharp=5:5:{:.3}:5:5:0", e.sharpen).unwrap();
    }
    if e.vignette > 0.0 {
        // ângulo 0..π/2 controla a intensidade
        write!(f, ",vignette=angle={:.4}", e.vignette as f64 * std::f64::consts::FRAC_PI_2 * 0.85).unwrap();
    }
}

fn push_audio_fx(f: &mut String, a: &AudioEffects) {
    if a.highpass {
        f.push_str(",highpass=f=80");
    }
    if a.denoise {
        f.push_str(",afftdn=nf=-25");
    }
    for (freq, g) in [(100, a.eq_low_db), (1000, a.eq_mid_db), (8000, a.eq_high_db)] {
        if g.abs() > 0.05 {
            write!(f, ",equalizer=f={freq}:t=o:w=1.5:g={g:.2}").unwrap();
        }
    }
    if a.compressor {
        f.push_str(",acompressor=threshold=0.125:ratio=3:attack=15:release=200:makeup=2");
    }
}

fn flush(g: &mut String, n: &mut usize, groups: &mut Vec<String>, cur: Option<String>) -> String {
    if let Some(c) = cur {
        groups.push(c);
    }
    let out = if groups.len() == 1 {
        groups[0].clone()
    } else {
        let l = format!("c{n}");
        *n += 1;
        let mut s = String::new();
        for x in groups.iter() {
            write!(s, "[{x}]").unwrap();
        }
        writeln!(g, "{s}concat=n={}:v=1:a=0[{l}];", groups.len()).unwrap();
        l
    };
    groups.clear();
    out
}

/// Escreve os arquivos de texto referenciados pelo grafo.
pub fn write_text_files(g: &CompiledGraph) -> Result<()> {
    for (p, content) in &g.text_files {
        if !p.is_file() {
            if let Some(d) = p.parent() {
                std::fs::create_dir_all(d)?;
            }
            std::fs::write(p, content)?;
        }
    }
    Ok(())
}
