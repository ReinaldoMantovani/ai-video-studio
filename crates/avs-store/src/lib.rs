//! # avs-store
//!
//! Persistência local:
//! - `library.db` (SQLite em modo WAL): índice de projetos, fila de jobs,
//!   histórico de exportações e configurações. Rápido para listar/filtrar.
//! - `projects/<id>/project.avsproj`: documento JSON versionado com a
//!   timeline completa (fonte da verdade do projeto, portátil).
//! - `autosave.avsproj`, `snapshots/` e `.bak` para recuperação.
//!
//! Gravações de arquivo são atômicas (temporário + fsync + rename).

pub mod db;
pub mod files;

pub use db::*;
pub use files::*;

#[derive(Debug, thiserror::Error)]
pub enum StoreError {
    #[error("banco de dados: {0}")]
    Db(#[from] rusqlite::Error),
    #[error("E/S: {0}")]
    Io(#[from] std::io::Error),
    #[error("JSON: {0}")]
    Json(#[from] serde_json::Error),
    #[error("projeto não encontrado: {0}")]
    NotFound(String),
    #[error("arquivo de projeto com versão {found} mais nova que a suportada ({supported})")]
    UnsupportedVersion { found: u32, supported: u32 },
    #[error("caminho inválido: {0}")]
    InvalidPath(String),
}

pub type Result<T> = std::result::Result<T, StoreError>;
