use crate::bili::{BiliState, BILI_ORIGIN, BILI_REFERER};
use futures::StreamExt;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use url::Url;

/// 上游返回响应头的最长等待时间
const MEDIA_RESPONSE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(15);
/// 上游连续多久没有新数据才放弃。播放器按播放速度读取，一个请求可能持续几分钟，
/// 所以只限制空闲时间，不限制整个响应的时长。
const MEDIA_IDLE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

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

pub async fn serve_http(listener: tokio::net::TcpListener, token: String, state: Arc<BiliState>) {
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
            let _ = http_media(&mut socket, req, &allow_origin, &state).await;
        });
    }
}

pub fn parse_byte_range(range_header: Option<&str>, total_len: u64) -> Option<(u64, u64, u16)> {
    let Some(r) = range_header else {
        let end = total_len.saturating_sub(1);
        return Some((0, end, 200));
    };
    let spec = r.strip_prefix("bytes=")?;
    let (start_str, end_str) = spec.split_once('-')?;
    if total_len == 0 {
        return None;
    }
    if start_str.is_empty() {
        let suffix: u64 = end_str.parse().ok()?;
        if suffix == 0 {
            return None;
        }
        let start = total_len.saturating_sub(suffix);
        return Some((start, total_len - 1, 206));
    }
    let start: u64 = start_str.parse().ok()?;
    if start >= total_len {
        return None;
    }
    let end: u64 = if end_str.is_empty() {
        total_len - 1
    } else {
        end_str.parse::<u64>().ok()?.min(total_len - 1)
    };
    if start > end {
        return None;
    }
    Some((start, end, 206))
}

pub fn local_audio_mime(path: &std::path::Path) -> &'static str {
    match path
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or("")
        .to_ascii_lowercase()
        .as_str()
    {
        "mp3" => "audio/mpeg",
        "flac" => "audio/flac",
        "wav" => "audio/wav",
        "m4s" | "m4a" | "mp4" | "aac" => "audio/mp4",
        "ogg" => "audio/ogg",
        "wma" => "audio/x-ms-wma",
        "aiff" => "audio/aiff",
        _ => "application/octet-stream",
    }
}

pub fn normalize_media_content_type(upstream_ct: Option<&str>, path: &str) -> String {
    let raw = upstream_ct.unwrap_or("").trim();
    let media_type = raw
        .split(';')
        .next()
        .unwrap_or("")
        .trim()
        .to_ascii_lowercase();
    if !raw.is_empty()
        && !matches!(
            media_type.as_str(),
            "application/octet-stream" | "binary/octet-stream"
        )
    {
        return raw.to_string();
    }
    let guessed = local_audio_mime(std::path::Path::new(path));
    if guessed != "application/octet-stream" {
        return guessed.to_string();
    }
    if raw.is_empty() {
        "application/octet-stream".to_string()
    } else {
        raw.to_string()
    }
}

pub async fn serve_local_file(
    socket: &mut tokio::net::TcpStream,
    is_head: bool,
    range: Option<&str>,
    allow_origin: &str,
    state: &BiliState,
    raw_path: &str,
) -> Result<(), String> {
    let path = std::path::PathBuf::from(raw_path);
    if !state.is_local_file_allowed(&path) {
        let _ = socket
            .write_all(
                format!(
                    "HTTP/1.1 403 Forbidden\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await;
        return Err(format!("local file path not allowlisted: {raw_path}"));
    }

    let file = match tokio::fs::File::open(&path).await {
        Ok(f) => f,
        Err(e) => {
            let _ = socket
                .write_all(
                    format!(
                        "HTTP/1.1 404 Not Found\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await;
            return Err(e.to_string());
        }
    };

    serve_file(
        socket,
        is_head,
        range,
        allow_origin,
        file,
        local_audio_mime(&path),
    )
    .await
}

// Local allowlisting stays in serve_local_file. Cache readers supply an already
// opened file so LRU eviction cannot remove a hit between lookup and playback.
async fn serve_file(
    socket: &mut tokio::net::TcpStream,
    is_head: bool,
    range: Option<&str>,
    allow_origin: &str,
    mut file: tokio::fs::File,
    content_type: &str,
) -> Result<(), String> {
    use tokio::io::AsyncSeekExt;

    let total_len = file.metadata().await.map_err(|e| e.to_string())?.len();
    let Some((start, end, status)) = parse_byte_range(range, total_len) else {
        let _ = socket
            .write_all(
                format!(
                    "HTTP/1.1 416 Range Not Satisfiable\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nContent-Range: bytes */{total_len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                )
                .as_bytes(),
            )
            .await;
        return Ok(());
    };

    let count = if total_len == 0 { 0 } else { end - start + 1 };
    let reason = if status == 206 {
        "Partial Content"
    } else {
        "OK"
    };
    let content_range = if status == 206 {
        format!("content-range: bytes {start}-{end}/{total_len}\r\n")
    } else {
        String::new()
    };

    let header = format!(
        "HTTP/1.1 {status} {reason}\r\nConnection: close\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nAccess-Control-Expose-Headers: Content-Range, Accept-Ranges, Content-Length\r\nCache-Control: no-store\r\ncontent-type: {content_type}\r\ncontent-length: {count}\r\n{content_range}accept-ranges: bytes\r\n\r\n"
    );

    socket
        .write_all(header.as_bytes())
        .await
        .map_err(|e| e.to_string())?;

    if !is_head && count > 0 {
        file.seek(std::io::SeekFrom::Start(start))
            .await
            .map_err(|e| e.to_string())?;
        let mut take = file.take(count);
        let _ = tokio::io::copy(&mut take, socket).await;
    }
    let _ = socket.shutdown().await;
    Ok(())
}

pub async fn http_media(
    socket: &mut tokio::net::TcpStream,
    req: tauri::http::Request<Vec<u8>>,
    allow_origin: &str,
    state: &BiliState,
) -> Result<(), String> {
    let range = req
        .headers()
        .get("range")
        .and_then(|x| x.to_str().ok())
        .map(|s| s.to_string());
    let u = Url::parse(&req.uri().to_string()).map_err(|e| e.to_string())?;
    let q: HashMap<_, _> = u.query_pairs().into_owned().collect();

    if u.host_str() == Some("kuromoji") {
        let name = q.get("name").map(|s| s.as_str()).unwrap_or("");
        let app_data_dir = state
            .cookie_path
            .parent()
            .unwrap_or_else(|| std::path::Path::new("."));
        match crate::lyrics::read_kuromoji_dict_file(app_data_dir, name) {
            Ok(bytes) => {
                let header = format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n",
                    bytes.len()
                );
                let _ = socket.write_all(header.as_bytes()).await;
                if req.method() != "HEAD" {
                    let _ = socket.write_all(&bytes).await;
                }
                let _ = socket.shutdown().await;
                return Ok(());
            }
            Err(status) => {
                let reason = if status == 403 {
                    "Forbidden"
                } else {
                    "Not Found"
                };
                let _ = socket
                    .write_all(
                        format!(
                            "HTTP/1.1 {status} {reason}\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                        )
                        .as_bytes(),
                    )
                    .await;
                return Err(format!("kuromoji dict {status}"));
            }
        }
    }

    if u.host_str() == Some("local") {
        let Some(p) = q.get("p") else {
            let _ = socket
                .write_all(
                    format!(
                        "HTTP/1.1 400 Bad Request\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await;
            return Err("missing p query param".into());
        };
        return serve_local_file(
            socket,
            req.method() == "HEAD",
            range.as_deref(),
            allow_origin,
            state,
            p,
        )
        .await;
    }

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
        let host = parsed.host_str().unwrap_or("");
        log::warn!(
            "[media_proxy] {} {}{} denied by allowlist",
            req.method(),
            host,
            parsed.path()
        );
        return Err(format!("upstream host denied: {host}"));
    }

    let is_head = req.method() == "HEAD";
    if let Some((cache, key)) = state
        .media_cache
        .as_ref()
        .filter(|_| {
            parsed.host_str().is_some_and(|host| {
                state
                    .allowed_media_hosts
                    .read()
                    .unwrap()
                    .contains(&host.to_ascii_lowercase())
            })
        })
        .and_then(|cache| crate::media_cache::MediaCache::key(&parsed).map(|key| (cache, key)))
    {
        if let Some(entry) = cache.lookup(&key).await {
            let content_type =
                normalize_media_content_type(Some(&entry.content_type), parsed.path());
            return serve_file(
                socket,
                is_head,
                range.as_deref(),
                allow_origin,
                entry.file,
                &content_type,
            )
            .await;
        }
        if req.method() == "GET" {
            log::info!("[media_cache] miss {} 0", &key[..8]);
            cache.fill(key, parsed.clone(), state.client(), MEDIA_IDLE_TIMEOUT);
        }
    }
    let method_str = req.method().as_str().to_string();
    let range_owned = range.map(|r| r.to_string());
    let build_upstream_req = || {
        let mut request = if is_head {
            state.client().head(parsed.clone())
        } else {
            state.client().get(parsed.clone())
        }
        .header("Referer", BILI_REFERER)
        .header("Origin", BILI_ORIGIN);

        if let Some(r) = &range_owned {
            request = request.header("Range", r);
        }
        request
    };

    // 只给“等响应头”计时；响应体的读取在下面按空闲时间计时
    let send_upstream = || async {
        match tokio::time::timeout(MEDIA_RESPONSE_TIMEOUT, build_upstream_req().send()).await {
            Ok(Ok(r)) => Ok(r),
            Ok(Err(e)) => Err((
                e.is_connect() || e.is_timeout(),
                crate::bili::sanitize_reqwest_error(&e),
            )),
            Err(_) => Err((true, "timed out waiting for the response".to_string())),
        }
    };

    let response = match send_upstream().await {
        Ok(r) => r,
        Err((true, first_error)) => {
            log::warn!(
                "[media_proxy] {} {}{} retrying once after error: {}",
                method_str,
                parsed.host_str().unwrap_or(""),
                parsed.path(),
                first_error
            );
            match send_upstream().await {
                Ok(r) => r,
                Err((_, msg)) => {
                    log::warn!(
                        "[media_proxy] {} {}{} failed: {}",
                        method_str,
                        parsed.host_str().unwrap_or(""),
                        parsed.path(),
                        msg
                    );
                    let _ = socket
                        .write_all(
                            format!(
                                "HTTP/1.1 502 Bad Gateway\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                            )
                            .as_bytes(),
                        )
                        .await;
                    return Err(msg);
                }
            }
        }
        Err((false, msg)) => {
            log::warn!(
                "[media_proxy] {} {}{} failed: {}",
                method_str,
                parsed.host_str().unwrap_or(""),
                parsed.path(),
                msg
            );
            let _ = socket
                .write_all(
                    format!(
                        "HTTP/1.1 502 Bad Gateway\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n"
                    )
                    .as_bytes(),
                )
                .await;
            return Err(msg);
        }
    };

    let status = response.status().as_u16();
    let reason = response.status().canonical_reason().unwrap_or("OK");
    let mut header = format!(
        "HTTP/1.1 {status} {reason}\r\nConnection: close\r\nAccess-Control-Allow-Origin: {allow_origin}\r\nAccess-Control-Expose-Headers: Content-Range, Accept-Ranges, Content-Length\r\nCache-Control: no-store\r\n"
    );

    let mut has_accept_ranges = false;
    for key in ["content-length", "content-range", "accept-ranges"] {
        if let Some(v) = response.headers().get(key) {
            if let Ok(val_str) = v.to_str() {
                if key == "accept-ranges" {
                    has_accept_ranges = true;
                }
                header.push_str(&format!("{key}: {val_str}\r\n"));
            }
        }
    }
    let upstream_ct = response
        .headers()
        .get("content-type")
        .and_then(|v| v.to_str().ok());
    let content_type = normalize_media_content_type(upstream_ct, parsed.path());
    header.push_str(&format!("content-type: {content_type}\r\n"));
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
        let mut sent: u64 = 0;
        loop {
            match tokio::time::timeout(MEDIA_IDLE_TIMEOUT, stream.next()).await {
                Ok(Some(Ok(bytes))) => {
                    // 播放器关闭连接（切歌、跳转）是正常结束
                    if socket.write_all(&bytes).await.is_err() {
                        break;
                    }
                    sent += bytes.len() as u64;
                }
                Ok(None) => break,
                Ok(Some(Err(e))) => {
                    log::warn!(
                        "[media_proxy] {}{} body interrupted after {sent} bytes: {}",
                        parsed.host_str().unwrap_or(""),
                        parsed.path(),
                        crate::bili::sanitize_reqwest_error(&e)
                    );
                    break;
                }
                Err(_) => {
                    log::warn!(
                        "[media_proxy] {}{} body idle for {}s after {sent} bytes",
                        parsed.host_str().unwrap_or(""),
                        parsed.path(),
                        MEDIA_IDLE_TIMEOUT.as_secs()
                    );
                    break;
                }
            }
        }
    }
    let _ = socket.shutdown().await;
    Ok(())
}
