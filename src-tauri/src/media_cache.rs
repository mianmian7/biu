use crate::bili::{BILI_ORIGIN, BILI_REFERER};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::io::AsyncWriteExt;
use url::Url;

pub const AUDIO_CACHE_MAX_BYTES: u64 = 1024 * 1024 * 1024;

#[derive(Deserialize, Serialize)]
struct Metadata {
    content_type: String,
    total_len: u64,
}

pub struct CachedAudio {
    pub file: tokio::fs::File,
    pub content_type: String,
}

pub struct MediaCache {
    directory: PathBuf,
    max_bytes: u64,
    // Serialize lookup/open/touch and commit/eviction, never network or playback I/O.
    storage: tokio::sync::Mutex<()>,
    in_progress: Mutex<HashSet<String>>,
}

// A cancelled task must also release its key and remove its unfinished files.
struct DownloadGuard {
    cache: Arc<MediaCache>,
    key: String,
}

impl Drop for DownloadGuard {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(self.cache.path(&self.key, "part"));
        let _ = std::fs::remove_file(self.cache.path(&self.key, "json.part"));
        self.cache.in_progress.lock().unwrap().remove(&self.key);
    }
}

impl MediaCache {
    pub fn new(directory: PathBuf) -> std::io::Result<Self> {
        Self::with_limit(directory, AUDIO_CACHE_MAX_BYTES)
    }

    fn with_limit(directory: PathBuf, max_bytes: u64) -> std::io::Result<Self> {
        std::fs::create_dir_all(&directory)?;
        // A crash or kill during a fill leaves `.part` files behind; nothing is filling yet.
        for entry in std::fs::read_dir(&directory)?.flatten() {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) == Some("part") {
                let _ = std::fs::remove_file(path);
            }
        }
        Ok(Self {
            directory,
            max_bytes,
            storage: tokio::sync::Mutex::new(()),
            in_progress: Mutex::new(HashSet::new()),
        })
    }

    // Callers must check the registered-host allowlist before using this key.
    pub fn key(url: &Url) -> Option<String> {
        let path = url.path();
        if ![".m4s", ".flac", ".mp3", ".m4a", ".mp4"]
            .iter()
            .any(|suffix| path.ends_with(suffix))
        {
            return None;
        }
        Some(format!("{:x}", Sha256::digest(path.as_bytes())))
    }

    fn path(&self, key: &str, extension: &str) -> PathBuf {
        self.directory.join(format!("{key}.{extension}"))
    }

    pub async fn lookup(&self, key: &str) -> Option<CachedAudio> {
        let _storage = self.storage.lock().await;
        self.open_entry(key).await
    }

    async fn open_entry(&self, key: &str) -> Option<CachedAudio> {
        let metadata: Metadata =
            serde_json::from_slice(&tokio::fs::read(self.path(key, "json")).await.ok()?).ok()?;
        // Keep an open handle so an eviction cannot invalidate an active reader.
        let file = tokio::fs::OpenOptions::new()
            .read(true)
            .write(true) // Windows requires write access to update the LRU timestamp.
            .open(self.path(key, "bin"))
            .await
            .ok()?;
        if file.metadata().await.ok()?.len() != metadata.total_len
            || reqwest::header::HeaderValue::from_str(&metadata.content_type).is_err()
        {
            return None;
        }
        // File timestamps supply the persistent LRU order, including across restarts.
        let handle = file.into_std().await;
        let _ = handle.set_modified(SystemTime::now());
        log::info!("[media_cache] hit {} {}", &key[..8], metadata.total_len);
        Some(CachedAudio {
            file: tokio::fs::File::from_std(handle),
            content_type: metadata.content_type,
        })
    }

    pub fn fill(
        self: &Arc<Self>,
        key: String,
        url: Url,
        client: reqwest::Client,
        idle_timeout: Duration,
    ) {
        if !self.in_progress.lock().unwrap().insert(key.clone()) {
            return;
        }
        let guard = DownloadGuard {
            cache: self.clone(),
            key,
        };
        tauri::async_runtime::spawn(async move {
            let _ = guard
                .cache
                .download(&guard.key, url, client, idle_timeout)
                .await;
            // The guard cleans up partial files on every error (and on cancellation).
            drop(guard);
        });
    }

    async fn download(
        &self,
        key: &str,
        url: Url,
        client: reqwest::Client,
        idle_timeout: Duration,
    ) -> Result<(), ()> {
        // A request can miss just before another fill commits. Recheck under the
        // storage lock so that it does not download a second copy after that fill.
        {
            let _storage = self.storage.lock().await;
            if self.open_entry(key).await.is_some() {
                return Ok(());
            }
        }
        let response = tokio::time::timeout(
            idle_timeout,
            client
                .get(url.clone())
                .header("Referer", BILI_REFERER)
                .header("Origin", BILI_ORIGIN)
                .send(),
        )
        .await
        .map_err(|_| ())?
        .map_err(|_| ())?;
        // Never commit a range, an error page, or a body with no verifiable length.
        if response.status() != reqwest::StatusCode::OK
            || response.headers().contains_key("content-range")
        {
            return Err(());
        }
        let expected_len = response.content_length().ok_or(())?;
        if expected_len > self.max_bytes {
            return Err(());
        }
        let content_type = crate::media::normalize_media_content_type(
            response
                .headers()
                .get("content-type")
                .and_then(|v| v.to_str().ok()),
            url.path(),
        );
        let mut file = tokio::fs::File::create(self.path(key, "part"))
            .await
            .map_err(|_| ())?;
        let mut stream = response.bytes_stream();
        let mut total_len = 0u64;
        while let Some(chunk) = tokio::time::timeout(idle_timeout, stream.next())
            .await
            .map_err(|_| ())?
        {
            let chunk = chunk.map_err(|_| ())?;
            total_len = total_len.checked_add(chunk.len() as u64).ok_or(())?;
            if total_len > expected_len {
                return Err(());
            }
            file.write_all(&chunk).await.map_err(|_| ())?;
        }
        if total_len != expected_len {
            return Err(());
        }
        file.flush().await.map_err(|_| ())?;
        drop(file);
        let metadata = serde_json::to_vec(&Metadata {
            content_type,
            total_len,
        })
        .map_err(|_| ())?;
        tokio::fs::write(self.path(key, "json.part"), metadata)
            .await
            .map_err(|_| ())?;
        let _storage = self.storage.lock().await;
        // Invalidate any stale metadata before publishing the new data. The JSON
        // rename is the commit marker; readers require both files and equal sizes.
        let _ = tokio::fs::remove_file(self.path(key, "json")).await;
        tokio::fs::rename(self.path(key, "part"), self.path(key, "bin"))
            .await
            .map_err(|_| ())?;
        if tokio::fs::rename(self.path(key, "json.part"), self.path(key, "json"))
            .await
            .is_err()
        {
            let _ = tokio::fs::remove_file(self.path(key, "bin")).await;
            return Err(());
        }
        log::info!("[media_cache] stored {} {}", &key[..8], total_len);
        self.evict().await.map_err(|_| ())
    }

    async fn evict(&self) -> std::io::Result<()> {
        let mut entries = tokio::fs::read_dir(&self.directory).await?;
        let mut files = Vec::new();
        let mut total = 0u64;
        while let Some(entry) = entries.next_entry().await? {
            let path = entry.path();
            if path.extension().and_then(|s| s.to_str()) != Some("bin") {
                continue;
            }
            let metadata = entry.metadata().await?;
            let key = path.file_stem().and_then(|s| s.to_str()).unwrap_or("");
            if key.len() != 64 || !key.bytes().all(|b| b.is_ascii_hexdigit()) {
                continue;
            }
            total = total.saturating_add(metadata.len());
            files.push((metadata.modified()?, metadata.len(), key.to_string()));
        }
        files.sort_by_key(|(modified, _, _)| *modified);
        for (_, size, key) in files {
            if total <= self.max_bytes {
                break;
            }
            // A file that cannot be removed (locked on Windows) must not stop the rest.
            if tokio::fs::remove_file(self.path(&key, "bin"))
                .await
                .is_err()
            {
                continue;
            }
            let _ = tokio::fs::remove_file(self.path(&key, "json")).await;
            total = total.saturating_sub(size);
            log::info!("[media_cache] evicted {} {}", &key[..8], size);
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::bili::BiliState;
    use crate::media::serve_http;
    use std::sync::atomic::{AtomicUsize, Ordering};
    use tokio::io::AsyncReadExt;
    use tokio::sync::Semaphore;

    const PAYLOAD: &[u8] = b"complete audio bytes";

    struct TestDirectory(PathBuf);

    impl TestDirectory {
        fn new() -> Self {
            Self(std::env::temp_dir().join(format!("biu-cache-{}", uuid::Uuid::new_v4())))
        }
    }

    impl Drop for TestDirectory {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    struct Upstream {
        address: std::net::SocketAddr,
        requests: Arc<AtomicUsize>,
        full_requests: Arc<AtomicUsize>,
        task: tokio::task::JoinHandle<()>,
    }

    impl Drop for Upstream {
        fn drop(&mut self) {
            self.task.abort();
        }
    }

    impl Upstream {
        async fn start(interrupt: bool, gate: Option<Arc<Semaphore>>) -> Self {
            let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
            let address = listener.local_addr().unwrap();
            let requests = Arc::new(AtomicUsize::new(0));
            let full_requests = Arc::new(AtomicUsize::new(0));
            let count = requests.clone();
            let full = full_requests.clone();
            let task = tokio::spawn(async move {
                let mut connections = tokio::task::JoinSet::new();
                loop {
                    tokio::select! {
                        accepted = listener.accept() => {
                            let (mut socket, _) = accepted.unwrap();
                            let count = count.clone();
                            let full = full.clone();
                            let gate = gate.clone();
                            connections.spawn(async move {
                                let mut request = Vec::new();
                                while !request.windows(4).any(|w| w == b"\r\n\r\n") {
                                    let mut buf = [0; 2048];
                                    let n = socket.read(&mut buf).await.unwrap();
                                    assert_ne!(n, 0);
                                    request.extend_from_slice(&buf[..n]);
                                }
                                let request = String::from_utf8(request).unwrap().to_ascii_lowercase();
                                assert!(request.contains("referer: https://www.bilibili.com/\r\n"));
                                assert!(request.contains("origin: https://www.bilibili.com\r\n"));
                                count.fetch_add(1, Ordering::SeqCst);
                                let ranged = request.contains("range: bytes=2-5\r\n");
                                if !ranged {
                                    full.fetch_add(1, Ordering::SeqCst);
                                }
                                let (status, body, content_range) = if ranged {
                                    ("206 Partial Content", &PAYLOAD[2..6], format!("Content-Range: bytes 2-5/{}\r\n", PAYLOAD.len()))
                                } else {
                                    ("200 OK", PAYLOAD, String::new())
                                };
                                let header = format!("HTTP/1.1 {status}\r\nContent-Type: audio/mp4\r\nContent-Length: {}\r\n{content_range}Connection: close\r\n\r\n", body.len());
                                socket.write_all(header.as_bytes()).await.unwrap();
                                if let Some(gate) = gate {
                                    let _permit = gate.acquire().await.unwrap();
                                }
                                let body = if interrupt { &body[..2] } else { body };
                                let _ = socket.write_all(body).await;
                                let _ = socket.shutdown().await;
                            });
                        }
                        joined = connections.join_next(), if !connections.is_empty() => {
                            joined.unwrap().unwrap();
                        }
                    }
                }
            });
            Self {
                address,
                requests,
                full_requests,
                task,
            }
        }

        fn url(&self, path: &str) -> Url {
            Url::parse(&format!("http://{}{path}", self.address)).unwrap()
        }
    }

    async fn wait_until(mut ready: impl FnMut() -> bool) {
        tokio::time::timeout(Duration::from_secs(5), async {
            while !ready() {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("cache operation did not finish");
    }

    async fn fill_and_wait(cache: &Arc<MediaCache>, url: Url) {
        let key = MediaCache::key(&url).unwrap();
        cache.fill(key, url, test_client(), Duration::from_secs(2));
        wait_until(|| cache.in_progress.lock().unwrap().is_empty()).await;
    }

    fn test_client() -> reqwest::Client {
        reqwest::Client::builder().no_proxy().build().unwrap()
    }

    fn proxy_url(address: std::net::SocketAddr, remote: &Url) -> String {
        let encoded: String =
            url::form_urlencoded::byte_serialize(remote.as_str().as_bytes()).collect();
        format!("http://{address}/cache-test/remote?u={encoded}")
    }

    #[test]
    fn startup_removes_partial_files_left_by_a_crash() {
        let directory = TestDirectory::new();
        std::fs::create_dir_all(&directory.0).unwrap();
        let key = "a".repeat(64);
        for name in [format!("{key}.part"), format!("{key}.json.part")] {
            std::fs::write(directory.0.join(name), b"partial").unwrap();
        }
        std::fs::write(directory.0.join(format!("{key}.bin")), PAYLOAD).unwrap();
        MediaCache::new(directory.0.clone()).unwrap();
        assert!(!directory.0.join(format!("{key}.part")).exists());
        assert!(!directory.0.join(format!("{key}.json.part")).exists());
        assert!(directory.0.join(format!("{key}.bin")).exists());
    }

    #[tokio::test]
    async fn eviction_skips_a_file_it_cannot_remove() {
        let directory = TestDirectory::new();
        let cache = MediaCache::with_limit(directory.0.clone(), 30).unwrap();
        // The oldest entry cannot be removed: its `.bin` is a directory.
        let stuck = "a".repeat(64);
        std::fs::create_dir(directory.0.join(format!("{stuck}.bin"))).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let older = "b".repeat(64);
        std::fs::write(directory.0.join(format!("{older}.bin")), [0u8; 20]).unwrap();
        std::thread::sleep(std::time::Duration::from_millis(20));
        let newer = "c".repeat(64);
        std::fs::write(directory.0.join(format!("{newer}.bin")), [0u8; 20]).unwrap();
        let stuck_size = std::fs::metadata(directory.0.join(format!("{stuck}.bin")))
            .unwrap()
            .len();
        // Removing `older` brings the total within the limit only if the stuck entry is skipped.
        let cache = MediaCache {
            max_bytes: stuck_size + 20,
            ..cache
        };
        cache.evict().await.unwrap();
        assert!(directory.0.join(format!("{stuck}.bin")).exists());
        assert!(!directory.0.join(format!("{older}.bin")).exists());
        assert!(directory.0.join(format!("{newer}.bin")).exists());
    }

    #[test]
    fn keys_ignore_mirrors_and_signatures_but_preserve_path_and_quality() {
        let first =
            Url::parse("https://a.example/upgcxcode/123-1-30280.m4s?deadline=1&upsig=secret")
                .unwrap();
        let second =
            Url::parse("https://b.example/upgcxcode/123-1-30280.m4s?deadline=2&upsig=other")
                .unwrap();
        let key = MediaCache::key(&first).unwrap();
        assert_eq!(
            key,
            format!("{:x}", Sha256::digest(first.path().as_bytes()))
        );
        assert_eq!(Some(key.clone()), MediaCache::key(&second));
        assert_ne!(
            Some(key),
            MediaCache::key(&Url::parse("https://a.example/upgcxcode/123-1-30216.m4s").unwrap())
        );
        for suffix in ["m4s", "flac", "mp3", "m4a", "mp4"] {
            assert!(MediaCache::key(
                &Url::parse(&format!("https://a.example/a.{suffix}")).unwrap()
            )
            .is_some());
        }
        assert!(
            MediaCache::key(&Url::parse("https://a.example/audio.txt?name=a.mp3").unwrap())
                .is_none()
        );
    }

    #[tokio::test]
    async fn loopback_fills_cache_and_serves_mirror_range_and_head_without_upstream() {
        let dir = TestDirectory::new();
        let cache = Arc::new(MediaCache::new(dir.0.join("audio-cache")).unwrap());
        let upstream = Upstream::start(false, None).await;
        let remote = upstream.url("/audio.m4s?deadline=1&upsig=first");
        let mut state = BiliState::new(dir.0.join("cookies.json"), None).unwrap();
        state.media_cache = Some(cache.clone());
        state.register_media_host("127.0.0.1");
        state.register_media_host("localhost");
        // This CDN host is allowed by the proxy's built-in rules, but has not
        // been registered by playurl and therefore must not use the cache.
        *state.client.write().unwrap() = reqwest::Client::builder()
            .no_proxy()
            .resolve("cache-test.bilivideo.com", upstream.address)
            .build()
            .unwrap();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let proxy = tokio::spawn(serve_http(listener, "cache-test".into(), Arc::new(state)));
        let client = test_client();
        // First playback forwards its Range, while a separate full GET fills the cache.
        let first = client
            .get(proxy_url(address, &remote))
            .header("Range", "bytes=2-5")
            .send()
            .await
            .unwrap();
        assert_eq!(first.status(), 206);
        assert_eq!(first.bytes().await.unwrap().as_ref(), &PAYLOAD[2..6]);
        wait_until(|| cache.in_progress.lock().unwrap().is_empty()).await;
        assert_eq!(upstream.requests.load(Ordering::SeqCst), 2);
        assert_eq!(upstream.full_requests.load(Ordering::SeqCst), 1);
        let key = MediaCache::key(&remote).unwrap();
        assert_eq!(
            tokio::fs::read(cache.path(&key, "bin")).await.unwrap(),
            PAYLOAD
        );
        assert!(!cache.path(&key, "part").exists());
        // Reopen from disk and verify that metadata survives an application restart.
        let reopened = MediaCache::new(dir.0.join("audio-cache")).unwrap();
        assert!(reopened.lookup(&key).await.is_some());
        // A different host, port and signature must hit, even with an unreachable CDN.
        let mirror = Url::parse("http://localhost:1/audio.m4s?deadline=2&upsig=changed").unwrap();
        let second = client
            .get(proxy_url(address, &mirror))
            .send()
            .await
            .unwrap();
        assert_eq!(second.status(), 200);
        assert_eq!(second.headers()["content-type"], "audio/mp4");
        assert_eq!(second.bytes().await.unwrap().as_ref(), PAYLOAD);
        for (range, expected, content_range) in [
            (
                "bytes=2-5",
                &PAYLOAD[2..6],
                format!("bytes 2-5/{}", PAYLOAD.len()),
            ),
            (
                "bytes=-3",
                &PAYLOAD[PAYLOAD.len() - 3..],
                format!(
                    "bytes {}-{}/{}",
                    PAYLOAD.len() - 3,
                    PAYLOAD.len() - 1,
                    PAYLOAD.len()
                ),
            ),
        ] {
            let response = client
                .get(proxy_url(address, &mirror))
                .header("Range", range)
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 206);
            assert_eq!(response.headers()["content-range"], content_range);
            assert_eq!(response.bytes().await.unwrap().as_ref(), expected);
        }
        let head = client
            .head(proxy_url(address, &mirror))
            .header("Range", "bytes=2-5")
            .send()
            .await
            .unwrap();
        assert_eq!(head.status(), 206);
        assert_eq!(head.headers()["content-length"], "4");
        assert!(head.bytes().await.unwrap().is_empty());
        let head = client
            .head(proxy_url(address, &mirror))
            .send()
            .await
            .unwrap();
        assert_eq!(head.status(), 200);
        assert_eq!(head.headers()["content-length"], PAYLOAD.len().to_string());
        assert!(head.bytes().await.unwrap().is_empty());
        let invalid = client
            .get(proxy_url(address, &mirror))
            .header("Range", "bytes=999-")
            .send()
            .await
            .unwrap();
        assert_eq!(invalid.status(), 416);
        assert_eq!(
            invalid.headers()["content-range"],
            format!("bytes */{}", PAYLOAD.len())
        );
        // Cached data cannot bypass the host allowlist.
        let denied = Url::parse("https://evil.example/audio.m4s").unwrap();
        assert_eq!(
            client
                .get(proxy_url(address, &denied))
                .send()
                .await
                .unwrap()
                .status(),
            403
        );
        // HEAD misses and unsupported paths do not start a background fill.
        let missing = upstream.url("/head-only.m4s");
        assert_eq!(
            client
                .head(proxy_url(address, &missing))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        let unsupported = upstream.url("/audio.txt");
        assert_eq!(
            client
                .get(proxy_url(address, &unsupported))
                .send()
                .await
                .unwrap()
                .status(),
            200
        );
        assert_eq!(upstream.requests.load(Ordering::SeqCst), 4);
        let unregistered = Url::parse(&format!(
            "http://cache-test.bilivideo.com:{}/audio.m4s",
            upstream.address.port()
        ))
        .unwrap();
        let forwarded = client
            .get(proxy_url(address, &unregistered))
            .send()
            .await
            .unwrap();
        assert_eq!(forwarded.bytes().await.unwrap().as_ref(), PAYLOAD);
        assert_eq!(upstream.requests.load(Ordering::SeqCst), 5);
        assert!(cache.in_progress.lock().unwrap().is_empty());
        assert!(!cache
            .path(&MediaCache::key(&missing).unwrap(), "bin")
            .exists());
        proxy.abort();
        proxy.await.unwrap_err();
    }

    #[tokio::test]
    async fn concurrent_fills_are_deduplicated_and_partial_files_are_not_hits() {
        let dir = TestDirectory::new();
        let cache = Arc::new(MediaCache::new(dir.0.clone()).unwrap());
        let gate = Arc::new(Semaphore::new(0));
        let upstream = Upstream::start(false, Some(gate.clone())).await;
        let url = upstream.url("/deduplicated.flac");
        let key = MediaCache::key(&url).unwrap();
        cache.fill(
            key.clone(),
            url.clone(),
            test_client(),
            Duration::from_secs(2),
        );
        wait_until(|| {
            upstream.requests.load(Ordering::SeqCst) == 1 && cache.path(&key, "part").exists()
        })
        .await;
        for _ in 0..20 {
            cache.fill(
                key.clone(),
                url.clone(),
                test_client(),
                Duration::from_secs(2),
            );
        }
        assert!(cache.lookup(&key).await.is_none());
        assert!(!cache.path(&key, "bin").exists());
        gate.add_permits(1);
        wait_until(|| cache.in_progress.lock().unwrap().is_empty()).await;
        assert_eq!(upstream.requests.load(Ordering::SeqCst), 1);
        assert!(cache.lookup(&key).await.is_some());
    }

    #[tokio::test]
    async fn interrupted_and_idle_downloads_are_removed_and_can_retry() {
        let dir = TestDirectory::new();
        let cache = Arc::new(MediaCache::new(dir.0.clone()).unwrap());
        let interrupted = Upstream::start(true, None).await;
        let key = MediaCache::key(&interrupted.url("/retry.mp3")).unwrap();
        fill_and_wait(&cache, interrupted.url("/retry.mp3")).await;
        assert!(cache.lookup(&key).await.is_none());
        for extension in ["bin", "json", "part", "json.part"] {
            assert!(!cache.path(&key, extension).exists());
        }
        let gate = Arc::new(Semaphore::new(0));
        let idle = Upstream::start(false, Some(gate)).await;
        cache.fill(
            key.clone(),
            idle.url("/retry.mp3"),
            test_client(),
            Duration::from_millis(100),
        );
        wait_until(|| cache.in_progress.lock().unwrap().is_empty()).await;
        assert!(cache.lookup(&key).await.is_none());
        assert!(!cache.path(&key, "part").exists());
        let healthy = Upstream::start(false, None).await;
        fill_and_wait(&cache, healthy.url("/retry.mp3")).await;
        assert!(cache.lookup(&key).await.is_some());
    }

    #[tokio::test]
    async fn capacity_evicts_least_recently_used_and_preserves_open_readers() {
        let dir = TestDirectory::new();
        let cache =
            Arc::new(MediaCache::with_limit(dir.0.clone(), (PAYLOAD.len() * 2) as u64).unwrap());
        let upstream = Upstream::start(false, None).await;
        let first = upstream.url("/first.m4a");
        let second = upstream.url("/second.m4a");
        let third = upstream.url("/third.mp4");
        let first_key = MediaCache::key(&first).unwrap();
        let second_key = MediaCache::key(&second).unwrap();
        let third_key = MediaCache::key(&third).unwrap();
        fill_and_wait(&cache, first).await;
        fill_and_wait(&cache, second).await;
        // Fixed timestamps avoid depending on the filesystem's timestamp granularity.
        for (key, seconds) in [(&first_key, 1), (&second_key, 2)] {
            std::fs::File::open(cache.path(key, "bin"))
                .unwrap()
                .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds))
                .unwrap();
        }
        let mut reader = cache.lookup(&second_key).await.unwrap().file;
        std::fs::File::open(cache.path(&second_key, "bin"))
            .unwrap()
            .set_modified(SystemTime::UNIX_EPOCH + Duration::from_secs(2))
            .unwrap();
        // Touch the first entry; the second entry is now oldest.
        assert!(cache.lookup(&first_key).await.is_some());
        fill_and_wait(&cache, third).await;
        assert!(cache.lookup(&first_key).await.is_some());
        assert!(cache.lookup(&second_key).await.is_none());
        assert!(!cache.path(&second_key, "json").exists());
        assert!(cache.lookup(&third_key).await.is_some());
        let mut bytes = Vec::new();
        reader.read_to_end(&mut bytes).await.unwrap();
        assert_eq!(bytes, PAYLOAD);
        let total: u64 = std::fs::read_dir(&dir.0)
            .unwrap()
            .map(|e| e.unwrap().path())
            .filter(|p| p.extension().is_some_and(|e| e == "bin"))
            .map(|p| std::fs::metadata(p).unwrap().len())
            .sum();
        assert!(total <= cache.max_bytes);
    }

    #[tokio::test]
    async fn oversized_or_corrupt_entries_are_not_served() {
        let dir = TestDirectory::new();
        let cache = Arc::new(MediaCache::with_limit(dir.0.clone(), 2).unwrap());
        let upstream = Upstream::start(false, None).await;
        let url = upstream.url("/too-large.mp3");
        let key = MediaCache::key(&url).unwrap();
        fill_and_wait(&cache, url).await;
        assert!(cache.lookup(&key).await.is_none());
        assert!(!cache.path(&key, "part").exists());
        tokio::fs::write(cache.path(&key, "bin"), b"x")
            .await
            .unwrap();
        tokio::fs::write(
            cache.path(&key, "json"),
            br#"{"content_type":"audio/mp4","total_len":2}"#,
        )
        .await
        .unwrap();
        assert!(cache.lookup(&key).await.is_none());
        tokio::fs::write(
            cache.path(&key, "json"),
            br#"{"content_type":"audio/mp4\r\nx-injected: yes","total_len":1}"#,
        )
        .await
        .unwrap();
        assert!(cache.lookup(&key).await.is_none());
    }

    #[test]
    fn normalizes_octet_stream_content_type_from_audio_path() {
        use crate::media::normalize_media_content_type;

        assert_eq!(
            normalize_media_content_type(
                Some("application/octet-stream"),
                "/upgcxcode/00/56/1116735600/1116735600-1-30232.m4s"
            ),
            "audio/mp4"
        );
        assert_eq!(
            normalize_media_content_type(
                Some("binary/octet-stream"),
                "/upgcxcode/15/53/30058545315/30058545315-1-30251.m4s"
            ),
            "audio/mp4"
        );
        assert_eq!(
            normalize_media_content_type(None, "/audio/song.flac"),
            "audio/flac"
        );
        assert_eq!(
            normalize_media_content_type(Some("video/mp4"), "/upgcxcode/a.m4s"),
            "video/mp4"
        );
        assert_eq!(
            normalize_media_content_type(Some("application/octet-stream"), "/unknown/file.bin"),
            "application/octet-stream"
        );
    }
}
