//! Download HTTP com progresso real, cancelamento, verificação SHA-256 e
//! gravação atômica (`.part` → nome final).

use crate::{AiError, Progress, Result};
use avs_media::CancelToken;
use sha2::{Digest, Sha256};
use std::io::{Read, Write};
use std::path::Path;
use std::time::Duration;

pub fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new().timeout_connect(Duration::from_secs(20)).timeout_read(Duration::from_secs(60)).try_proxy_from_env(true).build()
}

pub fn download(url: &str, dest: &Path, sha256: Option<&str>, progress: Progress, cancel: &CancelToken) -> Result<u64> {
    let resp = agent().get(url).call().map_err(|e| AiError::Network(format!("{url}: {e}")))?;
    let total: u64 = resp.header("Content-Length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if let Some(d) = dest.parent() {
        std::fs::create_dir_all(d)?;
    }
    let part = dest.with_extension("part");
    let mut f = std::fs::File::create(&part)?;
    let mut r = resp.into_reader();
    let mut h = Sha256::new();
    let mut buf = vec![0u8; 1 << 16];
    let mut done = 0u64;
    loop {
        if cancel.is_cancelled() {
            drop(f);
            let _ = std::fs::remove_file(&part);
            return Err(AiError::Cancelled);
        }
        let n = r.read(&mut buf).map_err(|e| AiError::Network(e.to_string()))?;
        if n == 0 {
            break;
        }
        f.write_all(&buf[..n])?;
        h.update(&buf[..n]);
        done += n as u64;
        if total > 0 {
            progress("baixando", done as f32 / total as f32);
        }
    }
    f.sync_all()?;
    drop(f);
    let got = hex::encode(h.finalize());
    if let Some(want) = sha256 {
        if !got.eq_ignore_ascii_case(want) {
            let _ = std::fs::remove_file(&part);
            return Err(AiError::Integrity(format!("SHA-256 inesperado para {url}: {got}")));
        }
    }
    std::fs::rename(&part, dest)?;
    progress("baixando", 1.0);
    Ok(done)
}
