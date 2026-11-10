use super::*;
use crate::bili::ProxySettings;
use std::path::PathBuf;
use std::sync::Mutex;
use tokio::net::TcpListener;

struct Server {
    address: std::net::SocketAddr,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl Server {
    fn url(&self, path: &str) -> String {
        format!("http://{}{path}", self.address)
    }
}

async fn upstream(reply: impl Fn(&str) -> Vec<u8> + Send + Sync + 'static) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let reply = Arc::new(reply);
    let task = tokio::spawn(async move {
        let mut connections = tokio::task::JoinSet::new();
        loop {
            tokio::select! {
                accepted = listener.accept() => {
                    let (mut socket, _) = accepted.unwrap();
                    let reply = reply.clone();
                    connections.spawn(async move {
                        let mut request = Vec::new();
                        while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                            let mut buffer = [0; 2048];
                            let n = socket.read(&mut buffer).await.unwrap();
                            assert_ne!(n, 0);
                            request.extend_from_slice(&buffer[..n]);
                        }
                        let response = reply(&String::from_utf8(request).unwrap());
                        let _ = socket.write_all(&response).await;
                        let _ = socket.shutdown().await;
                    });
                }
                joined = connections.join_next(), if !connections.is_empty() => {
                    joined.unwrap().unwrap();
                }
            }
        }
    });
    Server { address, task }
}

fn response(status: &str, headers: &str, body: &[u8]) -> Vec<u8> {
    let mut data = format!(
        "HTTP/1.1 {status}\r\nContent-Length: {}\r\n{headers}Connection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    data.extend_from_slice(body);
    data
}

struct Proxy {
    server: Server,
    state: Arc<BiliState>,
    directory: PathBuf,
}

impl Drop for Proxy {
    fn drop(&mut self) {
        self.server.task.abort();
        let _ = std::fs::remove_dir_all(&self.directory);
    }
}

impl Proxy {
    async fn start() -> Self {
        let directory = std::env::temp_dir().join(format!("biu-media-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&directory).unwrap();
        let state = Arc::new(
            BiliState::new(
                directory.join("cookies.json"),
                Some(ProxySettings::default()),
            )
            .unwrap(),
        );
        state.register_media_host("127.0.0.1");
        state.register_media_host("localhost");
        state.register_local_dir(&directory);
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let task = tokio::spawn(serve_http(listener, "media-test".into(), state.clone()));
        Self {
            server: Server { address, task },
            state,
            directory,
        }
    }

    fn remote(&self, url: &str) -> String {
        let query: String = url::form_urlencoded::byte_serialize(url.as_bytes()).collect();
        self.server.url(&format!("/media-test/remote?u={query}"))
    }
}

fn client() -> reqwest::Client {
    reqwest::Client::builder().no_proxy().build().unwrap()
}

#[tokio::test]
async fn a01_proxy_requests_identity_and_preserves_unexpected_encoding() {
    use std::io::Write;
    let mut encoder = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::fast());
    encoder.write_all(b"audio representation").unwrap();
    let encoded = encoder.finish().unwrap();
    let body = encoded.clone();
    let requests = Arc::new(Mutex::new(Vec::new()));
    let captured = requests.clone();
    let upstream = upstream(move |request| {
        captured.lock().unwrap().push(request.to_ascii_lowercase());
        response(
            "206 Partial Content",
            "Content-Type: audio/mp4\r\nContent-Encoding: gzip\r\nContent-Range: bytes 0-39/40\r\n",
            &body,
        )
    })
    .await;
    let proxy = Proxy::start().await;
    let response = client()
        .get(proxy.remote(&upstream.url("/encoded.m4s")))
        .header("Range", "bytes=0-39")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 206);
    assert_eq!(response.headers().get("content-encoding").unwrap(), "gzip");
    assert_eq!(
        response.headers()["content-length"],
        encoded.len().to_string()
    );
    assert!(!response.headers().contains_key("transfer-encoding"));
    assert_eq!(response.bytes().await.unwrap().as_ref(), encoded);
    let requests = requests.lock().unwrap();
    assert!(requests[0].contains("accept-encoding: identity\r\n"));
    assert!(requests[0].contains("range: bytes=0-39\r\n"));
}

#[tokio::test]
async fn a03_proxy_preserves_chunked_completion_and_truncation() {
    for complete in [false, true] {
        let upstream = upstream(move |_| {
            let mut data = b"HTTP/1.1 200 OK\r\nContent-Type: audio/mp4\r\nTransfer-Encoding: chunked\r\nConnection: close\r\n\r\n4\r\nAUDI\r\n".to_vec();
            if complete {
                data.extend_from_slice(b"0\r\n\r\n");
            }
            data
        })
        .await;
        let proxy = Proxy::start().await;
        let response = client()
            .get(proxy.remote(&upstream.url("/chunked.m4s")))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        // Check the body first: a truncated upstream must not become successful EOF.
        let headers = response.headers().clone();
        let body = response.bytes().await;
        if complete {
            assert_eq!(body.unwrap().as_ref(), b"AUDI");
        } else {
            assert!(body.is_err(), "truncated upstream was reported as complete");
        }
        assert_eq!(headers["transfer-encoding"], "chunked");
        assert!(!headers.contains_key("content-length"));
    }
}

#[tokio::test]
async fn a03_proxy_keeps_known_length_and_head_framing() {
    let upstream = upstream(|request| {
        if request.starts_with("HEAD ") {
            b"HTTP/1.1 200 OK\r\nContent-Type: audio/mp4\r\nConnection: close\r\n\r\n".to_vec()
        } else {
            response("200 OK", "Content-Type: audio/mp4\r\n", b"audio")
        }
    })
    .await;
    let proxy = Proxy::start().await;
    let url = proxy.remote(&upstream.url("/framing.m4s"));
    let response = client().get(&url).send().await.unwrap();
    assert_eq!(response.headers()["content-length"], "5");
    assert!(!response.headers().contains_key("transfer-encoding"));
    assert_eq!(response.bytes().await.unwrap().as_ref(), b"audio");
    let response = client().head(&url).send().await.unwrap();
    assert!(!response.headers().contains_key("transfer-encoding"));
    assert!(response.bytes().await.unwrap().is_empty());
}

#[tokio::test]
async fn a04_media_redirects_follow_registered_hosts_but_api_stays_restricted() {
    let destination =
        upstream(|_| response("200 OK", "Content-Type: audio/mp4\r\n", b"audio")).await;
    let port = destination.address.port();
    for host in ["localhost", "127.0.0.1"] {
        let initial = upstream(move |_| {
            response(
                "302 Found",
                &format!("Location: http://{host}:{port}/redirected.m4s\r\n"),
                b"",
            )
        })
        .await;
        let proxy = Proxy::start().await;
        // Rebuilding both clients must retain the shared dynamic host registry.
        proxy
            .state
            .update_proxy(Some(ProxySettings::default()))
            .unwrap();
        let response = client()
            .get(proxy.remote(&initial.url("/original.m4s")))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 200);
        assert_eq!(response.bytes().await.unwrap().as_ref(), b"audio");
        let error = proxy
            .state
            .client()
            .get(initial.url("/api"))
            .send()
            .await
            .unwrap_err();
        assert!(error.is_redirect());
    }
}

#[tokio::test]
async fn a04_media_redirects_reject_unsafe_hops_and_loops() {
    let proxy = Proxy::start().await;
    for location in [
        "http://unregistered.invalid/audio.m4s",
        "http://user:pass@127.0.0.1/audio.m4s",
        "http://user:pass@cdn.bilivideo.com/audio.m4s",
        "ftp://127.0.0.1/audio.m4s",
        "ftp://cdn.bilivideo.com/audio.m4s",
        "/loop.m4s",
    ] {
        let initial =
            upstream(move |_| response("302 Found", &format!("Location: {location}\r\n"), b""))
                .await;
        let response = client()
            .get(proxy.remote(&initial.url("/original.m4s")))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status(), 502, "{location}");
    }
    // A permitted first hop must not authorize an unregistered second hop.
    let denied = upstream(|_| {
        response(
            "302 Found",
            "Location: http://unregistered.invalid/audio.m4s\r\n",
            b"",
        )
    })
    .await;
    let target = denied.url("/second-hop.m4s");
    let initial =
        upstream(move |_| response("302 Found", &format!("Location: {target}\r\n"), b"")).await;
    assert_eq!(
        client()
            .get(proxy.remote(&initial.url("/first-hop.m4s")))
            .send()
            .await
            .unwrap()
            .status(),
        502
    );
}

#[tokio::test]
async fn a05_local_adts_aac_and_mp4_use_distinct_mime_types() {
    let proxy = Proxy::start().await;
    for (name, body, expected) in [
        ("track.aac", &b"\xff\xf1ADTS"[..], "audio/aac"),
        ("track.m4a", &b"\0\0\0\x18ftypM4A "[..], "audio/mp4"),
    ] {
        let path = proxy.directory.join(name);
        std::fs::write(&path, body).unwrap();
        let query: String =
            url::form_urlencoded::byte_serialize(path.to_str().unwrap().as_bytes()).collect();
        let response = client()
            .get(proxy.server.url(&format!("/media-test/local?p={query}")))
            .send()
            .await
            .unwrap();
        assert_eq!(response.headers()["content-type"], expected);
        assert_eq!(response.bytes().await.unwrap().as_ref(), body);
    }
    assert_eq!(
        normalize_media_content_type(None, "/track.aac"),
        "audio/aac"
    );
}
