use crate::desktop_lyrics::{
    destroy_desktop_lyrics_window, destroy_mini_player, get_default_bounds, persist_window_bounds,
    DesktopLyricsState,
};
use crate::store::read_store_object;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use tauri::{
    menu::{CheckMenuItem, MenuBuilder, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, Runtime,
};

pub struct TrayState<R: Runtime> {
    pub quitting: AtomicBool,
    pub desktop_lyrics_enabled: AtomicBool,
    pub desktop_lyrics_locked: AtomicBool,
    pub check_desktop_lyrics: Mutex<Option<CheckMenuItem<R>>>,
    pub check_desktop_lyrics_lock: Mutex<Option<CheckMenuItem<R>>>,
}

impl<R: Runtime> Default for TrayState<R> {
    fn default() -> Self {
        Self {
            quitting: AtomicBool::new(false),
            desktop_lyrics_enabled: AtomicBool::new(false),
            desktop_lyrics_locked: AtomicBool::new(true),
            check_desktop_lyrics: Mutex::new(None),
            check_desktop_lyrics_lock: Mutex::new(None),
        }
    }
}

pub fn update_tray_desktop_lyrics_state<R: Runtime>(
    app: &AppHandle<R>,
    enabled: bool,
    locked: bool,
) {
    let Some(state) = app.try_state::<Arc<TrayState<R>>>() else {
        return;
    };
    let prev_enabled = state
        .desktop_lyrics_enabled
        .swap(enabled, Ordering::Relaxed);
    let prev_locked = state.desktop_lyrics_locked.swap(locked, Ordering::Relaxed);

    if prev_enabled != enabled {
        if let Some(item) = state.check_desktop_lyrics.lock().unwrap().as_ref() {
            let _ = item.set_checked(enabled);
        }
    }
    if prev_locked != locked {
        if let Some(item) = state.check_desktop_lyrics_lock.lock().unwrap().as_ref() {
            let _ = item.set_checked(locked);
        }
    }
}

pub fn setup_tray<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let (initial_enabled, initial_locked) = read_store_object(app, "app-settings")
        .ok()
        .and_then(|v| {
            let s = v.get("appSettings")?;
            let en = s
                .get("showDesktopLyrics")
                .and_then(|x| x.as_bool())
                .unwrap_or(false);
            let lk = s
                .get("desktopLyricsLocked")
                .and_then(|x| x.as_bool())
                .unwrap_or(true);
            Some((en, lk))
        })
        .unwrap_or((false, true));

    let tray_state = Arc::new(TrayState::<R> {
        quitting: AtomicBool::new(false),
        desktop_lyrics_enabled: AtomicBool::new(initial_enabled),
        desktop_lyrics_locked: AtomicBool::new(initial_locked),
        check_desktop_lyrics: Mutex::new(None),
        check_desktop_lyrics_lock: Mutex::new(None),
    });

    let item_toggle_play = MenuItem::with_id(app, "play_toggle", "播放/暂停", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let item_prev = MenuItem::with_id(app, "play_prev", "上一曲", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let item_next = MenuItem::with_id(app, "play_next", "下一曲", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let item_toggle_win =
        MenuItem::with_id(app, "toggle_window", "显示/隐藏界面", true, None::<&str>)
            .map_err(|e| e.to_string())?;
    let sep1 = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;

    let item_dl_toggle = CheckMenuItem::with_id(
        app,
        "desktop_lyrics_toggle",
        "桌面歌词",
        true,
        initial_enabled,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    let item_dl_lock = CheckMenuItem::with_id(
        app,
        "desktop_lyrics_lock",
        "锁定桌面歌词（鼠标穿透）",
        true,
        initial_locked,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    let item_dl_reset = MenuItem::with_id(
        app,
        "desktop_lyrics_reset",
        "重置桌面歌词位置",
        true,
        None::<&str>,
    )
    .map_err(|e| e.to_string())?;
    let sep2 = PredefinedMenuItem::separator(app).map_err(|e| e.to_string())?;

    let item_settings = MenuItem::with_id(app, "settings", "设置", true, None::<&str>)
        .map_err(|e| e.to_string())?;
    let item_quit = MenuItem::with_id(app, "quit", "退出程序", true, None::<&str>)
        .map_err(|e| e.to_string())?;

    *tray_state.check_desktop_lyrics.lock().unwrap() = Some(item_dl_toggle.clone());
    *tray_state.check_desktop_lyrics_lock.lock().unwrap() = Some(item_dl_lock.clone());
    app.manage(tray_state);

    let menu = MenuBuilder::new(app)
        .items(&[
            &item_toggle_play,
            &item_prev,
            &item_next,
            &item_toggle_win,
            &sep1,
            &item_dl_toggle,
            &item_dl_lock,
            &item_dl_reset,
            &sep2,
            &item_settings,
            &item_quit,
        ])
        .build()
        .map_err(|e| e.to_string())?;

    let mut builder = TrayIconBuilder::with_id("main-tray")
        .tooltip("Biu")
        .menu(&menu)
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "play_toggle" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.emit("player:toggle", ());
                }
            }
            "play_prev" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.emit("player:prev", ());
                }
            }
            "play_next" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.emit("player:next", ());
                }
            }
            "toggle_window" => {
                if let Some(w) = app.get_webview_window("main") {
                    if w.is_visible().unwrap_or(false) {
                        let _ = w.hide();
                    } else {
                        let _ = w.unminimize();
                        let _ = w.show();
                        let _ = w.set_focus();
                    }
                }
            }
            "desktop_lyrics_toggle" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.emit("desktop-lyrics:toggle-requested", ());
                }
            }
            "desktop_lyrics_lock" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.emit("desktop-lyrics:lock-toggle-requested", ());
                }
            }
            "desktop_lyrics_reset" => {
                if let (Some(win), Some(dl_state)) = (
                    app.get_webview_window("desktop-lyrics"),
                    app.try_state::<Arc<DesktopLyricsState>>(),
                ) {
                    let def = get_default_bounds(app, *dl_state.content_height.lock().unwrap());
                    let _ = win.set_position(LogicalPosition::new(def.x, def.y));
                    let _ = win.set_size(LogicalSize::new(def.width, def.height));
                    persist_window_bounds(app, &win);
                }
            }
            "settings" => {
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                    let _ = w.emit("router:navigate", "/settings");
                }
            }
            "quit" => {
                quit_application(app);
            }
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                let app = tray.app_handle();
                destroy_mini_player(app);
                if let Some(w) = app.get_webview_window("main") {
                    let _ = w.unminimize();
                    let _ = w.show();
                    let _ = w.set_focus();
                }
            }
        });

    if let Some(icon) = app.default_window_icon().cloned() {
        builder = builder.icon(icon);
    }

    builder.build(app).map_err(|e| e.to_string())?;
    log::info!(
        "[tray] system tray initialized (desktop_lyrics_enabled={initial_enabled}, locked={initial_locked})"
    );
    Ok(())
}

pub fn quit_application<R: Runtime>(app: &AppHandle<R>) {
    if let Some(ts) = app.try_state::<Arc<TrayState<R>>>() {
        ts.quitting.store(true, Ordering::Relaxed);
    }
    destroy_mini_player(app);
    if let Some(dl_state) = app.try_state::<Arc<DesktopLyricsState>>() {
        destroy_desktop_lyrics_window(app, &dl_state);
    }
    log::info!("[tray] quitting application");
    app.exit(0);
}

/// Returns true if the close request was handled by hiding the main window to the tray.
pub fn handle_main_window_close_request<R: Runtime>(app: &AppHandle<R>) -> bool {
    if let Some(ts) = app.try_state::<Arc<TrayState<R>>>() {
        if ts.quitting.load(Ordering::Relaxed) {
            return false;
        }
    }

    let close_option = read_store_object(app, "app-settings")
        .ok()
        .and_then(|v| {
            v.get("appSettings")?
                .get("closeWindowOption")?
                .as_str()
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "hide".to_string());

    if close_option == "hide" {
        if let Some(w) = app.get_webview_window("main") {
            let _ = w.hide();
        }
        log::info!("[tray] main window hidden to tray (closeWindowOption=hide)");
        true
    } else {
        quit_application(app);
        false
    }
}
