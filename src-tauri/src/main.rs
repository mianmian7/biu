#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod bili;
mod media;
mod store;

use bili::{
    bili_request, cookie_get, cookie_set, inject_auth_cookies, parse_proxy_settings_value,
    proxy_set, BiliState,
};
use media::{media_base, serve_http, MediaBase};
use std::sync::Arc;
use store::{get_close_window_option, get_saved_proxy_settings, store_clear, store_get, store_set};
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
    let builder = tauri::WebviewWindowBuilder::new(
        &app,
        "main",
        tauri::WebviewUrl::App("index.html".into()),
    )
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
    let opt = get_close_window_option(&app);
    if opt == "hide" {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.hide();
            return Ok(());
        }
    }
    app.exit(0);
    Ok(())
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
            if let Some(w) = app.get_webview_window("main") {
                let _ = w.unminimize();
                let _ = w.show();
                let _ = w.set_focus();
            }
        }))
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_log::Builder::new().build())
        .plugin(tauri_plugin_window_state::Builder::default().build())
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
            devtools_toggle,
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
                BiliState::new(cookie_path, saved_proxy)
                    .expect("failed to initialize BiliState"),
            );
            app.manage(bili_state.clone());

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

            if let Some(main_win) = app.get_webview_window("main") {
                let app_handle = app.handle().clone();
                let win_clone = main_win.clone();
                main_win.on_window_event(move |event| {
                    if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                        if get_close_window_option(&app_handle) == "hide" {
                            api.prevent_close();
                            let _ = win_clone.hide();
                        }
                    }
                });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running biu application");
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

        assert!(is_bili_audio_cdn_host(
            "upos-sz-mirrorcos.bilivideo.com"
        ));
        assert!(is_bili_audio_cdn_host(
            "xy123x45x67x89xy.mcdn.bilivideo.cn"
        ));
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
        assert!(!state2.is_media_url_allowed(
            &Url::parse("https://evil.example.com/audio.m4s").unwrap()
        ));

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
        assert_eq!(parse_http_request(&good, "wrongtok", host).unwrap_err(), 403);
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
                assert!(req_str.to_ascii_lowercase().contains("referer: https://www.bilibili.com/"));
                assert!(req_str.to_ascii_lowercase().contains("range: bytes=100-199"));
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
            let encoded: String = url::form_urlencoded::byte_serialize(target_url.as_bytes()).collect();
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
            assert!(head.to_ascii_lowercase().contains("content-range: bytes 100-199/4096"));
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
            },
        )
        .await;
        assert!(err.is_err());
    }
}
