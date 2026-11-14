//! Execução segura do FFmpeg/FFprobe.
//!
//! - Nunca usa shell: argumentos são passados como vetor (`Command::args`),
//!   então nomes de arquivo não podem injetar comandos.
//! - Progresso real via `-progress pipe:1` (out_time_us), sem porcentagens simuladas.
//! - Cancelamento cooperativo: o processo filho é finalizado.
//! - Erros carregam o final do stderr do FFmpeg para diagnóstico.

use crate::process::command;
use avs_core::Time;
use std::collections::VecDeque;
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

#[derive(Debug, thiserror::Error)]
pub enum MediaError {
    #[error(
        "FFmpeg/FFprobe não encontrado. Instale o FFmpeg (macOS: `brew install ffmpeg-full`; Linux: `sudo apt install ffmpeg`) ou defina AVS_FFMPEG_DIR. ({0})"
    )]
    ToolNotFound(String),
    #[error("falha ao executar {tool}: {message}")]
    Spawn { tool: String, message: String },
    #[error("{tool} terminou com erro (código {code:?}): {stderr}{}", missing_filter_hint(.stderr))]
    Failed { tool: String, code: Option<i32>, stderr: String },
    #[error("operação cancelada")]
    Cancelled,
    #[error("tempo limite excedido após {0:?}")]
    Timeout(Duration),
    #[error("{tool} parou de responder (sem avançar por {secs} s) e foi encerrado")]
    Stalled { tool: String, secs: u64 },
    #[error("arquivo inválido ou não suportado: {0}")]
    InvalidMedia(String),
    #[error("timeline inválida: {0}")]
    InvalidTimeline(String),
    #[error("validação do arquivo exportado falhou: {0}")]
    ValidationFailed(String),
    #[error("E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, MediaError>;

/// Token de cancelamento compartilhável entre threads.
#[derive(Debug, Clone, Default)]
pub struct CancelToken(Arc<AtomicBool>);
impl CancelToken {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst)
    }
    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Debug, Clone)]
pub struct FfmpegTools {
    pub ffmpeg: PathBuf,
    pub ffprobe: PathBuf,
    pub version: String,
    pub major: u32,
}

fn exe(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn find_in_path(name: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).chain(fallback_dirs()).map(|d| d.join(exe(name))).find(|p| p.is_file())
}

/// Apps abertos pelo Finder/Dock recebem um PATH mínimo (`/usr/bin:/bin:…`), sem
/// o Homebrew/MacPorts: um FFmpeg instalado por eles não seria achado.
fn fallback_dirs() -> Vec<PathBuf> {
    if cfg!(target_os = "macos") {
        ["/opt/homebrew/bin", "/usr/local/bin", "/opt/local/bin"].iter().map(PathBuf::from).collect()
    } else if cfg!(unix) {
        ["/usr/local/bin", "/usr/bin"].iter().map(PathBuf::from).collect()
    } else {
        vec![]
    }
}

impl FfmpegTools {
    /// Ordem de busca: `AVS_FFMPEG_DIR`, diretórios extras (ex.: sidecar do
    /// instalador), `PATH`.
    pub fn locate(extra_dirs: &[PathBuf]) -> Result<Self> {
        let mut dirs: Vec<PathBuf> = vec![];
        if let Some(d) = std::env::var_os("AVS_FFMPEG_DIR") {
            dirs.push(PathBuf::from(d));
        }
        dirs.extend(extra_dirs.iter().cloned());
        // macOS: a fórmula `ffmpeg` do Homebrew é enxuta (sem drawtext); a completa,
        // `ffmpeg-full`, não entra no PATH (keg-only). Se existir, tem preferência.
        if cfg!(target_os = "macos") {
            dirs.extend(["/opt/homebrew/opt/ffmpeg-full/bin", "/usr/local/opt/ffmpeg-full/bin"].map(PathBuf::from));
        }
        let pick = |name: &str| dirs.iter().map(|d| d.join(exe(name))).find(|p| p.is_file()).or_else(|| find_in_path(name));
        let ffmpeg = pick("ffmpeg").ok_or_else(|| MediaError::ToolNotFound("ffmpeg".into()))?;
        let ffprobe = pick("ffprobe").ok_or_else(|| MediaError::ToolNotFound("ffprobe".into()))?;
        let out =
            command(&ffmpeg).arg("-version").stdin(Stdio::null()).output().map_err(|e| MediaError::Spawn { tool: "ffmpeg".into(), message: e.to_string() })?;
        let first = String::from_utf8_lossy(&out.stdout).lines().next().unwrap_or_default().to_string();
        // "ffmpeg version 6.1.1-3ubuntu5 ..." | "ffmpeg version n7.0 ..." | "ffmpeg version N-1234-g..."
        let ver = first.split_whitespace().nth(2).unwrap_or("").to_string();
        let major = ver.trim_start_matches(['n', 'N']).split(['.', '-']).next().and_then(|s| s.parse().ok()).unwrap_or(7);
        Ok(FfmpegTools { ffmpeg, ffprobe, version: ver, major })
    }

    pub fn ffmpeg_cmd(&self) -> Command {
        let mut c = command(&self.ffmpeg);
        c.arg("-hide_banner").arg("-nostdin").stdin(Stdio::null());
        no_window(&mut c);
        c
    }
    pub fn ffprobe_cmd(&self) -> Command {
        let mut c = command(&self.ffprobe);
        c.arg("-hide_banner").stdin(Stdio::null());
        no_window(&mut c);
        c
    }

    /// Argumento para carregar um filtergraph de arquivo (sem limite de tamanho
    /// de linha de comando). FFmpeg 7 trocou `-filter_complex_script` por `-/filter_complex`.
    pub fn filter_script_args(&self, script: &Path) -> Vec<String> {
        if self.major >= 7 {
            vec!["-/filter_complex".into(), script.to_string_lossy().into_owned()]
        } else {
            vec!["-filter_complex_script".into(), script.to_string_lossy().into_owned()]
        }
    }
}

#[cfg(windows)]
fn no_window(c: &mut Command) {
    use std::os::windows::process::CommandExt;
    c.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
}
#[cfg(not(windows))]
fn no_window(_c: &mut Command) {}

fn tool_name(c: &Command) -> String {
    Path::new(c.get_program()).file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default()
}

/// Executa e captura stdout. Falha se o código de saída ≠ 0.
pub fn run_capture(mut cmd: Command, timeout: Option<Duration>) -> Result<Vec<u8>> {
    let tool = tool_name(&cmd);
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| MediaError::Spawn { tool: tool.clone(), message: e.to_string() })?;
    let mut so = child.stdout.take().unwrap();
    let mut se = child.stderr.take().unwrap();
    let t_out = std::thread::spawn(move || {
        let mut b = vec![];
        let _ = so.read_to_end(&mut b);
        b
    });
    let t_err = std::thread::spawn(move || {
        let mut b = vec![];
        let _ = se.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if let Some(t) = timeout {
            if start.elapsed() > t {
                let _ = child.kill();
                let _ = child.wait();
                return Err(MediaError::Timeout(t));
            }
        }
        std::thread::sleep(Duration::from_millis(15));
    };
    let out = t_out.join().unwrap_or_default();
    let err = t_err.join().unwrap_or_default();
    if !status.success() {
        return Err(MediaError::Failed { tool, code: status.code(), stderr: tail(&String::from_utf8_lossy(&err), 20) });
    }
    Ok(out)
}

/// Executa e devolve stderr completo (para filtros de análise como blackdetect).
pub fn run_stderr(cmd: Command, cancel: &CancelToken) -> Result<String> {
    run_stderr_timeout(cmd, cancel, None)
}

/// Como [`run_stderr`], com limite de tempo: um FFmpeg travado não pode deixar
/// o job (ou o app) esperando para sempre.
pub fn run_stderr_timeout(mut cmd: Command, cancel: &CancelToken, timeout: Option<Duration>) -> Result<String> {
    let tool = tool_name(&cmd);
    let mut child = cmd.stdout(Stdio::null()).stderr(Stdio::piped()).spawn().map_err(|e| MediaError::Spawn { tool: tool.clone(), message: e.to_string() })?;
    let mut se = child.stderr.take().unwrap();
    let t_err = std::thread::spawn(move || {
        let mut b = vec![];
        let _ = se.read_to_end(&mut b);
        b
    });
    let start = Instant::now();
    let status = loop {
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(MediaError::Cancelled);
        }
        if let Some(t) = timeout {
            if start.elapsed() > t {
                tracing::warn!(target: "avs::ffmpeg", args = ?cmd.get_args().collect::<Vec<_>>(), "tempo limite; encerrando");
                let _ = child.kill();
                let _ = child.wait();
                return Err(MediaError::Timeout(t));
            }
        }
        std::thread::sleep(Duration::from_millis(30));
    };
    let err = String::from_utf8_lossy(&t_err.join().unwrap_or_default()).into_owned();
    if !status.success() {
        return Err(MediaError::Failed { tool, code: status.code(), stderr: tail(&err, 20) });
    }
    Ok(err)
}

/// Tempo sem avanço do `out_time` a partir do qual um FFmpeg é considerado travado.
/// Mesmo efeitos pesados avançam alguns frames por minuto; parado por tanto tempo,
/// o processo não vai terminar (visto no Windows: render preso por horas).
pub const STALL_LIMIT: Duration = Duration::from_secs(180);

/// Executa um FFmpeg que tenha `-progress pipe:1 -nostats` e reporta o tempo
/// de saída processado. `on_progress(out_time)` é chamado a cada atualização.
/// Se o tempo de saída não avançar por [`STALL_LIMIT`], o processo é encerrado
/// e o erro é [`MediaError::Stalled`].
pub fn run_with_progress(cmd: Command, cancel: &CancelToken, on_progress: impl FnMut(Time)) -> Result<()> {
    run_with_progress_limit(cmd, cancel, STALL_LIMIT, on_progress)
}

pub fn run_with_progress_limit(mut cmd: Command, cancel: &CancelToken, stall_limit: Duration, mut on_progress: impl FnMut(Time)) -> Result<()> {
    let tool = tool_name(&cmd);
    tracing::debug!(target: "avs::ffmpeg", args = ?cmd.get_args().collect::<Vec<_>>(), "spawn");
    let mut child = cmd.stdout(Stdio::piped()).stderr(Stdio::piped()).spawn().map_err(|e| MediaError::Spawn { tool: tool.clone(), message: e.to_string() })?;

    let (tx, rx) = mpsc::channel::<Time>();
    let so = child.stdout.take().unwrap();
    let t_out = std::thread::spawn(move || {
        for line in BufReader::new(so).lines().map_while(|l| l.ok()) {
            if let Some(v) = line.strip_prefix("out_time_us=").or_else(|| line.strip_prefix("out_time_ms=")) {
                // out_time_ms é (historicamente) em microssegundos também.
                if let Ok(us) = v.trim().parse::<i64>() {
                    if us >= 0 {
                        let _ = tx.send(Time::from_flicks((us as i128 * avs_core::FLICKS_PER_SECOND as i128 / 1_000_000) as i64));
                    }
                }
            }
        }
    });
    let se = child.stderr.take().unwrap();
    let t_err = std::thread::spawn(move || {
        let mut ring: VecDeque<String> = VecDeque::with_capacity(40);
        for line in BufReader::new(se).lines().map_while(|l| l.ok()) {
            if ring.len() == 40 {
                ring.pop_front();
            }
            ring.push_back(line);
        }
        ring.into_iter().collect::<Vec<_>>().join("\n")
    });

    let mut last_t: Option<Time> = None;
    let mut moved_at = Instant::now();
    let status = loop {
        while let Ok(t) = rx.try_recv() {
            if last_t != Some(t) {
                last_t = Some(t);
                moved_at = Instant::now();
            }
            on_progress(t);
        }
        if let Some(s) = child.try_wait()? {
            break s;
        }
        if moved_at.elapsed() > stall_limit {
            tracing::warn!(target: "avs::ffmpeg", args = ?cmd.get_args().collect::<Vec<_>>(), "sem progresso; encerrando");
            let _ = child.kill();
            let _ = child.wait();
            let _ = t_out.join();
            let err = t_err.join().unwrap_or_default();
            tracing::warn!(target: "avs::ffmpeg", stderr = %tail(&err, 15), "stderr do processo travado");
            return Err(MediaError::Stalled { tool, secs: stall_limit.as_secs() });
        }
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            let _ = t_out.join();
            let _ = t_err.join();
            return Err(MediaError::Cancelled);
        }
        std::thread::sleep(Duration::from_millis(40));
    };
    let _ = t_out.join();
    while let Ok(t) = rx.try_recv() {
        on_progress(t);
    }
    let err = t_err.join().unwrap_or_default();
    if !status.success() {
        return Err(MediaError::Failed { tool, code: status.code(), stderr: tail(&err, 15) });
    }
    Ok(())
}

/// Builds enxutas do FFmpeg (ex.: a fórmula `ffmpeg` do Homebrew desde 2026) vêm
/// sem filtros que o app usa, como `drawtext` (textos e legendas).
fn missing_filter_hint(stderr: &str) -> &'static str {
    if !stderr.contains("No such filter") {
        ""
    } else if cfg!(target_os = "macos") {
        "\n\nEste FFmpeg não tem todos os filtros necessários. Instale a versão completa: `brew install ffmpeg-full`."
    } else {
        "\n\nEste FFmpeg não tem todos os filtros necessários (ex.: drawtext, que exige libfreetype). Instale uma build completa do FFmpeg."
    }
}

pub fn tail(s: &str, n: usize) -> String {
    let lines: Vec<&str> = s.lines().filter(|l| !l.trim().is_empty()).collect();
    lines[lines.len().saturating_sub(n)..].join("\n")
}

// ─────────────────────── Escape de filtergraph ───────────────────────

/// Escapa um valor de opção de filtro (nível 1) e o filtergraph (nível 2),
/// conforme a documentação "Notes on filtergraph escaping" do FFmpeg.
/// Necessário para caminhos de fonte/arquivo com `:` (Windows), aspas,
/// colchetes, vírgulas etc.
pub fn escape_filter_value(v: &str) -> String {
    // nível 1: dentro do valor da opção
    let mut l1 = String::with_capacity(v.len() + 8);
    for ch in v.chars() {
        if matches!(ch, '\\' | '\'' | ':') {
            l1.push('\\');
        }
        l1.push(ch);
    }
    // nível 2: dentro da descrição do filtergraph
    let mut l2 = String::with_capacity(l1.len() + 8);
    for ch in l1.chars() {
        if matches!(ch, '\\' | '\'' | '[' | ']' | ',' | ';') {
            l2.push('\\');
        }
        l2.push(ch);
    }
    l2
}

/// Escapa um caminho para o arquivo de lista do demuxer concat.
pub fn escape_concat_path(p: &Path) -> String {
    format!("'{}'", p.to_string_lossy().replace('\'', "'\\''"))
}
