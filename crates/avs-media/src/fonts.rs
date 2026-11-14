//! Resolução de famílias de fonte para arquivos (drawtext precisa de `fontfile`).

use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default)]
pub struct FontResolver {
    /// nome normalizado do arquivo (sem extensão) → caminho
    index: BTreeMap<String, PathBuf>,
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(|c| c.to_lowercase()).collect()
}

impl FontResolver {
    /// `extra` primeiro (fontes empacotadas com o app), depois diretórios do sistema.
    pub fn new(extra: &[PathBuf]) -> Self {
        let mut dirs: Vec<PathBuf> = extra.to_vec();
        if cfg!(target_os = "windows") {
            if let Some(w) = std::env::var_os("WINDIR") {
                dirs.push(PathBuf::from(w).join("Fonts"));
            }
            if let Some(l) = std::env::var_os("LOCALAPPDATA") {
                dirs.push(PathBuf::from(l).join("Microsoft/Windows/Fonts"));
            }
        } else if cfg!(target_os = "macos") {
            dirs.extend(["/System/Library/Fonts", "/Library/Fonts"].map(PathBuf::from));
            if let Some(h) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(h).join("Library/Fonts"));
            }
        } else {
            dirs.extend(["/usr/share/fonts", "/usr/local/share/fonts"].map(PathBuf::from));
            if let Some(h) = std::env::var_os("HOME") {
                dirs.push(PathBuf::from(&h).join(".fonts"));
                dirs.push(PathBuf::from(h).join(".local/share/fonts"));
            }
        }
        let mut index = BTreeMap::new();
        for d in dirs {
            scan(&d, 0, &mut index);
        }
        FontResolver { index }
    }

    pub fn is_empty(&self) -> bool {
        self.index.is_empty()
    }

    pub fn families(&self) -> Vec<String> {
        let mut v: Vec<String> =
            self.index.values().filter_map(|p| p.file_stem()).map(|s| s.to_string_lossy().split('-').next().unwrap_or("").to_string()).collect();
        v.sort();
        v.dedup();
        v
    }

    /// Prefere `<Família>-Bold`, depois `<Família>-Regular`, depois `<Família>`;
    /// cai para DejaVuSans/Arial/qualquer fonte. Legendas de vídeo ficam mais
    /// legíveis em negrito, por isso o padrão é Bold.
    pub fn resolve(&self, family: &str) -> Option<PathBuf> {
        let f = norm(family);
        for cand in [format!("{f}bold"), format!("{f}regular"), f.clone()] {
            if let Some(p) = self.index.get(&cand) {
                return Some(p.clone());
            }
        }
        if let Some((_, p)) = self.index.iter().find(|(k, _)| k.starts_with(&f) && !k.contains("italic")) {
            return Some(p.clone());
        }
        for fb in ["dejavusansbold", "dejavusans", "arialbold", "arial", "helveticabold", "helvetica", "segoeuibold"] {
            if let Some(p) = self.index.get(fb) {
                return Some(p.clone());
            }
        }
        self.index.values().next().cloned()
    }
}

fn scan(dir: &Path, depth: usize, out: &mut BTreeMap<String, PathBuf>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        let p = e.path();
        if p.is_dir() && depth < 4 {
            scan(&p, depth + 1, out);
        } else if let Some(ext) = p.extension().and_then(|e| e.to_str()) {
            if matches!(ext.to_ascii_lowercase().as_str(), "ttf" | "otf") {
                if let Some(stem) = p.file_stem() {
                    out.entry(norm(&stem.to_string_lossy())).or_insert(p);
                }
            }
        }
    }
}
