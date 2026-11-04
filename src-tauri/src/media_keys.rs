use souvlaki::{MediaControlEvent, MediaControls, MediaMetadata, MediaPlayback, PlatformConfig};
use std::sync::{mpsc, Mutex};
use tauri::{AppHandle, Emitter, Manager, Runtime};

pub enum MediaKeyMsg {
    Playback(bool),
    Metadata { title: String },
}

pub struct MediaKeysState {
    tx: Mutex<Option<mpsc::Sender<MediaKeyMsg>>>,
}

pub fn setup_media_keys<R: Runtime>(app: &AppHandle<R>) {
    let (tx, rx) = mpsc::channel::<MediaKeyMsg>();
    app.manage(MediaKeysState {
        tx: Mutex::new(Some(tx)),
    });

    #[cfg(target_os = "windows")]
    let hwnd_usize: Option<usize> = app
        .get_webview_window("main")
        .and_then(|w| w.hwnd().ok())
        .map(|h| h.0 as usize);
    #[cfg(not(target_os = "windows"))]
    let hwnd_usize: Option<usize> = None;

    let app_handle = app.clone();
    std::thread::spawn(move || {
        let config = PlatformConfig {
            dbus_name: "biu",
            display_name: "Biu",
            hwnd: hwnd_usize.map(|p| p as *mut std::ffi::c_void),
        };
        let mut controls = match MediaControls::new(config) {
            Ok(c) => c,
            Err(e) => {
                log::warn!("[media_keys] failed to initialize souvlaki MediaControls: {e:?}");
                return;
            }
        };

        let cb_app = app_handle.clone();
        if let Err(e) = controls.attach(move |event| match event {
            MediaControlEvent::Play | MediaControlEvent::Pause | MediaControlEvent::Toggle => {
                log::info!("[media_keys] received {event:?} -> player:toggle");
                let _ = cb_app.emit("player:toggle", ());
            }
            MediaControlEvent::Previous => {
                log::info!("[media_keys] received {event:?} -> player:prev");
                let _ = cb_app.emit("player:prev", ());
            }
            MediaControlEvent::Next => {
                log::info!("[media_keys] received {event:?} -> player:next");
                let _ = cb_app.emit("player:next", ());
            }
            MediaControlEvent::Raise => {
                if let Some(w) = cb_app.get_webview_window("main") {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
            MediaControlEvent::Quit => {
                crate::tray::quit_application(&cb_app);
            }
            _ => {}
        }) {
            log::warn!("[media_keys] failed to attach souvlaki handler: {e:?}");
            return;
        }

        let _ = controls.set_metadata(MediaMetadata {
            title: Some("Biu"),
            artist: Some("Bilibili Music"),
            ..Default::default()
        });
        let _ = controls.set_playback(MediaPlayback::Paused { progress: None });
        log::info!("[media_keys] souvlaki media controls attached (dbus_name=biu)");

        while let Ok(msg) = rx.recv() {
            match msg {
                MediaKeyMsg::Playback(is_playing) => {
                    let status = if is_playing {
                        MediaPlayback::Playing { progress: None }
                    } else {
                        MediaPlayback::Paused { progress: None }
                    };
                    if let Err(e) = controls.set_playback(status) {
                        log::warn!("[media_keys] set_playback({is_playing}) failed: {e:?}");
                    } else {
                        log::info!("[media_keys] playback state updated: is_playing={is_playing}");
                    }
                }
                MediaKeyMsg::Metadata { title } => {
                    let _ = controls.set_metadata(MediaMetadata {
                        title: Some(&title),
                        ..Default::default()
                    });
                }
            }
        }
    });
}

pub fn update_media_metadata<R: Runtime>(app: &AppHandle<R>, title: &str) {
    if let Some(state) = app.try_state::<MediaKeysState>() {
        if let Some(tx) = state.tx.lock().unwrap().as_ref() {
            let _ = tx.send(MediaKeyMsg::Metadata {
                title: title.to_string(),
            });
        }
    }
}

#[tauri::command]
pub fn update_playback_state<R: Runtime>(app: AppHandle<R>, is_playing: bool) {
    if let Some(state) = app.try_state::<MediaKeysState>() {
        if let Some(tx) = state.tx.lock().unwrap().as_ref() {
            let _ = tx.send(MediaKeyMsg::Playback(is_playing));
        }
    }
}
