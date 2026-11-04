use crate::store::{read_store_object, store_set};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};
use std::time::Duration;
use tauri::{
    AppHandle, Emitter, LogicalPosition, LogicalSize, Manager, PhysicalPosition, Runtime,
    WebviewUrl, WebviewWindow, WebviewWindowBuilder, WindowEvent,
};

const DEFAULT_WIDTH: f64 = 1000.0;
const DEFAULT_HEIGHT: f64 = 150.0;
const MIN_WIDTH: f64 = 360.0;
const MIN_HEIGHT: f64 = 60.0;
const MAX_HEIGHT: f64 = 520.0;
const UNBOUNDED_WIDTH: f64 = 20000.0;
const DEFAULT_BOTTOM_OFFSET: f64 = 72.0;
const MIN_VISIBLE_SIZE: f64 = 40.0;
const DRAG_INTERVAL_MS: u64 = 12;

const ALLOWED_COMMANDS: &[&str] = &[
    "prev",
    "next",
    "togglePlay",
    "toggleLock",
    "close",
    "fontLarger",
    "fontSmaller",
    "nextColor",
    "toggleLineMode",
    "toggleTranslation",
    "toggleRomaji",
    "openSettings",
];

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
pub struct DesktopLyricsBounds {
    pub x: f64,
    pub y: f64,
    pub width: f64,
    pub height: f64,
}

pub struct DesktopLyricsState {
    pub last_payload: Mutex<Option<Value>>,
    pub content_height: Mutex<Option<f64>>,
    pub hover_unlock_button: AtomicBool,
    pub locked: AtomicBool,
    pub ignore_mouse: AtomicBool,
    pub dragging: AtomicBool,
}

impl Default for DesktopLyricsState {
    fn default() -> Self {
        Self {
            last_payload: Mutex::new(None),
            content_height: Mutex::new(None),
            hover_unlock_button: AtomicBool::new(false),
            locked: AtomicBool::new(true),
            ignore_mouse: AtomicBool::new(true),
            dragging: AtomicBool::new(false),
        }
    }
}

fn primary_work_area<R: Runtime>(app: &AppHandle<R>) -> (f64, f64, f64, f64) {
    if let Ok(Some(m)) = app.primary_monitor() {
        let scale = m.scale_factor();
        let pos = m.position().to_logical::<f64>(scale);
        let size = m.size().to_logical::<f64>(scale);
        return (pos.x, pos.y, size.width, size.height);
    }
    if let Ok(monitors) = app.available_monitors() {
        if let Some(m) = monitors.first() {
            let scale = m.scale_factor();
            let pos = m.position().to_logical::<f64>(scale);
            let size = m.size().to_logical::<f64>(scale);
            return (pos.x, pos.y, size.width, size.height);
        }
    }
    (0.0, 0.0, 1280.0, 800.0)
}

pub fn get_default_bounds<R: Runtime>(
    app: &AppHandle<R>,
    content_height: Option<f64>,
) -> DesktopLyricsBounds {
    let (wx, wy, ww, wh) = primary_work_area(app);
    let width = DEFAULT_WIDTH.min((ww - 80.0).max(MIN_WIDTH));
    let preferred_h = content_height
        .map(|h| h.round().min(MAX_HEIGHT))
        .unwrap_or(DEFAULT_HEIGHT);
    let height = preferred_h.min((wh - 80.0).max(MIN_HEIGHT)).max(MIN_HEIGHT);

    DesktopLyricsBounds {
        x: (wx + (ww - width) / 2.0).round(),
        y: (wy + wh - height - DEFAULT_BOTTOM_OFFSET).round(),
        width,
        height,
    }
}

pub fn is_bounds_visible<R: Runtime>(app: &AppHandle<R>, bounds: &DesktopLyricsBounds) -> bool {
    let Ok(monitors) = app.available_monitors() else {
        return true;
    };
    if monitors.is_empty() {
        return true;
    }
    monitors.iter().any(|m| {
        let scale = m.scale_factor();
        let pos = m.position().to_logical::<f64>(scale);
        let size = m.size().to_logical::<f64>(scale);
        let overlap_w = (bounds.x + bounds.width).min(pos.x + size.width) - bounds.x.max(pos.x);
        let overlap_h = (bounds.y + bounds.height).min(pos.y + size.height) - bounds.y.max(pos.y);
        overlap_w >= MIN_VISIBLE_SIZE && overlap_h >= MIN_VISIBLE_SIZE
    })
}

pub fn get_initial_bounds<R: Runtime>(
    app: &AppHandle<R>,
    content_height: Option<f64>,
) -> DesktopLyricsBounds {
    if let Ok(store_val) = read_store_object(app, "desktop-lyrics") {
        if let Some(b_val) = store_val.get("bounds") {
            if let Ok(saved) = serde_json::from_value::<DesktopLyricsBounds>(b_val.clone()) {
                let normalized = DesktopLyricsBounds {
                    x: saved.x,
                    y: saved.y,
                    width: saved.width.max(MIN_WIDTH),
                    height: saved.height.max(MIN_HEIGHT),
                };
                if is_bounds_visible(app, &normalized) {
                    return normalized;
                }
            }
        }
    }
    get_default_bounds(app, content_height)
}

pub fn read_window_bounds<R: Runtime>(win: &WebviewWindow<R>) -> Option<DesktopLyricsBounds> {
    let scale = win.scale_factor().unwrap_or(1.0);
    let pos = win.outer_position().ok()?.to_logical::<f64>(scale);
    let size = win.inner_size().ok()?.to_logical::<f64>(scale);
    Some(DesktopLyricsBounds {
        x: pos.x.round(),
        y: pos.y.round(),
        width: size.width.round(),
        height: size.height.round(),
    })
}

pub fn persist_window_bounds<R: Runtime>(app: &AppHandle<R>, win: &WebviewWindow<R>) {
    if let Some(bounds) = read_window_bounds(win) {
        let _ = store_set(
            app.clone(),
            "desktop-lyrics".to_string(),
            json!({ "bounds": bounds }),
        );
    }
}

pub fn apply_locked_state<R: Runtime>(
    win: &WebviewWindow<R>,
    state: &DesktopLyricsState,
    locked: bool,
) {
    state.locked.store(locked, Ordering::Relaxed);
    if !locked {
        state.hover_unlock_button.store(false, Ordering::Relaxed);
    }
    let ignore = locked && !state.hover_unlock_button.load(Ordering::Relaxed);
    let prev = state.ignore_mouse.swap(ignore, Ordering::Relaxed);
    if let Err(e) = win.set_ignore_cursor_events(ignore) {
        log::warn!("[desktop-lyrics] set_ignore_cursor_events({ignore}) failed: {e}");
    } else if prev != ignore {
        log::info!("[desktop-lyrics] set_ignore_cursor_events({ignore}) (locked={locked})");
    }
}

pub fn apply_content_height<R: Runtime>(
    app: &AppHandle<R>,
    win: &WebviewWindow<R>,
    state: &DesktopLyricsState,
) {
    let Some(raw_h) = *state.content_height.lock().unwrap() else {
        return;
    };
    let height = raw_h.round().clamp(MIN_HEIGHT, MAX_HEIGHT);
    let _ = win.set_min_size(Some(LogicalSize::new(MIN_WIDTH, height)));
    let _ = win.set_max_size(Some(LogicalSize::new(UNBOUNDED_WIDTH, height)));

    let Some(bounds) = read_window_bounds(win) else {
        return;
    };
    if (bounds.height - height).abs() < 1.0 {
        return;
    }

    let (_, wy, _, wh) = primary_work_area(app);
    let anchor_bottom = bounds.y + bounds.height / 2.0 > wy + wh / 2.0;
    let y = if anchor_bottom {
        (bounds.y + bounds.height - height).round()
    } else {
        bounds.y
    };

    let _ = win.set_size(LogicalSize::new(bounds.width, height));
    let _ = win.set_position(LogicalPosition::new(bounds.x, y));
    persist_window_bounds(app, win);
}

pub async fn ensure_desktop_lyrics_window<R: Runtime>(
    app: &AppHandle<R>,
    state: &Arc<DesktopLyricsState>,
) -> Result<WebviewWindow<R>, String> {
    if let Some(w) = app.get_webview_window("desktop-lyrics") {
        return Ok(w);
    }

    let ch = *state.content_height.lock().unwrap();
    let bounds = get_initial_bounds(app, ch);

    let win = WebviewWindowBuilder::new(
        app,
        "desktop-lyrics",
        WebviewUrl::App("index.html#desktop-lyrics".into()),
    )
    .title("Biu Desktop Lyrics")
    .inner_size(bounds.width, bounds.height)
    .position(bounds.x, bounds.y)
    .min_inner_size(MIN_WIDTH, MIN_HEIGHT)
    .resizable(true)
    .maximizable(false)
    .minimizable(false)
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .focused(false)
    .visible(false)
    .build()
    .map_err(|e| e.to_string())?;

    let locked = state
        .last_payload
        .lock()
        .unwrap()
        .as_ref()
        .and_then(|p| p.get("locked"))
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    state.ignore_mouse.store(!locked, Ordering::Relaxed);
    apply_locked_state(&win, state, locked);

    log::info!(
        "[desktop-lyrics] window created (x={}, y={}, width={}, height={}, transparent=true, always_on_top=true, locked={locked})",
        bounds.x,
        bounds.y,
        bounds.width,
        bounds.height
    );

    let alive = Arc::new(AtomicBool::new(true));
    let alive_for_events = alive.clone();
    let app_for_events = app.clone();
    let state_for_events = state.clone();
    let win_for_events = win.clone();
    win.on_window_event(move |event| match event {
        WindowEvent::Moved(_) | WindowEvent::Resized(_) => {
            if !state_for_events.dragging.load(Ordering::Relaxed) {
                persist_window_bounds(&app_for_events, &win_for_events);
            }
        }
        WindowEvent::Focused(false) => {
            if state_for_events.dragging.swap(false, Ordering::Relaxed) {
                persist_window_bounds(&app_for_events, &win_for_events);
            }
        }
        WindowEvent::Destroyed => {
            alive_for_events.store(false, Ordering::Relaxed);
            state_for_events.dragging.store(false, Ordering::Relaxed);
        }
        _ => {}
    });

    // Poll cursor when locked & visible so hovering the window reveals the unlock button
    // even when OS-level click-through suppresses mousemove events.
    let poll_win = win.clone();
    let poll_state = state.clone();
    std::thread::spawn(move || {
        let mut was_inside_window = false;
        while alive.load(Ordering::Relaxed) {
            if poll_state.locked.load(Ordering::Relaxed) && poll_win.is_visible().unwrap_or(false) {
                if let (Ok(cursor), Ok(pos), Ok(size)) = (
                    poll_win.cursor_position(),
                    poll_win.outer_position(),
                    poll_win.outer_size(),
                ) {
                    let scale = poll_win.scale_factor().unwrap_or(1.0);
                    let rel_x = (cursor.x - pos.x as f64) / scale;
                    let rel_y = (cursor.y - pos.y as f64) / scale;
                    let w_logical = size.width as f64 / scale;
                    let h_logical = size.height as f64 / scale;

                    let inside_win =
                        rel_x >= 0.0 && rel_x <= w_logical && rel_y >= 0.0 && rel_y <= h_logical;
                    if inside_win {
                        let _ = poll_win.emit("desktop-lyrics:cursor-inside", ());
                    } else if was_inside_window {
                        let _ = poll_win.emit("desktop-lyrics:cursor-leave", ());
                    }
                    was_inside_window = inside_win;

                    // Top-center unlock button region (28x28 button inside 34px toolbar)
                    let center_x = w_logical / 2.0;
                    let hit_unlock = inside_win
                        && (rel_x - center_x).abs() <= 24.0
                        && (0.0..=38.0).contains(&rel_y);
                    let prev_hover = poll_state
                        .hover_unlock_button
                        .swap(hit_unlock, Ordering::Relaxed);
                    if prev_hover != hit_unlock {
                        apply_locked_state(
                            &poll_win,
                            &poll_state,
                            poll_state.locked.load(Ordering::Relaxed),
                        );
                    }
                }
            } else {
                was_inside_window = false;
            }
            std::thread::sleep(Duration::from_millis(80));
        }
    });

    Ok(win)
}

pub fn destroy_desktop_lyrics_window<R: Runtime>(app: &AppHandle<R>, state: &DesktopLyricsState) {
    state.dragging.store(false, Ordering::Relaxed);
    state.hover_unlock_button.store(false, Ordering::Relaxed);
    if let Some(w) = app.get_webview_window("desktop-lyrics") {
        let _ = w.destroy();
        log::info!("[desktop-lyrics] window destroyed");
    }
}

#[tauri::command]
pub async fn desktop_lyrics_sync<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, Arc<DesktopLyricsState>>,
    payload: Value,
) -> Result<(), String> {
    if !payload.is_object() {
        return Ok(());
    }

    let enabled = payload
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let locked = payload
        .get("locked")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    let active = payload
        .get("active")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);

    *state.last_payload.lock().unwrap() = Some(payload.clone());
    crate::tray::update_tray_desktop_lyrics_state(&app, enabled, locked);

    if let Some(text) = payload.get("text").and_then(|v| v.as_str()) {
        if !text.is_empty() {
            crate::media_keys::update_media_metadata(&app, text);
        }
    }

    if !enabled {
        destroy_desktop_lyrics_window(&app, &state);
        return Ok(());
    }

    let win = ensure_desktop_lyrics_window(&app, &state).await?;
    apply_locked_state(&win, &state, locked);
    apply_content_height(&app, &win, &state);

    let _ = win.emit("desktop-lyrics:content", &payload);

    if active || !locked {
        if let Some(b) = read_window_bounds(&win) {
            if !is_bounds_visible(&app, &b) {
                let def = get_default_bounds(&app, *state.content_height.lock().unwrap());
                let _ = win.set_position(LogicalPosition::new(def.x, def.y));
                let _ = win.set_size(LogicalSize::new(def.width, def.height));
            }
        }
        if !win.is_visible().unwrap_or(false) {
            let _ = win.show();
        }
    } else if win.is_visible().unwrap_or(false) {
        let _ = win.hide();
    }

    Ok(())
}

#[tauri::command]
pub fn desktop_lyrics_get_content<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, Arc<DesktopLyricsState>>,
) -> Option<Value> {
    let payload = state.last_payload.lock().unwrap().clone();
    if let (Some(p), Some(win)) = (&payload, app.get_webview_window("desktop-lyrics")) {
        let locked = p.get("locked").and_then(|v| v.as_bool()).unwrap_or(true);
        let active = p.get("active").and_then(|v| v.as_bool()).unwrap_or(false);
        apply_locked_state(&win, &state, locked);
        apply_content_height(&app, &win, &state);
        if (active || !locked) && !win.is_visible().unwrap_or(false) {
            let _ = win.show();
        }
    }
    payload
}

#[tauri::command]
pub fn desktop_lyrics_command<R: Runtime>(
    app: AppHandle<R>,
    command: String,
) -> Result<(), String> {
    if !ALLOWED_COMMANDS.contains(&command.as_str()) {
        return Ok(());
    }
    let Some(main_win) = app.get_webview_window("main") else {
        return Ok(());
    };
    if command == "openSettings" {
        let _ = main_win.unminimize();
        let _ = main_win.show();
        let _ = main_win.set_focus();
        let _ = main_win.emit("router:navigate", "/settings?tab=desktop-lyrics");
        return Ok(());
    }
    let _ = main_win.emit("desktop-lyrics:command", &command);
    Ok(())
}

#[tauri::command]
pub fn desktop_lyrics_start_drag<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, Arc<DesktopLyricsState>>,
) {
    if state.locked.load(Ordering::Relaxed) {
        return;
    }
    let Some(win) = app.get_webview_window("desktop-lyrics") else {
        return;
    };
    let (Ok(start_cursor), Ok(start_pos)) = (win.cursor_position(), win.outer_position()) else {
        let _ = win.start_dragging();
        return;
    };
    state.dragging.store(true, Ordering::Relaxed);
    let drag_state = state.inner().clone();
    std::thread::spawn(move || {
        while drag_state.dragging.load(Ordering::Relaxed) {
            let Ok(cursor) = win.cursor_position() else {
                break;
            };
            let dx = (cursor.x - start_cursor.x).round() as i32;
            let dy = (cursor.y - start_cursor.y).round() as i32;
            let _ = win.set_position(PhysicalPosition::new(start_pos.x + dx, start_pos.y + dy));
            std::thread::sleep(Duration::from_millis(DRAG_INTERVAL_MS));
        }
    });
}

#[tauri::command]
pub fn desktop_lyrics_end_drag<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, Arc<DesktopLyricsState>>,
) {
    if !state.dragging.swap(false, Ordering::Relaxed) {
        return;
    }
    if let Some(win) = app.get_webview_window("desktop-lyrics") {
        if let Some(b) = read_window_bounds(&win) {
            if !is_bounds_visible(&app, &b) {
                let def = get_default_bounds(&app, *state.content_height.lock().unwrap());
                let _ = win.set_position(LogicalPosition::new(def.x, def.y));
                let _ = win.set_size(LogicalSize::new(def.width, def.height));
            }
        }
        persist_window_bounds(&app, &win);
    }
}

#[tauri::command]
pub fn desktop_lyrics_set_ignore_mouse<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, Arc<DesktopLyricsState>>,
    ignore: bool,
) {
    state.hover_unlock_button.store(!ignore, Ordering::Relaxed);
    if let Some(win) = app.get_webview_window("desktop-lyrics") {
        apply_locked_state(&win, &state, state.locked.load(Ordering::Relaxed));
    }
}

#[tauri::command]
pub fn desktop_lyrics_set_content_height<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, Arc<DesktopLyricsState>>,
    height: f64,
) {
    if !height.is_finite() || height <= 0.0 {
        return;
    }
    *state.content_height.lock().unwrap() = Some(height);
    if let Some(win) = app.get_webview_window("desktop-lyrics") {
        apply_content_height(&app, &win, &state);
    }
}

#[tauri::command]
pub fn desktop_lyrics_reset_position<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, Arc<DesktopLyricsState>>,
) {
    if let Some(win) = app.get_webview_window("desktop-lyrics") {
        let def = get_default_bounds(&app, *state.content_height.lock().unwrap());
        let _ = win.set_position(LogicalPosition::new(def.x, def.y));
        let _ = win.set_size(LogicalSize::new(def.width, def.height));
        persist_window_bounds(&app, &win);
        log::info!("[desktop-lyrics] reset position to ({}, {})", def.x, def.y);
    }
}

pub fn destroy_mini_player<R: Runtime>(app: &AppHandle<R>) -> bool {
    if let Some(mini) = app.get_webview_window("mini-player") {
        let _ = mini.destroy();
        log::info!("[mini-player] window destroyed");
        true
    } else {
        false
    }
}

// Must stay async: building a window synchronously on the main thread deadlocks on WebView2.
#[tauri::command]
pub async fn toggle_mini_player<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    if destroy_mini_player(&app) {
        if let Some(main) = app.get_webview_window("main") {
            let _ = main.unminimize();
            let _ = main.show();
            let _ = main.set_focus();
        }
        return Ok(());
    }

    if let Some(main) = app.get_webview_window("main") {
        let _ = main.hide();
    }

    WebviewWindowBuilder::new(
        &app,
        "mini-player",
        WebviewUrl::App("index.html#mini-player".into()),
    )
    .title("Biu Mini Player")
    .inner_size(320.0, 100.0)
    .resizable(false)
    .maximizable(false)
    .minimizable(false)
    .center()
    .decorations(false)
    .transparent(true)
    .always_on_top(true)
    .skip_taskbar(true)
    .visible(true)
    .build()
    .map_err(|e| e.to_string())?;

    log::info!("[mini-player] window created (320x100)");
    Ok(())
}
