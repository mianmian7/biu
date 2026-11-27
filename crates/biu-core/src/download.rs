use crate::bili::BiliClient;
use crate::media_cache::media_content_type;
use crate::store::Store;
use crate::{AudioQuality, DownloadStatus, DownloadTask, PlayUrlInfo, TrackItem, TrackSource};
use anyhow::{Context, Result, bail, ensure};
use futures::StreamExt;
use serde_json::{Map, Value, json};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{Semaphore, watch};

const CONCURRENT_DOWNLOADS: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DownloadFormat {
    Native,
    Mp3,
    M4a,
    Flac,
}

impl DownloadFormat {
    fn extension(self) -> &'static str {
        match self {
            Self::Native => "native",
            Self::Mp3 => "mp3",
            Self::M4a => "m4a",
            Self::Flac => "flac",
        }
    }
}

pub struct DownloadManager {
    bili: Arc<BiliClient>,
    store: Store,
    settings_override: Option<DownloadSettings>,
    state: Mutex<State>,
    permits: Semaphore,
}

#[derive(Clone)]
struct DownloadSettings {
    directory: PathBuf,
    ffmpeg: Option<PathBuf>,
    quality: AudioQuality,
}

struct State {
    tasks: HashMap<String, DownloadTask>,
    running: HashMap<String, watch::Sender<bool>>,
}

impl DownloadManager {
    pub fn open(store: Store, bili: Arc<BiliClient>) -> Result<Arc<Self>> {
        Self::restore(store, bili, None)
    }

    pub fn new(
        store: Store,
        bili: Arc<BiliClient>,
        directory: PathBuf,
        ffmpeg: Option<PathBuf>,
        quality: AudioQuality,
    ) -> Result<Arc<Self>> {
        Self::restore(
            store,
            bili,
            Some(DownloadSettings {
                directory,
                ffmpeg,
                quality,
            }),
        )
    }

    fn restore(
        store: Store,
        bili: Arc<BiliClient>,
        settings_override: Option<DownloadSettings>,
    ) -> Result<Arc<Self>> {
        let data = store.read("media-downloads")?;
        let mut tasks = HashMap::new();
        if let Some(nested) = data["downloads"].as_object() {
            for entry in nested.values().cloned() {
                restore_task(&mut tasks, entry);
            }
        }
        for (key, entry) in data.as_object().unwrap() {
            if key != "downloads" {
                restore_task(&mut tasks, entry.clone());
            }
        }
        Ok(Arc::new(Self {
            bili,
            store,
            settings_override,
            state: Mutex::new(State {
                tasks,
                running: HashMap::new(),
            }),
            permits: Semaphore::new(CONCURRENT_DOWNLOADS),
        }))
    }

    fn download_settings(&self) -> Result<DownloadSettings> {
        if let Some(settings) = &self.settings_override {
            return Ok(settings.clone());
        }
        let settings = self.store.app_settings()?;
        Ok(DownloadSettings {
            directory: if settings.download_path.trim().is_empty() {
                dirs::download_dir().unwrap_or_else(|| self.store.directory().join("downloads"))
            } else {
                PathBuf::from(settings.download_path)
            },
            ffmpeg: (!settings.ffmpeg_path.trim().is_empty())
                .then(|| PathBuf::from(settings.ffmpeg_path)),
            quality: settings.audio_quality,
        })
    }

    pub fn tasks(&self) -> Vec<DownloadTask> {
        let mut tasks = self
            .state
            .lock()
            .unwrap()
            .tasks
            .values()
            .cloned()
            .collect::<Vec<_>>();
        tasks.sort_by(|a, b| {
            a.created_time
                .cmp(&b.created_time)
                .then_with(|| a.id.cmp(&b.id))
        });
        tasks
    }

    pub fn queue(self: &Arc<Self>, track: TrackItem, format: DownloadFormat) -> Result<String> {
        let runtime = tokio::runtime::Handle::try_current()
            .context("queueing a download requires a Tokio runtime")?;
        let (bvid, cid, sid) = match &track.source {
            TrackSource::BiliVideo { bvid, cid, .. } => {
                (Some(bvid.clone()), Some(json!(cid)), None)
            }
            TrackSource::BiliAudio { sid } => (None, None, Some(json!(sid))),
            TrackSource::LocalFile { .. } => (None, None, None),
        };
        let id = uuid::Uuid::new_v4().to_string();
        let mut extra = Map::new();
        extra.insert("track".into(), serde_json::to_value(&track)?);
        extra.insert("requestedFormat".into(), json!(format.extension()));
        let task = DownloadTask {
            id: id.clone(),
            output_file_type: "audio".into(),
            title: track.title.clone(),
            status: DownloadStatus::Queued,
            bvid,
            cid,
            sid,
            cover: Some(track.cover.clone()),
            save_path: None,
            total_bytes: 0,
            downloaded_bytes: 0,
            download_progress: 0,
            created_time: Some(SystemTime::now().duration_since(UNIX_EPOCH)?.as_millis() as u64),
            error: None,
            extra,
        };
        let (sender, receiver) = watch::channel(false);
        {
            let mut state = self.state.lock().unwrap();
            self.persist(&task)?;
            state.tasks.insert(id.clone(), task);
            state.running.insert(id.clone(), sender);
        }
        let manager = self.clone();
        let worker_id = id.clone();
        runtime.spawn(async move { manager.run(worker_id, track, format, receiver).await });
        Ok(id)
    }

    pub fn cancel(&self, id: &str) -> Result<bool> {
        let mut state = self.state.lock().unwrap();
        let Some(cancel) = state.running.get(id).cloned() else {
            return Ok(false);
        };
        let Some(task) = state.tasks.get_mut(id) else {
            return Ok(false);
        };
        if matches!(
            task.status,
            DownloadStatus::Done | DownloadStatus::Failed | DownloadStatus::Cancelled
        ) {
            return Ok(false);
        }
        let mut updated = task.clone();
        updated.status = DownloadStatus::Cancelled;
        self.persist(&updated)?;
        *task = updated;
        cancel.send_replace(true);
        Ok(true)
    }

    /// Restart a paused, failed, or cancelled task with its original source and format.
    pub fn retry(self: &Arc<Self>, id: &str) -> Result<bool> {
        let mut state = self.state.lock().unwrap();
        if state.running.contains_key(id) {
            return Ok(false);
        }
        let Some(task) = state.tasks.get_mut(id) else {
            return Ok(false);
        };
        if !matches!(
            task.status,
            DownloadStatus::Paused | DownloadStatus::Failed | DownloadStatus::Cancelled
        ) {
            return Ok(false);
        }
        let runtime = tokio::runtime::Handle::try_current()
            .context("retrying a download requires a Tokio runtime")?;
        let track = retry_track(task)?;
        let format = match task.extra.get("requestedFormat").and_then(Value::as_str) {
            Some("mp3") => DownloadFormat::Mp3,
            Some("m4a") => DownloadFormat::M4a,
            Some("flac") => DownloadFormat::Flac,
            _ => DownloadFormat::Native,
        };
        let mut updated = task.clone();
        updated.status = DownloadStatus::Queued;
        updated.save_path = None;
        updated.total_bytes = 0;
        updated.downloaded_bytes = 0;
        updated.download_progress = 0;
        updated.error = None;
        updated.extra.remove("cancelled");
        self.persist(&updated)?;
        *task = updated;

        let (sender, receiver) = watch::channel(false);
        state.running.insert(id.to_owned(), sender);
        let manager = self.clone();
        let id = id.to_owned();
        runtime.spawn(async move { manager.run(id, track, format, receiver).await });
        Ok(true)
    }

    /// Remove a task from history and cancel it if active. The completed file is preserved.
    pub fn delete(&self, id: &str) -> Result<bool> {
        let mut state = self.state.lock().unwrap();
        if !state.tasks.contains_key(id) {
            return Ok(false);
        }
        self.store.update("media-downloads", |entries| {
            remove_task(entries, id);
            Ok(())
        })?;
        state.tasks.remove(id);
        if let Some(cancel) = state.running.get(id) {
            cancel.send_replace(true);
        }
        Ok(true)
    }

    /// Clear all non-active download history entries (preserving downloaded files on disk).
    pub fn clear_finished(&self) -> Result<usize> {
        let mut state = self.state.lock().unwrap();
        let ids: Vec<String> = state
            .tasks
            .iter()
            .filter(|(id, task)| {
                !state.running.contains_key(*id)
                    && matches!(
                        task.status,
                        DownloadStatus::Done
                            | DownloadStatus::Failed
                            | DownloadStatus::Cancelled
                            | DownloadStatus::Paused
                            | DownloadStatus::Other
                    )
            })
            .map(|(id, _)| id.clone())
            .collect();
        if ids.is_empty() {
            return Ok(0);
        }
        self.store.update("media-downloads", |entries| {
            for id in &ids {
                remove_task(entries, id);
            }
            Ok(())
        })?;
        for id in &ids {
            state.tasks.remove(id);
        }
        Ok(ids.len())
    }

    fn persist(&self, task: &DownloadTask) -> Result<()> {
        self.store.update("media-downloads", |entries| {
            remove_task(entries, &task.id);
            let mut value = serde_json::to_value(task)?;
            if task.status == DownloadStatus::Cancelled {
                value["status"] = json!("failed");
                value["cancelled"] = json!(true);
            }
            entries.insert(format!("downloads.{}", task.id), value);
            Ok(())
        })
    }

    fn update_stage(&self, id: &str, status: DownloadStatus) -> Result<()> {
        let mut state = self.state.lock().unwrap();
        let task = state.tasks.get_mut(id).context("download was removed")?;
        ensure!(
            task.status != DownloadStatus::Cancelled,
            "download cancelled"
        );
        let mut updated = task.clone();
        updated.status = status;
        self.persist(&updated)?;
        *task = updated;
        Ok(())
    }

    fn progress(&self, id: &str, bytes: u64, total: Option<u64>) {
        if let Some(task) = self.state.lock().unwrap().tasks.get_mut(id)
            && task.status == DownloadStatus::Downloading
        {
            task.downloaded_bytes = bytes;
            task.total_bytes = total.unwrap_or_default();
            if let Some(total) = total.filter(|total| *total > 0) {
                task.download_progress = (bytes.saturating_mul(100) / total).min(100) as u32;
            }
        }
    }

    async fn run(
        self: Arc<Self>,
        id: String,
        track: TrackItem,
        format: DownloadFormat,
        mut cancelled: watch::Receiver<bool>,
    ) {
        let mut temporary = TempFiles::default();
        let result = tokio::select! {
            biased;
            () = wait_cancel(&mut cancelled) => Err(anyhow::anyhow!("download cancelled")),
            result = self.download(&id, &track, format, &mut temporary) => result,
        };
        let mut state = self.state.lock().unwrap();
        if let Some(task) = state.tasks.get_mut(&id)
            && task.status != DownloadStatus::Cancelled
        {
            let result = result.and_then(|(temp, extension)| {
                let mut updated = task.clone();
                let path = commit_output(&temp, &track.title, &extension)?;
                updated
                    .extra
                    .insert("fileExtension".into(), json!(extension));
                updated.save_path = Some(path.to_string_lossy().into_owned());
                updated.status = DownloadStatus::Done;
                updated.download_progress = 100;
                updated.total_bytes = updated.downloaded_bytes;
                *task = updated;
                Ok(())
            });
            if let Err(err) = result {
                task.status = DownloadStatus::Failed;
                task.error = Some(err.to_string());
            }
            if let Err(err) = self.persist(task) {
                log::error!("could not persist download task {id}: {err}");
                task.error = Some(format!("could not persist download history: {err}"));
            }
        }
        state.running.remove(&id);
    }

    async fn download(
        &self,
        id: &str,
        track: &TrackItem,
        format: DownloadFormat,
        temporary: &mut TempFiles,
    ) -> Result<(PathBuf, String)> {
        let _permit = self.permits.acquire().await?;
        let settings = self.download_settings()?;
        self.update_stage(id, DownloadStatus::Downloading)?;
        tokio::fs::create_dir_all(&settings.directory).await?;
        let input = temporary.path(&settings.directory, "source-part");
        let (extension, total) = match &track.source {
            TrackSource::LocalFile { path, format } => {
                let mut source = tokio::fs::File::open(path).await?;
                let extension = native_extension(None, format, &mut source).await?;
                let size = source.metadata().await?.len();
                ensure!(size > 0, "local audio is empty");
                let mut output = tokio::fs::File::create(&input).await?;
                let mut buffer = vec![0; 64 * 1024];
                let mut bytes = 0;
                loop {
                    let count = source.read(&mut buffer).await?;
                    if count == 0 {
                        break;
                    }
                    output.write_all(&buffer[..count]).await?;
                    bytes += count as u64;
                    self.progress(id, bytes, Some(size));
                }
                output.sync_all().await?;
                (extension, bytes)
            }
            source => {
                let playurl = self.resolve(source, settings.quality).await?;
                self.download_remote(id, &playurl, &input).await?
            }
        };
        ensure!(total > 0, "downloaded audio is empty");
        if format == DownloadFormat::Native || format.extension() == extension {
            return Ok((input, extension));
        }
        let Some(executable) = find_ffmpeg(settings.ffmpeg.as_deref()).await else {
            return Ok((input, extension));
        };
        self.update_stage(id, DownloadStatus::Converting)?;
        let output = temporary.path(&settings.directory, format.extension());
        convert(&executable, &input, &output, format).await?;
        Ok((output, format.extension().into()))
    }

    async fn resolve(&self, source: &TrackSource, quality: AudioQuality) -> Result<PlayUrlInfo> {
        match source {
            TrackSource::BiliVideo { bvid, cid, .. } => {
                self.bili.get_playurl(bvid, *cid, quality).await
            }
            TrackSource::BiliAudio { sid } => self.bili.get_audio_playurl(*sid).await,
            TrackSource::LocalFile { .. } => bail!("local file has no CDN URL"),
        }
    }

    async fn download_remote(
        &self,
        id: &str,
        playurl: &PlayUrlInfo,
        path: &Path,
    ) -> Result<(String, u64)> {
        let mut last_error = None;
        let urls = playurl.streams.iter().flat_map(|stream| &stream.urls);
        for url in urls {
            self.progress(id, 0, None);
            match self.fetch_remote(id, url, path).await {
                Ok(result) => return Ok(result),
                Err(err) => last_error = Some(err),
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("no audio CDN URLs")))
    }

    async fn fetch_remote(&self, id: &str, url: &url::Url, path: &Path) -> Result<(String, u64)> {
        ensure!(
            self.bili.is_media_url_allowed(url),
            "download CDN is not allowlisted"
        );
        let response = self
            .bili
            .media_client()
            .get(url.clone())
            .header("Accept-Encoding", "identity")
            .send()
            .await
            .map_err(reqwest::Error::without_url)?;
        ensure!(
            response.status() == reqwest::StatusCode::OK
                && !response.headers().contains_key("content-range"),
            "unexpected download HTTP status {}",
            response.status()
        );
        let mime = media_content_type(
            response
                .headers()
                .get("content-type")
                .and_then(|s| s.to_str().ok()),
            url.path(),
        );
        ensure!(
            mime.starts_with("audio/") || mime == "video/mp4" || mime == "application/octet-stream",
            "unexpected audio content type: {mime}"
        );
        let expected = response.content_length();
        let mut file = tokio::fs::OpenOptions::new()
            .create(true)
            .truncate(true)
            .read(true)
            .write(true)
            .open(path)
            .await?;
        let mut stream = response.bytes_stream();
        let mut bytes = 0;
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(reqwest::Error::without_url)?;
            bytes += chunk.len() as u64;
            ensure!(
                expected.is_none_or(|length| bytes <= length),
                "audio exceeds declared length"
            );
            file.write_all(&chunk).await?;
            self.progress(id, bytes, expected);
        }
        ensure!(
            bytes > 0 && expected.is_none_or(|length| length == bytes),
            "downloaded audio is incomplete"
        );
        file.sync_all().await?;
        use tokio::io::AsyncSeekExt;
        file.rewind().await?;
        let hint = Path::new(url.path())
            .extension()
            .and_then(|s| s.to_str())
            .unwrap_or_default();
        Ok((native_extension(Some(&mime), hint, &mut file).await?, bytes))
    }
}

fn remove_task(entries: &mut Map<String, Value>, id: &str) {
    let full_key = format!("downloads.{id}");
    entries.retain(|key, entry| key != &full_key && entry["id"] != id);
    if let Some(nested) = entries.get_mut("downloads").and_then(Value::as_object_mut) {
        nested.retain(|key, entry| key != id && entry["id"] != id);
    }
}

fn restore_task(tasks: &mut HashMap<String, DownloadTask>, value: Value) {
    if let Ok(mut task) = serde_json::from_value::<DownloadTask>(value) {
        if task.extra.get("cancelled").and_then(Value::as_bool) == Some(true) {
            task.status = DownloadStatus::Cancelled;
        } else if matches!(
            task.status,
            DownloadStatus::Queued
                | DownloadStatus::Downloading
                | DownloadStatus::Merging
                | DownloadStatus::Converting
        ) {
            task.status = DownloadStatus::Paused;
        }
        tasks.insert(task.id.clone(), task);
    }
}

fn retry_track(task: &DownloadTask) -> Result<TrackItem> {
    ensure!(
        task.output_file_type != "video",
        "video downloads are unsupported"
    );
    if let Some(track) = task.extra.get("track") {
        return serde_json::from_value(track.clone()).context("read saved download track");
    }
    // Tauri history stores source IDs directly, sometimes as strings.
    let parse_id = |value: &Value| value.as_u64().or_else(|| value.as_str()?.parse().ok());
    let (id, source) =
        if let Some(sid) = task.sid.as_ref().and_then(parse_id).filter(|sid| *sid != 0) {
            (format!("au:{sid}"), TrackSource::BiliAudio { sid })
        } else {
            let bvid = task
                .bvid
                .as_deref()
                .filter(|bvid| !bvid.is_empty())
                .context("download has no saved track source")?;
            let cid = task.cid.as_ref().and_then(parse_id).unwrap_or_default();
            (
                format!("{bvid}:{cid}"),
                TrackSource::BiliVideo {
                    bvid: bvid.to_owned(),
                    aid: 0,
                    cid,
                    part_title: None,
                },
            )
        };
    Ok(TrackItem {
        id,
        title: task.title.clone(),
        artist: String::new(),
        artist_mid: None,
        cover: task.cover.clone().unwrap_or_default(),
        duration_secs: 0.0,
        source,
    })
}

#[derive(Default)]
struct TempFiles(Vec<PathBuf>);

impl TempFiles {
    fn path(&mut self, directory: &Path, extension: &str) -> PathBuf {
        let path = directory.join(format!(".biu-{}.{extension}", uuid::Uuid::new_v4()));
        self.0.push(path.clone());
        path
    }
}

impl Drop for TempFiles {
    fn drop(&mut self) {
        for path in &self.0 {
            let _ = std::fs::remove_file(path);
        }
    }
}

async fn wait_cancel(receiver: &mut watch::Receiver<bool>) {
    let _ = receiver.wait_for(|cancelled| *cancelled).await;
}

pub(crate) async fn find_ffmpeg(configured: Option<&Path>) -> Option<PathBuf> {
    let platform = [
        Path::new("ffmpeg"),
        Path::new("/opt/homebrew/bin/ffmpeg"),
        Path::new("/usr/local/bin/ffmpeg"),
        Path::new("/usr/bin/ffmpeg"),
    ];
    for path in configured.into_iter().chain(platform) {
        let mut command = tokio::process::Command::new(path);
        command
            .kill_on_drop(true)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        if tokio::time::timeout(Duration::from_secs(3), command.status())
            .await
            .is_ok_and(|status| status.is_ok_and(|status| status.success()))
        {
            return Some(path.to_owned());
        }
    }
    None
}

async fn convert(ffmpeg: &Path, input: &Path, output: &Path, format: DownloadFormat) -> Result<()> {
    let codec = match format {
        DownloadFormat::Mp3 => "libmp3lame",
        DownloadFormat::M4a => "aac",
        DownloadFormat::Flac => "flac",
        DownloadFormat::Native => bail!("native download needs no conversion"),
    };
    let status = tokio::time::timeout(
        Duration::from_secs(10 * 60),
        tokio::process::Command::new(ffmpeg)
            .kill_on_drop(true)
            .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-n", "-i"])
            .arg(input)
            .args(["-map", "0:a:0", "-c:a", codec])
            .arg(output)
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status(),
    )
    .await??;
    ensure!(status.success(), "ffmpeg conversion failed ({status})");
    tokio::fs::OpenOptions::new()
        .write(true)
        .open(output)
        .await?
        .sync_all()
        .await?;
    Ok(())
}

async fn native_extension(
    mime: Option<&str>,
    hint: &str,
    file: &mut tokio::fs::File,
) -> Result<String> {
    use tokio::io::AsyncSeekExt;
    let mut header = Vec::new();
    (&mut *file).take(16).read_to_end(&mut header).await?;
    file.rewind().await?;
    let extension = if header.starts_with(b"fLaC") {
        "flac"
    } else if header.get(4..8) == Some(b"ftyp") || header.get(4..8) == Some(b"styp") {
        "m4a"
    } else if header.starts_with(b"ID3") {
        "mp3"
    } else if header.starts_with(b"OggS") {
        "ogg"
    } else if header.starts_with(b"RIFF") && header.get(8..12) == Some(b"WAVE") {
        "wav"
    } else {
        match mime
            .unwrap_or_default()
            .split(';')
            .next()
            .unwrap_or_default()
        {
            "audio/flac" | "audio/x-flac" => "flac",
            "audio/mpeg" => "mp3",
            "audio/mp4" | "video/mp4" => "m4a",
            "audio/aac" => "aac",
            "audio/wav" => "wav",
            "audio/ogg" => "ogg",
            _ => match hint.to_ascii_lowercase().as_str() {
                "flac" => "flac",
                "mp3" => "mp3",
                "mp4" | "m4a" | "m4s" => "m4a",
                "wav" => "wav",
                "ogg" => "ogg",
                "opus" => "opus",
                "aac" => "aac",
                "aiff" => "aiff",
                "wma" => "wma",
                _ => "audio",
            },
        }
    };
    Ok(extension.to_owned())
}

pub fn sanitize_filename(input: &str) -> String {
    let mut name = crate::bili::strip_html_tags(input)
        .chars()
        .map(|ch| {
            if ch.is_control() || "\\/:*?\"<>|".contains(ch) {
                '_'
            } else {
                ch
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    name.truncate(name.floor_char_boundary(200));
    let name = name.trim_matches(['.', ' ']);
    let base = name
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let device_name = matches!(base.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || base
            .strip_prefix("COM")
            .or_else(|| base.strip_prefix("LPT"))
            .is_some_and(|suffix| {
                matches!(
                    suffix,
                    "1" | "2" | "3" | "4" | "5" | "6" | "7" | "8" | "9" | "¹" | "²" | "³"
                )
            });
    if name.is_empty() {
        "Biu".into()
    } else if device_name {
        format!("_{name}")
    } else {
        name.to_owned()
    }
}

fn commit_output(temp: &Path, title: &str, extension: &str) -> Result<PathBuf> {
    let directory = temp
        .parent()
        .context("download file has no parent directory")?;
    let stem = sanitize_filename(title);
    for number in 0..10_000 {
        let filename = if number == 0 {
            format!("{stem}.{extension}")
        } else {
            format!("{stem} ({number}).{extension}")
        };
        let path = directory.join(filename);
        match std::fs::hard_link(temp, &path) {
            Ok(()) => return Ok(path),
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err)
                if matches!(
                    err.kind(),
                    std::io::ErrorKind::Unsupported | std::io::ErrorKind::PermissionDenied
                ) => {}
            Err(err) => return Err(err).context("publish download"),
        }
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(placeholder) => {
                drop(placeholder);
                if let Err(err) = std::fs::rename(temp, &path) {
                    let _ = std::fs::remove_file(&path);
                    return Err(err).context("publish download");
                }
                return Ok(path);
            }
            Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(err) => return Err(err).context("reserve download path"),
        }
    }
    bail!("too many downloads have the same title")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn writes_a_local_download_and_round_trips_legacy_history() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().join("settings"));
        store.write("media-downloads", json!({"downloads":{"old":{"id":"old","outputFileType":"m4a","title":"old","status":"completed"}}}))?;
        let bili = Arc::new(BiliClient::new(dir.path().join("cookies.json"), None)?);
        let target = dir.path().join("target");
        let manager = DownloadManager::new(
            store.clone(),
            bili.clone(),
            target.clone(),
            None,
            AudioQuality::Auto,
        )?;
        assert_eq!(manager.tasks()[0].status, DownloadStatus::Done);
        let input = dir.path().join("in.mp3");
        let bytes = b"ID3\x04\x00\x00\x00\x00\x00\x00sound";
        tokio::fs::write(&input, bytes).await?;
        let id = manager.queue(
            TrackItem {
                id: "local:test".into(),
                title: "CON".into(),
                artist: String::new(),
                artist_mid: None,
                cover: String::new(),
                duration_secs: 1.0,
                source: TrackSource::LocalFile {
                    path: input.to_string_lossy().into_owned(),
                    format: "mp3".into(),
                },
            },
            DownloadFormat::Native,
        )?;
        tokio::time::timeout(Duration::from_secs(3), async {
            loop {
                if manager
                    .tasks()
                    .iter()
                    .any(|task| task.id == id && task.status == DownloadStatus::Done)
                {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        assert_eq!(&tokio::fs::read(target.join("_CON.mp3")).await?, bytes);
        assert_eq!(
            store.read("media-downloads")?[format!("downloads.{id}")]["status"],
            "completed"
        );
        let restored = DownloadManager::new(store, bili, target.clone(), None, AudioQuality::Auto)?;
        assert_eq!(restored.tasks().len(), 2);
        assert!(restored.delete(&id)?);
        assert!(target.join("_CON.mp3").is_file());
        Ok(())
    }
    #[tokio::test]
    async fn cancels_a_queued_task_and_restores_its_cancelled_status() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path());
        let bili = Arc::new(BiliClient::new(dir.path().join("cookies.json"), None)?);
        let output = dir.path().join("output");
        let manager = DownloadManager::new(
            store.clone(),
            bili.clone(),
            output.clone(),
            None,
            AudioQuality::Auto,
        )?;
        let permits = manager
            .permits
            .acquire_many(CONCURRENT_DOWNLOADS as u32)
            .await?;
        let id = manager.queue(
            TrackItem {
                id: "local:fixture".into(),
                title: "Queued track".into(),
                artist: String::new(),
                artist_mid: None,
                cover: String::new(),
                duration_secs: 0.0,
                source: TrackSource::LocalFile {
                    path: "unused".into(),
                    format: "mp3".into(),
                },
            },
            DownloadFormat::Native,
        )?;
        assert!(manager.cancel(&id)?);
        drop(permits);
        assert_eq!(manager.tasks()[0].status, DownloadStatus::Cancelled);
        assert!(!output.exists());
        let restored = DownloadManager::new(store, bili, output, None, AudioQuality::Auto)?;
        assert_eq!(restored.tasks()[0].status, DownloadStatus::Cancelled);
        assert_eq!(restored.tasks()[0].output_file_type, "audio");
        Ok(())
    }

    #[tokio::test]
    async fn retries_restored_tasks_using_settings_at_execution_time() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path().join("settings"));
        let old_output = dir.path().join("old");
        let new_output = dir.path().join("new");
        let mut settings = store.app_settings()?;
        settings.download_path = old_output.to_string_lossy().into_owned();
        store.save_app_settings(&settings)?;

        let input = dir.path().join("in.mp3");
        let bytes = b"ID3\x04\x00\x00\x00\x00\x00\x00sound";
        tokio::fs::write(&input, bytes).await?;
        let fixtures = [
            ("paused", "downloadPaused", false, DownloadStatus::Paused),
            ("failed", "failed", false, DownloadStatus::Failed),
            ("cancelled", "failed", true, DownloadStatus::Cancelled),
        ];
        let mut entries = Map::new();
        for (id, status, cancelled, _) in &fixtures {
            let track = TrackItem {
                id: format!("local:{id}"),
                title: (*id).into(),
                artist: String::new(),
                artist_mid: None,
                cover: String::new(),
                duration_secs: 0.0,
                source: TrackSource::LocalFile {
                    path: input.to_string_lossy().into_owned(),
                    format: "mp3".into(),
                },
            };
            let task = json!({
                "id": id,
                "outputFileType": "audio",
                "title": id,
                "status": status,
                "cancelled": cancelled,
                "error": "old error",
                "downloadedBytes": 100,
                "track": track,
                "requestedFormat": "mp3"
            });
            entries.insert(format!("downloads.{id}"), task);
        }
        store.write("media-downloads", Value::Object(entries))?;
        let bili = Arc::new(BiliClient::new(dir.path().join("cookies.json"), None)?);
        let manager = DownloadManager::open(store.clone(), bili)?;
        let permits = manager
            .permits
            .acquire_many(CONCURRENT_DOWNLOADS as u32)
            .await?;
        for (id, _, _, status) in &fixtures {
            assert!(
                manager
                    .tasks()
                    .iter()
                    .any(|task| task.id == *id && task.status == *status)
            );
            assert!(manager.retry(id)?);
            assert!(!manager.retry(id)?);
        }
        settings.download_path = new_output.to_string_lossy().into_owned();
        settings.audio_quality = AudioQuality::Low;
        settings.ffmpeg_path = "/fixture/ffmpeg".into();
        store.save_app_settings(&settings)?;
        drop(permits);

        tokio::time::timeout(Duration::from_secs(3), async {
            while manager
                .tasks()
                .iter()
                .any(|task| task.status != DownloadStatus::Done)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await?;
        assert!(!old_output.exists());
        let history = store.read("media-downloads")?;
        for (id, _, _, _) in fixtures {
            assert_eq!(
                tokio::fs::read(new_output.join(format!("{id}.mp3"))).await?,
                bytes
            );
            let task = &history[format!("downloads.{id}")];
            assert_eq!(task["status"], "completed");
            assert_eq!(task["downloadedBytes"], bytes.len() as u64);
            assert!(task.get("cancelled").is_none());
            assert!(task.get("error").is_none());
            assert!(!manager.retry(id)?);
        }
        Ok(())
    }
}
