//! # avs-ai
//!
//! IA local do AI Video Studio. Funciona sem chaves pagas e, depois que os
//! componentes são instalados, offline.
//!
//! - [`runtime`]: Python gerenciado (venv próprio) e instalação de componentes.
//! - [`worker`]: cliente JSON-RPC dos workers Python (processo filho, stdio).
//! - [`analysis`]: pipeline de análise (fala, cenas, imagem, volume, transcrição),
//!   com alternativas via FFmpeg quando o Python não está disponível.
//! - [`ollama`]: cliente do Ollama local (127.0.0.1).
//! - [`profile`]: perfis de desempenho a partir do hardware medido.
//! - [`autoedit`]: planejadores que transformam análise em `EditCommand`s
//!   validados (remover pausas, cenas, legendas).
//! - [`scoring`]: pontuação editorial de trechos (0–100) com justificativa.
//! - [`autoplan`]: "Editar automaticamente" por preset, com relatório.
//! - [`review`]: revisão automática de qualidade.
//! - [`shorts`]: sugestão de cortes e reenquadramento vertical.
//! - [`assistant`]: pedidos em linguagem natural → ações validadas.

pub mod analysis;
pub mod assistant;
pub mod autoedit;
pub mod autoplan;
pub mod download;
pub mod ollama;
pub mod profile;
pub mod review;
pub mod runtime;
pub mod scoring;
pub mod shorts;
pub mod worker;

#[derive(Debug, thiserror::Error)]
pub enum AiError {
    #[error("componente de IA ausente: {0}")]
    ComponentMissing(String),
    #[error("modelo não instalado: {0}")]
    ModelMissing(String),
    #[error("Python 3.11 a 3.14 não encontrado (recomendado: 3.12 ou 3.13, em python.org). Instale-o ou defina AVS_PYTHON.")]
    PythonNotFound,
    #[error("falha no worker de IA: {0}")]
    Worker(String),
    #[error("operação cancelada")]
    Cancelled,
    #[error("rede: {0}")]
    Network(String),
    #[error("integridade: {0}")]
    Integrity(String),
    #[error("{0}")]
    Invalid(String),
    #[error("mídia: {0}")]
    Media(#[from] avs_media::MediaError),
    #[error("E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
}

impl AiError {
    pub fn code(&self) -> &'static str {
        match self {
            AiError::ComponentMissing(_) => "ai_component_missing",
            AiError::ModelMissing(_) => "ai_model_missing",
            AiError::PythonNotFound => "ai_python_missing",
            AiError::Worker(_) => "ai_worker",
            AiError::Cancelled => "cancelled",
            AiError::Network(_) => "network",
            AiError::Integrity(_) => "integrity",
            AiError::Invalid(_) => "invalid",
            AiError::Media(avs_media::MediaError::Cancelled) => "cancelled",
            AiError::Media(_) => "media",
            AiError::Io(_) => "io",
            AiError::Json(_) => "json",
        }
    }
}

pub type Result<T> = std::result::Result<T, AiError>;

/// Progresso: (etapa, fração 0..1).
pub type Progress<'a> = &'a (dyn Fn(&str, f32) + Sync);
