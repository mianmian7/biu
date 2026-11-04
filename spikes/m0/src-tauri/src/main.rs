#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]
use serde_json::{json, Value};
use sha2::Digest;
use std::{
    fs::{self, OpenOptions},
    io::{Read, Seek, SeekFrom, Write},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    time::Instant,
};
use tauri::{Emitter, Manager};

static LOG_LOCK: Mutex<()> = Mutex::new(());
fn app_origin() -> &'static str {
    if cfg!(target_os = "windows") {
        "http://tauri.localhost"
    } else {
        "tauri://localhost"
    }
}
fn fixture_enabled() -> bool {
    std::env::var("BIU_SPIKE_FIXTURES").as_deref() == Ok("1")
}
fn fixture_upstream_allowed(u: &url::Url, enabled: bool) -> bool {
    enabled
        && u.scheme() == "http"
        && u.host_str() == Some("127.0.0.1")
        && u.port() == Some(18765)
        && u.username().is_empty()
        && u.password().is_none()
        && ["/aac.m4a", "/tone.flac", "/expired"].contains(&u.path())
}
fn record(value: Value) {
    let _guard = LOG_LOCK.lock().unwrap();
    let path = std::env::var("BIU_LOG").unwrap_or_else(|_| "spike.jsonl".into());
    if let Ok(mut f) = OpenOptions::new().create(true).append(true).open(path) {
        let _ = writeln!(f, "{}", value);
    }
}
fn fixture(name: &str) -> Result<std::path::PathBuf, String> {
    if !fixture_enabled() {
        return Err(
            "spike fixture mode disabled; set BIU_SPIKE_FIXTURES=1 for M0 tests only".into(),
        );
    }
    if ![
        "aac.m4a",
        "tone.flac",
        "flac-1.m4a",
        "flac-2.m4a",
        "flac-3.m4a",
        "dolby-1.m4a",
        "dolby-2.m4a",
        "dolby-3.m4a",
    ]
    .contains(&name)
    {
        return Err("fixture not allowed".into());
    }
    Ok(
        std::path::PathBuf::from(std::env::var("BIU_FIXTURES").map_err(|e| e.to_string())?)
            .join(name),
    )
}
// WebView2 opens media with `bytes=0-`; a custom protocol response must stay bounded.
fn capped_remote_range(range: Option<&str>) -> String {
    const CHUNK: u64 = 1024 * 1024;
    let Some(value) = range else {
        return format!("bytes=0-{}", CHUNK - 1);
    };
    let parsed = value
        .strip_prefix("bytes=")
        .and_then(|spec| spec.split_once('-'))
        .and_then(|(start, end)| Some((start.parse::<u64>().ok()?, end)));
    let Some((start, end)) = parsed else {
        return value.to_string();
    };
    let last = start + CHUNK - 1;
    format!(
        "bytes={start}-{}",
        end.parse::<u64>().map_or(last, |end| end.min(last))
    )
}
fn bounds(range: Option<&str>, len: u64) -> Result<(u64, u64), String> {
    // A bounded 1 MiB response: custom protocols expose Response<Vec<u8>>, not a stream.
    // A server may return a smaller satisfiable range than requested (RFC 9110).
    let (start, requested) = match range {
        Some(s) => {
            let r = http_range::HttpRange::parse(s, len).map_err(|e| format!("{e:?}"))?;
            if r.len() != 1 {
                return Err("multipart unsupported in spike".into());
            }
            (r[0].start, r[0].length)
        }
        None => (0, len),
    };
    Ok((start, requested.min(1024 * 1024)))
}
async fn media(
    req: tauri::http::Request<Vec<u8>>,
    client: reqwest::Client,
) -> Result<tauri::http::Response<Vec<u8>>, String> {
    let now = Instant::now();
    let url = url::Url::parse(&req.uri().to_string()).map_err(|e| e.to_string())?;
    let q: std::collections::HashMap<_, _> = url.query_pairs().into_owned().collect();
    let round = q.get("round").and_then(|s| s.parse::<u64>().ok());
    let range = req.headers().get("range").and_then(|v| v.to_str().ok());
    let mut response;
    if url.host_str() == Some("local") || url.path().contains("/local") {
        let p = fixture(q.get("name").map(|x| x.as_str()).unwrap_or("aac.m4a"))?;
        let mut f = fs::File::open(&p).map_err(|e| e.to_string())?;
        let len = f.metadata().map_err(|e| e.to_string())?.len();
        let (start, count) = match bounds(range, len) {
            Ok(b) => b,
            Err(_) => {
                return Ok(tauri::http::Response::builder()
                    .status(416)
                    .header("Content-Range", format!("bytes */{len}"))
                    .body(vec![])
                    .unwrap())
            }
        };
        f.seek(SeekFrom::Start(start)).map_err(|e| e.to_string())?;
        let mut body = vec![0; count as usize];
        f.read_exact(&mut body).map_err(|e| e.to_string())?;
        response = tauri::http::Response::builder()
            .status(206)
            .header(
                "Content-Range",
                format!("bytes {}-{}/{}", start, start + count - 1, len),
            )
            .header("Accept-Ranges", "bytes")
            .header(
                "Content-Type",
                if p.extension().unwrap() == "flac" {
                    "audio/flac"
                } else {
                    "audio/mp4"
                },
            )
            .header("Content-Length", count)
            .body(body)
            .unwrap();
    } else {
        let remote = q.get("u").ok_or("missing u")?;
        let u = url::Url::parse(remote).map_err(|e| e.to_string())?;
        let fixture_server = fixture_upstream_allowed(&u, fixture_enabled());
        // No suffix-based CDN allowlist in the spike. M2 must pin hosts returned by playurl.
        if !fixture_server {
            return Err("remote denied".into());
        }
        let r = client
            .get(remote)
            .header("Range", capped_remote_range(range))
            .header("Referer", "https://www.bilibili.com/")
            .send()
            .await
            .map_err(|e| e.to_string())?;
        // Test-only address refresh exercises re-request with the same Range.
        let r = if fixture_server && r.status().as_u16() == 403 {
            record(json!({"event":"url_refresh", "range":range}));
            client
                .get("http://127.0.0.1:18765/aac.m4a")
                .header("Referer", "https://www.bilibili.com/")
                .header("Range", capped_remote_range(range))
                .send()
                .await
                .map_err(|e| e.to_string())?
        } else {
            r
        };
        let status = r.status().as_u16();
        let headers = r.headers().clone();
        // Limit remote response too; refuse a CDN that ignores ranges instead of buffering a whole file.
        if r.content_length().unwrap_or(u64::MAX) > 1024 * 1024 {
            return Err("upstream range too large".into());
        }
        let body = r.bytes().await.map_err(|e| e.to_string())?.to_vec();
        let mut builder = tauri::http::Response::builder().status(status);
        for key in [
            "content-type",
            "content-range",
            "accept-ranges",
            "content-length",
        ] {
            if let Some(v) = headers.get(key) {
                builder = builder.header(key, v);
            }
        }
        response = builder.body(body).unwrap();
    }
    response
        .headers_mut()
        .insert("Access-Control-Allow-Origin", app_origin().parse().unwrap());
    response.headers_mut().insert(
        "Access-Control-Expose-Headers",
        "Content-Range,Accept-Ranges".parse().unwrap(),
    );
    record(
        json!({"event":"range", "round":round, "range":range, "status":response.status().as_u16(), "bytes":response.body().len(), "ms":now.elapsed().as_secs_f64()*1000.0}),
    );
    Ok(response)
}
struct MediaBase(String);
#[tauri::command]
fn media_base(state: tauri::State<MediaBase>) -> String {
    state.0.clone()
}
fn parse_http_request(
    text: &str,
    token: &str,
    expected_host: &str,
) -> Result<tauri::http::Request<Vec<u8>>, u16> {
    let mut lines = text.lines();
    let words: Vec<_> = lines.next().unwrap_or("").split_whitespace().collect();
    if words.len() != 3 || words[0] != "GET" || words[2] != "HTTP/1.1" {
        return Err(400);
    }
    let mut host = None;
    let mut origin = None;
    let mut builder = tauri::http::Request::builder();
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
    // Native media backends may omit Origin. Explicit foreign origins are refused;
    // every response advertises only the app origin, never '*' or an echoed origin.
    if origin.is_some_and(|o| o != app_origin()) {
        return Err(403);
    }
    let prefix = format!("/{token}/");
    let path = words[1].strip_prefix(&prefix).ok_or(403u16)?;
    builder
        .uri(format!("biu-media://{path}"))
        .body(vec![])
        .map_err(|_| 400)
}
async fn serve_http(listener: tokio::net::TcpListener, token: String, client: reqwest::Client) {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    let expected_host = match listener.local_addr() {
        Ok(a) => a.to_string(),
        Err(_) => return,
    };
    while let Ok((mut socket, _)) = listener.accept().await {
        let token = token.clone();
        let client = client.clone();
        let expected_host = expected_host.clone();
        tauri::async_runtime::spawn(async move {
            let mut data = vec![];
            loop {
                let mut buf = [0; 1024];
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
                if data.ends_with(b"\r\n\r\n") {
                    break;
                }
            }
            let text = String::from_utf8_lossy(&data);
            let req = match parse_http_request(&text, &token, &expected_host) {
                Ok(r) => r,
                Err(status) => {
                    let _ = socket.write_all(format!("HTTP/1.1 {status} Rejected\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await;
                    record(json!({"event":"http_rejected","status":status}));
                    return;
                }
            };
            if let Err(e) = http_media(&mut socket, req, client).await {
                record(json!({"event":"http_error","message":e}));
            }
        });
    }
}
async fn http_media(
    socket: &mut tokio::net::TcpStream,
    req: tauri::http::Request<Vec<u8>>,
    client: reqwest::Client,
) -> Result<(), String> {
    use futures::StreamExt;
    use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
    let range = req.headers().get("range").and_then(|x| x.to_str().ok());
    let u = url::Url::parse(&req.uri().to_string()).map_err(|e| e.to_string())?;
    let q: std::collections::HashMap<_, _> = u.query_pairs().into_owned().collect();
    let now = Instant::now();
    let round = q.get("round").and_then(|s| s.parse::<u64>().ok());
    let origin = app_origin();
    let mut transferred = 0u64;
    if u.host_str() == Some("local") {
        let p = fixture(q.get("name").map(|s| s.as_str()).unwrap_or("aac.m4a"))?;
        let mut f = tokio::fs::File::open(&p).await.map_err(|e| e.to_string())?;
        let len = f.metadata().await.map_err(|e| e.to_string())?.len();
        let (start, count, status) = if let Some(r) = range {
            match http_range::HttpRange::parse(r, len) {
                Ok(v) if v.len() == 1 => (v[0].start, v[0].length, 206),
                _ => {
                    socket.write_all(format!("HTTP/1.1 416 Range Not Satisfiable\r\nContent-Range: bytes */{len}\r\nContent-Length: 0\r\nConnection: close\r\n\r\n").as_bytes()).await.map_err(|e|e.to_string())?;
                    return Ok(());
                }
            }
        } else {
            (0, len, 200)
        };
        let content_type = if p.extension().unwrap() == "flac" {
            "audio/flac"
        } else {
            "audio/mp4"
        };
        let cr = if status == 206 {
            format!(
                "Content-Range: bytes {}-{}/{}\r\n",
                start,
                start + count - 1,
                len
            )
        } else {
            String::new()
        };
        socket.write_all(format!("HTTP/1.1 {status} OK\r\nContent-Type: {content_type}\r\nContent-Length: {count}\r\n{cr}Accept-Ranges: bytes\r\nAccess-Control-Allow-Origin: {origin}\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n").as_bytes()).await.map_err(|e|e.to_string())?;
        f.seek(SeekFrom::Start(start))
            .await
            .map_err(|e| e.to_string())?;
        let result = tokio::io::copy(&mut f.take(count), socket).await;
        if let Ok(n) = result {
            transferred = n;
        }
        record(
            json!({"event":"http_range","round":round,"range":range,"status":status,"bytes":transferred,"ms":now.elapsed().as_secs_f64()*1000.0}),
        );
    } else {
        let remote = q.get("u").ok_or("missing url")?;
        let parsed = url::Url::parse(remote).map_err(|e| e.to_string())?;
        // This executable's HTTP fallback deliberately supports only the loopback fixture origin.
        // Production CDN policy is not implemented here.
        if !fixture_upstream_allowed(&parsed, fixture_enabled()) {
            return Err("spike HTTP upstream denied".into());
        }
        let mut request = client
            .get(remote)
            .header("Referer", "https://www.bilibili.com/");
        if let Some(r) = range {
            request = request.header("Range", r);
        }
        let mut response = request.send().await.map_err(|e| e.to_string())?;
        if fixture_enabled() && response.status().as_u16() == 403 {
            record(json!({"event":"url_refresh","range":range}));
            let mut request = client
                .get("http://127.0.0.1:18765/aac.m4a")
                .header("Referer", "https://www.bilibili.com/");
            if let Some(r) = range {
                request = request.header("Range", r);
            }
            response = request.send().await.map_err(|e| e.to_string())?;
        }
        let status = response.status().as_u16();
        let mut header = format!(
            "HTTP/1.1 {status} OK\r\nConnection: close\r\nAccess-Control-Allow-Origin: {origin}\r\nCache-Control: no-store\r\n"
        );
        for key in [
            "content-length",
            "content-range",
            "content-type",
            "accept-ranges",
        ] {
            if let Some(v) = response.headers().get(key) {
                header.push_str(&format!("{key}: {}\r\n", v.to_str().unwrap_or("")));
            }
        }
        header.push_str("\r\n");
        socket
            .write_all(header.as_bytes())
            .await
            .map_err(|e| e.to_string())?;
        let mut stream = response.bytes_stream();
        while let Some(Ok(bytes)) = stream.next().await {
            if socket.write_all(&bytes).await.is_err() {
                break;
            }
            transferred += bytes.len() as u64;
        }
        record(
            json!({"event":"http_range","round":round,"range":range,"status":status,"bytes":transferred,"ms":now.elapsed().as_secs_f64()*1000.0}),
        );
    }
    Ok(())
}
#[tauri::command]
fn report(event: Value) {
    record(event);
}
#[tauri::command]
fn finish(app: tauri::AppHandle) {
    app.exit(0);
}
#[tauri::command]
fn mode() -> Value {
    json!({"mode": std::env::var("BIU_MODE").unwrap_or_else(|_| "interactive".into()), "seconds": std::env::var("BIU_SOAK_SECONDS").ok().and_then(|s| s.parse::<u64>().ok()).unwrap_or(1800), "silent": std::env::var_os("BIU_SILENT").is_some()})
}
struct LyricsState {
    locked: Arc<AtomicBool>,
    hovered: Arc<AtomicBool>,
}
fn apply_lyrics_state(app: &tauri::AppHandle, locked: bool) -> Result<(), String> {
    let state = app.state::<LyricsState>();
    state.locked.store(locked, Ordering::Relaxed);
    state.hovered.store(false, Ordering::Relaxed);
    if let Some(w) = app.get_webview_window("lyrics") {
        w.set_ignore_cursor_events(locked)
            .map_err(|e| e.to_string())?;
        let label = if locked { "locked" } else { "unlocked" };
        let _ = w.emit("lyrics-state", json!({"state": label}));
        record(json!({"event":"lyrics_state","state": label}));
    }
    Ok(())
}
#[tauri::command]
fn lyrics_set_state(app: tauri::AppHandle, state: Option<String>) -> Result<String, String> {
    let current = app.state::<LyricsState>().locked.load(Ordering::Relaxed);
    let next = match state.as_deref() {
        Some("locked") => true,
        Some("unlocked") => false,
        Some("toggle") | None => !current,
        Some(_) => return Err("invalid lyrics state".into()),
    };
    apply_lyrics_state(&app, next)?;
    Ok(if next { "locked" } else { "unlocked" }.into())
}
// Must stay async: a sync command runs on the main thread, and building a window there deadlocks on WebView2.
#[tauri::command]
async fn lyrics(app: tauri::AppHandle) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("lyrics") {
        let _ = w.show();
        return Ok(());
    }
    let w = tauri::WebviewWindowBuilder::new(
        &app,
        "lyrics",
        tauri::WebviewUrl::App("lyrics.html".into()),
    )
    .title("M0 Lyrics")
    .inner_size(700., 100.)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .build()
    .map_err(|e| e.to_string())?;
    let initial_locked = std::env::var("BIU_MODE")
        .map(|m| m != "interactive")
        .unwrap_or(false);
    let ls = app.state::<LyricsState>();
    ls.locked.store(initial_locked, Ordering::Relaxed);
    ls.hovered.store(false, Ordering::Relaxed);
    w.set_ignore_cursor_events(initial_locked)
        .map_err(|e| e.to_string())?;
    let initial_label = if initial_locked { "locked" } else { "unlocked" };
    let _ = w.emit("lyrics-state", json!({"state": initial_label}));
    record(json!({"event":"lyrics_state","state": initial_label}));
    let alive = Arc::new(AtomicBool::new(true));
    let destroyed = alive.clone();
    w.on_window_event(move |event| {
        if matches!(event, tauri::WindowEvent::Destroyed) {
            destroyed.store(false, Ordering::Relaxed);
        }
    });
    let locked = ls.locked.clone();
    let hovered = ls.hovered.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let mut last_heartbeat = 0;
        while alive.load(Ordering::Relaxed) {
            let seconds = started.elapsed().as_secs();
            if seconds >= last_heartbeat + 60 {
                record(json!({"event":"lyrics_poll_alive","seconds":seconds}));
                last_heartbeat = seconds;
            }
            if locked.load(Ordering::Relaxed) && w.is_visible().unwrap_or(false) {
                if let (Ok(cursor), Ok(pos)) = (w.cursor_position(), w.outer_position()) {
                    // cursor and window positions are physical pixels; the region is logical.
                    let scale = w.scale_factor().unwrap_or(1.0);
                    let hit = cursor.x >= pos.x as f64
                        && cursor.x < pos.x as f64 + 100. * scale
                        && cursor.y >= pos.y as f64
                        && cursor.y < pos.y as f64 + 60. * scale;
                    let was_hovered = hovered.swap(hit, Ordering::Relaxed);
                    if hit != was_hovered {
                        let t0 = Instant::now();
                        let _ = w.set_ignore_cursor_events(!hit);
                        let unix_ms = std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .unwrap_or_default()
                            .as_millis() as u64;
                        let _ = w.emit(
                            "lyrics-hover",
                            json!({"entered": hit, "entered_unix_ms": unix_ms, "rust_ms": t0.elapsed().as_secs_f64() * 1000.0}),
                        );
                    }
                }
            } else {
                hovered.store(false, Ordering::Relaxed);
            }
            std::thread::sleep(std::time::Duration::from_millis(100));
        }
    });
    record(
        json!({"event":"lyrics_api", "transparent":true,"always_on_top":true,"ignore_cursor":initial_locked,"poll_ms":100}),
    );
    Ok(())
}
#[tauri::command]
async fn dependency_probe() -> Result<Value, String> {
    use aes::cipher::{BlockEncryptMut, KeyIvInit};
    use hmac::Mac;
    use rubato::Resampler;
    let p = fixture("aac.m4a")?;
    let f = fs::File::open(&p).map_err(|e| e.to_string())?;
    let mp4 = mp4::Mp4Reader::read_header(f, fs::metadata(&p).unwrap().len())
        .map_err(|e| e.to_string())?;
    let tags = lofty::read_from_path(&p).is_ok();
    let mut db = fontdb::Database::new();
    db.load_system_fonts();
    let mut key = [0u8; 32];
    pbkdf2::pbkdf2_hmac::<sha2::Sha512>(b"spike", b"salt", 10_000, &mut key);
    let encrypted = cbc::Encryptor::<aes::Aes256>::new(&key.into(), &[0u8; 16].into())
        .encrypt_padded_vec_mut::<aes::cipher::block_padding::Pkcs7>(b"fixture");
    let mut mac = <hmac::Hmac<sha2::Sha256> as Mac>::new_from_slice(&key).unwrap();
    mac.update(&encrypted);
    let hash = sha2::Sha256::digest(mac.finalize().into_bytes());
    let files = walkdir::WalkDir::new(p.parent().unwrap())
        .into_iter()
        .count();
    let resampler =
        rubato::FftFixedIn::<f32>::new(44100, 22050, 1024, 2, 1).map_err(|e| e.to_string())?;
    let flac = fs::File::open(fixture("tone.flac")?).map_err(|e| e.to_string())?;
    let mss = symphonia::core::io::MediaSourceStream::new(Box::new(flac), Default::default());
    let mut probed = symphonia::default::get_probe()
        .format(
            &Default::default(),
            mss,
            &Default::default(),
            &Default::default(),
        )
        .map_err(|e| e.to_string())?;
    let track = probed.format.default_track().ok_or("no track")?;
    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &Default::default())
        .map_err(|e| e.to_string())?;
    let packet = probed.format.next_packet().map_err(|e| e.to_string())?;
    let frames = decoder.decode(&packet).map_err(|e| e.to_string())?.frames();
    let _cookies = reqwest_cookie_store::CookieStoreMutex::new(Default::default());
    let _ = futures::future::ready(()).await;
    Ok(
        json!({"mp4_tracks":mp4.tracks().len(), "tags":tags,"fonts":db.faces().count(),"files":files,"resample_frames":resampler.output_frames_max(),"flac_frames":frames,"md5":format!("{:x}",md5::compute(hash))}),
    )
}
#[cfg(target_os = "linux")]
fn native_controls(app: tauri::AppHandle) {
    std::thread::spawn(move || {
        let result = (|| -> Result<(), String> {
            let mut controls = souvlaki::MediaControls::new(souvlaki::PlatformConfig {
                dbus_name: "biu_m0",
                display_name: "Biu M0",
                hwnd: None,
            })
            .map_err(|e| format!("{e:?}"))?;
            controls
                .attach(move |event| {
                    let name = format!("{event:?}");
                    record(json!({"event":"native_media_action","action":name}));
                    let _ = app.emit("native-media", name);
                })
                .map_err(|e| format!("{e:?}"))?;
            controls
                .set_metadata(souvlaki::MediaMetadata {
                    title: Some("Biu M0 native fixture"),
                    artist: Some("Synthetic test"),
                    cover_url: Some("http://127.0.0.1:18765/art.svg"),
                    duration: Some(std::time::Duration::from_secs(300)),
                    ..Default::default()
                })
                .map_err(|e| format!("{e:?}"))?;
            controls
                .set_playback(souvlaki::MediaPlayback::Playing { progress: None })
                .map_err(|e| format!("{e:?}"))?;
            record(json!({"event":"native_media_ready"}));
            loop {
                std::thread::sleep(std::time::Duration::from_secs(1));
            }
        })();
        if let Err(e) = result {
            record(json!({"event":"native_media_failure","message":e}));
        }
    });
}
#[tauri::command]
async fn update_test(app: tauri::AppHandle) -> Result<(), String> {
    use tauri_plugin_updater::UpdaterExt;
    let dir =
        std::path::PathBuf::from(std::env::var("BIU_UPDATE_PRIVATE").map_err(|e| e.to_string())?);
    #[cfg(target_os = "linux")]
    let image = std::path::PathBuf::from(
        std::env::var("APPIMAGE").map_err(|_| "S7 requires a private AppImage copy")?,
    )
    .canonicalize()
    .map_err(|e| e.to_string())?;
    #[cfg(not(target_os = "linux"))]
    let image = std::env::current_exe()
        .map_err(|e| e.to_string())?
        .canonicalize()
        .map_err(|e| e.to_string())?;
    #[cfg(target_os = "macos")]
    if image.to_string_lossy().contains("/AppTranslocation/") || image.starts_with("/Volumes") {
        record(json!({"event":"update_manual","reason":"dmg or translocated"}));
        app.exit(0);
        return Ok(());
    }
    let allowed = image.starts_with(dir.canonicalize().map_err(|e| e.to_string())?);
    #[cfg(target_os = "macos")]
    let allowed = allowed
        || (std::env::var_os("BIU_UPDATE_ALLOW_MAC_TEST_LOCATIONS").is_some()
            && [
                std::path::PathBuf::from("/Applications/Biu M0.app"),
                std::path::PathBuf::from(std::env::var("HOME").unwrap_or_default())
                    .join("Applications/Biu M0.app"),
            ]
            .iter()
            .any(|p| image.starts_with(p)));
    if !allowed {
        return Err("refusing update outside explicit M0 test locations".into());
    }
    let pubkey = fs::read_to_string(dir.join("updater.key.pub")).map_err(|e| e.to_string())?;
    use tauri_plugin_store::StoreExt;
    let store = app.store("m0-preserve.json").map_err(|e| e.to_string())?;
    store.set("theme", json!("spike-dark"));
    store.set("login", json!({"fixture":true}));
    store.save().map_err(|e| e.to_string())?;
    let updater = app
        .updater_builder()
        .pubkey(pubkey.trim())
        .endpoints(vec!["http://127.0.0.1:18766/latest.json".parse().unwrap()])
        .map_err(|e| e.to_string())?
        .build()
        .map_err(|e| e.to_string())?;
    let update = updater
        .check()
        .await
        .map_err(|e| e.to_string())?
        .ok_or("no update")?;
    let bytes = update
        .download(
            |chunk, total| record(json!({"event":"update_download","chunk":chunk,"total":total})),
            || record(json!({"event":"update_downloaded"})),
        )
        .await
        .map_err(|e| e.to_string())?;
    record(json!({"event":"update_verified","version":update.version}));
    // Linux/Windows use private test copies; macOS additionally allows explicitly opted-in Biu M0.app test locations.
    update.install(bytes).map_err(|e| e.to_string())?;
    record(json!({"event":"update_installed"}));
    app.exit(0);
    Ok(())
}
#[tauri::command]
fn update_verify(app: tauri::AppHandle) -> Result<Value, String> {
    use tauri_plugin_store::StoreExt;
    let store = app.store("m0-preserve.json").map_err(|e| e.to_string())?;
    Ok(
        json!({"event":"update_restart_verified","version":app.package_info().version.to_string(),"settings_preserved":store.get("theme")==Some(json!("spike-dark")),"login_fixture_preserved":store.get("login")==Some(json!({"fixture":true}))}),
    )
}
fn main() {
    let cookies = Arc::new(reqwest_cookie_store::CookieStoreMutex::new(
        Default::default(),
    ));
    let client = reqwest::Client::builder()
        .cookie_provider(cookies)
        .user_agent("biu-m0/0.0.1")
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap();
    let server_client = client.clone();
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|_, _, _| {}))
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .plugin(tauri_plugin_process::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .register_asynchronous_uri_scheme_protocol("biu-media", move |_ctx, req, responder| {
            let c = client.clone();
            tauri::async_runtime::spawn(async move {
                let response = media(req, c).await.unwrap_or_else(|e| {
                    record(json!({"event":"protocol_error", "message":e}));
                    tauri::http::Response::builder()
                        .status(502)
                        .body(e.into_bytes())
                        .unwrap()
                });
                responder.respond(response);
            });
        })
        .invoke_handler(tauri::generate_handler![
            report,
            finish,
            mode,
            lyrics,
            lyrics_set_state,
            dependency_probe,
            media_base,
            update_test,
            update_verify
        ])
        .setup(move |app| {
            let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
            listener.set_nonblocking(true)?;
            let port = listener.local_addr()?.port();
            let token = uuid::Uuid::new_v4().to_string();
            app.manage(MediaBase(format!("http://127.0.0.1:{port}/{token}/")));
            app.manage(LyricsState {
                locked: Arc::new(AtomicBool::new(false)),
                hovered: Arc::new(AtomicBool::new(false)),
            });
            tauri::async_runtime::spawn(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                serve_http(listener, token, server_client).await;
            });
            record(json!({"event":"startup", "version":app.package_info().version.to_string()}));
            #[cfg(target_os = "linux")]
            if std::env::var_os("BIU_NATIVE_MEDIA").is_some() {
                native_controls(app.handle().clone());
            }
            if let Ok(proxy) = std::env::var("BIU_WEBVIEW_PROXY") {
                let builder = tauri::WebviewWindowBuilder::new(
                    app,
                    "proxy",
                    tauri::WebviewUrl::App("proxy.html".into()),
                )
                .proxy_url(proxy.parse()?);
                // WebView2 refuses different environment options on one user data folder.
                #[cfg(windows)]
                let builder =
                    builder.data_directory(std::env::temp_dir().join("biu-m0-proxy-webview"));
                builder.build()?;
            }
            let _ = app.emit("ready", ());
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("Tauri spike failed");
}

#[cfg(test)]
mod tests {
    use super::*;
    fn ensure_test_fixtures() {
        static INIT: std::sync::Once = std::sync::Once::new();
        INIT.call_once(|| {
            let dir = std::env::temp_dir().join("biu-m0-cargo-test-fixtures");
            let _ = fs::create_dir_all(&dir);
            let aac = dir.join("aac.m4a");
            if !aac.exists() {
                let _ = fs::write(&aac, vec![42u8; 4096]);
            }
            std::env::set_var("BIU_SPIKE_FIXTURES", "1");
            std::env::set_var("BIU_FIXTURES", &dir);
            std::env::set_var("BIU_LOG", dir.join("test.jsonl"));
        });
    }
    #[tokio::test]
    async fn local_protocol_bytes_and_416() {
        ensure_test_fixtures();
        let req = tauri::http::Request::builder()
            .uri("biu-media://local?name=aac.m4a")
            .header("Range", "bytes=100-199")
            .body(vec![])
            .unwrap();
        let r = media(req, reqwest::Client::new()).await.unwrap();
        assert_eq!(r.status(), 206);
        assert_eq!(
            r.headers()["Content-Range"],
            format!(
                "bytes 100-199/{}",
                fs::metadata(fixture("aac.m4a").unwrap()).unwrap().len()
            )
        );
        assert_eq!(
            r.body(),
            &fs::read(fixture("aac.m4a").unwrap()).unwrap()[100..200]
        );
        let req = tauri::http::Request::builder()
            .uri("biu-media://local?name=aac.m4a")
            .header("Range", "bytes=999999999-")
            .body(vec![])
            .unwrap();
        assert_eq!(
            media(req, reqwest::Client::new()).await.unwrap().status(),
            416
        );
        assert!(fixture("../../etc/passwd").is_err());
    }
    #[tokio::test]
    async fn http_fallback_bytes() {
        ensure_test_fixtures();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let (mut socket, _) = listener.accept().await.unwrap();
            let req = tauri::http::Request::builder()
                .uri("biu-media://local?name=aac.m4a")
                .header("Range", "bytes=100-199")
                .body(vec![])
                .unwrap();
            http_media(&mut socket, req, reqwest::Client::new())
                .await
                .unwrap();
            socket.shutdown().await.unwrap();
        });
        let mut s = tokio::net::TcpStream::connect(addr).await.unwrap();
        let mut bytes = vec![];
        s.read_to_end(&mut bytes).await.unwrap();
        task.await.unwrap();
        let split = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        assert!(String::from_utf8_lossy(&bytes[..split]).starts_with("HTTP/1.1 206"));
        assert_eq!(
            &bytes[split + 4..],
            &fs::read(fixture("aac.m4a").unwrap()).unwrap()[100..200]
        );
    }
    #[test]
    fn remote_ranges_stay_within_one_chunk() {
        assert_eq!(capped_remote_range(None), "bytes=0-1048575");
        assert_eq!(capped_remote_range(Some("bytes=0-")), "bytes=0-1048575");
        assert_eq!(
            capped_remote_range(Some("bytes=917504-")),
            "bytes=917504-1966079"
        );
        assert_eq!(capped_remote_range(Some("bytes=0-1445")), "bytes=0-1445");
        assert_eq!(
            capped_remote_range(Some("bytes=0-9999999")),
            "bytes=0-1048575"
        );
    }
    #[test]
    fn http_host_origin_and_uri_validation() {
        let host = "127.0.0.1:12345";
        let good=format!("GET /test/local?name=aac.m4a HTTP/1.1\r\nHost: {host}\r\nOrigin: {}\r\nRange: bytes=0-99\r\n\r\n", app_origin());
        assert!(parse_http_request(&good, "test", host).is_ok());
        assert_eq!(
            parse_http_request(&good.replace(host, "attacker.invalid"), "test", host).unwrap_err(),
            403
        );
        assert_eq!(
            parse_http_request(
                &good.replace(app_origin(), "https://attacker.invalid"),
                "test",
                host
            )
            .unwrap_err(),
            403
        );
        assert_eq!(parse_http_request(&good, "wrong", host).unwrap_err(), 403);
        let malformed = format!("GET /test/[ HTTP/1.1\r\nHost: {host}\r\n\r\n");
        assert_eq!(
            parse_http_request(&malformed, "test", host).unwrap_err(),
            400
        );
        let duplicate = format!("GET /test/local HTTP/1.1\r\nHost: {host}\r\nHost: {host}\r\n\r\n");
        assert_eq!(
            parse_http_request(&duplicate, "test", host).unwrap_err(),
            400
        );
    }
    #[test]
    fn fixture_rules_are_opt_in_and_exact() {
        let u = url::Url::parse("http://127.0.0.1:18765/aac.m4a?round=1").unwrap();
        assert!(!fixture_upstream_allowed(&u, false));
        assert!(fixture_upstream_allowed(&u, true));
        for u in [
            "https://other.akamaized.net/aac.m4a",
            "http://localhost:18765/aac.m4a",
            "http://127.0.0.1:18765/not-a-fixture",
            "http://user:pass@127.0.0.1:18765/aac.m4a",
        ] {
            assert!(!fixture_upstream_allowed(
                &url::Url::parse(u).unwrap(),
                true
            ));
        }
    }
    #[tokio::test]
    async fn http_wire_rejects_bad_input_and_stays_alive() {
        ensure_test_fixtures();
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let server = tokio::spawn(serve_http(listener, "test".into(), reqwest::Client::new()));
        for (path, host, origin, status) in [
            (
                "local?name=aac.m4a",
                "wrong.invalid".to_string(),
                app_origin(),
                403,
            ),
            ("[", addr.to_string(), app_origin(), 400),
            (
                "local?name=aac.m4a",
                addr.to_string(),
                "https://attacker.invalid",
                403,
            ),
            ("local?name=aac.m4a", addr.to_string(), app_origin(), 206),
        ] {
            let mut socket = tokio::net::TcpStream::connect(addr).await.unwrap();
            socket.write_all(format!("GET /test/{path} HTTP/1.1\r\nHost: {host}\r\nOrigin: {origin}\r\nRange: bytes=100-199\r\n\r\n").as_bytes()).await.unwrap();
            let mut data = vec![];
            tokio::time::timeout(
                std::time::Duration::from_secs(5),
                socket.read_to_end(&mut data),
            )
            .await
            .unwrap()
            .unwrap();
            assert!(String::from_utf8_lossy(&data).starts_with(&format!("HTTP/1.1 {status}")));
            if status == 206 {
                let h = String::from_utf8_lossy(&data);
                assert!(h.contains(&format!("Access-Control-Allow-Origin: {}", app_origin())));
                assert!(!h.contains("Access-Control-Allow-Origin: *"));
            }
        }
        server.abort();
    }
    #[test]
    fn byte_ranges() {
        assert_eq!(bounds(Some("bytes=100-199"), 1000).unwrap(), (100, 100));
        assert_eq!(bounds(Some("bytes=0-"), 2_000_000).unwrap(), (0, 1_048_576));
        assert_eq!(bounds(Some("bytes=-100"), 1000).unwrap(), (900, 100));
        assert!(bounds(Some("bytes=1000-"), 1000).is_err());
    }
}
