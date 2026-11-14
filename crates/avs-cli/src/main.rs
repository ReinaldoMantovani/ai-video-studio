//! `avs` — interface de linha de comando do AI Video Studio.
//!
//! Usa exatamente a mesma camada de serviço do app desktop. Útil para
//! automação, integração contínua, benchmarks e para diagnosticar a
//! instalação do FFmpeg.

mod bench;
use avs_app::*;
use avs_core::*;
use clap::{Parser, Subcommand};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(name = "avs", version, about = "AI Video Studio — CLI")]
struct Cli {
    /// Pasta da biblioteca (padrão: AVS_LIBRARY ou diretório de dados do usuário)
    #[arg(long, global = true)]
    library: Option<PathBuf>,
    /// Pasta de presets (padrão: AVS_PRESETS_DIR ou ./presets)
    #[arg(long, global = true)]
    presets: Option<PathBuf>,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Mostra metadados de um arquivo de mídia
    Probe {
        file: PathBuf,
    },
    /// Testa encoders (software e hardware) com codificação real
    Encoders,
    /// Informações de hardware
    System,
    /// Lista presets editoriais e perfis de exportação
    Presets,
    /// Lista projetos
    List,
    /// Cria um projeto
    New {
        name: String,
        #[arg(long)]
        preset: Option<String>,
    },
    /// Importa arquivos/pastas e aguarda a análise
    Import {
        project: String,
        paths: Vec<PathBuf>,
    },
    /// Aplica um comando de edição em JSON (ver EditCommand)
    Edit {
        project: String,
        json: String,
    },
    /// Mostra a timeline
    Show {
        project: String,
    },
    Undo {
        project: String,
    },
    Redo {
        project: String,
    },
    Save {
        project: String,
    },
    /// Exporta e valida
    Export {
        project: String,
        output_dir: PathBuf,
        #[arg(long, default_value = "youtube_1080p")]
        profile: String,
        #[arg(long)]
        name: Option<String>,
    },
    /// Analisa (se preciso), sugere cortes verticais e cria/exporta os melhores
    Shorts {
        project: String,
        /// Preset vertical (youtube_shorts, instagram_reels, tiktok_dinamico…)
        #[arg(long, default_value = "youtube_shorts")]
        preset: String,
        #[arg(long, default_value_t = 3)]
        count: u32,
        /// face | center | blur_background
        #[arg(long, default_value = "face")]
        framing: String,
        /// Só lista as sugestões, sem criar projetos
        #[arg(long)]
        dry_run: bool,
        /// Exporta cada corte nesta pasta (perfil do preset)
        #[arg(long)]
        export: Option<PathBuf>,
    },
    /// Benchmark de gravação longa: importação, análise, pausas, cortes, scrubbing, exportação e pico de memória
    Bench {
        #[arg(long)]
        out: PathBuf,
        /// Duração da gravação sintética, em minutos
        #[arg(long, default_value_t = 30.0)]
        minutes: f64,
        /// Altura do vídeo gerado (720 = 1280x720)
        #[arg(long, default_value_t = 720)]
        height: u32,
    },
    /// Fluxo completo com mídia sintética: importar → editar → preview → exportar → validar
    Demo {
        #[arg(long)]
        out: PathBuf,
        /// Duração da mídia gerada, em segundos (benchmark de arquivos longos)
        #[arg(long, default_value_t = 20)]
        seconds: u32,
    },
}

struct PrintSink;
impl EventSink for PrintSink {
    fn emit(&self, e: AppEvent) {
        if let AppEvent::Job { job } = e {
            if job.state == "running" {
                eprint!("\r  {:<28} {:<24} {:>5.1}%   ", job.title.chars().take(28).collect::<String>(), job.stage.unwrap_or_default(), job.progress * 100.0);
            } else {
                eprintln!("\r  {:<28} {:<24}         ", job.title.chars().take(28).collect::<String>(), job.state);
            }
        }
    }
}

fn presets_dir(arg: Option<PathBuf>) -> PathBuf {
    arg.or_else(|| std::env::var_os("AVS_PRESETS_DIR").map(PathBuf::from))
        .or_else(|| {
            let exe = std::env::current_exe().ok()?;
            exe.ancestors().map(|d| d.join("presets")).find(|d| d.join("export_profiles.json").is_file())
        })
        .unwrap_or_else(|| PathBuf::from("presets"))
}

fn find_project(st: &Studio, key: &str) -> Result<Id, AppError> {
    if let Ok(id) = Id::parse_str(key) {
        return Ok(id);
    }
    st.list_projects()?.into_iter().find(|p| p.name == key).map(|p| p.id).ok_or_else(|| AppError::new("not_found", format!("projeto '{key}' não encontrado")))
}

fn wait(st: &Studio, job: Id) -> Result<serde_json::Value, AppError> {
    let r = st.wait_job(job, Duration::from_secs(24 * 3600))?;
    match r.state.as_str() {
        "completed" => Ok(r.result.unwrap_or_default()),
        s => Err(AppError::new("job", format!("job terminou como '{s}': {}", r.error.unwrap_or_default()))),
    }
}

fn print_report(v: &serde_json::Value) {
    if let Some(checks) = v["report"]["checks"].as_array() {
        for c in checks {
            let icon = match c["status"].as_str() {
                Some("pass") => "✔",
                Some("warn") => "⚠",
                _ => "✘",
            };
            println!("    {icon} {:<36} {}", c["label"].as_str().unwrap_or(""), c["detail"].as_str().unwrap_or(""));
        }
    }
}

fn main() {
    tracing_subscriber::fmt().with_env_filter(tracing_subscriber::EnvFilter::from_default_env()).with_writer(std::io::stderr).init();
    let cli = Cli::parse();
    if let Err(e) = run(cli) {
        eprintln!("erro [{}]: {}", e.code, e.message);
        std::process::exit(1);
    }
}

fn run(cli: Cli) -> Result<(), AppError> {
    let presets = presets_dir(cli.presets);
    let library = cli.library.unwrap_or_else(default_library_root);
    if let Cmd::Probe { file } = &cli.cmd {
        let tools = avs_media::FfmpegTools::locate(&[])?;
        let info = avs_media::probe::probe(&tools, file)?;
        let a = avs_media::probe::build_asset(&info, file, avs_media::probe::fingerprint(file)?);
        println!("{}", serde_json::to_string_pretty(&a).unwrap());
        return Ok(());
    }
    let st = Studio::start(StudioConfig::new(library.clone(), presets), Arc::new(PrintSink))?;
    let r = dispatch(&st, cli.cmd, &library);
    st.shutdown();
    r
}

fn dispatch(st: &Studio, cmd: Cmd, library: &Path) -> Result<(), AppError> {
    match cmd {
        Cmd::Probe { .. } => unreachable!(),
        Cmd::Encoders => {
            for e in st.encoders() {
                let s = if e.working {
                    "funciona"
                } else if e.compiled {
                    "indisponível"
                } else {
                    "não compilado"
                };
                println!("{:<20} {:<5} {:<20} {}", e.name, e.codec, e.vendor, s);
            }
        }
        Cmd::System => println!("{}", serde_json::to_string_pretty(&st.system_info()).unwrap()),
        Cmd::Presets => {
            for p in st.presets() {
                println!("{:<24} {:<6} {}x{:<5} {}", p.id, p.aspect, p.resolution.width, p.resolution.height, p.name);
            }
            println!();
            for p in st.export_profiles() {
                println!("{:<16} {:<5} {:<5} {}", p.id, p.codec, p.container.ext(), p.name);
            }
            for e in st.preset_errors() {
                eprintln!("aviso: {e}");
            }
        }
        Cmd::List => {
            println!("biblioteca: {}", library.display());
            for p in st.list_projects()? {
                println!("{}  {:<28} {}x{}  {}  {}", p.id, p.name, p.width, p.height, p.duration, p.updated_at);
            }
        }
        Cmd::New { name, preset } => {
            let s = st.create_project(&name, preset.as_deref(), None)?;
            println!("{}", s.project.id);
        }
        Cmd::Import { project, paths } => {
            let id = find_project(st, &project)?;
            let r = st.import_media(id, &paths)?;
            st.wait_idle(Duration::from_secs(24 * 3600))?;
            for a in &r.assets {
                println!("importado: {} ({}, {})", a.name, a.id, a.duration);
            }
            for s in &r.skipped {
                println!("ignorado: {s}");
            }
            for e in &r.errors {
                eprintln!("erro: {e}");
            }
            st.save_project(id)?;
        }
        Cmd::Edit { project, json } => {
            let id = find_project(st, &project)?;
            let cmd: EditCommand = serde_json::from_str(&json).map_err(|e| AppError::new("json", e.to_string()))?;
            let (out, _) = st.execute(id, &cmd)?;
            st.save_project(id)?;
            println!("{} ok; criados: {:?}", out.label, out.created);
        }
        Cmd::Show { project } => {
            let id = find_project(st, &project)?;
            let p = st.open_project(id)?.project;
            println!("{} — {}x{} @ {} — duração {}", p.name, p.settings.width, p.settings.height, p.settings.fps, p.timeline.duration());
            for t in &p.timeline.tracks {
                println!("  [{:?}] {} ({})", t.kind, t.name, t.id);
                for c in &t.clips {
                    println!("     {} → {}  {:<24} {}", c.start, c.end(), c.name, c.id);
                }
            }
            for a in &p.assets {
                println!("  mídia {} {:?} {} {}", a.id, a.kind, a.name, a.duration);
            }
        }
        Cmd::Undo { project } => {
            let id = find_project(st, &project)?;
            println!("{:?}", st.undo(id)?.history.redo_label);
            st.save_project(id)?;
        }
        Cmd::Redo { project } => {
            let id = find_project(st, &project)?;
            println!("{:?}", st.redo(id)?.history.undo_label);
            st.save_project(id)?;
        }
        Cmd::Save { project } => {
            st.save_project(find_project(st, &project)?)?;
        }
        Cmd::Export { project, output_dir, profile, name } => {
            let id = find_project(st, &project)?;
            let p = st.open_project(id)?.project;
            let prof =
                st.export_profiles().into_iter().find(|x| x.id == profile).ok_or_else(|| AppError::new("not_found", "perfil de exportação desconhecido"))?;
            let out = std::path::absolute(&output_dir)?;
            let req = ExportRequest::from_profile(&prof, out, name.unwrap_or(p.name));
            let v = wait(st, st.export(id, req)?)?;
            println!("exportado: {}  (encoder {}, {} ms)", v["path"].as_str().unwrap_or(""), v["encoder"].as_str().unwrap_or(""), v["elapsed_ms"]);
            print_report(&v);
        }
        Cmd::Shorts { project, preset, count, framing, dry_run, export } => {
            let id = find_project(st, &project)?;
            let framing: avs_ai::shorts::FramingMode =
                serde_json::from_value(serde_json::json!(framing)).map_err(|_| AppError::new("invalid", "enquadramento: face | center | blur_background"))?;
            // Analisa o que faltar (usa o cache por impressão digital).
            for j in st.analyze_media(id, &[], false, false)? {
                wait(st, j)?;
            }
            let d = st.shorts_defaults(id, Some(&preset))?;
            let mut opts = d.options.clone();
            opts.count = count;
            let cands = st.suggest_shorts(id, &d.preset_id, &opts)?;
            if cands.is_empty() {
                println!("nenhum corte encontrado");
                return Ok(());
            }
            for c in &cands {
                println!(
                    "#{} [{:.0}] {:.1}–{:.1} s ({:.1} s) {}\n    {}",
                    c.rank,
                    c.score,
                    c.range.start.as_secs_f64(),
                    c.range.end.as_secs_f64(),
                    c.range.duration().as_secs_f64(),
                    c.title,
                    c.explanation
                );
            }
            if dry_run {
                return Ok(());
            }
            let export = match (export, d.export_profile.clone()) {
                (Some(dir), Some(profile_id)) => Some(avs_app::shorts::ShortsExport { output_dir: std::path::absolute(dir)?, profile_id }),
                (Some(_), None) => return Err(AppError::new("not_found", "o preset não tem perfil de exportação")),
                _ => None,
            };
            let req = avs_app::shorts::ShortsRequest { preset_id: d.preset_id, framing, candidates: cands, captions: true, remove_pauses: true, export };
            let v = wait(st, st.create_shorts_start(id, req)?)?;
            for s in v["shorts"].as_array().cloned().unwrap_or_default() {
                println!("criado: {} ({:.1} s)", s["name"].as_str().unwrap_or(""), s["duration"].as_f64().unwrap_or(0.0) / 705_600_000.0);
                for w in s["warnings"].as_array().cloned().unwrap_or_default() {
                    println!("  aviso: {}", w.as_str().unwrap_or(""));
                }
                if let Ok(job) = serde_json::from_value::<uuid::Uuid>(s["export_job"].clone()) {
                    let e = wait(st, job)?;
                    println!("  exportado: {}", e["path"].as_str().unwrap_or(""));
                    print_report(&e);
                }
            }
            for e in v["errors"].as_array().cloned().unwrap_or_default() {
                println!("erro: {}", e.as_str().unwrap_or(""));
            }
        }
        Cmd::Bench { out, minutes, height } => bench::run(st, &std::path::absolute(&out)?, minutes, height)?,
        Cmd::Demo { out, seconds } => demo(st, &std::path::absolute(&out)?, seconds)?,
    }
    Ok(())
}

/// Fluxo E2E com mídia sintética; registra tempos reais por etapa.
fn demo(st: &Studio, out: &Path, seconds: u32) -> Result<(), AppError> {
    std::fs::create_dir_all(out)?;
    let media = out.join("media");
    std::fs::create_dir_all(&media)?;
    let mut times: Vec<(&str, u128)> = vec![];
    let t = Instant::now();
    let gen = |args: &[&str], file: &Path| -> Result<(), AppError> {
        let mut c = st.ffmpeg().ffmpeg_cmd();
        c.args(["-y", "-v", "error"]).args(args).arg(file);
        avs_media::ffmpeg::run_capture(c, None)?;
        Ok(())
    };
    let d = seconds.to_string();
    gen(
        &[
            "-f",
            "lavfi",
            "-i",
            &format!("testsrc2=s=1920x1080:r=30:d={d}"),
            "-f",
            "lavfi",
            "-i",
            &format!("sine=f=440:d={d}"),
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
        &media.join("gravacao.mp4"),
    )?;
    gen(&["-f", "lavfi", "-i", "sine=f=220:d=30", "-c:a", "libmp3lame"], &media.join("trilha.mp3"))?;
    times.push(("gerar mídia sintética", t.elapsed().as_millis()));

    let t = Instant::now();
    let ps = st.create_project("Demo", Some("gameplay_youtube"), None)?;
    let id = ps.project.id;
    let imp = st.import_media(id, &[media])?;
    st.wait_idle(Duration::from_secs(3600))?;
    times.push(("importação + análise", t.elapsed().as_millis()));
    let video = imp.assets.iter().find(|a| a.kind == AssetKind::Video).unwrap().clone();
    let music = imp.assets.iter().find(|a| a.kind == AssetKind::Audio).unwrap().clone();

    let t = Instant::now();
    let p = st.project_state(id)?.project;
    let (vt, tt, at) = (p.timeline.tracks[0].id, p.timeline.tracks[1].id, p.timeline.tracks[2].id);
    st.execute(id, &EditCommand::InsertClip { track_id: vt, asset_id: video.id, start: Time::ZERO, source_in: None, duration: None, ripple: false })?;
    let full = st.project_state(id)?.project.timeline.duration();
    // Cenário E: remove um trecho no meio e liga as partes com transição suave.
    let a = Time::from_flicks(full.flicks() * 2 / 5);
    let b = Time::from_flicks(full.flicks() / 2);
    st.execute(id, &EditCommand::DeleteRange { start: a, end: b, track_ids: Some(vec![vt]), ripple: true })?;
    let clips: Vec<Id> = st.project_state(id)?.project.timeline.tracks[0].clips.iter().map(|c| c.id).collect();
    st.execute(id, &EditCommand::AddTransition { from_clip: clips[0], to_clip: clips[1], kind: TransitionKind::Crossfade, duration: Time::from_millis(500) })?;
    st.execute(
        id,
        &EditCommand::AddText { track_id: tt, start: Time::from_secs(1), duration: Time::from_secs(3), text: "AI Video Studio".into(), style: None },
    )?;
    let dur = st.project_state(id)?.project.timeline.duration();
    st.execute(
        id,
        &EditCommand::InsertClip {
            track_id: at,
            asset_id: music.id,
            start: Time::ZERO,
            source_in: None,
            duration: Some(dur.min(music.duration)),
            ripple: false,
        },
    )?;
    let m = st.project_state(id)?.project.timeline.tracks[2].clips[0].id;
    st.execute(id, &EditCommand::UpdateClip { clip_id: m, patch: ClipPatch { volume: Some(0.25), fade_out: Some(Time::from_secs(1)), ..Default::default() } })?;
    st.save_project(id)?;
    times.push(("edição (comandos)", t.elapsed().as_millis()));

    let t = Instant::now();
    let pv = wait(st, st.request_preview(id)?)?;
    times.push(("preview 540p", t.elapsed().as_millis()));

    let t = Instant::now();
    let prof = st.export_profiles().into_iter().find(|p| p.id == "youtube_1080p").unwrap();
    let mut req = ExportRequest::from_profile(&prof, out.join("exports"), "demo_final".into());
    req.speed = "veryfast".into();
    let v = wait(st, st.export(id, req.clone())?)?;
    times.push(("exportação 1080p + validação", t.elapsed().as_millis()));

    // Edição pequena (só o título, no início) + reexportação: apenas o bloco
    // afetado é renderizado de novo.
    let tid = st.project_state(id)?.project.timeline.tracks[1].clips[0].id;
    st.execute(id, &EditCommand::UpdateClip { clip_id: tid, patch: ClipPatch { text: Some("Editado".into()), ..Default::default() } })?;
    let t = Instant::now();
    req.file_name = "demo_reexport".into();
    let v2 = wait(st, st.export(id, req)?)?;
    times.push(("reexportação após edição", t.elapsed().as_millis()));

    println!("\nPreview: {}", pv["path"].as_str().unwrap_or(""));
    println!("Exportado: {}", v["path"].as_str().unwrap_or(""));
    print_report(&v);
    println!("Reexportado: {} (blocos reaproveitados {}/{})", v2["path"].as_str().unwrap_or(""), v2["chunks_reused"], v2["chunks_total"]);
    println!("\nTempos reais (mídia de {seconds} s):");
    for (k, ms) in &times {
        println!("  {k:<32} {:>8.2} s", *ms as f64 / 1000.0);
    }
    let bench = serde_json::json!({
        "seconds": seconds,
        "ffmpeg": st.ffmpeg().version,
        "system": st.system_info(),
        "timings_ms": times.iter().map(|(k, v)| (k.to_string(), *v)).collect::<std::collections::BTreeMap<_, _>>(),
        "reexport_chunks": { "reused": v2["chunks_reused"], "total": v2["chunks_total"] }
    });
    std::fs::write(out.join("benchmark.json"), serde_json::to_string_pretty(&bench).unwrap())?;
    println!("\nBenchmark salvo em {}", out.join("benchmark.json").display());
    Ok(())
}
