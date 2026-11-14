//! Cliente do Ollama local (somente 127.0.0.1 por padrão).
//! Usado para planejamento e textos (Fases 4–6); aqui: estado, modelos,
//! download com progresso, geração com saída JSON e teste.

use crate::{AiError, Progress, Result};
use avs_media::CancelToken;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::io::{BufRead, BufReader};
use std::time::Duration;
use ts_rs::TS;

pub const DEFAULT_URL: &str = "http://127.0.0.1:11434";

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct OllamaModel {
    pub name: String,
    #[ts(type = "number")]
    pub size_bytes: u64,
    pub family: Option<String>,
    pub parameter_size: Option<String>,
    pub quantization: Option<String>,
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct OllamaStatus {
    pub url: String,
    pub running: bool,
    pub version: Option<String>,
    pub models: Vec<OllamaModel>,
    pub error: Option<String>,
}

#[derive(Clone)]
pub struct Ollama {
    pub base: String,
    agent: ureq::Agent,
}

#[derive(Deserialize)]
struct Tags {
    #[serde(default)]
    models: Vec<TagModel>,
}
#[derive(Deserialize)]
struct TagModel {
    name: String,
    #[serde(default)]
    size: u64,
    #[serde(default)]
    details: Option<Details>,
}
#[derive(Deserialize)]
struct Details {
    family: Option<String>,
    parameter_size: Option<String>,
    quantization_level: Option<String>,
}

impl Ollama {
    pub fn new(base: &str) -> Self {
        // Sem proxy: o Ollama é local.
        let agent = ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(2)).timeout_read(Duration::from_secs(600)).build();
        Ollama { base: base.trim_end_matches('/').to_string(), agent }
    }

    fn url(&self, p: &str) -> String {
        format!("{}{p}", self.base)
    }

    pub fn status(&self) -> OllamaStatus {
        let mut st = OllamaStatus { url: self.base.clone(), running: false, version: None, models: vec![], error: None };
        match self.agent.get(&self.url("/api/version")).timeout(Duration::from_secs(3)).call() {
            Ok(r) => {
                st.running = true;
                st.version = r.into_json::<Value>().ok().and_then(|v| v["version"].as_str().map(String::from));
                match self.models() {
                    Ok(m) => st.models = m,
                    Err(e) => st.error = Some(e.to_string()),
                }
            }
            Err(e) => st.error = Some(format!("Ollama não está em execução em {} ({e})", self.base)),
        }
        st
    }

    pub fn models(&self) -> Result<Vec<OllamaModel>> {
        let t: Tags = self.agent.get(&self.url("/api/tags")).call().map_err(|e| AiError::Network(e.to_string()))?.into_json()?;
        Ok(t.models
            .into_iter()
            .map(|m| OllamaModel {
                name: m.name,
                size_bytes: m.size,
                family: m.details.as_ref().and_then(|d| d.family.clone()),
                parameter_size: m.details.as_ref().and_then(|d| d.parameter_size.clone()),
                quantization: m.details.as_ref().and_then(|d| d.quantization_level.clone()),
            })
            .collect())
    }

    /// Baixa um modelo com progresso real (bytes reportados pelo Ollama).
    pub fn pull(&self, name: &str, progress: Progress, cancel: &CancelToken) -> Result<()> {
        let r = self.agent.post(&self.url("/api/pull")).send_json(json!({"model": name, "stream": true})).map_err(|e| AiError::Network(e.to_string()))?;
        for line in BufReader::new(r.into_reader()).lines() {
            if cancel.is_cancelled() {
                return Err(AiError::Cancelled);
            }
            let line = line.map_err(|e| AiError::Network(e.to_string()))?;
            let v: Value = match serde_json::from_str(&line) {
                Ok(v) => v,
                Err(_) => continue,
            };
            if let Some(e) = v["error"].as_str() {
                return Err(AiError::Network(format!("Ollama: {e}")));
            }
            let status = v["status"].as_str().unwrap_or("");
            match (v["total"].as_u64(), v["completed"].as_u64()) {
                (Some(t), Some(c)) if t > 0 => progress(status, c as f32 / t as f32),
                _ => progress(status, if status == "success" { 1.0 } else { 0.0 }),
            }
            if status == "success" {
                return Ok(());
            }
        }
        Err(AiError::Network("o download foi interrompido".into()))
    }

    pub fn delete(&self, name: &str) -> Result<()> {
        self.agent.delete(&self.url("/api/delete")).send_json(json!({"model": name})).map_err(|e| AiError::Network(e.to_string()))?;
        Ok(())
    }

    /// Geração sem streaming. Com `schema`, o Ollama restringe a saída a esse JSON Schema.
    pub fn generate(&self, model: &str, system: &str, prompt: &str, schema: Option<&Value>, temperature: f32) -> Result<String> {
        let mut body = json!({
            "model": model,
            "system": system,
            "prompt": prompt,
            "stream": false,
            "options": {"temperature": temperature}
        });
        if let Some(s) = schema {
            body["format"] = s.clone();
        }
        let v: Value = self.agent.post(&self.url("/api/generate")).send_json(body).map_err(|e| AiError::Network(e.to_string()))?.into_json()?;
        if let Some(e) = v["error"].as_str() {
            return Err(AiError::Worker(format!("Ollama: {e}")));
        }
        v["response"].as_str().map(String::from).ok_or_else(|| AiError::Worker("resposta do Ollama sem campo 'response'".into()))
    }

    /// Teste rápido do modelo: pede um JSON fixo e confere a resposta.
    pub fn test_model(&self, model: &str) -> Result<String> {
        let schema = json!({"type": "object", "properties": {"ok": {"type": "boolean"}, "language": {"type": "string"}}, "required": ["ok", "language"]});
        let r = self.generate(model, "Responda apenas com JSON.", "Retorne ok=true e o idioma desta frase: 'Olá, tudo bem?'", Some(&schema), 0.0)?;
        let v: Value = serde_json::from_str(&r).map_err(|e| AiError::Worker(format!("o modelo não retornou JSON válido: {e}: {r}")))?;
        if v["ok"].as_bool() != Some(true) {
            return Err(AiError::Worker(format!("resposta inesperada: {r}")));
        }
        Ok(r)
    }
}
