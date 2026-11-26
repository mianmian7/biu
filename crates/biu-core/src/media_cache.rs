use crate::bili::{BILI_ORIGIN, BILI_REFERER};
use anyhow::{Context, Result, ensure};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, SystemTime};
use tokio::{
    io::AsyncWriteExt,
    sync::{Mutex as AsyncMutex, Semaphore, watch},
};
use url::Url;

pub const AUDIO_CACHE_MAX_BYTES: u64 = 1024 * 1024 * 1024;
const IDLE_TIMEOUT: Duration = Duration::from_secs(30);

#[derive(Debug, Clone)]
pub enum PreparedAudio {
    Ready {
        path: PathBuf,
        content_type: String,
    },
    Streaming {
        path: PathBuf,
        total_bytes: u64,
        done: Arc<AtomicBool>,
        aborted: Arc<AtomicBool>,
        error: Arc<Mutex<Option<String>>>,
    },
}

impl PreparedAudio {
    pub fn open(&self) -> io::Result<GrowingFile> {
        let (file, done, aborted, error) = match self {
            Self::Ready { path, .. } => (std::fs::File::open(path)?, None, None, None),
            Self::Streaming {
                path,
                done,
                aborted,
                error,
                ..
            } => {
                let file = std::fs::File::open(path).or_else(|err| {
                    if err.kind() == io::ErrorKind::NotFound {
                        std::fs::File::open(path.with_extension("audio"))
                    } else {
                        Err(err)
                    }
                })?;
                (
                    file,
                    Some(done.clone()),
                    Some(aborted.clone()),
                    Some(error.clone()),
                )
            }
        };
        let total_bytes = match self {
            Self::Ready { .. } => file.metadata()?.len(),
            Self::Streaming { total_bytes, .. } => *total_bytes,
        };
        Ok(GrowingFile {
            file,
            total_bytes,
            done,
            aborted,
            error,
        })
    }

    pub fn abort(&self) {
        if let Self::Streaming { aborted, done, .. } = self
            && !done.load(Ordering::Acquire)
        {
            aborted.store(true, Ordering::Release);
        }
    }
}

/// A blocking reader for a decoder thread. EOF is only returned after the fill finishes.
pub struct GrowingFile {
    file: std::fs::File,
    total_bytes: u64,
    done: Option<Arc<AtomicBool>>,
    aborted: Option<Arc<AtomicBool>>,
    error: Option<Arc<Mutex<Option<String>>>>,
}

impl GrowingFile {
    pub fn total_bytes(&self) -> Option<u64> {
        (self.total_bytes > 0).then_some(self.total_bytes)
    }
}

impl Read for GrowingFile {
    fn read(&mut self, buffer: &mut [u8]) -> io::Result<usize> {
        if buffer.is_empty() {
            return Ok(0);
        }
        loop {
            if self
                .aborted
                .as_ref()
                .is_some_and(|flag| flag.load(Ordering::Acquire))
            {
                return Err(io::Error::new(
                    io::ErrorKind::ConnectionAborted,
                    "audio cache fill cancelled",
                ));
            }
            let complete = self
                .done
                .as_ref()
                .is_none_or(|flag| flag.load(Ordering::Acquire));
            if let Some(err) = self
                .error
                .as_ref()
                .and_then(|error| error.lock().unwrap().clone())
            {
                return Err(io::Error::other(err));
            }
            let read = self.file.read(buffer)?;
            if read != 0 || complete {
                return Ok(read);
            }
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Seek for GrowingFile {
    fn seek(&mut self, position: SeekFrom) -> io::Result<u64> {
        if let SeekFrom::End(delta) = position
            && let Some(total) = self.total_bytes()
        {
            let start = total
                .checked_add_signed(delta)
                .ok_or_else(|| io::Error::from(io::ErrorKind::InvalidInput))?;
            return self.file.seek(SeekFrom::Start(start));
        }
        if matches!(position, SeekFrom::End(_))
            && self
                .done
                .as_ref()
                .is_some_and(|flag| !flag.load(Ordering::Acquire))
        {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "audio length is not yet known",
            ));
        }
        self.file.seek(position)
    }
}

#[derive(Deserialize, Serialize)]
struct Metadata {
    content_type: String,
    total_len: u64,
}

#[derive(Clone, Default)]
struct FillState {
    done: Arc<AtomicBool>,
    aborted: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
}

type ActiveFill = (FillState, watch::Receiver<Option<PreparedAudio>>);

pub struct MediaCache {
    directory: PathBuf,
    max_bytes: u64,
    storage: AsyncMutex<()>,
    fills: Mutex<HashMap<String, ActiveFill>>,
    permits: Semaphore,
}

struct FillGuard {
    cache: Arc<MediaCache>,
    key: String,
    state: FillState,
}

impl Drop for FillGuard {
    fn drop(&mut self) {
        let mut fills = self.cache.fills.lock().unwrap();
        for extension in ["part", "json.part"] {
            let _ = std::fs::remove_file(self.cache.path(&self.key, extension));
        }
        if !self.state.done.load(Ordering::Acquire) {
            let mut error = self.state.error.lock().unwrap();
            if error.is_none() {
                *error = Some("audio cache fill stopped".into());
            }
            self.state.done.store(true, Ordering::Release);
        }
        fills.remove(&self.key);
    }
}

impl MediaCache {
    pub fn new(directory: impl Into<PathBuf>) -> io::Result<Self> {
        Self::with_limit(directory, AUDIO_CACHE_MAX_BYTES)
    }

    pub fn with_limit(directory: impl Into<PathBuf>, max_bytes: u64) -> io::Result<Self> {
        let directory = directory.into();
        std::fs::create_dir_all(&directory)?;
        Ok(Self {
            directory,
            max_bytes,
            storage: AsyncMutex::new(()),
            fills: Mutex::new(HashMap::new()),
            permits: Semaphore::new(2),
        })
    }

    fn path(&self, key: &str, extension: &str) -> PathBuf {
        self.directory.join(format!("{key}.{extension}"))
    }

    /// Known media paths share cache entries across CDN mirrors and signed query renewals.
    pub fn key(url: &Url) -> String {
        let path = url.path();
        let resource = if [".m4s", ".flac", ".mp3", ".m4a", ".mp4"]
            .iter()
            .any(|extension| path.ends_with(extension))
        {
            path
        } else {
            url.as_str()
        };
        format!("{:x}", Sha256::digest(resource.as_bytes()))
    }

    async fn lookup(&self, key: &str) -> Option<PreparedAudio> {
        let meta: Metadata =
            serde_json::from_slice(&tokio::fs::read(self.path(key, "json")).await.ok()?).ok()?;
        if !valid_content_type(&meta.content_type) || meta.total_len > self.max_bytes {
            return None;
        }
        for extension in ["audio", "bin"] {
            let path = self.path(key, extension);
            let Ok(file) = tokio::fs::OpenOptions::new()
                .write(true)
                .read(true)
                .open(&path)
                .await
            else {
                continue;
            };
            if file.metadata().await.ok()?.len() == meta.total_len {
                let _ = file.into_std().await.set_modified(SystemTime::now());
                return Some(PreparedAudio::Ready {
                    path,
                    content_type: meta.content_type,
                });
            }
        }
        None
    }

    /// Returns when a cache entry is ready or the first chunk can be consumed.
    ///
    /// `media_client` should enforce the same allowlist on redirects as `allowed_check`
    /// (the Bilibili media client already does this).
    pub async fn prepare_audio_file(
        self: &Arc<Self>,
        url: Url,
        media_client: reqwest::Client,
        allowed_check: impl Fn(&Url) -> bool + Send,
    ) -> Result<PreparedAudio> {
        ensure!(allowed_check(&url), "audio source is not allowlisted");
        let key = Self::key(&url);
        let (state, mut receiver) = loop {
            let mut cancelled = {
                let _storage = self.storage.lock().await;
                if let Some(ready) = self.lookup(&key).await {
                    return Ok(ready);
                }
                let mut fills = self.fills.lock().unwrap();
                if let Some((state, receiver)) = fills.get(&key) {
                    if !state.aborted.load(Ordering::Acquire) {
                        break (state.clone(), receiver.clone());
                    }
                    receiver.clone()
                } else {
                    let state = FillState::default();
                    let (sender, receiver) = watch::channel(None);
                    fills.insert(key.clone(), (state.clone(), receiver.clone()));
                    let guard = FillGuard {
                        cache: self.clone(),
                        key,
                        state: state.clone(),
                    };
                    tokio::spawn(async move {
                        let result = guard.cache.fill(&guard, url, media_client, &sender).await;
                        if let Err(err) = result {
                            *guard.state.error.lock().unwrap() = Some(err.to_string());
                        }
                        guard.state.done.store(true, Ordering::Release);
                        drop(guard);
                    });
                    break (state, receiver);
                }
            };
            // Wait for the cancelled worker to release the shared staging paths.
            while cancelled.changed().await.is_ok() {}
        };
        loop {
            if let Some(prepared) = receiver.borrow_and_update().clone() {
                if state.done.load(Ordering::Acquire)
                    && let Some(err) = &*state.error.lock().unwrap()
                {
                    anyhow::bail!("{err}");
                }
                return Ok(prepared);
            }
            if receiver.changed().await.is_err() {
                let error = state
                    .error
                    .lock()
                    .unwrap()
                    .clone()
                    .unwrap_or_else(|| "audio cache fill stopped".into());
                anyhow::bail!("{error}");
            }
        }
    }

    async fn fill(
        &self,
        guard: &FillGuard,
        url: Url,
        client: reqwest::Client,
        sender: &watch::Sender<Option<PreparedAudio>>,
    ) -> Result<()> {
        let _permit = until_aborted(&guard.state.aborted, self.permits.acquire()).await??;
        let response = until_aborted(
            &guard.state.aborted,
            tokio::time::timeout(
                IDLE_TIMEOUT,
                client
                    .get(url.clone())
                    .header("Referer", BILI_REFERER)
                    .header("Origin", BILI_ORIGIN)
                    .header("Accept-Encoding", "identity")
                    .send(),
            ),
        )
        .await??
        .map_err(reqwest::Error::without_url)?;
        let headers = response.headers();
        ensure!(
            response.status() == reqwest::StatusCode::OK && !headers.contains_key("content-range"),
            "unexpected audio HTTP status {}",
            response.status()
        );
        ensure!(
            headers.get_all("content-encoding").iter().all(|value| value
                .to_str()
                .is_ok_and(|encoding| encoding.eq_ignore_ascii_case("identity"))),
            "unexpected audio content encoding"
        );
        let expected = response.content_length();
        ensure!(
            expected.is_none_or(|len| len <= self.max_bytes),
            "audio exceeds disk cache limit"
        );
        let content_type = media_content_type(
            headers
                .get("content-type")
                .and_then(|value| value.to_str().ok()),
            url.path(),
        );
        ensure!(
            valid_content_type(&content_type),
            "unexpected audio content type"
        );
        let part = self.path(&guard.key, "part");
        let mut file = tokio::fs::File::create(&part).await?;
        let mut stream = response.bytes_stream();
        let mut bytes = 0;
        while let Some(chunk) = until_aborted(
            &guard.state.aborted,
            tokio::time::timeout(IDLE_TIMEOUT, stream.next()),
        )
        .await??
        {
            let chunk = chunk.map_err(reqwest::Error::without_url)?;
            if chunk.is_empty() {
                continue;
            }
            let first = bytes == 0;
            bytes += chunk.len() as u64;
            ensure!(
                bytes <= expected.unwrap_or(self.max_bytes),
                "audio length exceeds expected size"
            );
            file.write_all(&chunk).await?;
            if first {
                file.flush().await?;
                sender.send_replace(Some(PreparedAudio::Streaming {
                    path: part.clone(),
                    total_bytes: expected.unwrap_or_default(),
                    done: guard.state.done.clone(),
                    aborted: guard.state.aborted.clone(),
                    error: guard.state.error.clone(),
                }));
            }
        }
        ensure!(
            bytes > 0 && expected.is_none_or(|len| len == bytes),
            "audio response is empty or incomplete"
        );
        file.sync_all().await?;
        drop(file);
        let meta = serde_json::to_vec(&Metadata {
            content_type: content_type.clone(),
            total_len: bytes,
        })?;
        let staging_metadata = self.path(&guard.key, "json.part");
        tokio::fs::write(&staging_metadata, meta).await?;
        let _storage = self.storage.lock().await;
        ensure!(
            !guard.state.aborted.load(Ordering::Acquire),
            "audio cache fill cancelled"
        );
        let audio = self.path(&guard.key, "audio");
        let marker = self.path(&guard.key, "json");
        remove_if_present(&marker).await?;
        tokio::fs::rename(&part, &audio).await?;
        if let Err(err) = tokio::fs::rename(staging_metadata, marker).await {
            let _ = tokio::fs::remove_file(&audio).await;
            return Err(err).context("commit audio metadata");
        }
        let _ = tokio::fs::remove_file(self.path(&guard.key, "bin")).await;
        if let Err(err) = self.evict(&audio).await {
            log::warn!("audio cache eviction failed: {err}");
        }
        sender.send_replace(Some(PreparedAudio::Ready {
            path: audio,
            content_type,
        }));
        Ok(())
    }

    async fn evict(&self, protected: &Path) -> io::Result<()> {
        let mut files = cache_files(&self.directory, &["audio", "bin"]).await?;
        files.sort_by_key(|file| file.modified);
        let mut total = files.iter().map(|file| file.size).sum::<u64>();
        for file in files {
            if total <= self.max_bytes {
                break;
            }
            if file.path != protected && tokio::fs::remove_file(&file.path).await.is_ok() {
                let _ = remove_if_present(&file.path.with_extension("json")).await;
                total -= file.size;
            }
        }
        Ok(())
    }

    pub async fn cache_size_bytes(&self) -> io::Result<u64> {
        let _storage = self.storage.lock().await;
        Ok(cache_files(&self.directory, &["audio", "bin"])
            .await?
            .iter()
            .map(|file| file.size)
            .sum())
    }

    /// Remove committed entries while allowing active fills to finish.
    pub async fn clear_cache(&self) -> io::Result<()> {
        let _storage = self.storage.lock().await;
        for file in cache_files(&self.directory, &["audio", "bin", "json"]).await? {
            remove_if_present(&file.path).await?;
        }
        Ok(())
    }
}

async fn until_aborted<T>(aborted: &AtomicBool, operation: impl Future<Output = T>) -> Result<T> {
    if aborted.load(Ordering::Acquire) {
        anyhow::bail!("audio cache fill cancelled");
    }
    tokio::select! {
        result = operation => Ok(result),
        () = async {
            while !aborted.load(Ordering::Acquire) {
                tokio::time::sleep(Duration::from_millis(40)).await;
            }
        } => anyhow::bail!("audio cache fill cancelled"),
    }
}

fn valid_content_type(content_type: &str) -> bool {
    let mime = content_type
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    mime == "video/mp4"
        || mime
            .strip_prefix("audio/")
            .is_some_and(|subtype| !subtype.is_empty())
}

pub(crate) fn media_content_type(upstream: Option<&str>, path: &str) -> String {
    let value = upstream.unwrap_or_default().trim();
    let mime = value
        .split(';')
        .next()
        .unwrap_or_default()
        .trim()
        .to_ascii_lowercase();
    if !matches!(
        mime.as_str(),
        "" | "application/octet-stream" | "binary/octet-stream"
    ) {
        return value.to_owned();
    }
    let extension = Path::new(path)
        .extension()
        .and_then(|s| s.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    match extension.as_str() {
        "m4s" | "m4a" => "audio/mp4",
        "mp4" => "video/mp4",
        "flac" => "audio/flac",
        "mp3" => "audio/mpeg",
        "wav" => "audio/wav",
        "ogg" | "opus" => "audio/ogg",
        "aac" => "audio/aac",
        _ => "application/octet-stream",
    }
    .to_owned()
}

pub(crate) struct DiskEntry {
    pub path: PathBuf,
    pub modified: SystemTime,
    pub size: u64,
}

pub(crate) async fn cache_files(
    directory: &Path,
    extensions: &[&str],
) -> io::Result<Vec<DiskEntry>> {
    let mut entries = tokio::fs::read_dir(directory).await?;
    let mut files = Vec::new();
    while let Some(entry) = entries.next_entry().await? {
        let path = entry.path();
        let extension = path
            .extension()
            .and_then(|ext| ext.to_str())
            .unwrap_or_default();
        let stem = path
            .file_stem()
            .and_then(|stem| stem.to_str())
            .unwrap_or_default();
        if !extensions.contains(&extension)
            || stem.len() != 64
            || !stem.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            continue;
        }
        let metadata = entry.metadata().await?;
        if metadata.is_file() {
            files.push(DiskEntry {
                path,
                modified: metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH),
                size: metadata.len(),
            });
        }
    }
    Ok(files)
}

pub(crate) async fn remove_if_present(path: &Path) -> io::Result<()> {
    match tokio::fs::remove_file(path).await {
        Err(err) if err.kind() == io::ErrorKind::NotFound => Ok(()),
        result => result,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::{io::AsyncReadExt, net::TcpListener, sync::oneshot};

    #[tokio::test]
    async fn replays_immediately_after_aborting_a_stream() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = Arc::new(MediaCache::new(dir.path())?);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = Url::parse(&format!("http://{}/track.m4s", listener.local_addr()?))?;
        let server = tokio::spawn(async move {
            let stalled = serve_audio(&listener, b"old").await?;
            let _replay = serve_audio(&listener, b"replay").await?;
            drop(stalled);
            Ok::<_, io::Error>(())
        });
        let client = reqwest::Client::builder().no_proxy().build()?;
        let prepare = || cache.prepare_audio_file(url.clone(), client.clone(), |_| true);
        let audio = prepare().await?;
        assert!(matches!(audio, PreparedAudio::Streaming { .. }));
        audio.abort();

        let replay = tokio::time::timeout(Duration::from_secs(3), prepare()).await??;
        assert_eq!(read_audio(replay).await?, b"replay");
        server.await??;
        Ok(())
    }

    #[tokio::test]
    async fn clearing_cache_preserves_an_active_stream() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = Arc::new(MediaCache::new(dir.path())?);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let url = Url::parse(&format!("http://{}/track.m4s", listener.local_addr()?))?;
        let (finish, finished) = oneshot::channel();
        let server = tokio::spawn(async move {
            let mut socket = serve_audio(&listener, b"abc").await?;
            finished.await?;
            socket.write_all(b"def").await?;
            Ok::<_, anyhow::Error>(())
        });
        let client = reqwest::Client::builder().no_proxy().build()?;
        let audio = cache.prepare_audio_file(url, client, |_| true).await?;
        let PreparedAudio::Streaming { path, .. } = &audio else {
            panic!("expected an active stream");
        };
        cache.clear_cache().await?;
        assert!(path.exists());
        finish.send(()).unwrap();
        assert_eq!(read_audio(audio).await?, b"abcdef");
        server.await??;
        Ok(())
    }

    async fn serve_audio(
        listener: &TcpListener,
        bytes: &[u8],
    ) -> io::Result<tokio::net::TcpStream> {
        let (mut socket, _) = listener.accept().await?;
        let mut request = Vec::new();
        while !request.ends_with(b"\r\n\r\n") {
            request.push(socket.read_u8().await?);
        }
        socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nContent-Type: audio/mp4\r\nConnection: close\r\n\r\n").await?;
        socket.write_all(bytes).await?;
        Ok(socket)
    }

    async fn read_audio(audio: PreparedAudio) -> Result<Vec<u8>> {
        Ok(tokio::task::spawn_blocking(move || {
            let mut bytes = Vec::new();
            audio.open()?.read_to_end(&mut bytes)?;
            Ok::<_, io::Error>(bytes)
        })
        .await??)
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn streams_real_http_bytes_then_commits_and_evicts_lru() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = Arc::new(MediaCache::with_limit(dir.path(), 12)?);
        let listener = TcpListener::bind("127.0.0.1:0").await?;
        let addr = listener.local_addr()?;
        let (finish, finished) = oneshot::channel();
        let server = tokio::spawn(async move {
            let mut finished = Some(finished);
            for i in 0..3 {
                let (mut stream, _) = listener.accept().await?;
                let mut request = Vec::new();
                while !request.ends_with(b"\r\n\r\n") {
                    request.push(stream.read_u8().await?);
                }
                stream.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 6\r\nContent-Type: audio/mp4\r\nConnection: close\r\n\r\nabc").await?;
                if i == 0 {
                    finished.take().unwrap().await.unwrap();
                }
                stream.write_all(b"def").await?;
            }
            Ok::<_, io::Error>(())
        });
        let client = reqwest::Client::builder().no_proxy().build()?;
        let prepare = |name: &str| {
            cache.prepare_audio_file(
                Url::parse(&format!("http://{addr}/{name}.m4s")).unwrap(),
                client.clone(),
                |_| true,
            )
        };
        let first = prepare("first").await?;
        let PreparedAudio::Streaming { path, done, .. } = &first else {
            panic!("expected active streaming")
        };
        assert!(path.exists());
        assert!(!done.load(Ordering::Acquire));
        assert!(matches!(
            prepare("first").await?,
            PreparedAudio::Streaming { .. }
        ));
        let mut reader = first.open()?;
        let mut beginning = [0; 3];
        reader.read_exact(&mut beginning)?;
        assert_eq!(&beginning, b"abc");
        finish.send(()).unwrap();
        let remainder = tokio::task::spawn_blocking(move || {
            let mut data = Vec::new();
            reader.read_to_end(&mut data)?;
            Ok::<_, io::Error>(data)
        })
        .await??;
        assert_eq!(remainder, b"def");
        assert!(matches!(
            prepare("first").await?,
            PreparedAudio::Ready { .. }
        ));
        for name in ["second", "third"] {
            let audio = prepare(name).await?;
            if let PreparedAudio::Streaming { done, .. } = audio {
                tokio::time::timeout(Duration::from_secs(5), async {
                    while !done.load(Ordering::Acquire) {
                        tokio::task::yield_now().await;
                    }
                })
                .await?;
            }
            if name == "second" {
                assert!(matches!(
                    prepare("first").await?,
                    PreparedAudio::Ready { .. }
                ));
            }
        }
        server.await??;
        assert_eq!(cache.cache_size_bytes().await?, 12);
        assert!(
            dir.path()
                .join(format!(
                    "{}.audio",
                    MediaCache::key(&Url::parse(&format!("http://{addr}/first.m4s"))?)
                ))
                .exists()
        );
        assert!(
            !dir.path()
                .join(format!(
                    "{}.audio",
                    MediaCache::key(&Url::parse(&format!("http://{addr}/second.m4s"))?)
                ))
                .exists()
        );
        cache.clear_cache().await?;
        assert_eq!(cache.cache_size_bytes().await?, 0);
        Ok(())
    }
}
