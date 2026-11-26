//! Playable audio sources, including a progressive "growing file" media source that lets
//! playback start while `MediaCache` is still downloading the file.

use std::fs::File;
use std::io::{self, Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use symphonia::core::io::MediaSource;

/// Default poll interval while waiting for more bytes of a growing file.
pub const GROWING_POLL_INTERVAL: Duration = Duration::from_millis(10);
/// Default idle timeout: give up if no new bytes arrive for this long.
pub const GROWING_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

/// Something the engine can play.
#[derive(Clone, Debug)]
pub enum AudioSource {
    /// A complete local or disk-cached file.
    File(PathBuf),
    /// A file that is still being written (progressive download into the media cache).
    StreamingFile {
        path: PathBuf,
        /// Final size in bytes if known (e.g. from `Content-Length`).
        total_bytes: Option<u64>,
        /// Set by the downloader once the file is complete.
        done: Arc<AtomicBool>,
        /// Set by the downloader (or owner) to abandon this download.
        aborted: Arc<AtomicBool>,
        /// Set by the downloader on a fatal download error.
        error: Arc<Mutex<Option<String>>>,
    },
    /// In-memory encoded bytes.
    Bytes {
        data: Arc<[u8]>,
        /// File extension hint (e.g. `"wav"`, `"m4a"`), without the dot.
        hint_ext: Option<String>,
    },
}

impl AudioSource {
    /// Convenience constructor for a streaming file with fresh control flags.
    pub fn streaming_file(path: impl Into<PathBuf>, total_bytes: Option<u64>) -> Self {
        Self::StreamingFile {
            path: path.into(),
            total_bytes,
            done: Arc::new(AtomicBool::new(false)),
            aborted: Arc::new(AtomicBool::new(false)),
            error: Arc::new(Mutex::new(None)),
        }
    }

    /// Convenience constructor for in-memory bytes.
    pub fn bytes(data: impl Into<Arc<[u8]>>, hint_ext: Option<&str>) -> Self {
        Self::Bytes {
            data: data.into(),
            hint_ext: hint_ext.map(|s| s.trim_start_matches('.').to_ascii_lowercase()),
        }
    }

    /// Lower-case file extension used as a probe hint.
    pub fn hint_extension(&self) -> Option<String> {
        match self {
            Self::File(path) | Self::StreamingFile { path, .. } => ext_of(path),
            Self::Bytes { hint_ext, .. } => hint_ext.clone(),
        }
    }

    /// Path on disk, if this source is file-backed.
    pub fn path(&self) -> Option<&Path> {
        match self {
            Self::File(path) | Self::StreamingFile { path, .. } => Some(path),
            Self::Bytes { .. } => None,
        }
    }

    /// Whether the source is a still-downloading file (not yet `done`).
    pub fn is_growing(&self) -> bool {
        matches!(self, Self::StreamingFile { done, .. } if !done.load(Ordering::Acquire))
    }

    /// Open a fresh Symphonia media source positioned at byte 0.
    pub fn open_media_source(&self) -> io::Result<Box<dyn MediaSource>> {
        self.open_media_source_cancellable(&[])
    }

    /// Like [`open_media_source`](Self::open_media_source), but a growing file stops waiting
    /// (open or read) as soon as any flag in `cancel` is set, returning a
    /// [`is_cancelled`]-style error within about one poll interval.
    pub fn open_media_source_cancellable(
        &self,
        cancel: &[Arc<AtomicBool>],
    ) -> io::Result<Box<dyn MediaSource>> {
        Ok(match self {
            Self::File(path) => Box::new(open_with_part_fallback(path)?),
            Self::StreamingFile {
                path,
                total_bytes,
                done,
                aborted,
                error,
            } => Box::new(GrowingFileMediaSource::open_cancellable(
                path.clone(),
                *total_bytes,
                done.clone(),
                aborted.clone(),
                error.clone(),
                GROWING_IDLE_TIMEOUT,
                cancel.to_vec(),
            )?),
            Self::Bytes { data, .. } => Box::new(Cursor::new(data.clone())),
        })
    }
}

/// Open `path`; if it is a `*.part` download file that no longer exists, open the finished
/// `*.audio` file instead (`MediaCache` renames `.part` -> `.audio` on completion).
pub fn open_with_part_fallback(path: &Path) -> io::Result<File> {
    match File::open(path) {
        Err(e) if e.kind() == io::ErrorKind::NotFound && is_part_file(path) => {
            File::open(path.with_extension("audio")).map_err(|e2| {
                if e2.kind() == io::ErrorKind::NotFound {
                    e
                } else {
                    e2
                }
            })
        }
        other => other,
    }
}

fn is_part_file(path: &Path) -> bool {
    path.extension().is_some_and(|e| e == "part")
}

/// Message of the error returned when a read is interrupted by a cancel flag.
const CANCELLED_MSG: &str = "streaming read cancelled";

/// Whether `err` was produced because a cancel flag was set (see
/// [`AudioSource::open_media_source_cancellable`]).
pub fn is_cancelled(err: &io::Error) -> bool {
    err.kind() == io::ErrorKind::Other && err.to_string() == CANCELLED_MSG
}

fn ext_of(path: &Path) -> Option<String> {
    path.extension()
        .and_then(|e| e.to_str())
        .map(|e| e.to_ascii_lowercase())
}

/// A `Read + Seek + MediaSource` over a file that may still be growing.
///
/// Reads that hit the current end of file block (polling every [`GROWING_POLL_INTERVAL`]) until
/// more bytes are written, the download is marked `done`, `aborted`, or errors, or no progress is
/// made for the idle timeout.
pub struct GrowingFileMediaSource {
    file: File,
    pos: u64,
    total_bytes: Option<u64>,
    done: Arc<AtomicBool>,
    aborted: Arc<AtomicBool>,
    error: Arc<Mutex<Option<String>>>,
    poll_interval: Duration,
    idle_timeout: Duration,
    /// Any set flag aborts a blocked open/read promptly (track switch / stop).
    cancel: Vec<Arc<AtomicBool>>,
}

impl GrowingFileMediaSource {
    /// Open the file, waiting (up to the idle timeout) for the downloader to create it.
    pub fn open(
        path: PathBuf,
        total_bytes: Option<u64>,
        done: Arc<AtomicBool>,
        aborted: Arc<AtomicBool>,
        error: Arc<Mutex<Option<String>>>,
    ) -> io::Result<Self> {
        Self::open_with_timeout(
            path,
            total_bytes,
            done,
            aborted,
            error,
            GROWING_IDLE_TIMEOUT,
        )
    }

    /// Like [`open`](Self::open) with a custom idle timeout.
    pub fn open_with_timeout(
        path: PathBuf,
        total_bytes: Option<u64>,
        done: Arc<AtomicBool>,
        aborted: Arc<AtomicBool>,
        error: Arc<Mutex<Option<String>>>,
        idle_timeout: Duration,
    ) -> io::Result<Self> {
        Self::open_cancellable(
            path,
            total_bytes,
            done,
            aborted,
            error,
            idle_timeout,
            Vec::new(),
        )
    }

    /// Like [`open_with_timeout`](Self::open_with_timeout); setting any flag in `cancel` makes a
    /// blocked open or read return an error (see [`is_cancelled`]) within one poll interval.
    pub fn open_cancellable(
        path: PathBuf,
        total_bytes: Option<u64>,
        done: Arc<AtomicBool>,
        aborted: Arc<AtomicBool>,
        error: Arc<Mutex<Option<String>>>,
        idle_timeout: Duration,
        cancel: Vec<Arc<AtomicBool>>,
    ) -> io::Result<Self> {
        let started = Instant::now();
        let file = loop {
            // Falls back to `*.audio` if the download finished and was renamed meanwhile.
            match open_with_part_fallback(&path) {
                Ok(f) => break f,
                Err(e) if e.kind() == io::ErrorKind::NotFound => {
                    check_cancel(&cancel)?;
                    check_flags(&aborted, &error)?;
                    if done.load(Ordering::Acquire) || started.elapsed() >= idle_timeout {
                        return Err(e);
                    }
                    std::thread::sleep(GROWING_POLL_INTERVAL);
                }
                Err(e) => return Err(e),
            }
        };
        Ok(Self {
            file,
            pos: 0,
            total_bytes,
            done,
            aborted,
            error,
            poll_interval: GROWING_POLL_INTERVAL,
            idle_timeout,
            cancel,
        })
    }

    /// Add a cancel flag: once set, blocked reads return an error (see [`is_cancelled`]).
    pub fn with_cancel(mut self, cancel: Arc<AtomicBool>) -> Self {
        self.cancel.push(cancel);
        self
    }

    /// Override the idle timeout (mainly for tests).
    pub fn with_idle_timeout(mut self, timeout: Duration) -> Self {
        self.idle_timeout = timeout;
        self
    }

    fn current_len(&self) -> io::Result<u64> {
        Ok(self.file.metadata()?.len())
    }

    fn is_done(&self) -> bool {
        self.done.load(Ordering::Acquire)
    }
}

fn check_cancel(cancel: &[Arc<AtomicBool>]) -> io::Result<()> {
    if cancel.iter().any(|c| c.load(Ordering::Acquire)) {
        return Err(io::Error::other(CANCELLED_MSG));
    }
    Ok(())
}

fn check_flags(aborted: &AtomicBool, error: &Mutex<Option<String>>) -> io::Result<()> {
    if aborted.load(Ordering::Acquire) {
        return Err(io::Error::new(
            io::ErrorKind::ConnectionAborted,
            "streaming download aborted",
        ));
    }
    if let Some(msg) = error.lock().unwrap_or_else(|p| p.into_inner()).as_ref() {
        return Err(io::Error::other(format!(
            "streaming download failed: {msg}"
        )));
    }
    Ok(())
}

impl Read for GrowingFileMediaSource {
    fn read(&mut self, buf: &mut [u8]) -> io::Result<usize> {
        if buf.is_empty() {
            return Ok(0);
        }
        let mut idle_since = Instant::now();
        loop {
            check_cancel(&self.cancel)?;
            check_flags(&self.aborted, &self.error)?;
            // Sample `done` *before* reading so a final write that races with `done = true`
            // is still observed by the read below.
            let done_before = self.is_done();
            let n = self.file.read(buf)?;
            if n > 0 {
                self.pos += n as u64;
                return Ok(n);
            }
            if done_before {
                return Ok(0);
            }
            if self.total_bytes.is_some_and(|t| self.pos >= t) {
                return Ok(0);
            }
            if idle_since.elapsed() >= self.idle_timeout {
                return Err(io::Error::new(
                    io::ErrorKind::TimedOut,
                    "streaming download stalled",
                ));
            }
            std::thread::sleep(self.poll_interval);
            // Re-sync the OS cursor in case the platform advanced it past EOF.
            self.file.seek(SeekFrom::Start(self.pos))?;
            if self.current_len()? > self.pos {
                idle_since = Instant::now();
            }
        }
    }
}

impl Seek for GrowingFileMediaSource {
    fn seek(&mut self, pos: SeekFrom) -> io::Result<u64> {
        let target = match pos {
            SeekFrom::Start(n) => n as i128,
            SeekFrom::Current(d) => self.pos as i128 + d as i128,
            SeekFrom::End(d) => {
                let end = match self.total_bytes {
                    Some(t) => t,
                    None => self.current_len()?,
                };
                end as i128 + d as i128
            }
        };
        if target < 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "seek before start of stream",
            ));
        }
        let target = target as u64;
        self.file.seek(SeekFrom::Start(target))?;
        self.pos = target;
        Ok(target)
    }
}

impl MediaSource for GrowingFileMediaSource {
    fn is_seekable(&self) -> bool {
        if self.is_done() {
            return true;
        }
        // Fully written even though the downloader has not flagged completion yet.
        match (self.total_bytes, self.current_len()) {
            (Some(t), Ok(len)) => len >= t,
            _ => false,
        }
    }

    fn byte_len(&self) -> Option<u64> {
        if let Some(t) = self.total_bytes {
            return Some(t);
        }
        if self.is_done() {
            return self.current_len().ok();
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    #[test]
    fn reads_until_done_with_trickle() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("grow.bin");
        let payload: Vec<u8> = (0..50_000u32).map(|i| (i % 251) as u8).collect();
        File::create(&path).unwrap();
        let src = AudioSource::streaming_file(&path, None);
        let AudioSource::StreamingFile { done, .. } = &src else {
            unreachable!()
        };
        let done = done.clone();
        let p2 = path.clone();
        let data = payload.clone();
        let producer = std::thread::spawn(move || {
            let mut f = std::fs::OpenOptions::new().append(true).open(&p2).unwrap();
            for chunk in data.chunks(4096) {
                f.write_all(chunk).unwrap();
                f.flush().unwrap();
                std::thread::sleep(Duration::from_millis(5));
            }
            done.store(true, Ordering::Release);
        });
        let mut ms = src.open_media_source().unwrap();
        assert!(ms.byte_len().is_none());
        let mut out = Vec::new();
        ms.read_to_end(&mut out).unwrap();
        producer.join().unwrap();
        assert_eq!(out, payload);
        assert!(ms.is_seekable());
    }

    #[test]
    fn aborted_returns_error() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("abort.bin");
        std::fs::write(&path, [1u8; 100]).unwrap();
        let src = AudioSource::streaming_file(&path, Some(10_000));
        let AudioSource::StreamingFile { aborted, .. } = &src else {
            unreachable!()
        };
        let aborted = aborted.clone();
        let mut ms = src.open_media_source().unwrap();
        let mut buf = [0u8; 100];
        assert_eq!(ms.read(&mut buf).unwrap(), 100);
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(50));
            aborted.store(true, Ordering::Release);
        });
        let err = ms.read(&mut buf).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::ConnectionAborted);
    }

    #[test]
    fn error_flag_and_idle_timeout() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("err.bin");
        std::fs::write(&path, [0u8; 4]).unwrap();
        let error = Arc::new(Mutex::new(None));
        let mut ms = GrowingFileMediaSource::open(
            path.clone(),
            None,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicBool::new(false)),
            error.clone(),
        )
        .unwrap()
        .with_idle_timeout(Duration::from_millis(60));
        let mut buf = [0u8; 8];
        assert_eq!(ms.read(&mut buf).unwrap(), 4);
        assert_eq!(
            ms.read(&mut buf).unwrap_err().kind(),
            io::ErrorKind::TimedOut
        );
        *error.lock().unwrap() = Some("http 403".into());
        let err = ms.read(&mut buf).unwrap_err();
        assert!(err.to_string().contains("http 403"));
    }

    #[test]
    fn part_renamed_to_audio_before_open_and_reopen() {
        let dir = tempfile::tempdir().unwrap();
        let part = dir.path().join("track.part");
        let audio = dir.path().join("track.audio");
        let payload: Vec<u8> = (0..10_000u32).map(|i| (i % 253) as u8).collect();

        // Reopen case: first open succeeds on `.part`, then the cache finishes and renames it
        // right before a reopen (e.g. backwards seek / ffmpeg restart).
        std::fs::write(&part, &payload).unwrap();
        let src = AudioSource::streaming_file(&part, Some(payload.len() as u64));
        let mut first = src.open_media_source().unwrap();
        let mut head = [0u8; 16];
        first.read_exact(&mut head).unwrap();
        assert_eq!(&head, &payload[..16]);
        std::fs::rename(&part, &audio).unwrap();
        let AudioSource::StreamingFile { done, .. } = &src else {
            unreachable!()
        };
        done.store(true, Ordering::Release);
        let mut out = Vec::new();
        src.open_media_source()
            .expect("reopen must fall back to .audio")
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, payload);

        // Open case: the rename already happened before the very first open.
        let src = AudioSource::streaming_file(&part, None);
        let mut out = Vec::new();
        let mut ms = src
            .open_media_source()
            .expect("open must fall back to .audio");
        if let AudioSource::StreamingFile { done, .. } = &src {
            done.store(true, Ordering::Release);
        }
        ms.read_to_end(&mut out).unwrap();
        assert_eq!(out, payload);
        // Complete files referenced by their old `.part` path also resolve.
        let mut out = Vec::new();
        AudioSource::File(part.clone())
            .open_media_source()
            .unwrap()
            .read_to_end(&mut out)
            .unwrap();
        assert_eq!(out, payload);

        // Neither exists: the original NotFound for `.part` is reported.
        std::fs::remove_file(&audio).unwrap();
        let err = open_with_part_fallback(&part).unwrap_err();
        assert_eq!(err.kind(), io::ErrorKind::NotFound);
    }

    #[test]
    fn cancel_interrupts_blocked_read_and_open_quickly() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("stall.bin");
        std::fs::write(&path, [5u8; 4]).unwrap();
        let src = AudioSource::streaming_file(&path, Some(1_000_000));
        let cancel = Arc::new(AtomicBool::new(false));
        let mut ms = src
            .open_media_source_cancellable(std::slice::from_ref(&cancel))
            .unwrap();
        let mut buf = [0u8; 64];
        assert_eq!(ms.read(&mut buf).unwrap(), 4);

        let setter = {
            let cancel = cancel.clone();
            std::thread::spawn(move || {
                std::thread::sleep(Duration::from_millis(60));
                cancel.store(true, Ordering::Release);
                Instant::now()
            })
        };
        // Blocks waiting for bytes that never come (idle timeout is 30 s).
        let err = ms.read(&mut buf).unwrap_err();
        let returned = Instant::now();
        let cancelled_at = setter.join().unwrap();
        assert!(is_cancelled(&err), "{err}");
        let lag = returned.saturating_duration_since(cancelled_at);
        assert!(lag <= Duration::from_millis(20), "cancel took {lag:?}");

        // Waiting for a file that does not exist yet is cancellable too.
        let missing = AudioSource::streaming_file(dir.path().join("never.part"), None);
        let started = Instant::now();
        let err = missing
            .open_media_source_cancellable(&[cancel])
            .err()
            .unwrap();
        assert!(is_cancelled(&err), "{err}");
        assert!(started.elapsed() < Duration::from_millis(20));
    }

    #[test]
    fn hint_extension_lowercases() {
        assert_eq!(
            AudioSource::File("a/B.M4S".into())
                .hint_extension()
                .as_deref(),
            Some("m4s")
        );
        assert_eq!(
            AudioSource::bytes(vec![0u8; 1], Some(".WAV"))
                .hint_extension()
                .as_deref(),
            Some("wav")
        );
    }
}
