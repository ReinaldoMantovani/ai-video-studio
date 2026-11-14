//! Servidor local de mídia (preview no Linux): acesso restrito e `Range`.

use avs_app::media_server::MediaServer;
use std::io::{Read, Write};
use std::net::TcpStream;

fn get(base: &str, path_part: &str, range: Option<&str>) -> (u16, String, Vec<u8>) {
    let url = format!("{base}{path_part}");
    let rest = url.strip_prefix("http://").unwrap();
    let (host, path) = rest.split_at(rest.find('/').unwrap());
    let mut s = TcpStream::connect(host).unwrap();
    let r = range.map(|r| format!("Range: {r}\r\n")).unwrap_or_default();
    write!(s, "GET {path} HTTP/1.1\r\nHost: {host}\r\n{r}Connection: close\r\n\r\n").unwrap();
    let mut buf = vec![];
    s.read_to_end(&mut buf).unwrap();
    let split = buf.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
    let head = String::from_utf8_lossy(&buf[..split]).to_string();
    let code = head.split_whitespace().nth(1).unwrap().parse().unwrap();
    (code, head, buf[split + 4..].to_vec())
}

fn enc(p: &std::path::Path) -> String {
    p.to_string_lossy().bytes().map(|b| if b.is_ascii_alphanumeric() || b"-_.".contains(&b) { (b as char).to_string() } else { format!("%{b:02X}") }).collect()
}

#[test]
fn serves_ranges_only_inside_roots_with_token() {
    let tmp = tempfile::tempdir().unwrap();
    let projects = tmp.path().join("lib/projects");
    let renders = projects.join("p1/renders");
    std::fs::create_dir_all(&renders).unwrap();
    let file = renders.join("preview r1 ção.mp4");
    let data: Vec<u8> = (0..100_000u32).map(|i| (i % 251) as u8).collect();
    std::fs::write(&file, &data).unwrap();
    std::fs::write(projects.join("p1/project.avsproj"), b"{}").unwrap();
    std::fs::write(tmp.path().join("segredo.mp4"), b"x").unwrap();

    let srv = MediaServer::start(vec![projects.clone()]).unwrap();
    let base = srv.base_url();
    assert!(base.starts_with("http://127.0.0.1:"));

    // Arquivo inteiro e trechos (o <video> pede Range).
    let (c, h, b) = get(&base, &enc(&file), None);
    assert_eq!(c, 200, "{h}");
    assert!(h.contains("Content-Type: video/mp4") && h.contains("Accept-Ranges: bytes"));
    assert_eq!(b, data);
    let (c, h, b) = get(&base, &enc(&file), Some("bytes=1000-1999"));
    assert_eq!(c, 206);
    assert!(h.contains("Content-Range: bytes 1000-1999/100000"), "{h}");
    assert_eq!(b, data[1000..2000]);
    let (c, _, b) = get(&base, &enc(&file), Some("bytes=99990-"));
    assert_eq!((c, b.len()), (206, 10));
    let (c, _, b) = get(&base, &enc(&file), Some("bytes=-5"));
    assert_eq!((c, b), (206, data[99_995..].to_vec()));
    assert_eq!(get(&base, &enc(&file), Some("bytes=200000-")).0, 416);

    // Token errado, fora das raízes, travessia, extensão não-mídia: recusados.
    let wrong = base.replace(&base[base.len() - 33..base.len() - 1], &"0".repeat(32));
    assert_eq!(get(&wrong, &enc(&file), None).0, 403);
    assert_eq!(get(&base, &enc(&tmp.path().join("segredo.mp4")), None).0, 403);
    let trav = format!("{}%2F..%2F..%2Fsegredo.mp4", enc(&renders));
    assert_eq!(get(&base, &trav, None).0, 403);
    assert_eq!(get(&base, &enc(&projects.join("p1/project.avsproj")), None).0, 403);
    assert_eq!(get(&base, &enc(&renders.join("nao-existe.mp4")), None).0, 404);
    // Link simbólico apontando para fora: recusado (o caminho real é conferido).
    #[cfg(unix)]
    {
        let link = renders.join("link.mp4");
        std::os::unix::fs::symlink(tmp.path().join("segredo.mp4"), &link).unwrap();
        assert_eq!(get(&base, &enc(&link), None).0, 403);
    }
}
