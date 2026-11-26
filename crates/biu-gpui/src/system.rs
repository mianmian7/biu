//! Process-level integration: main-window lifecycle (close-to-tray), the
//! native system tray, and global hotkeys. Everything here degrades gracefully
//! (logs and continues) when the platform refuses tray/hotkey registration.

use crate::desktop_lyrics::{open_or_toggle_desktop_lyrics, set_desktop_lyrics_locked};
use crate::mini_player::open_or_toggle_mini_player;
use crate::shell::MainShell;
use crate::state::AppState;
use gpui_kit::*;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

static QUITTING: AtomicBool = AtomicBool::new(false);

pub fn is_quitting() -> bool {
    QUITTING.load(Ordering::SeqCst)
}

/// Quit the whole app (bypasses close-to-tray).
pub fn request_quit(cx: &mut App) {
    QUITTING.store(true, Ordering::SeqCst);
    cx.quit();
}

/// Global handle to the single `AppState` (for dock re-open / tray callbacks).
pub struct GlobalAppState(pub Entity<AppState>);
impl Global for GlobalAppState {}

pub fn global_state(cx: &App) -> Option<Entity<AppState>> {
    cx.try_global::<GlobalAppState>().map(|g| g.0.clone())
}

pub const MAIN_WINDOW_TITLE: &str = "Biu";

/// Focus the main window, or rebuild it (with all playback/route state intact)
/// after it was closed to the tray.
pub fn open_or_focus_main_window(state: Entity<AppState>, cx: &mut App) {
    if let Some(handle) = state.read(cx).windows.main {
        let focused = handle
            .update(cx, |_, window, _| window.activate_window())
            .is_ok();
        if focused {
            cx.activate(true);
            return;
        }
        state.update(cx, |s, _| s.windows.main = None);
    }

    let bounds = Bounds::centered(None, size(px(1180.0), px(780.0)), cx);
    let mac = cfg!(target_os = "macos");
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        window_min_size: Some(size(px(900.0), px(600.0))),
        titlebar: Some(TitlebarOptions {
            title: Some(MAIN_WINDOW_TITLE.into()),
            // Only macOS keeps native traffic lights over a transparent titlebar;
            // elsewhere keep the native frame so min/max/close always work.
            appears_transparent: mac,
            traffic_light_position: mac.then(|| point(px(16.0), px(19.0))),
        }),
        app_owns_titlebar_drag: mac,
        app_id: Some("com.biu.wood3n".into()),
        focus: true,
        show: true,
        ..Default::default()
    };

    let st = state.clone();
    match gpui_kit::open_window(opts, cx, move |window, cx| {
        cx.new(|cx| MainShell::new(st, window, cx))
    }) {
        Ok((handle, _)) => {
            state.update(cx, |s, cx| {
                s.windows.main = Some(handle);
                cx.notify();
            });
            cx.activate(true);
        }
        Err(err) => tracing::error!("failed to open main window: {err:#}"),
    }
}

/// Keep `WindowRegistry` honest when any window is destroyed by the OS.
pub fn observe_window_closures(state: Entity<AppState>, cx: &mut App) {
    cx.on_window_closed(move |cx, id| {
        state.update(cx, |s, cx| {
            let mut changed = false;
            if s.windows.main.is_some_and(|h| h.window_id() == id) {
                s.windows.main = None;
                s.covers.clear();
                changed = true;
            }
            if s.windows
                .desktop_lyrics
                .is_some_and(|h| h.window_id() == id)
            {
                s.windows.desktop_lyrics = None;
                changed = true;
            }
            if s.windows.mini_player.is_some_and(|h| h.window_id() == id) {
                s.windows.mini_player = None;
                changed = true;
            }
            if changed {
                cx.notify();
            }
        });
    })
    .detach();
}

// ---------------------------------------------------------------------------
// Commands shared by tray menu + global hotkeys
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SystemCommand {
    ShowMain,
    TogglePlay,
    Prev,
    Next,
    VolumeUp,
    VolumeDown,
    ToggleDesktopLyrics,
    ToggleDesktopLyricsLock,
    ToggleMiniPlayer,
    Quit,
}

impl SystemCommand {
    pub const MENU: [SystemCommand; 8] = [
        SystemCommand::ShowMain,
        SystemCommand::TogglePlay,
        SystemCommand::Prev,
        SystemCommand::Next,
        SystemCommand::ToggleDesktopLyrics,
        SystemCommand::ToggleDesktopLyricsLock,
        SystemCommand::ToggleMiniPlayer,
        SystemCommand::Quit,
    ];

    pub fn menu_id(self) -> &'static str {
        match self {
            Self::ShowMain => "biu.show",
            Self::TogglePlay => "biu.toggle-play",
            Self::Prev => "biu.prev",
            Self::Next => "biu.next",
            Self::VolumeUp => "biu.volume-up",
            Self::VolumeDown => "biu.volume-down",
            Self::ToggleDesktopLyrics => "biu.desktop-lyrics",
            Self::ToggleDesktopLyricsLock => "biu.desktop-lyrics-lock",
            Self::ToggleMiniPlayer => "biu.mini-player",
            Self::Quit => "biu.quit",
        }
    }

    pub fn from_menu_id(id: &str) -> Option<Self> {
        [
            Self::ShowMain,
            Self::TogglePlay,
            Self::Prev,
            Self::Next,
            Self::VolumeUp,
            Self::VolumeDown,
            Self::ToggleDesktopLyrics,
            Self::ToggleDesktopLyricsLock,
            Self::ToggleMiniPlayer,
            Self::Quit,
        ]
        .into_iter()
        .find(|c| c.menu_id() == id)
    }
}

/// Dynamic tray labels reflecting current state.
pub fn menu_label(
    cmd: SystemCommand,
    playing: bool,
    lyrics_open: bool,
    lyrics_locked: bool,
    mini_open: bool,
) -> &'static str {
    match cmd {
        SystemCommand::ShowMain => "显示主窗口",
        SystemCommand::TogglePlay => {
            if playing {
                "暂停"
            } else {
                "播放"
            }
        }
        SystemCommand::Prev => "上一首",
        SystemCommand::Next => "下一首",
        SystemCommand::VolumeUp => "音量 +",
        SystemCommand::VolumeDown => "音量 -",
        SystemCommand::ToggleDesktopLyrics => {
            if lyrics_open {
                "关闭桌面歌词"
            } else {
                "打开桌面歌词"
            }
        }
        SystemCommand::ToggleDesktopLyricsLock => {
            if lyrics_locked {
                "解锁桌面歌词"
            } else {
                "锁定桌面歌词"
            }
        }
        SystemCommand::ToggleMiniPlayer => {
            if mini_open {
                "关闭 Mini 播放器"
            } else {
                "Mini 播放器"
            }
        }
        SystemCommand::Quit => "退出 Biu",
    }
}

pub fn dispatch(cmd: SystemCommand, state: &Entity<AppState>, cx: &mut App) {
    match cmd {
        SystemCommand::ShowMain => open_or_focus_main_window(state.clone(), cx),
        SystemCommand::TogglePlay => state.update(cx, |s, cx| s.toggle_play(cx)),
        SystemCommand::Prev => state.update(cx, |s, cx| s.prev_track(cx)),
        SystemCommand::Next => state.update(cx, |s, cx| s.next_track(false, cx)),
        SystemCommand::VolumeUp => state.update(cx, |s, cx| s.adjust_volume(0.05, cx)),
        SystemCommand::VolumeDown => state.update(cx, |s, cx| s.adjust_volume(-0.05, cx)),
        SystemCommand::ToggleDesktopLyrics => open_or_toggle_desktop_lyrics(state.clone(), cx),
        SystemCommand::ToggleDesktopLyricsLock => {
            let locked = !state.read(cx).settings.desktop_lyrics_locked;
            set_desktop_lyrics_locked(state, locked, cx);
        }
        SystemCommand::ToggleMiniPlayer => open_or_toggle_mini_player(state.clone(), cx),
        SystemCommand::Quit => request_quit(cx),
    }
}

// ---------------------------------------------------------------------------
// Tray icon bitmap (generated, no asset files)
// ---------------------------------------------------------------------------

/// Generate a crisp `size × size` RGBA tray icon: an emerald disc with a white
/// play triangle, anti-aliased by 4× supersampling.
pub fn tray_icon_rgba(size: u32) -> Vec<u8> {
    let n = size.max(8) as usize;
    let mut out = vec![0u8; n * n * 4];
    let (er, eg, eb) = (0x1e_u8, 0xd7_u8, 0x60_u8);
    let c = n as f32 / 2.0;
    let radius = c - 0.5;
    // Play triangle (slightly right of center for optical balance).
    let tri = [
        (c - n as f32 * 0.16, c - n as f32 * 0.22),
        (c - n as f32 * 0.16, c + n as f32 * 0.22),
        (c + n as f32 * 0.24, c),
    ];
    let ss = 4;
    for y in 0..n {
        for x in 0..n {
            let mut disc = 0u32;
            let mut glyph = 0u32;
            for sy in 0..ss {
                for sx in 0..ss {
                    let px_ = x as f32 + (sx as f32 + 0.5) / ss as f32;
                    let py = y as f32 + (sy as f32 + 0.5) / ss as f32;
                    let d = ((px_ - c).powi(2) + (py - c).powi(2)).sqrt();
                    if d <= radius {
                        disc += 1;
                        if point_in_triangle((px_, py), tri) {
                            glyph += 1;
                        }
                    }
                }
            }
            let total = (ss * ss) as f32;
            let a = disc as f32 / total;
            if a <= 0.0 {
                continue;
            }
            let w = if disc > 0 {
                glyph as f32 / disc as f32
            } else {
                0.0
            };
            let mix = |e: u8| -> u8 { (e as f32 * (1.0 - w) + 255.0 * w).round() as u8 };
            let i = (y * n + x) * 4;
            out[i] = mix(er);
            out[i + 1] = mix(eg);
            out[i + 2] = mix(eb);
            out[i + 3] = (a * 255.0).round() as u8;
        }
    }
    out
}

fn point_in_triangle(p: (f32, f32), t: [(f32, f32); 3]) -> bool {
    let sign = |a: (f32, f32), b: (f32, f32), c: (f32, f32)| {
        (a.0 - c.0) * (b.1 - c.1) - (b.0 - c.0) * (a.1 - c.1)
    };
    let d1 = sign(p, t[0], t[1]);
    let d2 = sign(p, t[1], t[2]);
    let d3 = sign(p, t[2], t[0]);
    let neg = d1 < 0.0 || d2 < 0.0 || d3 < 0.0;
    let pos = d1 > 0.0 || d2 > 0.0 || d3 > 0.0;
    !(neg && pos)
}

// ---------------------------------------------------------------------------
// Tray + global hotkeys
// ---------------------------------------------------------------------------

struct TrayHandle {
    _tray: tray_icon::TrayIcon,
    items: Vec<(SystemCommand, tray_icon::menu::MenuItem)>,
    labels: Vec<&'static str>,
}

impl TrayHandle {
    fn refresh_labels(&mut self, playing: bool, lyrics_open: bool, locked: bool, mini_open: bool) {
        for (i, (cmd, item)) in self.items.iter().enumerate() {
            let label = menu_label(*cmd, playing, lyrics_open, locked, mini_open);
            if self.labels.get(i).copied() != Some(label) {
                item.set_text(label);
                if let Some(slot) = self.labels.get_mut(i) {
                    *slot = label;
                }
            }
        }
    }
}

fn build_tray(state: &AppState) -> anyhow::Result<TrayHandle> {
    use tray_icon::menu::{Menu, MenuItem, PredefinedMenuItem};
    use tray_icon::{Icon, TrayIconBuilder};

    let playing = state.snapshot.state.is_playing();
    let lyrics_open = state.windows.desktop_lyrics.is_some();
    let locked = state.settings.desktop_lyrics_locked;
    let mini_open = state.windows.mini_player.is_some();

    let menu = Menu::new();
    let mut items = Vec::new();
    let mut labels = Vec::new();
    for cmd in SystemCommand::MENU {
        if matches!(
            cmd,
            SystemCommand::TogglePlay | SystemCommand::ToggleDesktopLyrics | SystemCommand::Quit
        ) {
            menu.append(&PredefinedMenuItem::separator())?;
        }
        let label = menu_label(cmd, playing, lyrics_open, locked, mini_open);
        let item = MenuItem::with_id(cmd.menu_id(), label, true, None);
        menu.append(&item)?;
        items.push((cmd, item));
        labels.push(label);
    }

    let icon = Icon::from_rgba(tray_icon_rgba(32), 32, 32)?;
    let tray = TrayIconBuilder::new()
        .with_id("biu-tray")
        .with_tooltip("Biu")
        .with_icon(icon)
        .with_menu(Box::new(menu))
        .with_menu_on_left_click(false)
        .build()?;
    Ok(TrayHandle {
        _tray: tray,
        items,
        labels,
    })
}

#[derive(Debug, Clone, Copy)]
struct HotkeyBinding {
    id: u32,
    cmd: SystemCommand,
}

fn register_hotkeys() -> anyhow::Result<(global_hotkey::GlobalHotKeyManager, Vec<HotkeyBinding>)> {
    use global_hotkey::hotkey::{Code, HotKey, Modifiers};
    let manager = global_hotkey::GlobalHotKeyManager::new()?;
    let primary = if cfg!(target_os = "macos") {
        Modifiers::META
    } else {
        Modifiers::CONTROL
    };
    let mods = primary | Modifiers::ALT;
    let table = [
        (Code::KeyP, SystemCommand::TogglePlay),
        (Code::ArrowLeft, SystemCommand::Prev),
        (Code::ArrowRight, SystemCommand::Next),
        (Code::ArrowUp, SystemCommand::VolumeUp),
        (Code::ArrowDown, SystemCommand::VolumeDown),
        (Code::KeyM, SystemCommand::ToggleMiniPlayer),
        (Code::KeyL, SystemCommand::ToggleDesktopLyrics),
    ];
    let mut bindings = Vec::new();
    for (code, cmd) in table {
        let hk = HotKey::new(Some(mods), code);
        match manager.register(hk) {
            Ok(()) => bindings.push(HotkeyBinding { id: hk.id(), cmd }),
            Err(err) => tracing::warn!("global hotkey {code:?} unavailable: {err}"),
        }
    }
    Ok((manager, bindings))
}

/// Create the tray icon + global hotkeys and start a 100 ms poller that keeps
/// working while the main window is closed to the tray.
pub fn init_tray_and_hotkeys(state: Entity<AppState>, cx: &mut App) {
    let tray = if cfg!(target_os = "linux") {
        // tray-icon on Linux needs a running GTK main loop which GPUI doesn't provide.
        tracing::info!("system tray skipped on Linux (no GTK main loop)");
        None
    } else {
        let snapshot = state.read(cx);
        match std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| build_tray(snapshot))) {
            Ok(Ok(t)) => Some(t),
            Ok(Err(err)) => {
                tracing::warn!("system tray unavailable: {err:#}");
                None
            }
            Err(_) => {
                tracing::warn!("system tray init panicked; continuing without tray");
                None
            }
        }
    };

    let hotkeys = match std::panic::catch_unwind(register_hotkeys) {
        Ok(Ok(h)) => Some(h),
        Ok(Err(err)) => {
            tracing::warn!("global hotkeys unavailable: {err:#}");
            None
        }
        Err(_) => {
            tracing::warn!("global hotkey init panicked; continuing without hotkeys");
            None
        }
    };

    let weak = state.downgrade();
    cx.spawn(async move |cx: &mut AsyncApp| {
        let mut tray = tray;
        // Keep the manager alive for the lifetime of the poller.
        let (_manager, bindings) = match hotkeys {
            Some((m, b)) => (Some(m), b),
            None => (None, Vec::new()),
        };
        loop {
            cx.background_executor()
                .timer(Duration::from_millis(100))
                .await;
            let mut cmds: Vec<SystemCommand> = Vec::new();

            while let Ok(ev) = tray_icon::menu::MenuEvent::receiver().try_recv() {
                if let Some(cmd) = SystemCommand::from_menu_id(ev.id.as_ref()) {
                    cmds.push(cmd);
                }
            }
            while let Ok(ev) = tray_icon::TrayIconEvent::receiver().try_recv() {
                match ev {
                    tray_icon::TrayIconEvent::Click {
                        button: tray_icon::MouseButton::Left,
                        button_state: tray_icon::MouseButtonState::Up,
                        ..
                    }
                    | tray_icon::TrayIconEvent::DoubleClick {
                        button: tray_icon::MouseButton::Left,
                        ..
                    } => cmds.push(SystemCommand::ShowMain),
                    _ => {}
                }
            }
            while let Ok(ev) = global_hotkey::GlobalHotKeyEvent::receiver().try_recv() {
                if ev.state == global_hotkey::HotKeyState::Pressed
                    && let Some(b) = bindings.iter().find(|b| b.id == ev.id)
                {
                    cmds.push(b.cmd);
                }
            }

            let alive = cx.update(|cx| {
                let Some(state) = weak.upgrade() else {
                    return false;
                };
                for cmd in cmds.drain(..) {
                    dispatch(cmd, &state, cx);
                }
                if let Some(t) = tray.as_mut() {
                    let s = state.read(cx);
                    t.refresh_labels(
                        s.snapshot.state.is_playing(),
                        s.windows.desktop_lyrics.is_some(),
                        s.settings.desktop_lyrics_locked,
                        s.windows.mini_player.is_some(),
                    );
                }
                true
            });
            if !alive || is_quitting() {
                break;
            }
        }
    })
    .detach();
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn tray_icon_has_expected_shape() {
        let px = tray_icon_rgba(32);
        assert_eq!(px.len(), 32 * 32 * 4);
        let at = |x: usize, y: usize| {
            let i = (y * 32 + x) * 4;
            (px[i], px[i + 1], px[i + 2], px[i + 3])
        };
        // Corners are fully transparent (outside the disc).
        assert_eq!(at(0, 0).3, 0);
        assert_eq!(at(31, 31).3, 0);
        // Center is inside the white play glyph.
        let center = at(16, 16);
        assert_eq!(center.3, 255);
        assert!(center.0 > 240 && center.1 > 240 && center.2 > 240);
        // Left edge inside the disc but outside the glyph is emerald.
        let emerald = at(4, 16);
        assert_eq!(emerald.3, 255);
        assert_eq!((emerald.0, emerald.1, emerald.2), (0x1e, 0xd7, 0x60));
    }

    #[test]
    fn menu_ids_round_trip() {
        for cmd in SystemCommand::MENU {
            assert_eq!(SystemCommand::from_menu_id(cmd.menu_id()), Some(cmd));
        }
        assert_eq!(SystemCommand::from_menu_id("nope"), None);
    }

    #[test]
    fn lock_label_always_offers_escape_hatch() {
        assert_eq!(
            menu_label(
                SystemCommand::ToggleDesktopLyricsLock,
                false,
                true,
                true,
                false
            ),
            "解锁桌面歌词"
        );
        assert_eq!(
            menu_label(
                SystemCommand::ToggleDesktopLyricsLock,
                false,
                true,
                false,
                false
            ),
            "锁定桌面歌词"
        );
        assert_eq!(
            menu_label(SystemCommand::TogglePlay, true, false, false, false),
            "暂停"
        );
    }
}
