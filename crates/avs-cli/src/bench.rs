//! `avs bench`: benchmark de gravação longa (Fase 7).
//!
//! Gera uma "palestra" sintética de N minutos (vídeo em movimento + fala real
//! do espeak-ng intercalada com pausas curtas e longas) e mede cada etapa do
//! fluxo real: importação, análise, remoção de pausas na timeline longa,
//! revisão, sugestão de cortes, scrubbing, desfazer/refazer, salvar,
//! exportação de um trecho e de um corte vertical. Registra o pico de memória
//! do app e o maior pico entre os processos filhos (FFmpeg, workers Python).

use avs_app::*;
use avs_core::*;
use std::path::Path;
use std::time::{Duration, Instant};

/// Pico de memória residente deste processo (MB), quando o SO informa.
fn self_peak_mb() -> Option<f64> {
    #[cfg(target_os = "linux")]
    {
        let s = std::fs::read_to_string("/proc/self/status").ok()?;
        let kb: f64 = s.lines().find(|l| l.starts_with("VmHWM:"))?.split_whitespace().nth(1)?.parse().ok()?;
        Some(kb / 1024.0)
    }
    #[cfg(not(target_os = "linux"))]
    {
        None
    }
}

/// Maior pico de memória entre os processos filhos já encerrados (MB).
fn children_peak_mb() -> Option<f64> {
    #[cfg(unix)]
    unsafe {
        let mut u: libc::rusage = std::mem::zeroed();
        if libc::getrusage(libc::RUSAGE_CHILDREN, &mut u) == 0 {
            // Linux informa em KB; macOS em bytes.
            let v = u.ru_maxrss as f64;
            return Some(if cfg!(target_os = "macos") { v / 1048576.0 } else { v / 1024.0 });
        }
        None
    }
    #[cfg(not(unix))]
    {
        None
    }
}

struct Rec {
    rows: Vec<serde_json::Value>,
}

impl Rec {
    fn stage<T>(&mut self, name: &str, f: impl FnOnce() -> Result<(T, String), AppError>) -> Result<T, AppError> {
        eprint!("• {name}… ");
        let t = Instant::now();
        let (v, detail) = f()?;
        let ms = t.elapsed().as_millis() as u64;
        let (sp, cp) = (self_peak_mb(), children_peak_mb());
        eprintln!("{:.1} s  {detail}", ms as f64 / 1000.0);
        self.rows.push(serde_json::json!({ "stage": name, "ms": ms, "detail": detail, "app_peak_mb": sp, "children_peak_mb": cp }));
        Ok(v)
    }
}

fn wait(st: &Studio, job: Id) -> Result<serde_json::Value, AppError> {
    let r = st.wait_job(job, Duration::from_secs(48 * 3600))?;
    match r.state.as_str() {
        "completed" => Ok(r.result.unwrap_or_default()),
        _ => Err(AppError::new("job", r.error.unwrap_or_else(|| r.state.clone()))),
    }
}

pub fn run(st: &Studio, out: &Path, minutes: f64, height: u32) -> Result<(), AppError> {
    std::fs::create_dir_all(out)?;
    let media_dir = out.join("media");
    std::fs::create_dir_all(&media_dir)?;
    let mut rec = Rec { rows: vec![] };
    let secs = (minutes * 60.0).round().max(30.0);
    let width = (height * 16 / 9) & !1;
    let ff = |args: Vec<String>| -> Result<(), AppError> {
        let mut c = st.ffmpeg().ffmpeg_cmd();
        c.args(["-y", "-v", "error"]).args(&args);
        avs_media::ffmpeg::run_capture(c, None)?;
        Ok(())
    };

    // 1. Mídia: um "bloco" de 20 s de áudio (fala, pausa longa, fala, respiro curto…) repetido.
    let media = media_dir.join(format!("palestra_{}min.mp4", minutes));
    rec.stage("gerar mídia sintética", || {
        let phrase = media_dir.join("frase.wav");
        let speech_ok = std::process::Command::new("espeak-ng")
            .args(["-v", "pt-br", "-s", "150", "-w"])
            .arg(&phrase)
            .arg("Hoje vamos falar sobre edição de vídeo e como deixar o ritmo mais interessante.")
            .status()
            .map(|s| s.success())
            .unwrap_or(false);
        let unit = media_dir.join("bloco.wav");
        let speech_src: Vec<String> = if speech_ok {
            vec!["-i".into(), phrase.to_string_lossy().into()]
        } else {
            vec!["-f".into(), "lavfi".into(), "-i".into(), "sine=f=220:d=4.5".into()]
        };
        let mut a: Vec<String> = vec![];
        for d in ["0.4", "2.2", "0.3", "1.6"] {
            a.extend(speech_src.clone());
            a.extend(["-f", "lavfi", "-i"].iter().map(|s| s.to_string()));
            a.push(format!("anullsrc=r=22050:cl=mono:d={d}"));
        }
        a.extend(["-filter_complex".into(), "[0:a]aresample=22050[s0];[2:a]aresample=22050[s1];[4:a]aresample=22050[s2];[6:a]aresample=22050[s3];[s0][1:a][s1][3:a][s2][5:a][s3][7:a]concat=n=8:v=0:a=1[a]".into()]);
        a.extend(["-map".into(), "[a]".into(), "-ac".into(), "1".into()]);
        a.push(unit.to_string_lossy().into());
        ff(a)?;
        ff(vec![
            "-f".into(), "lavfi".into(), "-i".into(), format!("testsrc2=s={width}x{height}:r=30:d={secs}"),
            "-stream_loop".into(), "-1".into(), "-i".into(), unit.to_string_lossy().into(),
            "-t".into(), format!("{secs}"),
            "-c:v".into(), "libx264".into(), "-preset".into(), "ultrafast".into(), "-crf".into(), "28".into(), "-g".into(), "60".into(),
            "-c:a".into(), "aac".into(), "-b:a".into(), "96k".into(), "-shortest".into(),
            media.to_string_lossy().into(),
        ])?;
        let mb = std::fs::metadata(&media).map(|m| m.len() as f64 / 1048576.0).unwrap_or(0.0);
        Ok(((), format!("{width}x{height}, {:.0} min, {mb:.0} MB, fala {}", secs / 60.0, if speech_ok { "espeak-ng" } else { "tom (sem espeak-ng)" })))
    })?;

    let pid = st.create_project(&format!("Benchmark {minutes} min"), Some("entrevistas"), None)?.project.id;
    let asset = rec.stage("importar (probe, poster, miniaturas, waveform, proxy)", || {
        let r = st.import_media(pid, std::slice::from_ref(&media))?;
        let a = r.assets.first().cloned().ok_or_else(|| AppError::new("import", "falhou"))?;
        // Espera os jobs de derivação desta mídia.
        loop {
            let busy = st.jobs()?.into_iter().any(|j| j.kind == "import" && (j.state == "queued" || j.state == "running"));
            if !busy {
                break;
            }
            std::thread::sleep(Duration::from_millis(200));
        }
        let a2 = st.project_state(pid)?.project.asset(a.id).cloned().unwrap_or(a);
        Ok((a2.clone(), format!("proxy: {}", if a2.proxy_path.is_some() { "sim" } else { "não necessário" })))
    })?;
    rec.stage("colocar na timeline", || Ok((st.place_assets(pid, &[asset.id])?, String::new())))?;

    rec.stage("análise (fala, cenas, imagem)", || {
        for j in st.analyze_media(pid, &[], false, true)? {
            wait(st, j)?;
        }
        let an = st.analyses(pid)?;
        let r = an.first().and_then(|a| a.result.clone());
        let d = r
            .map(|r| {
                format!(
                    "fala: {} trechos ({}), cenas: {}, amostras de imagem: {}",
                    r.speech.as_ref().map(|s| s.segments.len()).unwrap_or(0),
                    r.speech.as_ref().map(|s| s.method.clone()).unwrap_or_default(),
                    r.scenes.as_ref().map(|s| s.method.clone()).unwrap_or_default(),
                    r.visual.as_ref().map(|v| v.samples.len()).unwrap_or(0)
                )
            })
            .unwrap_or_default();
        Ok(((), d))
    })?;

    let before = st.project_state(pid)?.project.timeline.duration();
    rec.stage("remover pausas (plano + aplicação)", || {
        let rules = st.pause_rules(pid)?;
        let plan = st.plan_pauses(pid, None, rules)?;
        let n = plan.cuts.len();
        let ps = st.apply_pauses(pid, &plan, true)?;
        let clips: usize = ps.project.timeline.tracks.iter().map(|t| t.clips.len()).sum();
        Ok(((), format!("{n} cortes, {:.0} s → {:.0} s, {clips} clipes", before.as_secs_f64(), ps.project.timeline.duration().as_secs_f64())))
    })?;

    rec.stage("revisão de qualidade", || {
        let r = st.quality_review(pid)?;
        Ok(((), format!("{} ponto(s)", r.checks.iter().map(|c| c.issues.len()).sum::<usize>())))
    })?;

    let cands = rec.stage("sugerir cortes verticais", || {
        let d = st.shorts_defaults(pid, Some("youtube_shorts"))?;
        let c = st.suggest_shorts(pid, &d.preset_id, &d.options)?;
        let n = c.len();
        Ok((c, format!("{n} sugestões")))
    })?;

    rec.stage("scrubbing (5 frames em pontos aleatórios, 540p)", || {
        let d = st.project_state(pid)?.project.timeline.duration().as_secs_f64();
        let mut worst = 0u128;
        for k in [0.11, 0.37, 0.52, 0.78, 0.93] {
            let t = Instant::now();
            st.render_frame(pid, Time::from_secs_f64(d * k), 540)?;
            worst = worst.max(t.elapsed().as_millis());
        }
        Ok(((), format!("pior frame: {worst} ms")))
    })?;

    rec.stage("desfazer + refazer na timeline longa", || {
        let t = Instant::now();
        st.undo(pid)?;
        let u = t.elapsed().as_millis();
        let t = Instant::now();
        st.redo(pid)?;
        Ok(((), format!("desfazer {u} ms, refazer {} ms", t.elapsed().as_millis())))
    })?;

    rec.stage("salvar projeto", || {
        st.save_project(pid)?;
        let f = st.library_root().join("projects").join(pid.to_string()).join("project.avsproj");
        let kb = std::fs::metadata(&f).map(|m| m.len() / 1024).unwrap_or(0);
        Ok(((), format!("{kb} KB")))
    })?;

    let prof = st.export_profiles().into_iter().find(|p| p.id == "youtube_1080p").ok_or_else(|| AppError::new("not_found", "perfil"))?;
    rec.stage("exportar 60 s do meio (1080p + validação completa)", || {
        let d = st.project_state(pid)?.project.timeline.duration();
        let mid = (d / 2).snap_to_frame(Rational::new(30, 1));
        let mut req = ExportRequest::from_profile(&prof, out.join("exports"), "trecho_60s".into());
        req.range = Some(TimeRange::new(mid, (mid + Time::from_secs(60)).min(d)));
        let v = wait(st, st.export(pid, req)?)?;
        Ok(((), format!("{} ({} ms de codificação+validação)", v["encoder"].as_str().unwrap_or(""), v["elapsed_ms"])))
    })?;

    if let Some(c) = cands.first() {
        rec.stage("criar e exportar 1 corte vertical (1080x1920)", || {
            let req = avs_app::shorts::ShortsRequest {
                preset_id: "youtube_shorts".into(),
                framing: avs_ai::shorts::FramingMode::BlurBackground,
                candidates: vec![c.clone()],
                captions: false,
                remove_pauses: false,
                export: Some(avs_app::shorts::ShortsExport { output_dir: out.join("exports"), profile_id: "youtube_shorts".into() }),
            };
            let v = wait(st, st.create_shorts_start(pid, req)?)?;
            let job: Id = serde_json::from_value(v["shorts"][0]["export_job"].clone()).map_err(|_| AppError::new("bench", "sem exportação"))?;
            let e = wait(st, job)?;
            Ok(((), format!("{:.1} s de vídeo", e["duration"].as_f64().unwrap_or(0.0) / 705_600_000.0)))
        })?;
    }

    let total: u64 = rec.rows.iter().map(|r| r["ms"].as_u64().unwrap_or(0)).sum();
    let bench = serde_json::json!({
        "version": env!("CARGO_PKG_VERSION"),
        "minutes": minutes,
        "resolution": format!("{width}x{height}"),
        "system": st.system_info(),
        "ffmpeg": st.ffmpeg().version.clone(),
        "stages": rec.rows,
        "total_ms": total,
        "app_peak_mb": self_peak_mb(),
        "children_peak_mb": children_peak_mb(),
    });
    let path = out.join("benchmark.json");
    std::fs::write(&path, serde_json::to_string_pretty(&bench).unwrap())?;
    println!("\n| Etapa | Tempo | Detalhe |\n|---|---|---|");
    for r in bench["stages"].as_array().unwrap() {
        println!("| {} | {:.1} s | {} |", r["stage"].as_str().unwrap_or(""), r["ms"].as_f64().unwrap_or(0.0) / 1000.0, r["detail"].as_str().unwrap_or(""));
    }
    println!("\nPico de memória: app {:.0} MB, maior processo filho {:.0} MB", self_peak_mb().unwrap_or(0.0), children_peak_mb().unwrap_or(0.0));
    println!("Benchmark salvo em {}", path.display());
    Ok(())
}
