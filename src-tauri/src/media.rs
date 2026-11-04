use crate::bili::{BiliState, BILI_ORIGIN, BILI_REFERER};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

pub struct MediaBase(pub String);

#[tauri::command]
pub fn media_base(state: tauri::State<'_, MediaBase>) -> String {
    state.0.clone()
}

pub fn app_origin() -> &'static str {
    if cfg!(target_os = "windows") {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    }
}

pub fn is_allowed_origin(origin: &str) -> bool {
    if origin == app_origin() {
        return true;
    }
    if cfg!(debug_assertions)
        && (origin.starts_with("http://localhost:") || origin.starts_with("http://127.0.0.1:"))
    {
        return true;
    }
    false
}

pub fn parse_http_request(
    text: &str,
    token: &str,
    expected_host: &str,
) -> Result<(tauri::http::Request<Vec<u8>>, String), u16> {
    let mut lines = text.lines();
    let words: Vec<_> = lines.next().unwrap_or("").split_whitespace().collect();
    if words.len() != 3 || !matches!(words[0], "GET" | "HEAD" | "OPTIONS") || words[2] != "HTTP/1.1"
    {
        return Err(400);
    }
    let method = words[0];
    let mut host = None;
    let mut origin: Option<&str> = None;
    let mut builder = tauri::http::Request::builder().method(method);
    for line in lines {
        if line.is_empty() {
            break;
        }
        let (k, v) = line.split_once(':').ok_or(400u16)?;
        let v = v.trim();
        if k.eq_ignore_ascii_case("host") {
            if host.replace(v).is_some() {
                return Err(400);
            }
        } else if k.eq_ignore_ascii_case("origin") {
            if origin.replace(v).is_some() {
                return Err(400);
            }
        } else if k.eq_ignore_ascii_case("range") {
            builder = builder.header("Range", v);
        }
    }
    if host != Some(expected_host) {
        return Err(403);
    }
    if let Some(o) = origin {
        if !is_allowed_origin(o) {
            return Err(403);
        }
    }
    let allow_origin = origin
        .filter(|o| is_allowed_origin(o))
        .unwrap_or_else(|| app_origin())
        .to_string();
    let prefix = format!("/{token}/");
    let path = words[1].strip_prefix(&prefix).ok_or(403u16)?;
    let req = builder
        .uri(format!("biu-media://{path}"))
        .body(vec![])
        .map_err(|_| 400u16)?;
    Ok((req, allow_origin))
}

pub async fn serve_http(
    listener: tokio::net::TcpListener,
    token: String,
    state: Arc<BiliState>,
) {
    let expected_host = match listener.local_addr() {
        Ok(a) => a.to_string(),
        Err(_) => return,
    };
    while let Ok((mut socket, _)) = listener.accept().await {
        let token = token.clone();
        let state = state.clone();
        let expected_host = expected_host.clone();
        tauri::async_runtime::spawn(async move {
            let mut data = vec![];
            loop {
                let mut buf = [0u8; 1024];
                let n = match tokio::time::timeout(
                    std::time::Duration::from_secs(5),
                    socket.read(&mut buf),
                )
                .await
                {
                    Ok(Ok(n)) => n,
                    _ => return,
                };
                if n == 0 {
                    return;
                }
                data.extend_from_slice(&buf[..n]);
                if data.len() > 16384 {
                    return;
                }
                if data.windows(4).any(|w| w == b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&data);
            let (req, allow_origin) = match parse_http_request(&text, &token, &expected_host) {
                Ok(r) => r,
                Err(status) => {
                    let _ = socket
                        .write_all(
                            format!(
                                "HTTP/1.1 {status} Rejected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            )
                            .as_bytes(),
                        )
                        .await;
                    return;
                }
            };
            if req.method() == "OPTIONS" {
                let _ = socket
                    .write_all(
                        format!(
                            "HTTP/1.1 204 No Content\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nAccess-Control-Allow-Methods: GET, HEAD, OPTIONS\r\nAccess-Control-Allow-Headers: Range\r\nAccess-Control-Expose-Headers: Content-Range, Accept-Ranges, Content-Length\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await;
                return;
            }
            if let Err(e) = http_media(&mut socket, req, &allow_origin, &state).await {
                log::warn!("[media_proxy] error: {e}");
            }
        });
    }
}

pub async fn http_media(
    socket: &mut tokio::net::TcpStream,
    req: tauri::http::Request<Vec<u8>>,
    allow_origin: &str,
    state: &BiliState,
) -> Result<(), String> {
    let range = req.headers().get("range").and_then(|x| x.to_str().ok());
    let u = Url::parse(&req.uri().to_string()).map_err(|e| e.to_string())?;
    let q: HashMap<_, _> = u.query_pairs().into_owned().collect();

    if u.host_str() != Some("remote") {
        let _ = socket
            .write_all(
                format!(
                    "HTTP/1.1 404 Not Found\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await;
        return Err("unsupported media route".into());
    }

    let remote = match q.get("u") {
        Some(r) => r,
        None => {
            let _ = socket
                .write_all(
                    format!(
                        "HTTP/1.1 400 Bad Request\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await;
            return Err("missing u query param".into());
        }
    };

    let parsed = match Url::parse(remote) {
        Ok(p) => p,
        Err(_) => {
            let _ = socket
                .write_all(
                    format!(
                        "HTTP/1.1 400 Bad Request\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await;
            return Err("invalid upstream url".into());
        }
    };

    if !state.is_media_url_allowed(&parsed) {
        let _ = socket
            .write_all(
                format!(
                    "HTTP/1.1 403 Forbidden\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await;
        return Err(format!(
            "upstream host denied: {}",
            parsed.host_str().unwrap_or("")
        ));
    }

    let is_head = req.method() == "HEAD";
    let mut request = if is_head {
        state.client().head(parsed.clone())
    } else {
        state.client().get(parsed.clone())
    }
    .header("Referer", BILI_REFERER)
    .header("Origin", BILI_ORIGIN);

    if let Some(r) = range {
        request = request.header("Range", r);
    }

    let response = match request.send().await {
        Ok(r) => r,
        Err(e) => {
            let _ = socket
                .write_all(
                    format!(
                        "HTTP/1.1 502 Bad Gateway\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await;
            return Err(e.to_string());
        }
    };

    let status = response.status().as_u16();
    let reason = response.status().canonical_reason().unwrap_or("OK");
    let mut header = format!(
        "HTTP/1.1 {status} {reason}\r\nConnection: close\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nAccess-Control-Expose-Headers: Content-Range, Accept-Ranges, Content-Length\r\nCache-Control: no-store\r\n"
    );

    let mut has_accept_ranges = false;
    for key in [
        "content-length",
        "content-range",
        "content-type",
        "accept-ranges",
    ] {
        if let Some(v) = response.headers().get(key) {
            if let Ok(val_str) = v.to_str() {
                if key == "accept-ranges" {
                    has_accept_ranges = true;
                }
                header.push_str(&format!("{key}: {val_str}\r\n"));
            }
        }
    }
    if !has_accept_ranges {
        header.push_str("accept-ranges: bytes\r\n");
    }
    header.push_str("\r\n");

    socket
        .write_all(header.as_bytes())
        .await
        .map_err(|e| e.to_string())?;

    if !is_head {
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            match chunk {
                Ok(bytes) => {
                    if socket.write_all(&bytes).await.is_err() {
                        break;
                    }
                }
                Err(_) => break,
            }
        }
    }
    let _ = socket.shutdown().await;
    Ok(())
}
