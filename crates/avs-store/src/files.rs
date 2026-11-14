//! Arquivos de projeto, snapshots, autosave e utilidades de caminho seguro.

use crate::{Result, StoreError};
use avs_core::{Id, Project, PROJECT_SCHEMA_VERSION};
use serde::Serialize;
use std::io::Write;
use std::path::{Component, Path, PathBuf};
use ts_rs::TS;

pub const PROJECT_FILE: &str = "project.avsproj";
pub const AUTOSAVE_FILE: &str = "autosave.avsproj";

/// Estrutura de diretórios de um projeto.
#[derive(Debug, Clone)]
pub struct ProjectPaths {
    pub dir: PathBuf,
}

impl ProjectPaths {
    pub fn new(library_root: &Path, id: Id) -> Self {
        ProjectPaths { dir: library_root.join("projects").join(id.to_string()) }
    }
    pub fn file(&self) -> PathBuf {
        self.dir.join(PROJECT_FILE)
    }
    pub fn autosave(&self) -> PathBuf {
        self.dir.join(AUTOSAVE_FILE)
    }
    pub fn snapshots(&self) -> PathBuf {
        self.dir.join("snapshots")
    }
    pub fn asset_cache(&self, asset: Id) -> PathBuf {
        self.dir.join("cache").join("assets").join(asset.to_string())
    }
    pub fn chunks(&self, kind: &str) -> PathBuf {
        self.dir.join("cache").join("chunks").join(kind)
    }
    pub fn work(&self) -> PathBuf {
        self.dir.join("cache").join("work")
    }
    pub fn renders(&self) -> PathBuf {
        self.dir.join("renders")
    }
    pub fn proxies(&self) -> PathBuf {
        self.dir.join("proxies")
    }
    pub fn create_all(&self) -> Result<()> {
        for d in [self.dir.clone(), self.snapshots(), self.renders(), self.proxies(), self.work()] {
            std::fs::create_dir_all(d)?;
        }
        Ok(())
    }
}

/// Grava atomicamente: escreve em `.tmp`, faz fsync e renomeia.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> Result<()> {
    let dir = path.parent().ok_or_else(|| StoreError::InvalidPath(path.display().to_string()))?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
    {
        let mut f = std::fs::File::create(&tmp)?;
        f.write_all(bytes)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path)?;
    Ok(())
}

pub fn save_project_file(path: &Path, p: &Project) -> Result<()> {
    if path.is_file() {
        // Mantém a versão anterior como .bak (recuperação de gravação corrompida).
        let _ = std::fs::copy(path, path.with_extension("avsproj.bak"));
    }
    write_atomic(path, &serde_json::to_vec_pretty(p)?)
}

pub fn load_project_file(path: &Path) -> Result<Project> {
    let bytes =
        std::fs::read(path).map_err(|e| if e.kind() == std::io::ErrorKind::NotFound { StoreError::NotFound(path.display().to_string()) } else { e.into() })?;
    let v: serde_json::Value = serde_json::from_slice(&bytes)?;
    let found = v.get("schema_version").and_then(|x| x.as_u64()).unwrap_or(0) as u32;
    if found > PROJECT_SCHEMA_VERSION {
        return Err(StoreError::UnsupportedVersion { found, supported: PROJECT_SCHEMA_VERSION });
    }
    // Migrações futuras: transformar `v` de `found` até a versão atual aqui.
    Ok(serde_json::from_value(v)?)
}

/// Carrega o projeto; se o arquivo principal estiver corrompido, tenta o `.bak`.
pub fn load_with_fallback(paths: &ProjectPaths) -> Result<(Project, bool)> {
    match load_project_file(&paths.file()) {
        Ok(p) => Ok((p, false)),
        Err(StoreError::Json(_)) => {
            let bak = paths.file().with_extension("avsproj.bak");
            Ok((load_project_file(&bak)?, true))
        }
        Err(e) => Err(e),
    }
}

#[derive(Debug, Clone, Serialize, TS)]
#[ts(export)]
pub struct SnapshotInfo {
    pub file_name: String,
    pub label: String,
    pub created_at: String,
}

const SNAPSHOT_LABELS: &str = "labels.json";

fn read_labels(paths: &ProjectPaths) -> std::collections::BTreeMap<String, String> {
    std::fs::read(paths.snapshots().join(SNAPSHOT_LABELS)).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

/// Rótulo original (com espaços e acentos) guardado num índice ao lado dos
/// arquivos; o nome do arquivo continua sanitizado.
pub fn create_snapshot(paths: &ProjectPaths, p: &Project, label: &str) -> Result<SnapshotInfo> {
    let ts = chrono::Utc::now();
    let name = format!("{}_{}.avsproj", ts.format("%Y%m%d-%H%M%S%3f"), sanitize_file_stem(label, 40));
    write_atomic(&paths.snapshots().join(&name), &serde_json::to_vec_pretty(p)?)?;
    let mut labels = read_labels(paths);
    labels.insert(name.clone(), label.to_string());
    write_atomic(&paths.snapshots().join(SNAPSHOT_LABELS), &serde_json::to_vec_pretty(&labels)?)?;
    Ok(SnapshotInfo { file_name: name, label: label.into(), created_at: ts.to_rfc3339() })
}

pub fn list_snapshots(paths: &ProjectPaths) -> Result<Vec<SnapshotInfo>> {
    let labels = read_labels(paths);
    let mut out = vec![];
    if let Ok(rd) = std::fs::read_dir(paths.snapshots()) {
        for e in rd.flatten() {
            let name = e.file_name().to_string_lossy().into_owned();
            if let Some(stem) = name.strip_suffix(".avsproj") {
                let (ts, label) = stem.split_once('_').unwrap_or((stem, ""));
                let label = labels.get(&name).cloned().unwrap_or_else(|| label.replace(['-', '_'], " "));
                let created_at =
                    chrono::NaiveDateTime::parse_from_str(ts, "%Y%m%d-%H%M%S%3f").map(|d| d.and_utc().to_rfc3339()).unwrap_or_else(|_| ts.to_string());
                out.push(SnapshotInfo { file_name: name.clone(), label, created_at });
            }
        }
    }
    out.sort_by(|a, b| b.file_name.cmp(&a.file_name));
    Ok(out)
}

pub fn load_snapshot(paths: &ProjectPaths, file_name: &str) -> Result<Project> {
    let safe = safe_join(&paths.snapshots(), file_name)?;
    load_project_file(&safe)
}

/// Nome de arquivo seguro: remove separadores, caracteres reservados
/// (Windows inclusive), controles e nomes reservados (CON, NUL…).
pub fn sanitize_file_stem(name: &str, max_len: usize) -> String {
    let mut s: String =
        name.chars().map(|c| if c.is_control() || matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '_' } else { c }).collect();
    s = s.trim().trim_matches('.').replace(' ', "_");
    let upper = s.to_ascii_uppercase();
    let reserved = ["CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "LPT1", "LPT2", "LPT3"];
    if s.is_empty() || reserved.contains(&upper.split('.').next().unwrap_or("")) {
        s = format!("_{s}");
    }
    s.chars().take(max_len).collect()
}

/// Junta `base` + `name` garantindo que o resultado continua dentro de `base`
/// (proteção contra path traversal: `..`, caminhos absolutos, prefixos).
pub fn safe_join(base: &Path, name: &str) -> Result<PathBuf> {
    let rel = Path::new(name);
    if rel.components().any(|c| !matches!(c, Component::Normal(_))) || name.is_empty() {
        return Err(StoreError::InvalidPath(name.into()));
    }
    Ok(base.join(rel))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sanitize_and_traversal() {
        assert_eq!(sanitize_file_stem("Meu vídeo: final?", 80), "Meu_vídeo__final_");
        assert_eq!(sanitize_file_stem("../../etc/passwd", 80), "_.._etc_passwd");
        assert_eq!(sanitize_file_stem("CON", 80), "_CON");
        assert!(safe_join(Path::new("/x"), "../y").is_err());
        assert!(safe_join(Path::new("/x"), "/etc/passwd").is_err());
        assert!(safe_join(Path::new("/x"), "a/../../b").is_err());
        assert_eq!(safe_join(Path::new("/x"), "a.avsproj").unwrap(), PathBuf::from("/x/a.avsproj"));
    }

    #[test]
    fn atomic_save_load_and_bak_fallback() {
        let d = tempfile::tempdir().unwrap();
        let p = Project::new("x", Default::default());
        let paths = ProjectPaths::new(d.path(), p.id);
        paths.create_all().unwrap();
        save_project_file(&paths.file(), &p).unwrap();
        save_project_file(&paths.file(), &p).unwrap(); // cria .bak
        std::fs::write(paths.file(), b"{corrompido").unwrap();
        let (back, recovered) = load_with_fallback(&paths).unwrap();
        assert!(recovered);
        assert_eq!(back, p);
        let s = create_snapshot(&paths, &p, "Edição IA v1").unwrap();
        assert_eq!(list_snapshots(&paths).unwrap().len(), 1);
        assert_eq!(load_snapshot(&paths, &s.file_name).unwrap(), p);
    }

    #[test]
    fn rejects_future_schema() {
        let d = tempfile::tempdir().unwrap();
        let f = d.path().join("p.avsproj");
        let mut v = serde_json::to_value(Project::new("x", Default::default())).unwrap();
        v["schema_version"] = 99.into();
        std::fs::write(&f, v.to_string()).unwrap();
        assert!(matches!(load_project_file(&f), Err(StoreError::UnsupportedVersion { .. })));
    }
}
