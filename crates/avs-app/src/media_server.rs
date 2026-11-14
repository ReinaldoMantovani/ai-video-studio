//! Servidor local de mídia para o `<video>` do preview no Linux.
//!
//! O player de mídia do WebKitGTK não carrega fontes do protocolo `asset://`
//! do Tauri (o mesmo arquivo toca via `file://` no MiniBrowser), então o
//! preview nunca reproduzia no Linux. Este servidor resolve isso com o mínimo:
//! - escuta só em 127.0.0.1, porta aleatória;
//! - exige um token aleatório de 128 bits no caminho (comparação em tempo constante);
//! - só leitura (GET/HEAD), só extensões de mídia, só arquivos dentro das
//!   raízes permitidas (pastas de projetos da biblioteca), após canonicalizar
//!   (sem `..`, sem links para fora);
//! - suporta `Range` (o `<video>` pede trechos).

use std::io::{BufRead, BufReader, Read, Seek, SeekFrom, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

const MEDIA_EXT: &[&str] = &["mp4", "m4v", "mov", "webm", "m4a", "mp3", "wav", "aac", "ogg", "opus"];

#[derive(Debug, Clone)]
pub struct MediaServer {
    pub port: u16,
    token: String,
}

impl MediaServer {
    /// Inicia o servidor em segundo plano. `roots` são as únicas pastas servidas.
    pub fn start(roots: Vec<PathBuf>) -> std::io::Result<MediaServer> {
        let l = TcpListener::bind("127.0.0.1:0")?;
        let port = l.local_addr()?.port();
        let token = random_token();
        let roots: Arc<Vec<PathBuf>> = Arc::new(roots.into_iter().filter_map(|r| std::fs::create_dir_all(&r).ok().and(r.canonicalize().ok())).collect());
        let tk = token.clone();
        std::thread::Builder::new().name("avs-media-server".into()).spawn(move || {
            for s in l.incoming().flatten() {
                let (roots, tk) = (roots.clone(), tk.clone());
                let _ = std::thread::Builder::new().name("avs-media-conn".into()).spawn(move || {
                    let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
                    let _ = handle(s, &roots, &tk);
                });
            }
        })?;
        Ok(MediaServer { port, token })
    }

    /// Prefixo das URLs: `http://127.0.0.1:<porta>/<token>/` + caminho absoluto codificado.
    pub fn base_url(&self) -> String {
        format!("http://127.0.0.1:{}/{}/", self.port, self.token)
    }
}

fn random_token() -> String {
    let mut b = [0u8; 16];
    if let Ok(mut f) = std::fs::File::open("/dev/urandom") {
        let _ = f.read_exact(&mut b);
    } else {
        // Sem /dev/urandom (não-Unix): UUID v4 (aleatório do SO).
        b.copy_from_slice(uuid::Uuid::new_v4().as_bytes());
    }
    b.iter().map(|x| format!("{x:02x}")).collect()
}

fn ct_eq(a: &str, b: &str) -> bool {
    a.len() == b.len() && a.bytes().zip(b.bytes()).fold(0u8, |acc, (x, y)| acc | (x ^ y)) == 0
}

fn percent_decode(s: &str) -> Option<String> {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%' {
            let h = std::str::from_utf8(b.get(i + 1..i + 3)?).ok()?;
            out.push(u8::from_str_radix(h, 16).ok()?);
            i += 3;
        } else {
            out.push(b[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

fn content_type(p: &Path) -> &'static str {
    match p.extension().and_then(|e| e.to_str()).map(|e| e.to_ascii_lowercase()).as_deref() {
        Some("mp4") | Some("m4v") => "video/mp4",
        Some("mov") => "video/quicktime",
        Some("webm") => "video/webm",
        Some("m4a") | Some("aac") => "audio/mp4",
        Some("mp3") => "audio/mpeg",
        Some("wav") => "audio/wav",
        Some("ogg") | Some("opus") => "audio/ogg",
        _ => "application/octet-stream",
    }
}

fn reply(s: &mut TcpStream, code: u16, reason: &str) -> std::io::Result<()> {
    write!(s, "HTTP/1.1 {code} {reason}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n")
}

/// Resolve o caminho pedido e confere token, raízes e extensão.
pub(crate) fn resolve(target: &str, roots: &[PathBuf], token: &str) -> Result<PathBuf, u16> {
    let rest = target.strip_prefix('/').ok_or(400u16)?;
    let (tk, enc) = rest.split_once('/').ok_or(404u16)?;
    if !ct_eq(tk, token) {
        return Err(403);
    }
    let enc = enc.split('?').next().unwrap_or("");
    let raw = percent_decode(enc).ok_or(400u16)?;
    let p = PathBuf::from(&raw);
    if !p.is_absolute() || p.components().any(|c| matches!(c, std::path::Component::ParentDir)) {
        return Err(403);
    }
    let ext_ok = p.extension().and_then(|e| e.to_str()).map(|e| MEDIA_EXT.contains(&e.to_ascii_lowercase().as_str())).unwrap_or(false);
    if !ext_ok {
        return Err(403);
    }
    let canon = p.canonicalize().map_err(|_| 404u16)?;
    if !roots.iter().any(|r| canon.starts_with(r)) {
        return Err(403);
    }
    Ok(canon)
}

fn handle(mut s: TcpStream, roots: &[PathBuf], token: &str) -> std::io::Result<()> {
    let mut r = BufReader::new(s.try_clone()?);
    let mut line = String::new();
    r.read_line(&mut line)?;
    let mut parts = line.split_whitespace();
    let (method, target) = (parts.next().unwrap_or(""), parts.next().unwrap_or(""));
    let mut range: Option<String> = None;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            if k.trim().eq_ignore_ascii_case("range") {
                range = Some(v.trim().to_string());
            }
        }
    }
    if method != "GET" && method != "HEAD" {
        return reply(&mut s, 405, "Method Not Allowed");
    }
    let path = match resolve(target, roots, token) {
        Ok(p) => p,
        Err(403) => return reply(&mut s, 403, "Forbidden"),
        Err(404) => return reply(&mut s, 404, "Not Found"),
        Err(_) => return reply(&mut s, 400, "Bad Request"),
    };
    let mut f = std::fs::File::open(&path)?;
    let len = f.metadata()?.len();
    let (start, end, partial) = match range.as_deref().and_then(|v| v.strip_prefix("bytes=")).and_then(|v| v.split(',').next()).and_then(|v| v.split_once('-'))
    {
        Some((a, b)) => {
            let (start, end) = if a.is_empty() {
                // "bytes=-N": últimos N bytes
                let n: u64 = b.parse().unwrap_or(0).min(len);
                (len - n, len.saturating_sub(1))
            } else {
                let a: u64 = a.parse().unwrap_or(0);
                let b: u64 = if b.is_empty() { len.saturating_sub(1) } else { b.parse().unwrap_or(len.saturating_sub(1)).min(len.saturating_sub(1)) };
                (a, b)
            };
            if len == 0 || start > end || start >= len {
                return write!(s, "HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n");
            }
            (start, end, true)
        }
        None => (0, len.saturating_sub(1), false),
    };
    let n = if len == 0 { 0 } else { end - start + 1 };
    let head =
        if partial { format!("HTTP/1.1 206 Partial Content\r\nContent-Range: bytes {start}-{end}/{len}\r\n") } else { "HTTP/1.1 200 OK\r\n".to_string() };
    write!(
        s,
        "{head}Content-Type: {}\r\nContent-Length: {n}\r\nAccept-Ranges: bytes\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
        content_type(&path)
    )?;
    if method == "HEAD" || n == 0 {
        return Ok(());
    }
    f.seek(SeekFrom::Start(start))?;
    let mut left = n;
    let mut buf = vec![0u8; 256 * 1024];
    while left > 0 {
        let want = left.min(buf.len() as u64) as usize;
        let k = f.read(&mut buf[..want])?;
        if k == 0 {
            break;
        }
        s.write_all(&buf[..k])?;
        left -= k as u64;
    }
    Ok(())
}
