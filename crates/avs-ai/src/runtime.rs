//! Runtime Python gerenciado e componentes de IA.
//!
//! O app cria um venv próprio em `<biblioteca>/ai/venv` — nada é instalado no
//! Python do sistema. Cada componente é instalado sob demanda pelo
//! Gerenciador de IA, com progresso e cancelamento.

use crate::download::download;
use crate::worker::WorkerEnv;
use crate::{AiError, Progress, Result};
use avs_media::CancelToken;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
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

/// Python do sistema ≥ 3.10 (usado só para criar o venv).
pub fn find_system_python() -> Option<(PathBuf, String)> {
    let mut cands: Vec<(PathBuf, Vec<&str>)> = vec![];
    if let Some(p) = std::env::var_os("AVS_PYTHON") {
        cands.push((PathBuf::from(p), vec![]));
    }
    for n in ["python3", "python"] {
        cands.push((PathBuf::from(n), vec![]));
    }
    if cfg!(windows) {
        cands.push((PathBuf::from("py"), vec!["-3"]));
    }
    for (exe, pre) in cands {
        let out = Command::new(&exe).args(&pre).args(["-c", "import sys;print('%d.%d.%d'%sys.version_info[:3])"]).stdin(Stdio::null()).output();
        if let Ok(o) = out {
            if o.status.success() {
                let v = String::from_utf8_lossy(&o.stdout).trim().to_string();
                let mut it = v.split('.').map(|x| x.parse::<u32>().unwrap_or(0));
                let (maj, min) = (it.next().unwrap_or(0), it.next().unwrap_or(0));
                if maj == 3 && min >= 10 {
                    // `py -3` não é um caminho de executável; resolve o real.
                    let real = Command::new(&exe)
                        .args(&pre)
                        .args(["-c", "import sys;print(sys.executable)"])
                        .output()
                        .ok()
                        .map(|o| PathBuf::from(String::from_utf8_lossy(&o.stdout).trim()))
                        .unwrap_or(exe);
                    return Some((real, v));
                }
            }
        }
    }
    None
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
    let caps = paths.worker().and_then(|w| w.capabilities(&paths.models()).ok());
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
    if !rt.installed {
        rt.note = Some(match find_system_python() {
            Some((p, v)) => format!("Será criado a partir do Python {v} ({})", p.display()),
            None => "Instale o Python 3.10+ (python.org) para habilitar a IA local.".into(),
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
    let mut cmd = Command::new(python);
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
pub fn install(paths: &AiPaths, id: &str, progress: Progress, cancel: &CancelToken) -> Result<()> {
    let ensure_runtime = |progress: Progress| -> Result<PathBuf> {
        let py = paths.venv_python();
        if !py.is_file() {
            let (sys_py, v) = find_system_python().ok_or(AiError::PythonNotFound)?;
            progress(&format!("criando ambiente Python {v}"), 0.02);
            std::fs::create_dir_all(&paths.root)?;
            let o = Command::new(&sys_py).args(["-m", "venv"]).arg(paths.venv()).stdin(Stdio::null()).output()?;
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
                let o = Command::new(py).args(["-m", "pip", "uninstall", "-y"]).args(pkgs).stdin(Stdio::null()).output()?;
                if !o.status.success() {
                    return Err(AiError::Worker(String::from_utf8_lossy(&o.stderr).trim().to_string()));
                }
            }
        }
        other => return Err(AiError::Invalid(format!("componente desconhecido: {other}"))),
    }
    Ok(())
}
