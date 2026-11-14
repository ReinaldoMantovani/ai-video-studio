//! Cliente dos workers Python: um processo filho por chamada, JSON-RPC 2.0
//! por stdin/stdout. Progresso real (`$/progress`), cancelamento cooperativo
//! (`$/cancel`) com encerramento forçado se o worker não responder.

use crate::{AiError, Progress, Result};
use avs_media::CancelToken;
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
pub struct WorkerEnv {
    /// Python do venv gerenciado.
    pub python: PathBuf,
    /// Pasta que contém o pacote `avs_worker`.
    pub worker_dir: PathBuf,
}

impl WorkerEnv {
    /// Executa `method` e devolve `result`. Erros do worker viram `AiError`
    /// com a mensagem real.
    pub fn call(&self, method: &str, params: Value, progress: Progress, cancel: &CancelToken, timeout: Option<Duration>) -> Result<Value> {
        let mut cmd = Command::new(&self.python);
        cmd.arg("-u").arg("-m").arg("avs_worker");
        cmd.env("PYTHONPATH", &self.worker_dir).env("PYTHONIOENCODING", "utf-8").env("PYTHONNOUSERSITE", "1");
        cmd.current_dir(&self.worker_dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::piped());
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x0800_0000);
        }
        let mut child = cmd.spawn().map_err(|e| AiError::Worker(format!("não foi possível iniciar o Python ({}): {e}", self.python.display())))?;
        let mut stdin = child.stdin.take().unwrap();
        let req = json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params});
        writeln!(stdin, "{req}")?;
        stdin.flush()?;

        let (tx, rx) = mpsc::channel::<Value>();
        let out = child.stdout.take().unwrap();
        let reader = std::thread::spawn(move || {
            for line in BufReader::new(out).lines().map_while(|l| l.ok()) {
                if let Ok(v) = serde_json::from_str::<Value>(&line) {
                    if tx.send(v).is_err() {
                        break;
                    }
                }
            }
        });
        let mut err = child.stderr.take().unwrap();
        let err_t = std::thread::spawn(move || {
            let mut s = String::new();
            let _ = err.read_to_string(&mut s);
            s
        });

        let start = Instant::now();
        let mut cancel_sent: Option<Instant> = None;
        let result = loop {
            match rx.recv_timeout(Duration::from_millis(100)) {
                Ok(msg) => {
                    if msg.get("method").and_then(|m| m.as_str()) == Some("$/progress") {
                        let p = &msg["params"];
                        progress(p["stage"].as_str().unwrap_or(""), p["fraction"].as_f64().unwrap_or(0.0) as f32);
                        continue;
                    }
                    if msg.get("id") == Some(&json!(1)) {
                        if let Some(r) = msg.get("result") {
                            break Ok(r.clone());
                        }
                        let e = &msg["error"];
                        let code = e["code"].as_str().unwrap_or("");
                        let m = e["message"].as_str().unwrap_or("erro desconhecido").to_string();
                        break Err(match code {
                            "cancelled" => AiError::Cancelled,
                            "component_missing" => AiError::ComponentMissing(m),
                            "model_missing" => AiError::ModelMissing(m),
                            "network" => AiError::Network(m),
                            "invalid_media" | "invalid_audio" | "invalid_params" => AiError::Invalid(m),
                            _ => AiError::Worker(m),
                        });
                    }
                }
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Err(mpsc::RecvTimeoutError::Disconnected) => {
                    let _ = child.wait();
                    let stderr = err_t.join().unwrap_or_default();
                    let tail: Vec<&str> = stderr.lines().rev().take(8).collect::<Vec<_>>().into_iter().rev().collect();
                    return Err(AiError::Worker(format!("o worker terminou inesperadamente: {}", tail.join(" | "))));
                }
            }
            if cancel.is_cancelled() && cancel_sent.is_none() {
                let _ = writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "method": "$/cancel"}));
                let _ = stdin.flush();
                cancel_sent = Some(Instant::now());
            }
            if let Some(t) = cancel_sent {
                if t.elapsed() > Duration::from_secs(3) {
                    break Err(AiError::Cancelled);
                }
            }
            if let Some(t) = timeout {
                if start.elapsed() > t {
                    break Err(AiError::Worker(format!("tempo limite de {t:?} excedido em '{method}'")));
                }
            }
        };
        let _ = writeln!(stdin, "{}", json!({"jsonrpc": "2.0", "id": 2, "method": "shutdown"}));
        drop(stdin);
        // Encerramento: aguarda um pouco e força se necessário.
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            if child.try_wait()?.is_some() {
                break;
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                let _ = child.wait();
                break;
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        let _ = reader.join();
        let stderr = err_t.join().unwrap_or_default();
        if !stderr.trim().is_empty() {
            tracing::debug!(target: "avs::ai", method, stderr = %stderr.trim(), "stderr do worker");
        }
        result
    }

    pub fn capabilities(&self, models_dir: &Path) -> Result<Value> {
        self.call("capabilities", json!({"models_dir": models_dir}), &|_, _| {}, &CancelToken::new(), Some(Duration::from_secs(60)))
    }
}
