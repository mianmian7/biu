//! OS media controls (macOS Now Playing, Windows SMTC, Linux MPRIS) via `souvlaki`.
//!
//! The `MediaControls` object lives on its own thread (mirroring `src-tauri/src/media_keys.rs`)
//! so platform quirks never block or panic the UI thread. If the platform integration cannot be
//! initialised (headless Linux without a session bus, Windows without an HWND, ...) the bridge
//! degrades to a no-op and `is_available()` returns false.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use std::time::Duration;

use souvlaki::{
    MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, MediaPosition, PlatformConfig,
    SeekDirection,
};

/// Seek step used for direction-only seek requests (no explicit amount).
const DEFAULT_SEEK_STEP_SECS: f64 = 10.0;

/// Action requested by the OS media controls.
#[derive(Debug, Clone, PartialEq)]
pub enum MediaControlAction {
    Play,
    Pause,
    Toggle,
    Next,
    Previous,
    Stop,
    /// Absolute position in seconds.
    Seek(f64),
    /// Relative seek in seconds (negative = backwards).
    SeekBy(f64),
    SetVolume(f64),
    /// Bring the main window to front.
    Raise,
    Quit,
}

impl MediaControlAction {
    fn from_event(event: MediaControlEvent) -> Option<Self> {
        Some(match event {
            MediaControlEvent::Play => Self::Play,
            MediaControlEvent::Pause => Self::Pause,
            MediaControlEvent::Toggle => Self::Toggle,
            MediaControlEvent::Next => Self::Next,
            MediaControlEvent::Previous => Self::Previous,
            MediaControlEvent::Stop => Self::Stop,
            MediaControlEvent::Seek(dir) => Self::SeekBy(signed(dir, DEFAULT_SEEK_STEP_SECS)),
            MediaControlEvent::SeekBy(dir, d) => Self::SeekBy(signed(dir, d.as_secs_f64())),
            MediaControlEvent::SetPosition(MediaPosition(d)) => Self::Seek(d.as_secs_f64()),
            MediaControlEvent::SetVolume(v) => Self::SetVolume(v),
            MediaControlEvent::Raise => Self::Raise,
            MediaControlEvent::Quit => Self::Quit,
            MediaControlEvent::OpenUri(_) => return None,
        })
    }
}

fn signed(dir: SeekDirection, secs: f64) -> f64 {
    match dir {
        SeekDirection::Forward => secs,
        SeekDirection::Backward => -secs,
    }
}

/// Configuration for [`MediaControlsBridge`].
#[derive(Debug, Clone)]
pub struct MediaControlsConfig {
    /// D-Bus name (Linux MPRIS: `org.mpris.MediaPlayer2.<dbus_name>`).
    pub dbus_name: String,
    pub display_name: String,
    /// Main window HWND as an integer (required on Windows, ignored elsewhere).
    pub hwnd: Option<usize>,
}

impl Default for MediaControlsConfig {
    fn default() -> Self {
        Self {
            dbus_name: "biu".into(),
            display_name: "Biu".into(),
            hwnd: None,
        }
    }
}

enum Msg {
    Metadata {
        title: String,
        artist: String,
        cover_url: Option<String>,
        duration_secs: Option<f64>,
    },
    Playback {
        playing: bool,
        position_secs: f64,
    },
    Stopped,
    Shutdown,
}

/// Handle to the OS media-controls thread. Dropping it detaches the controls.
pub struct MediaControlsBridge {
    tx: Sender<Msg>,
    available: Arc<AtomicBool>,
    thread: Mutex<Option<JoinHandle<()>>>,
}

impl MediaControlsBridge {
    /// Start the bridge; actions are delivered on the returned channel.
    pub fn new(config: MediaControlsConfig) -> (Self, Receiver<MediaControlAction>) {
        let (action_tx, action_rx) = mpsc::channel();
        let action_tx = Mutex::new(action_tx);
        let bridge = Self::with_callback(config, move |action| {
            let _ = action_tx
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .send(action);
        });
        (bridge, action_rx)
    }

    /// Start the bridge with a callback invoked (on a platform thread) for every action.
    pub fn with_callback<F>(config: MediaControlsConfig, callback: F) -> Self
    where
        F: Fn(MediaControlAction) + Send + Sync + 'static,
    {
        let (tx, rx) = mpsc::channel::<Msg>();
        let available = Arc::new(AtomicBool::new(false));
        let (ready_tx, ready_rx) = mpsc::channel::<()>();
        let avail = available.clone();
        let thread = std::thread::Builder::new()
            .name("biu-media-controls".into())
            .spawn(move || {
                let controls = init_controls(&config, callback);
                avail.store(controls.is_some(), Ordering::Release);
                let _ = ready_tx.send(());
                if let Some(controls) = controls {
                    run_controls(controls, rx);
                } else {
                    // Drain so senders never error; nothing to update.
                    while let Ok(msg) = rx.recv() {
                        if matches!(msg, Msg::Shutdown) {
                            break;
                        }
                    }
                }
            })
            .map_err(|e| log::warn!("[media_controls] failed to spawn thread: {e}"))
            .ok();
        // Wait briefly so `is_available()` is meaningful right after construction.
        let _ = ready_rx.recv_timeout(Duration::from_secs(2));
        Self {
            tx,
            available,
            thread: Mutex::new(thread),
        }
    }

    /// Whether OS media controls were successfully attached.
    pub fn is_available(&self) -> bool {
        self.available.load(Ordering::Acquire)
    }

    pub fn set_metadata(
        &self,
        title: &str,
        artist: &str,
        cover_url: Option<&str>,
        duration_secs: Option<f64>,
    ) {
        let _ = self.tx.send(Msg::Metadata {
            title: title.to_owned(),
            artist: artist.to_owned(),
            cover_url: cover_url.and_then(sanitize_cover_url),
            duration_secs: duration_secs.filter(|d| d.is_finite() && *d > 0.0),
        });
    }

    pub fn set_playback(&self, playing: bool, position_secs: f64) {
        let _ = self.tx.send(Msg::Playback {
            playing,
            position_secs,
        });
    }

    pub fn set_stopped(&self) {
        let _ = self.tx.send(Msg::Stopped);
    }
}

impl Drop for MediaControlsBridge {
    fn drop(&mut self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(h) = self.thread.lock().unwrap_or_else(|p| p.into_inner()).take() {
            let _ = h.join();
        }
    }
}

/// Filter cover URLs the platform backend can handle safely.
///
/// souvlaki 0.8 on macOS loads artwork via `NSImage initWithContentsOfURL` on a GCD thread and
/// then calls `[image size]` without a nil check: an unreachable/404/undecodable URL aborts the
/// whole process. So on macOS only `file://` URLs of existing files with a recognised image
/// signature are passed through (callers should hand over the locally cached cover).
/// Windows (SMTC thumbnail URI) and Linux (MPRIS `artUrl`) accept any URL.
pub fn sanitize_cover_url(url: &str) -> Option<String> {
    let url = url.trim();
    if url.is_empty() {
        return None;
    }
    if cfg!(target_os = "macos") {
        let path = url::Url::parse(url)
            .ok()
            .filter(|u| u.scheme() == "file")?
            .to_file_path()
            .ok()?;
        let mut head = [0u8; 12];
        let n = std::io::Read::read(&mut std::fs::File::open(&path).ok()?, &mut head).ok()?;
        return is_image_signature(&head[..n]).then(|| url.to_owned());
    }
    Some(url.to_owned())
}

fn is_image_signature(h: &[u8]) -> bool {
    h.starts_with(&[0xFF, 0xD8, 0xFF])
        || h.starts_with(b"\x89PNG\r\n\x1a\n")
        || h.starts_with(b"GIF8")
        || (h.len() >= 12 && &h[..4] == b"RIFF" && &h[8..12] == b"WEBP")
}

fn init_controls<F>(config: &MediaControlsConfig, callback: F) -> Option<MediaControls>
where
    F: Fn(MediaControlAction) + Send + Sync + 'static,
{
    #[cfg(target_os = "windows")]
    if config.hwnd.is_none() {
        log::warn!("[media_controls] no HWND provided; SMTC disabled");
        return None;
    }
    let platform = PlatformConfig {
        dbus_name: &config.dbus_name,
        display_name: &config.display_name,
        hwnd: config.hwnd.map(|p| p as *mut std::ffi::c_void),
    };
    // Some backends can panic in exotic environments; contain it.
    let created = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        MediaControls::new(platform)
    }));
    let mut controls = match created {
        Ok(Ok(c)) => c,
        Ok(Err(e)) => {
            log::warn!("[media_controls] unavailable: {e:?}");
            return None;
        }
        Err(_) => {
            log::warn!("[media_controls] backend panicked during init");
            return None;
        }
    };
    let attached = controls.attach(move |event| {
        if let Some(action) = MediaControlAction::from_event(event) {
            callback(action);
        }
    });
    if let Err(e) = attached {
        log::warn!("[media_controls] attach failed: {e:?}");
        return None;
    }
    let _ = controls.set_playback(MediaPlayback::Stopped);
    log::info!("[media_controls] attached (dbus_name={})", config.dbus_name);
    Some(controls)
}

fn run_controls(mut controls: MediaControls, rx: Receiver<Msg>) {
    while let Ok(msg) = rx.recv() {
        let result = match msg {
            Msg::Metadata {
                title,
                artist,
                cover_url,
                duration_secs,
            } => controls.set_metadata(MediaMetadata {
                title: Some(&title),
                artist: Some(&artist),
                album: None,
                cover_url: cover_url.as_deref(),
                duration: duration_secs.map(Duration::from_secs_f64),
            }),
            Msg::Playback {
                playing,
                position_secs,
            } => {
                let progress = (position_secs.is_finite() && position_secs >= 0.0)
                    .then(|| MediaPosition(Duration::from_secs_f64(position_secs)));
                controls.set_playback(if playing {
                    MediaPlayback::Playing { progress }
                } else {
                    MediaPlayback::Paused { progress }
                })
            }
            Msg::Stopped => controls.set_playback(MediaPlayback::Stopped),
            Msg::Shutdown => break,
        };
        if let Err(e) = result {
            log::debug!("[media_controls] update failed: {e:?}");
        }
    }
    let _ = controls.detach();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_platform_events() {
        use MediaControlAction as A;
        let cases = [
            (MediaControlEvent::Toggle, Some(A::Toggle)),
            (
                MediaControlEvent::SetPosition(MediaPosition(Duration::from_millis(1500))),
                Some(A::Seek(1.5)),
            ),
            (
                MediaControlEvent::SeekBy(SeekDirection::Backward, Duration::from_secs(5)),
                Some(A::SeekBy(-5.0)),
            ),
            (
                MediaControlEvent::Seek(SeekDirection::Forward),
                Some(A::SeekBy(DEFAULT_SEEK_STEP_SECS)),
            ),
            (MediaControlEvent::OpenUri("x".into()), None),
        ];
        for (ev, expected) in cases {
            assert_eq!(MediaControlAction::from_event(ev), expected);
        }
    }

    #[test]
    fn cover_url_sanitizing() {
        assert_eq!(sanitize_cover_url("  "), None);
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("c.png");
        std::fs::write(&png, b"\x89PNG\r\n\x1a\nrest").unwrap();
        let junk = dir.path().join("c.jpg");
        std::fs::write(&junk, b"<html>404</html>").unwrap();
        let png_url = url::Url::from_file_path(&png).unwrap().to_string();
        let junk_url = url::Url::from_file_path(&junk).unwrap().to_string();
        assert_eq!(sanitize_cover_url(&png_url), Some(png_url.clone()));
        if cfg!(target_os = "macos") {
            assert_eq!(sanitize_cover_url("https://i0.hdslb.com/x.jpg"), None);
            assert_eq!(sanitize_cover_url(&junk_url), None);
            assert_eq!(sanitize_cover_url("file:///nonexistent/a.png"), None);
        } else {
            assert!(sanitize_cover_url("https://i0.hdslb.com/x.jpg").is_some());
        }
    }

    #[test]
    fn bridge_never_panics_even_if_unavailable() {
        // Use a unique bus name so parallel runs don't collide on Linux.
        let (bridge, _rx) = MediaControlsBridge::new(MediaControlsConfig {
            dbus_name: format!("biu_test_{}", std::process::id()),
            ..Default::default()
        });
        bridge.set_metadata("Title", "Artist", Some("https://example.com/c.jpg"), Some(200.0));
        bridge.set_playback(true, 12.5);
        bridge.set_playback(false, f64::NAN);
        bridge.set_stopped();
        let _ = bridge.is_available();
        drop(bridge);
    }
}
