//! Runtime Python gerenciado e componentes de IA.
//!
//! O app cria um venv próprio em `<biblioteca>/ai/venv` — nada é instalado no
//! Python do sistema. Cada componente é instalado sob demanda pelo
//! Gerenciador de IA, com progresso e cancelamento.

use crate::download::download;
use crate::worker::WorkerEnv;
use crate::{AiError, Progress, Result};
use avs_media::process::command;
use avs_media::CancelToken;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::Duration;
use ts_rs::TS;

/// Modelo YuNet (OpenCV Zoo, MIT) — detecção de rostos.
pub const YUNET_URL: &str = "https://media.githubusercontent.com/media/opencv/opencv_zoo/main/models/face_detection_yunet/face_detection_yunet_2023mar.onnx";
pub const YUNET_SHA256: &str = "8f2383e4dd3cfbb4553ea8718107fc0423210dc964f9f4280604804ed2552fa4";
pub const YUNET_FILE: &str = "face_detection_yunet_2023mar.onnx";

#[derive(Debug, Clone)]
pub struct AiPaths {
    pub root: PathBuf,
    /// Pasta que contém `avs_worker` e os `requirements-*.txt`.
    pub worker_dir: PathBuf,
}

impl AiPaths {
    pub fn new(library_root: &Path, worker_dir: PathBuf) -> Self {
        AiPaths { root: library_root.join("ai"), worker_dir }
    }
    pub fn venv(&self) -> PathBuf {
        self.root.join("venv")
    }
    pub fn models(&self) -> PathBuf {
        self.root.join("models")
    }
    pub fn vision_models(&self) -> PathBuf {
        self.models().join("vision")
    }
    pub fn venv_python(&self) -> PathBuf {
        if cfg!(windows) {
            self.venv().join("Scripts").join("python.exe")
        } else {
            self.venv().join("bin").join("python")
        }
    }
    pub fn worker(&self) -> Option<WorkerEnv> {
        let py = self.venv_python();
        py.is_file().then(|| WorkerEnv { python: py, worker_dir: self.worker_dir.clone() })
    }
}

/// Versões do Python com wheels para todos os componentes. Mínimo: onnxruntime
/// exige 3.11. Máximo: Python novo demais ainda não tem wheels de onnxruntime/av
/// (em nov/2026, nada para 3.15) e a instalação falharia no meio, compilando.
pub const PY_MIN_MINOR: u32 = 11;
pub const PY_MAX_MINOR: u32 = 14;

pub fn python_supported(version: &str) -> bool {
    let mut it = version.split('.').map(|x| x.parse::<u32>().unwrap_or(0));
    let (maj, min) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
    maj == 3 && (PY_MIN_MINOR..=PY_MAX_MINOR).contains(&min)
}

/// Python do sistema numa versão suportada (usado só para criar o venv).
pub fn find_system_python() -> Option<(PathBuf, String)> {
    detect_pythons().into_iter().find(|(_, v)| python_supported(v))
}

/// Cache das consultas que iniciam processos Python (detecção de versões e
/// capacidades do worker). O Gerenciador de IA e os painéis consultam o estado
/// com frequência; sem cache, cada consulta iniciava até 9 processos.
/// Invalidado ao instalar/remover componentes ([`invalidate_cache`]).
struct ProbeCache {
    pythons: Option<(std::time::Instant, Vec<(PathBuf, String)>)>,
    caps: Option<(std::time::Instant, PathBuf, Option<Value>)>,
}
static CACHE: std::sync::Mutex<ProbeCache> = std::sync::Mutex::new(ProbeCache { pythons: None, caps: None });
const CACHE_TTL: std::time::Duration = std::time::Duration::from_secs(120);

pub fn invalidate_cache() {
    let mut c = CACHE.lock().unwrap();
    c.pythons = None;
    c.caps = None;
}

fn cached_caps(paths: &AiPaths) -> Option<Value> {
    let venv = paths.venv();
    if let Some((at, p, v)) = CACHE.lock().unwrap().caps.as_ref() {
        if at.elapsed() < CACHE_TTL && *p == venv {
            return v.clone();
        }
    }
    let v = paths.worker().and_then(|w| w.capabilities(&paths.models()).ok());
    CACHE.lock().unwrap().caps = Some((std::time::Instant::now(), venv, v.clone()));
    v
}

/// Todos os Pythons encontrados, na ordem de preferência. No Windows, o
/// lançador `py` escolhe a versão mais nova com `-3`; por isso as suportadas
/// são pedidas explicitamente antes (3.12 e 3.13 primeiro: as mais testadas).
pub fn detect_pythons() -> Vec<(PathBuf, String)> {
    if let Some((at, v)) = CACHE.lock().unwrap().pythons.as_ref() {
        if at.elapsed() < CACHE_TTL {
            return v.clone();
        }
    }
    let v = detect_pythons_uncached();
    CACHE.lock().unwrap().pythons = Some((std::time::Instant::now(), v.clone()));
    v
}

fn detect_pythons_uncached() -> Vec<(PathBuf, String)> {
    let mut cands: Vec<(PathBuf, Vec<String>)> = vec![];
    if let Some(p) = std::env::var_os("AVS_PYTHON") {
        cands.push((PathBuf::from(p), vec![]));
    }
    if cfg!(windows) {
        for m in [12, 13, 14, 11] {
            cands.push((PathBuf::from("py"), vec![format!("-3.{m}")]));
        }
    } else {
        for m in [12, 13, 14, 11] {
            cands.push((PathBuf::from(format!("python3.{m}")), vec![]));
        }
    }
    for n in ["python3", "python"] {
        cands.push((PathBuf::from(n), vec![]));
    }
    if cfg!(windows) {
        cands.push((PathBuf::from("py"), vec!["-3".into()]));
    }
    let mut found: Vec<(PathBuf, String)> = vec![];
    for (exe, pre) in cands {
        let out =
            command(&exe).args(&pre).args(["-c", "import sys;print('%d.%d.%d'%sys.version_info[:3]);print(sys.executable)"]).stdin(Stdio::null()).output();
        let Ok(o) = out else { continue };
        if !o.status.success() {
            continue;
        }
        let text = String::from_utf8_lossy(&o.stdout).into_owned();
        let mut lines = text.lines();
        let v = lines.next().unwrap_or_default().trim().to_string();
        // `py -3.x` não é um caminho de executável; usa o real.
        let real = lines.next().map(|l| PathBuf::from(l.trim())).filter(|p| !p.as_os_str().is_empty()).unwrap_or(exe);
        if v.starts_with("3.") && !found.iter().any(|(p, _)| *p == real) {
            found.push((real, v));
        }
    }
    found
}

/// Orientação quando não há Python utilizável (mostrada no Gerenciador de IA).
pub fn python_help() -> String {
    let found = detect_pythons();
    let range = format!("3.{PY_MIN_MINOR} a 3.{PY_MAX_MINOR}");
    match found.first() {
        Some((_, v)) => format!(
            "Python {v} encontrado, mas os componentes de IA só funcionam com o Python {range} (bibliotecas como onnxruntime ainda não existem para versões mais novas). Instale o Python 3.12 ou 3.13 em python.org — ele convive com o {v} — e reabra o app."
        ),
        None => format!("Instale o Python {range} (recomendado: 3.12 ou 3.13, em python.org) para habilitar a IA local."),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS)]
#[serde(rename_all = "snake_case")]
#[ts(export)]
pub enum ComponentKind {
    Runtime,
    Packages,
    Model,
    External,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct AiComponent {
    pub id: String,
    pub name: String,
    pub description: String,
    pub kind: ComponentKind,
    pub installed: bool,
    pub version: Option<String>,
    /// Tamanho aproximado do download (MB).
    pub download_mb: u32,
    /// Memória aproximada em uso (MB).
    pub memory_mb: u32,
    pub languages: Vec<String>,
    pub capabilities: Vec<String>,
    pub requires: Vec<String>,
    pub license: String,
    pub note: Option<String>,
}

pub const WHISPER_MODELS: &[(&str, u32, u32)] = &[
    // (tamanho, download MB, RAM MB aproximada em int8)
    ("tiny", 75, 400),
    ("base", 145, 600),
    ("small", 484, 1200),
    ("medium", 1530, 2600),
    ("large-v3-turbo", 1620, 3200),
    ("large-v3", 3090, 4800),
];

#[allow(clippy::too_many_arguments)]
fn comp(id: &str, name: &str, desc: &str, kind: ComponentKind, dl: u32, mem: u32, caps: &[&str], req: &[&str], lic: &str) -> AiComponent {
    AiComponent {
        id: id.into(),
        name: name.into(),
        description: desc.into(),
        kind,
        installed: false,
        version: None,
        download_mb: dl,
        memory_mb: mem,
        languages: vec![],
        capabilities: caps.iter().map(|s| s.to_string()).collect(),
        requires: req.iter().map(|s| s.to_string()).collect(),
        license: lic.into(),
        note: None,
    }
}

/// Estado atual dos componentes (consulta o worker quando o runtime existe).
pub fn components(paths: &AiPaths) -> (Vec<AiComponent>, Option<Value>) {
    let caps = cached_caps(paths);
    let c = |k: &str| caps.as_ref().and_then(|v| v.get(k)).filter(|v| !v.is_null()).map(|v| v.as_str().map(String::from).unwrap_or_else(|| v.to_string()));
    let mut out = vec![];

    let mut rt = comp(
        "runtime",
        "Ambiente Python",
        "Python isolado do app (venv) onde os componentes de IA são instalados.",
        ComponentKind::Runtime,
        15,
        0,
        &[],
        &[],
        "PSF",
    );
    rt.installed = caps.is_some();
    rt.version = c("python");
    if let Some(v) = rt.version.clone().filter(|v| !python_supported(v)) {
        // venv antigo com Python sem suporte: reinstalar o recria com um suportado.
        rt.installed = false;
        rt.note = Some(match find_system_python() {
            Some((_, nv)) => format!("O ambiente atual usa o Python {v}, sem suporte. Reinstale para recriá-lo com o Python {nv}."),
            None => python_help(),
        });
    } else if !rt.installed {
        rt.note = Some(match find_system_python() {
            Some((p, v)) => format!("Será criado a partir do Python {v} ({})", p.display()),
            None => python_help(),
        });
    }
    out.push(rt);

    let mut an = comp(
        "analysis",
        "Análise de fala e imagem",
        "Detecção de fala (Silero VAD), cortes de cena (PySceneDetect) e movimento/brilho (OpenCV). Sem PyTorch.",
        ComponentKind::Packages,
        140,
        300,
        &["fala", "cenas", "movimento"],
        &["runtime"],
        "MIT / BSD-3 / Apache-2.0",
    );
    an.installed = caps
        .as_ref()
        .map(|v| v["silero_vad_model"].as_bool() == Some(true) && !v["scenedetect"].is_null() && !v["opencv"].is_null() && !v["onnxruntime"].is_null())
        .unwrap_or(false);
    an.version = c("scenedetect").map(|v| format!("scenedetect {v}, opencv {}", c("opencv").unwrap_or_default()));
    an.languages = vec!["qualquer idioma".into()];
    out.push(an);

    let mut fc = comp(
        "faces",
        "Detecção de rostos (YuNet)",
        "Modelo YuNet do OpenCV Zoo para localizar rostos (base do reenquadramento).",
        ComponentKind::Model,
        1,
        50,
        &["rostos"],
        &["analysis"],
        "MIT",
    );
    fc.installed = paths.vision_models().join(YUNET_FILE).is_file();
    out.push(fc);

    let mut tr = comp(
        "transcription",
        "Transcrição (faster-whisper)",
        "Motor de transcrição local com timestamps por palavra. Requer também um modelo Whisper.",
        ComponentKind::Packages,
        120,
        0,
        &["transcrição", "idioma"],
        &["runtime"],
        "MIT",
    );
    tr.installed = caps.as_ref().map(|v| !v["faster_whisper"].is_null() && !v["huggingface_hub"].is_null()).unwrap_or(false);
    tr.version = c("faster_whisper");
    out.push(tr);

    let installed: Vec<String> =
        caps.as_ref().and_then(|v| v["whisper_models"].as_array().cloned()).unwrap_or_default().iter().filter_map(|x| x.as_str().map(String::from)).collect();
    for (size, dl, mem) in WHISPER_MODELS {
        let mut m = comp(
            &format!("whisper:{size}"),
            &format!("Whisper {size}"),
            "Modelo de transcrição multilíngue (pt-BR, inglês, espanhol e outros).",
            ComponentKind::Model,
            *dl,
            *mem,
            &["transcrição"],
            &["transcription"],
            "MIT",
        );
        m.installed = installed.iter().any(|x| x == size);
        m.languages = vec!["pt".into(), "en".into(), "es".into(), "+96".into()];
        out.push(m);
    }
    (out, caps)
}

/// Linhas do pip viram etapas reais (sem porcentagem inventada).
fn run_pip(python: &Path, args: &[&str], what: &str, progress: Progress, cancel: &CancelToken) -> Result<()> {
    let mut cmd = command(python);
    cmd.args(["-m", "pip", "install", "--disable-pip-version-check", "--no-input", "--progress-bar", "off"]).args(args);
    cmd.stdin(Stdio::null()).stdout(Stdio::piped()).stderr(Stdio::piped());
    let mut child = cmd.spawn()?;
    let out = child.stdout.take().unwrap();
    let (tx, rx) = std::sync::mpsc::channel::<String>();
    let t = std::thread::spawn(move || {
        for l in BufReader::new(out).lines().map_while(|l| l.ok()) {
            let _ = tx.send(l);
        }
    });
    let mut errbuf = child.stderr.take().unwrap();
    let et = std::thread::spawn(move || {
        let mut s = String::new();
        let _ = std::io::Read::read_to_string(&mut errbuf, &mut s);
        s
    });
    let mut collected = 0u32;
    let mut total: Option<u32> = None;
    loop {
        while let Ok(l) = rx.try_recv() {
            if l.starts_with("Collecting ") {
                collected += 1;
                progress(&format!("{what}: resolvendo pacotes ({collected})"), 0.05);
            } else if let Some(rest) = l.strip_prefix("Installing collected packages: ") {
                let n = rest.split(',').count() as u32;
                total = Some(n);
                progress(&format!("{what}: instalando {n} pacote(s)"), 0.6);
            } else if l.starts_with("Successfully installed") || l.starts_with("Requirement already satisfied") && total.is_none() {
                progress(&format!("{what}: concluído"), if l.starts_with("Successfully") { 0.95 } else { 0.1 });
            }
        }
        if let Some(s) = child.try_wait()? {
            let _ = t.join();
            let err = et.join().unwrap_or_default();
            if !s.success() {
                let tail: Vec<&str> = err.lines().filter(|l| !l.trim().is_empty()).rev().take(6).collect::<Vec<_>>().into_iter().rev().collect();
                return Err(AiError::Network(format!("pip falhou ao instalar {what}: {}", tail.join(" | "))));
            }
            return Ok(());
        }
        if cancel.is_cancelled() {
            let _ = child.kill();
            let _ = child.wait();
            return Err(AiError::Cancelled);
        }
        std::thread::sleep(Duration::from_millis(100));
    }
}

/// Instala um componente (e suas dependências) com progresso.
///
/// Dependências que faltam são instaladas antes, na ordem certa (ex.: um modelo
/// Whisper instala também o ambiente e o motor de transcrição). Ids especiais:
/// - `recommended:<modelo>`: análise, rostos, transcrição e o modelo Whisper;
/// - `system_python`: instala o Python 3.13 no Windows (lançador `py` ou winget).
pub fn install(paths: &AiPaths, id: &str, progress: Progress, cancel: &CancelToken) -> Result<()> {
    invalidate_cache();
    let r = install_plan(paths, id, progress, cancel);
    invalidate_cache();
    r
}

/// Ids a instalar, em ordem, para ter `id` funcionando.
pub fn install_order(paths: &AiPaths, id: &str) -> Vec<String> {
    let (comps, _) = components(paths);
    let targets: Vec<String> = match id.strip_prefix("recommended:") {
        Some(model) => ["analysis", "faces", "transcription"].iter().map(|s| s.to_string()).chain([format!("whisper:{model}")]).collect(),
        None => vec![id.to_string()],
    };
    let mut order: Vec<String> = vec![];
    fn visit(id: &str, comps: &[AiComponent], order: &mut Vec<String>, depth: u8) {
        if depth > 8 || order.iter().any(|o| o == id) {
            return;
        }
        let c = comps.iter().find(|c| c.id == id);
        if let Some(c) = c {
            for r in &c.requires {
                visit(r, comps, order, depth + 1);
            }
            if c.installed {
                return;
            }
        }
        order.push(id.to_string());
    }
    for t in &targets {
        visit(t, &comps, &mut order, 0);
    }
    order
}

fn install_plan(paths: &AiPaths, id: &str, progress: Progress, cancel: &CancelToken) -> Result<()> {
    if id == "system_python" {
        return install_system_python(progress, cancel);
    }
    let order = install_order(paths, id);
    let n = order.len().max(1) as f32;
    for (i, step) in order.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(AiError::Cancelled);
        }
        let name = components(paths).0.into_iter().find(|c| c.id == *step).map(|c| c.name).unwrap_or_else(|| step.clone());
        let base = i as f32 / n;
        let sub = |stage: &str, f: f32| {
            let label = if order.len() > 1 { format!("{name} ({}/{}): {stage}", i + 1, order.len()) } else { stage.to_string() };
            progress(&label, base + f.clamp(0.0, 1.0) / n);
        };
        install_inner(paths, step, &sub, cancel)?;
        invalidate_cache();
    }
    progress("concluído", 1.0);
    Ok(())
}

/// Instala o Python 3.13 (versão com suporte) no Windows, sem administrador:
/// pelo Python Install Manager (`py install 3.13`) ou pelo winget.
/// Só roda por pedido explícito do usuário (botão no Gerenciador de IA).
fn install_system_python(progress: Progress, cancel: &CancelToken) -> Result<()> {
    if !cfg!(windows) {
        return Err(AiError::Invalid("instalação automática do Python só no Windows; use o gerenciador de pacotes do sistema".into()));
    }
    let attempts: [(&str, &[&str]); 2] = [
        ("py", &["install", "3.13"]),
        (
            "winget",
            &["install", "--id", "Python.Python.3.13", "-e", "--scope", "user", "--silent", "--accept-package-agreements", "--accept-source-agreements"],
        ),
    ];
    let mut last_err = String::new();
    for (i, (exe, args)) in attempts.iter().enumerate() {
        if cancel.is_cancelled() {
            return Err(AiError::Cancelled);
        }
        progress(&format!("instalando o Python 3.13 com `{exe}` (download de python.org)"), 0.1 + i as f32 * 0.4);
        match command(exe).args(*args).stdin(Stdio::null()).output() {
            Ok(o) if o.status.success() => {
                invalidate_cache();
                if find_system_python().is_some() {
                    progress("concluído", 1.0);
                    return Ok(());
                }
                last_err = format!("`{exe}` terminou, mas o Python 3.13 não foi encontrado");
            }
            Ok(o) => {
                last_err = format!(
                    "`{exe}`: {}",
                    String::from_utf8_lossy(if o.stderr.is_empty() { &o.stdout } else { &o.stderr }).trim().chars().take(400).collect::<String>()
                )
            }
            Err(e) => last_err = format!("`{exe}` indisponível ({e})"),
        }
    }
    Err(AiError::Worker(format!("não foi possível instalar o Python 3.13 automaticamente: {last_err}. Instale pelo site python.org.")))
}

fn install_inner(paths: &AiPaths, id: &str, progress: Progress, cancel: &CancelToken) -> Result<()> {
    let ensure_runtime = |progress: Progress| -> Result<PathBuf> {
        let py = paths.venv_python();
        // Um venv criado (por versões anteriores do app) com um Python sem suporte
        // faria a instalação dos pacotes falhar: é refeito com um Python suportado.
        if py.is_file() {
            let v = command(&py)
                .args(["-c", "import sys;print('%d.%d.%d'%sys.version_info[:3])"])
                .stdin(Stdio::null())
                .output()
                .ok()
                .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
                .unwrap_or_default();
            if !python_supported(&v) && find_system_python().is_some() {
                progress(&format!("recriando o ambiente (Python {v} não é suportado)"), 0.01);
                std::fs::remove_dir_all(paths.venv())?;
            }
        }
        if !py.is_file() {
            let (sys_py, v) = find_system_python().ok_or(AiError::PythonNotFound)?;
            progress(&format!("criando ambiente Python {v}"), 0.02);
            std::fs::create_dir_all(&paths.root)?;
            let o = command(&sys_py).args(["-m", "venv"]).arg(paths.venv()).stdin(Stdio::null()).output()?;
            if !o.status.success() {
                return Err(AiError::Worker(format!("não foi possível criar o venv: {}", String::from_utf8_lossy(&o.stderr).trim())));
            }
        }
        Ok(py)
    };
    let req = |name: &str| paths.worker_dir.join(name).to_string_lossy().into_owned();
    match id {
        "runtime" => {
            ensure_runtime(progress)?;
        }
        "analysis" => {
            let py = ensure_runtime(progress)?;
            run_pip(&py, &["-r", &req("requirements-core.txt")], "análise", progress, cancel)?;
            // Apenas o modelo ONNX empacotado; dispensa PyTorch.
            run_pip(&py, &["--no-deps", "-r", &req("requirements-vad-model.txt")], "modelo de fala", progress, cancel)?;
        }
        "faces" => {
            download(YUNET_URL, &paths.vision_models().join(YUNET_FILE), Some(YUNET_SHA256), &|_, f| progress("baixando YuNet", f), cancel)?;
        }
        "transcription" => {
            let py = ensure_runtime(progress)?;
            run_pip(&py, &["-r", &req("requirements-transcribe.txt")], "transcrição", progress, cancel)?;
        }
        m if m.starts_with("whisper:") => {
            let size = &m["whisper:".len()..];
            let w = paths.worker().ok_or_else(|| AiError::ComponentMissing("instale primeiro o componente 'Transcrição'".into()))?;
            w.call("download_whisper", json!({"model": size, "models_dir": paths.models()}), &|s, f| progress(s, f), cancel, None)?;
        }
        other => return Err(AiError::Invalid(format!("componente desconhecido: {other}"))),
    }
    progress("concluído", 1.0);
    Ok(())
}

/// Remove um componente (modelos apagados; pacotes desinstalados; runtime apaga o venv).
pub fn remove(paths: &AiPaths, id: &str) -> Result<()> {
    let r = remove_inner(paths, id);
    invalidate_cache();
    r
}

fn remove_inner(paths: &AiPaths, id: &str) -> Result<()> {
    match id {
        "runtime" => {
            if paths.venv().is_dir() {
                std::fs::remove_dir_all(paths.venv())?;
            }
        }
        "faces" => {
            let _ = std::fs::remove_file(paths.vision_models().join(YUNET_FILE));
        }
        m if m.starts_with("whisper:") => {
            let d = paths.models().join("whisper").join(&m["whisper:".len()..]);
            if d.starts_with(paths.models()) && d.is_dir() {
                std::fs::remove_dir_all(d)?;
            }
        }
        "analysis" | "transcription" => {
            let py = paths.venv_python();
            if py.is_file() {
                let pkgs: &[&str] =
                    if id == "analysis" { &["scenedetect", "opencv-python-headless", "silero-vad"] } else { &["faster-whisper", "ctranslate2"] };
                let o = command(py).args(["-m", "pip", "uninstall", "-y"]).args(pkgs).stdin(Stdio::null()).output()?;
                if !o.status.success() {
                    return Err(AiError::Worker(String::from_utf8_lossy(&o.stderr).trim().to_string()));
                }
            }
        }
        other => return Err(AiError::Invalid(format!("componente desconhecido: {other}"))),
    }
    Ok(())
}

#[cfg(test)]
mod python_version_tests {
    use super::python_supported;

    #[test]
    fn supported_range() {
        for v in ["3.11.9", "3.12.4", "3.13.1", "3.14.0"] {
            assert!(python_supported(v), "{v}");
        }
        for v in ["3.10.12", "3.15.2", "2.7.18", "", "4.0.0"] {
            assert!(!python_supported(v), "{v}");
        }
    }
}

#[cfg(test)]
mod install_order_tests {
    use super::*;

    #[test]
    fn dependencies_come_first_and_once() {
        let dir = tempfile::tempdir().unwrap();
        let p = AiPaths::new(dir.path(), dir.path().join("workers"));
        assert_eq!(install_order(&p, "whisper:small"), ["runtime", "transcription", "whisper:small"]);
        assert_eq!(install_order(&p, "faces"), ["runtime", "analysis", "faces"]);
        assert_eq!(install_order(&p, "recommended:base"), ["runtime", "analysis", "faces", "transcription", "whisper:base"]);
    }
}
