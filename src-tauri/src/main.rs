#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bili;
mod desktop_lyrics;
mod lyrics;
mod media;
mod media_keys;
mod shortcut;
mod store;
mod tray;

use bili::{
    bili_request, cookie_get, cookie_set, inject_auth_cookies, parse_proxy_settings_value,
    proxy_set, BiliState,
};
use desktop_lyrics::{
    desktop_lyrics_command, desktop_lyrics_end_drag, desktop_lyrics_get_content,
    desktop_lyrics_reset_position, desktop_lyrics_set_content_height,
    desktop_lyrics_set_ignore_mouse, desktop_lyrics_start_drag, desktop_lyrics_sync,
    toggle_mini_player, DesktopLyricsState,
};
use lyrics::{
    kuromoji_ensure_dict, lyrics_get_netease, lyrics_search_lrclib, lyrics_search_netease,
};
use media::{media_base, serve_http, MediaBase};
use media_keys::update_playback_state;
use shortcut::{
    shortcut_register, shortcut_register_all, shortcut_unregister, shortcut_unregister_all,
};
use std::sync::Arc;
use store::{get_saved_proxy_settings, store_clear, store_get, store_set};
use tauri::{AppHandle, Manager, Runtime};

// Must stay async: a sync command runs on the main thread, and building a window there deadlocks on WebView2.
#[tauri::command]
async fn ensure_main_window<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.unminimize();
        let _ = w.show();
        let _ = w.set_focus();
        return Ok(());
    }
    let builder =
        tauri::WebviewWindowBuilder::new(&app, "main", tauri::WebviewUrl::App("index.html".into()))
            .title("Biu")
            .inner_size(1200.0, 800.0)
            .min_inner_size(1200.0, 800.0)
            .resizable(true)
            .center()
            .decorations(false);
    builder.build().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
fn window_close<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let _ = tray::handle_main_window_close_request(&app);
    Ok(())
}

#[tauri::command]
fn frontend_log(level: String, message: String) {
    match level.to_ascii_lowercase().as_str() {
        "error" => log::error!("[frontend] {message}"),
        "warn" | "warning" => log::warn!("[frontend] {message}"),
        _ => log::info!("[frontend] {message}"),
    }
}

#[tauri::command]
fn is_silent_mode() -> bool {
    std::env::var("BIU_SILENT")
        .map(|v| v == "1" || v.eq_ignore_ascii_case("true"))
        .unwrap_or(false)
}

#[tauri::command]
fn devtools_toggle<R: Runtime>(app: AppHandle<R>) {
    #[cfg(debug_assertions)]
    if let Some(w) = app.get_webview_window("main") {
        if w.is_devtools_open() {
            w.close_devtools();
        } else {
            w.open_devtools();
        }
    }
    #[cfg(not(debug_assertions))]
    let _ = app;
}

fn main() {
    tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            desktop_lyrics::destroy_mini_player(app);
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(
            tauri_plugin_log::Builder::new()
                // Lower levels make cookie_store log Set-Cookie values, including login cookies.
                .level(log::LevelFilter::Info)
                .build(),
        )
        .plugin(tauri_plugin_window_state::Builder::default().build())
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .on_window_event(|window, event| {
            if window.label() == "main" {
                if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                    if tray::handle_main_window_close_request(window.app_handle()) {
                        api.prevent_close();
                    }
                }
            }
        })
        .invoke_handler(tauri::generate_handler![
            store_get,
            store_set,
            store_clear,
            bili_request,
            cookie_get,
            cookie_set,
            proxy_set,
            media_base,
            ensure_main_window,
            window_close,
            frontend_log,
            is_silent_mode,
            devtools_toggle,
            lyrics_search_netease,
            lyrics_get_netease,
            lyrics_search_lrclib,
            kuromoji_ensure_dict,
            desktop_lyrics_sync,
            desktop_lyrics_get_content,
            desktop_lyrics_command,
            desktop_lyrics_start_drag,
            desktop_lyrics_end_drag,
            desktop_lyrics_set_ignore_mouse,
            desktop_lyrics_set_content_height,
            desktop_lyrics_reset_position,
            toggle_mini_player,
            shortcut_register,
            shortcut_unregister,
            shortcut_register_all,
            shortcut_unregister_all,
            update_playback_state,
        ])
        .setup(|app| {
            let app_data_dir = app
                .path()
                .app_data_dir()
                .unwrap_or_else(|_| std::env::temp_dir().join("com.biu.wood3n"));
            let _ = std::fs::create_dir_all(&app_data_dir);
            let cookie_path = app_data_dir.join("cookies.json");
            let saved_proxy = parse_proxy_settings_value(get_saved_proxy_settings(app.handle()));
            let bili_state = Arc::new(
                BiliState::new(cookie_path, saved_proxy).expect("failed to initialize BiliState"),
            );
            app.manage(bili_state.clone());
            app.manage(Arc::new(DesktopLyricsState::default()));

            if let Err(e) = tray::setup_tray(app.handle()) {
                log::warn!("[tray] failed to initialize system tray: {e}");
            }
            media_keys::setup_media_keys(app.handle());

            let listener = std::net::TcpListener::bind("127.0.0.1:0")?;
            listener.set_nonblocking(true)?;
            let port = listener.local_addr()?.port();
            let token = uuid::Uuid::new_v4().to_string();
            app.manage(MediaBase(format!("http://127.0.0.1:{port}/{token}/")));

            let server_state = bili_state.clone();
            tauri::async_runtime::spawn(async move {
                let listener = tokio::net::TcpListener::from_std(listener).unwrap();
                serve_http(listener, token, server_state).await;
            });

            let auth_state = bili_state.clone();
            tauri::async_runtime::spawn(async move {
                inject_auth_cookies(auth_state).await;
            });

            let shortcut_app = app.handle().clone();
            tauri::async_runtime::spawn(async move {
                shortcut::register_all_shortcuts_internal(&shortcut_app);
            });

            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("error while building biu application")
        .run(|app, event| {
            // macOS: clicking the Dock icon after the main window was hidden to the tray.
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Reopen { .. } = event {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            #[cfg(not(target_os = "macos"))]
            let _ = (app, event);
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bili::{
        append_query_params, compute_bili_ticket_hexsign, extract_and_register_playurl_hosts,
        is_bili_api_url_allowed, is_bili_audio_cdn_host, BiliRequestInput,
    };
    use crate::media::{app_origin, parse_http_request};
    use crate::store::{file_store_clear, file_store_get, file_store_set, mark_manual_edits};
    use serde_json::json;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};
    use url::Url;

    #[test]
    fn store_read_write_clear_and_unknown_name() {
        let dir = std::env::temp_dir().join(format!("biu-test-store-{}", uuid::Uuid::new_v4()));
        let _ = std::fs::remove_dir_all(&dir);

        assert!(file_store_get(&dir, "non-existent-store").is_err());

        // Default values for app-settings
        let initial = file_store_get(&dir, "app-settings").unwrap();
        assert_eq!(initial["appSettings"]["primaryColor"], "#1ed760");

        // Write and read back user-login-info
        file_store_set(
            &dir,
            "user-login-info",
            json!({"isLogin": true, "uname": "tester", "mid": 12345}),
        )
        .unwrap();
        let user = file_store_get(&dir, "user-login-info").unwrap();
        assert_eq!(user["isLogin"], true);
        assert_eq!(user["mid"], 12345);

        // Clear user-login-info
        file_store_clear(&dir, "user-login-info").unwrap();
        let cleared = file_store_get(&dir, "user-login-info").unwrap();
        assert!(cleared.as_object().unwrap().is_empty());

        // Manual edit marking on lyrics-cache
        let prev = json!({"BV1-1": {"lyrics": "[00:01.00]a", "offset": 0, "source": "auto"}});
        let inc = json!({"BV1-1": {"lyrics": "[00:01.00]a", "offset": 200, "source": "auto"}});
        let marked = mark_manual_edits(&prev, &inc);
        assert_eq!(marked["BV1-1"]["source"], "manual");

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bili_api_and_media_allowlist_rules() {
        assert!(is_bili_api_url_allowed(
            &Url::parse("https://api.bilibili.com/x/web-interface/nav").unwrap()
        ));
        assert!(is_bili_api_url_allowed(
            &Url::parse("https://passport.bilibili.com/x/passport-login/web/qrcode/generate")
                .unwrap()
        ));
        assert!(is_bili_api_url_allowed(
            &Url::parse("https://s.search.bilibili.com/main/suggest").unwrap()
        ));
        assert!(is_bili_api_url_allowed(
            &Url::parse("https://i0.hdslb.com/bfs/music/test.lrc").unwrap()
        ));
        assert!(!is_bili_api_url_allowed(
            &Url::parse("https://example.com/api").unwrap()
        ));
        assert!(!is_bili_api_url_allowed(
            &Url::parse("https://bilibili.com.evil.com/x").unwrap()
        ));
        assert!(!is_bili_api_url_allowed(
            &Url::parse("https://user:pass@api.bilibili.com/x").unwrap()
        ));

        assert!(is_bili_audio_cdn_host("upos-sz-mirrorcos.bilivideo.com"));
        assert!(is_bili_audio_cdn_host("xy123x45x67x89xy.mcdn.bilivideo.cn"));
        assert!(is_bili_audio_cdn_host("upos-hz-mirrorakam.akamaized.net"));
        assert!(!is_bili_audio_cdn_host("other.akamaized.net"));
        assert!(!is_bili_audio_cdn_host("attacker.invalid"));
    }

    #[test]
    fn playurl_dynamic_host_registration_and_cookie_persistence() {
        let dir = std::env::temp_dir().join(format!("biu-test-cookies-{}", uuid::Uuid::new_v4()));
        let cookie_path = dir.join("cookies.json");
        let state = BiliState::new(cookie_path.clone(), None).unwrap();

        state
            .set_cookie("bili_jct", "test_csrf_value", Some(4102444800))
            .unwrap();
        assert_eq!(
            state.get_cookie("bili_jct").as_deref(),
            Some("test_csrf_value")
        );
        assert!(cookie_path.exists());

        // Reload from disk into a new BiliState
        let state2 = BiliState::new(cookie_path, None).unwrap();
        assert_eq!(
            state2.get_cookie("bili_jct").as_deref(),
            Some("test_csrf_value")
        );

        let playurl_json = json!({
            "code": 0,
            "data": {
                "dash": {
                    "audio": [
                        {
                            "baseUrl": "https://cn-gdgz-fx-01-01.bilivideo.com/upgcxcode/audio.m4s?deadline=1999999999",
                            "backupUrl": ["https://upos-sz-estgoss.bilivideo.com/upgcxcode/audio.m4s"]
                        }
                    ]
                }
            }
        });
        extract_and_register_playurl_hosts(&state2, &playurl_json);
        assert!(state2.is_media_url_allowed(
            &Url::parse("https://cn-gdgz-fx-01-01.bilivideo.com/upgcxcode/audio.m4s").unwrap()
        ));
        assert!(!state2
            .is_media_url_allowed(&Url::parse("https://evil.example.com/audio.m4s").unwrap()));

        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn ticket_hexsign_and_query_params() {
        let sign = compute_bili_ticket_hexsign(1700000000);
        assert_eq!(sign.len(), 64);

        let mut u = Url::parse("https://api.bilibili.com/x/test").unwrap();
        append_query_params(&mut u, &json!({"a": 1, "b": "hello", "c": null}));
        assert_eq!(u.query(), Some("a=1&b=hello"));
    }

    #[test]
    fn http_proxy_request_validation() {
        let host = "127.0.0.1:12345";
        let good = format!(
            "GET /tok123/remote?u=https%3A%2F%2Fupos-sz-mirrorcos.bilivideo.com%2Fa.m4s HTTP/1.1\r\nHost: {host}\r\nOrigin: {}\r\nRange: bytes=0-99\r\n\r\n",
            app_origin()
        );
        assert!(parse_http_request(&good, "tok123", host).is_ok());
        assert_eq!(
            parse_http_request(&good.replace(host, "evil.invalid"), "tok123", host).unwrap_err(),
            403
        );
        assert_eq!(
            parse_http_request(
                &good.replace(app_origin(), "https://attacker.invalid"),
                "tok123",
                host
            )
            .unwrap_err(),
            403
        );
        assert_eq!(
            parse_http_request(&good, "wrongtok", host).unwrap_err(),
            403
        );
    }

    #[tokio::test]
    async fn loopback_proxy_streams_range_and_rejects_disallowed_hosts() {
        // Start a mock upstream server simulating a Bilibili audio CDN supporting Range
        let upstream_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let upstream_addr = upstream_listener.local_addr().unwrap();
        let payload: Vec<u8> = (0..4096u32).map(|i| (i % 251) as u8).collect();
        let payload_clone = payload.clone();

        let upstream_task = tokio::spawn(async move {
            if let Ok((mut sock, _)) = upstream_listener.accept().await {
                let mut req_buf = vec![0u8; 2048];
                let n = sock.read(&mut req_buf).await.unwrap_or(0);
                let req_str = String::from_utf8_lossy(&req_buf[..n]);
                assert!(req_str
                    .to_ascii_lowercase()
                    .contains("referer: https://www.bilibili.com/"));
                assert!(req_str
                    .to_ascii_lowercase()
                    .contains("range: bytes=100-199"));
                let slice = &payload_clone[100..200];
                let header = format!(
                    "HTTP/1.1 206 Partial Content\r\nContent-Type: audio/mp4\r\nContent-Length: {}\r\nContent-Range: bytes 100-199/{}\r\nAccept-Ranges: bytes\r\nConnection: close\r\n\r\n",
                    slice.len(),
                    payload_clone.len()
                );
                sock.write_all(header.as_bytes()).await.unwrap();
                sock.write_all(slice).await.unwrap();
                let _ = sock.shutdown().await;
            }
        });

        let dir = std::env::temp_dir().join(format!("biu-test-proxy-{}", uuid::Uuid::new_v4()));
        let state = Arc::new(BiliState::new(dir.join("cookies.json"), None).unwrap());
        // Register the mock upstream host (127.0.0.1) as a playurl-returned host for this test instance
        state.register_media_host("127.0.0.1");

        let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy_listener.local_addr().unwrap();
        let proxy_server = tokio::spawn(serve_http(
            proxy_listener,
            "secret-token".into(),
            state.clone(),
        ));

        // 1. Disallowed host should return 403
        {
            let mut client_sock = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();
            let bad_req = format!(
                "GET /secret-token/remote?u=https%3A%2F%2Fevil.example.com%2Fa.m4s HTTP/1.1\r\nHost: {proxy_addr}\r\nOrigin: {}\r\n\r\n",
                app_origin()
            );
            client_sock.write_all(bad_req.as_bytes()).await.unwrap();
            let mut resp = vec![];
            client_sock.read_to_end(&mut resp).await.unwrap();
            assert!(String::from_utf8_lossy(&resp).starts_with("HTTP/1.1 403"));
        }

        // 2. Allowed host with Range should stream 206 Partial Content and exact bytes
        {
            let mut client_sock = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();
            let target_url = format!("http://{upstream_addr}/audio.m4s");
            let encoded: String =
                url::form_urlencoded::byte_serialize(target_url.as_bytes()).collect();
            let range_req = format!(
                "GET /secret-token/remote?u={encoded} HTTP/1.1\r\nHost: {proxy_addr}\r\nOrigin: {}\r\nRange: bytes=100-199\r\n\r\n",
                app_origin()
            );
            client_sock.write_all(range_req.as_bytes()).await.unwrap();
            let mut resp = vec![];
            client_sock.read_to_end(&mut resp).await.unwrap();
            let split = resp.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            let head = String::from_utf8_lossy(&resp[..split]);
            assert!(head.starts_with("HTTP/1.1 206"));
            assert!(head
                .to_ascii_lowercase()
                .contains("content-range: bytes 100-199/4096"));
            assert_eq!(&resp[split + 4..], &payload[100..200]);
        }

        upstream_task.await.unwrap();
        proxy_server.abort();
        let _ = std::fs::remove_dir_all(&dir);

        // Ensure BiliRequestInput rejects non-bilibili URLs
        let err = bili::execute_bili_request(
            &state,
            BiliRequestInput {
                method: Some("GET".into()),
                url: "https://evil.example.com/steal".into(),
                params: None,
                body: None,
                headers: None,
                timeout_ms: None,
            },
        )
        .await;
        assert!(err.is_err());
    }

    #[test]
    fn lyrics_allowlist_and_kuromoji_sha256_verification() {
        use crate::lyrics::{
            is_lyrics_url_allowed, is_valid_kuromoji_dict_filename, verify_dict_file,
            KUROMOJI_DICT_FILES,
        };

        assert!(is_lyrics_url_allowed(
            &Url::parse("https://interface.music.163.com/api/search/get").unwrap()
        ));
        assert!(is_lyrics_url_allowed(
            &Url::parse("https://lrclib.net/api/search").unwrap()
        ));
        assert!(!is_lyrics_url_allowed(
            &Url::parse("https://evil.music.163.com/api").unwrap()
        ));
        assert!(!is_lyrics_url_allowed(
            &Url::parse("https://user:pass@lrclib.net/api/search").unwrap()
        ));

        assert!(is_valid_kuromoji_dict_filename("base.dat.gz"));
        assert!(!is_valid_kuromoji_dict_filename("../base.dat.gz"));
        assert!(!is_valid_kuromoji_dict_filename("unknown.dat.gz"));

        let repo_dict_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../node_modules/kuromoji/dict");
        if repo_dict_dir.exists() {
            assert_eq!(KUROMOJI_DICT_FILES.len(), 12);
            for (filename, expected_sha256) in KUROMOJI_DICT_FILES {
                let p = repo_dict_dir.join(filename);
                assert!(
                    verify_dict_file(&p, expected_sha256),
                    "sha256 mismatch for {filename}"
                );
                assert!(!verify_dict_file(
                    &p,
                    "0000000000000000000000000000000000000000000000000000000000000000"
                ));
            }
        }
    }

    #[tokio::test]
    async fn kuromoji_dict_download_fallback_and_loopback_route() {
        use crate::lyrics::{ensure_kuromoji_dict_in_dir, verify_dict_file, KUROMOJI_DICT_FILES};

        let repo_dict_dir =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../node_modules/kuromoji/dict");
        if !repo_dict_dir.exists() {
            return;
        }

        let dir = std::env::temp_dir().join(format!("biu-test-kuromoji-{}", uuid::Uuid::new_v4()));
        let dict_dir = dir.join("kuromoji-dict");
        std::fs::create_dir_all(&dict_dir).unwrap();

        // Pre-copy 11 files so only the 12th file needs to be downloaded
        for (filename, _) in &KUROMOJI_DICT_FILES[..11] {
            std::fs::copy(repo_dict_dir.join(filename), dict_dir.join(filename)).unwrap();
        }
        let (target_file, target_expected_sha) = KUROMOJI_DICT_FILES[11];
        let target_bytes = std::fs::read(repo_dict_dir.join(target_file)).unwrap();

        // Primary mirror returns corrupted bytes (wrong sha256)
        let bad_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let bad_addr = bad_listener.local_addr().unwrap();
        let bad_task = tokio::spawn(async move {
            if let Ok((mut sock, _)) = bad_listener.accept().await {
                let mut buf = vec![0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let body = b"corrupted-gz-data";
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.write_all(body).await;
                let _ = sock.shutdown().await;
            }
        });

        // Backup mirror returns valid target_file bytes
        let good_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let good_addr = good_listener.local_addr().unwrap();
        let target_bytes_clone = target_bytes.clone();
        let good_task = tokio::spawn(async move {
            if let Ok((mut sock, _)) = good_listener.accept().await {
                let mut buf = vec![0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let resp = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    target_bytes_clone.len()
                );
                let _ = sock.write_all(resp.as_bytes()).await;
                let _ = sock.write_all(&target_bytes_clone).await;
                let _ = sock.shutdown().await;
            }
        });

        let bad_base = format!("http://{bad_addr}");
        let good_base = format!("http://{good_addr}");
        ensure_kuromoji_dict_in_dir(&dict_dir, &[bad_base.as_str(), good_base.as_str()], None)
            .await
            .unwrap();
        assert!(verify_dict_file(
            &dict_dir.join(target_file),
            target_expected_sha
        ));

        // Verify loopback proxy serves /<token>/kuromoji?name=<target_file> and rejects traversal
        let state = Arc::new(BiliState::new(dir.join("cookies.json"), None).unwrap());
        let proxy_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let proxy_addr = proxy_listener.local_addr().unwrap();
        let proxy_server =
            tokio::spawn(serve_http(proxy_listener, "tok-kuro".into(), state.clone()));

        {
            let mut client_sock = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();
            let req = format!(
                "GET /tok-kuro/kuromoji?name={target_file} HTTP/1.1\r\nHost: {proxy_addr}\r\nOrigin: {}\r\n\r\n",
                app_origin()
            );
            client_sock.write_all(req.as_bytes()).await.unwrap();
            let mut resp = vec![];
            client_sock.read_to_end(&mut resp).await.unwrap();
            let split = resp.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
            let head = String::from_utf8_lossy(&resp[..split]);
            assert!(head.starts_with("HTTP/1.1 200"));
            assert_eq!(&resp[split + 4..], &target_bytes[..]);
        }

        {
            let mut client_sock = tokio::net::TcpStream::connect(proxy_addr).await.unwrap();
            let req = format!(
                "GET /tok-kuro/kuromoji?name=..%2Fcookies.json HTTP/1.1\r\nHost: {proxy_addr}\r\nOrigin: {}\r\n\r\n",
                app_origin()
            );
            client_sock.write_all(req.as_bytes()).await.unwrap();
            let mut resp = vec![];
            client_sock.read_to_end(&mut resp).await.unwrap();
            assert!(String::from_utf8_lossy(&resp).starts_with("HTTP/1.1 403"));
        }

        let _ = bad_task.await;
        let _ = good_task.await;
        proxy_server.abort();
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn kuromoji_dict_falls_back_to_tarball() {
        use crate::lyrics::{ensure_kuromoji_dict_in_dir, verify_dict_file, KUROMOJI_DICT_FILES};
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let src =
            std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../node_modules/kuromoji/dict");
        let mut builder = tar::Builder::new(flate2::write::GzEncoder::new(
            Vec::new(),
            flate2::Compression::fast(),
        ));
        for (name, _) in KUROMOJI_DICT_FILES {
            builder
                .append_path_with_name(src.join(name), format!("package/dict/{name}"))
                .unwrap();
        }
        let tgz = builder.into_inner().unwrap().finish().unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            if let Ok((mut sock, _)) = listener.accept().await {
                let mut buf = vec![0u8; 1024];
                let _ = sock.read(&mut buf).await;
                let head = format!(
                    "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                    tgz.len()
                );
                let _ = sock.write_all(head.as_bytes()).await;
                let _ = sock.write_all(&tgz).await;
                let _ = sock.shutdown().await;
            }
        });

        let dict_dir = std::env::temp_dir().join(format!("biu-kuro-tgz-{}", uuid::Uuid::new_v4()));
        let url = format!("http://{addr}/kuromoji-0.1.2.tgz");
        ensure_kuromoji_dict_in_dir(&dict_dir, &[], Some(url.as_str()))
            .await
            .unwrap();
        for (name, sha) in KUROMOJI_DICT_FILES {
            assert!(verify_dict_file(&dict_dir.join(name), sha), "{name}");
        }
        let _ = std::fs::remove_dir_all(&dict_dir);
    }
}
