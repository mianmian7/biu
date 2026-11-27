//! Transparent, borderless, always-on-top desktop lyrics window with karaoke
//! highlight, a hover toolbar and OS-level click-through when locked.

use crate::player_bar::{icon, is_playing};
use crate::state::AppState;
use crate::theme::parse_hex_hsla;
use biu_core::LyricLine;
use biu_core::lyrics::{active_line_index, word_progress};
use gpui_kit::assets::IconName;
use gpui_kit::prelude::*;
use gpui_kit::*;

pub const DESKTOP_LYRICS_TITLE: &str = "Biu 桌面歌词";
pub const DESKTOP_LYRICS_WIDTH: f32 = 720.0;
pub const DESKTOP_LYRICS_MIN_HEIGHT: f32 = 118.0;
pub const FONT_MIN: u32 = 18;
pub const FONT_MAX: u32 = 64;

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Fraction (0..=1) of `line` already sung at `pos`.
pub fn line_progress(line: &LyricLine, next_start: Option<f64>, pos: f64) -> f64 {
    if !pos.is_finite() || pos < line.start_secs {
        return 0.0;
    }
    if !line.words.is_empty() {
        let total: usize = line.words.iter().map(|w| w.text.chars().count()).sum();
        if total > 0 {
            let played: f64 = line
                .words
                .iter()
                .map(|w| w.text.chars().count() as f64 * word_progress(w, pos))
                .sum();
            return (played / total as f64).clamp(0.0, 1.0);
        }
    }
    let end = line
        .duration_secs
        .filter(|d| *d > 0.0)
        .map(|d| line.start_secs + d)
        .or(next_start.filter(|n| *n > line.start_secs))
        .unwrap_or(line.start_secs + 5.0);
    let span = (end - line.start_secs).max(0.001);
    ((pos - line.start_secs) / span).clamp(0.0, 1.0)
}

/// Byte offset in `text` covering `fraction` of its characters (char-boundary safe).
pub fn byte_index_at_fraction(text: &str, fraction: f64) -> usize {
    let n = text.chars().count();
    if n == 0 {
        return 0;
    }
    let k = ((fraction.clamp(0.0, 1.0)) * n as f64).round() as usize;
    if k >= n {
        return text.len();
    }
    text.char_indices()
        .nth(k)
        .map(|(i, _)| i)
        .unwrap_or(text.len())
}

pub fn window_height_for(font: u32, double: bool, translation: bool) -> f32 {
    let f = font as f32;
    let mut h = 20.0 + f * 1.3;
    if translation {
        h += f * 0.5 * 1.35;
    }
    if double {
        h += f * 0.72 * 1.3;
    }
    h.max(DESKTOP_LYRICS_MIN_HEIGHT).ceil()
}

#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
const WS_EX_TRANSPARENT_BITS: isize = 0x0000_0020;
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
const WS_EX_LAYERED_BITS: isize = 0x0008_0000;

/// Windows extended style for click-through: locked windows get
/// `WS_EX_TRANSPARENT | WS_EX_LAYERED`; unlocking only drops `WS_EX_TRANSPARENT`.
#[cfg_attr(not(any(target_os = "windows", test)), allow(dead_code))]
pub fn click_through_ex_style(ex: isize, locked: bool) -> isize {
    if locked {
        ex | WS_EX_TRANSPARENT_BITS | WS_EX_LAYERED_BITS
    } else {
        ex & !WS_EX_TRANSPARENT_BITS
    }
}

// ---------------------------------------------------------------------------
// Native click-through
// ---------------------------------------------------------------------------

#[cfg(target_os = "macos")]
fn apply_native_click_through(locked: bool) -> bool {
    use objc2::MainThreadMarker;
    use objc2_app_kit::NSApplication;
    let Some(mtm) = MainThreadMarker::new() else {
        return false;
    };
    let app = NSApplication::sharedApplication(mtm);
    let windows = app.windows();
    let mut applied = false;
    for i in 0..windows.count() {
        let w = windows.objectAtIndex(i);
        if w.title().to_string() == DESKTOP_LYRICS_TITLE {
            w.setIgnoresMouseEvents(locked);
            applied = true;
        }
    }
    applied
}

#[cfg(target_os = "windows")]
fn apply_native_click_through(locked: bool) -> bool {
    use windows::Win32::Foundation::COLORREF;
    use windows::Win32::UI::WindowsAndMessaging::{
        FindWindowW, GWL_EXSTYLE, GetWindowLongPtrW, LWA_ALPHA, SetLayeredWindowAttributes,
        SetWindowLongPtrW,
    };
    use windows::core::PCWSTR;
    let wide: Vec<u16> = DESKTOP_LYRICS_TITLE.encode_utf16().chain(Some(0)).collect();
    unsafe {
        let Ok(hwnd) = FindWindowW(PCWSTR::null(), PCWSTR(wide.as_ptr())) else {
            return false;
        };
        if hwnd.is_invalid() {
            return false;
        }
        let ex = GetWindowLongPtrW(hwnd, GWL_EXSTYLE);
        let next = click_through_ex_style(ex, locked);
        SetWindowLongPtrW(hwnd, GWL_EXSTYLE, next);
        if ex & WS_EX_LAYERED_BITS == 0 && next & WS_EX_LAYERED_BITS != 0 {
            // A freshly layered window is invisible until its attributes are set.
            let _ = SetLayeredWindowAttributes(hwnd, COLORREF(0), 255, LWA_ALPHA);
        }
    }
    true
}

#[cfg(not(any(target_os = "macos", target_os = "windows")))]
fn apply_native_click_through(_locked: bool) -> bool {
    false
}

/// Persist the lock flag and apply OS click-through to the open window.
pub fn set_desktop_lyrics_locked(state: &Entity<AppState>, locked: bool, cx: &mut App) {
    state.update(cx, |s, cx| {
        s.update_settings(|st| st.desktop_lyrics_locked = locked, cx);
        let msg = if locked {
            "桌面歌词已锁定（可在托盘菜单解锁）"
        } else {
            "桌面歌词已解锁"
        };
        s.show_toast(msg, false, cx);
    });
    if state.read(cx).windows.desktop_lyrics.is_some() && !apply_native_click_through(locked) {
        tracing::debug!("desktop lyrics click-through not applied on this platform");
    }
}

// ---------------------------------------------------------------------------
// Window lifecycle
// ---------------------------------------------------------------------------

pub fn open_or_toggle_desktop_lyrics(state: Entity<AppState>, cx: &mut App) {
    if let Some(handle) = state.read(cx).windows.desktop_lyrics {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        state.update(cx, |s, cx| {
            s.windows.desktop_lyrics = None;
            s.update_settings(|st| st.show_desktop_lyrics = false, cx);
        });
        return;
    }

    let (font, double, translation, locked) = {
        let s = &state.read(cx).settings;
        (
            s.desktop_lyrics_font_size.clamp(FONT_MIN, FONT_MAX),
            s.desktop_lyrics_style.layout == "double",
            s.desktop_lyrics_style.show_translation,
            s.desktop_lyrics_locked,
        )
    };
    let w = DESKTOP_LYRICS_WIDTH;
    let h = window_height_for(font, double, translation);
    let display = cx.primary_display();
    let bounds = match display.as_ref() {
        Some(d) => {
            let db = d.bounds();
            let x = db.origin.x + (db.size.width - px(w)) / 2.0;
            let y = db.origin.y + db.size.height - px(h) - px(96.0);
            Bounds::new(point(x, y), size(px(w), px(h)))
        }
        None => Bounds::centered(None, size(px(w), px(h)), cx),
    };

    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        kind: WindowKind::PopUp,
        focus: false,
        show: true,
        is_movable: true,
        is_resizable: false,
        is_minimizable: false,
        window_background: WindowBackgroundAppearance::Transparent,
        display_id: display.as_ref().map(|d| d.id()),
        app_id: Some("com.biu.wood3n".into()),
        ..Default::default()
    };

    let st = state.clone();
    match gpui_kit::open_window(opts, cx, move |window, cx| {
        window.set_window_title(DESKTOP_LYRICS_TITLE);
        cx.new(|cx| DesktopLyricsWindow::new(st, window, cx))
    }) {
        Ok((handle, _)) => {
            state.update(cx, |s, cx| {
                s.windows.desktop_lyrics = Some(handle);
                if !s.settings.show_desktop_lyrics {
                    s.update_settings(|st| st.show_desktop_lyrics = true, cx);
                }
                cx.notify();
            });
            if locked {
                cx.defer(|_| {
                    apply_native_click_through(true);
                });
            }
        }
        Err(err) => tracing::error!("failed to open desktop lyrics window: {err:#}"),
    }
}

// ---------------------------------------------------------------------------
// View
// ---------------------------------------------------------------------------

pub struct DesktopLyricsWindow {
    state: Entity<AppState>,
    hovered: bool,
    _observe: Subscription,
}

impl DesktopLyricsWindow {
    pub fn new(state: Entity<AppState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            hovered: false,
            _observe: observe,
        }
    }

    fn change_font(&mut self, delta: i32, window: &mut Window, cx: &mut Context<Self>) {
        let (font, double, translation) = {
            let s = &self.state.read(cx).settings;
            let f = (s.desktop_lyrics_font_size as i32 + delta)
                .clamp(FONT_MIN as i32, FONT_MAX as i32) as u32;
            (
                f,
                s.desktop_lyrics_style.layout == "double",
                s.desktop_lyrics_style.show_translation,
            )
        };
        self.state.update(cx, |s, cx| {
            s.update_settings(|st| st.desktop_lyrics_font_size = font, cx)
        });
        window.resize(size(
            px(DESKTOP_LYRICS_WIDTH),
            px(window_height_for(font, double, translation)),
        ));
    }
}

fn toolbar_button(id: &'static str, name: IconName) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(26.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded(px(6.0))
        .cursor_pointer()
        .hover(|s| s.bg(hsla(0.0, 0.0, 1.0, 0.18)))
        .child(icon(name, 14.0, hsla(0.0, 0.0, 1.0, 0.92)))
}

impl Render for DesktopLyricsWindow {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.state.read(cx);
        let style = &s.settings.desktop_lyrics_style;
        let font = s
            .settings
            .desktop_lyrics_font_size
            .clamp(FONT_MIN, FONT_MAX) as f32;
        let locked = s.settings.desktop_lyrics_locked;
        let played_color =
            parse_hex_hsla(&style.played_color).unwrap_or_else(|| rgb(0x1ed760).into());
        let unplayed_color = parse_hex_hsla(&style.unplayed_color).unwrap_or_else(gpui_kit::white);
        let stroke_color = parse_hex_hsla(&style.stroke_color)
            .unwrap_or_else(gpui_kit::black)
            .opacity(0.55);
        let karaoke = style.karaoke;
        let double = style.layout == "double";
        let show_translation = style.show_translation;
        let show_romaji = style.show_romaji;
        let align = style.align.clone();
        let playing = is_playing(s.snapshot.state);
        let pos = (s.snapshot.position_secs + s.lyrics_offset_secs).max(0.0);
        let lines = &s.lyrics.lines;
        let idx = active_line_index(lines, pos);

        let (main_text, main_progress, tr_text, ro_text, next_text) =
            match idx.and_then(|i| lines.get(i)) {
                Some(line) => {
                    let next_start = idx.and_then(|i| lines.get(i + 1)).map(|l| l.start_secs);
                    let tr = show_translation
                        .then(|| line.translation.clone())
                        .flatten()
                        .filter(|t| !t.trim().is_empty());
                    let ro = show_romaji
                        .then(|| line.romaji.clone())
                        .flatten()
                        .filter(|t| !t.trim().is_empty());
                    let next = idx
                        .and_then(|i| lines.get(i + 1))
                        .map(|l| l.text.clone())
                        .filter(|t| !t.trim().is_empty());
                    (
                        line.text.clone(),
                        line_progress(line, next_start, pos),
                        tr,
                        ro,
                        next,
                    )
                }
                None => {
                    let fallback = s
                        .current_track()
                        .map(|t| {
                            if t.artist.is_empty() {
                                t.title.clone()
                            } else {
                                format!("{} - {}", t.title, t.artist)
                            }
                        })
                        .unwrap_or_else(|| "Biu 桌面歌词".to_string());
                    let next = lines.first().map(|l| l.text.clone());
                    (fallback, 0.0, None, None, next)
                }
            };

        let main_text = if main_text.trim().is_empty() {
            "♪".to_string()
        } else {
            main_text
        };
        let split = if karaoke {
            byte_index_at_fraction(&main_text, main_progress)
        } else {
            main_text.len()
        };
        let highlighted = StyledText::new(main_text.clone()).with_highlights(vec![(
            0..split,
            HighlightStyle {
                color: Some(played_color),
                ..Default::default()
            },
        )]);

        let aligned = |d: Div| match align.as_str() {
            "left" => d.text_left(),
            "right" => d.text_right(),
            _ => d.text_center(),
        };

        let text_layer =
            |content: AnyElement, color: Hsla, size_px: f32, weight: FontWeight, shadow: String| {
                // Two-layer pseudo-stroke keeps lyrics legible on any wallpaper.
                div()
                    .relative()
                    .w_full()
                    .child(
                        aligned(div().absolute().top(px(1.5)).left(px(1.5)).w_full())
                            .text_size(px(size_px))
                            .font_weight(weight)
                            .text_color(stroke_color)
                            .truncate()
                            .child(shadow),
                    )
                    .child(
                        aligned(div().relative().w_full())
                            .text_size(px(size_px))
                            .font_weight(weight)
                            .text_color(color)
                            .truncate()
                            .child(content),
                    )
            };

        let hover_ui = self.hovered && !locked;

        let content = div()
            .id("dl-content")
            .flex_1()
            .w_full()
            .flex()
            .flex_col()
            .justify_center()
            .gap(px(2.0))
            .px_5()
            .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
            .child(text_layer(
                highlighted.into_any_element(),
                unplayed_color,
                font,
                FontWeight::BOLD,
                main_text.clone(),
            ))
            .when_some(tr_text, |d, sub| {
                d.child(text_layer(
                    sub.clone().into_any_element(),
                    unplayed_color.opacity(0.88),
                    font * 0.5,
                    FontWeight::MEDIUM,
                    sub,
                ))
            })
            .when_some(ro_text, |d, ro| {
                d.child(text_layer(
                    ro.clone().into_any_element(),
                    unplayed_color.opacity(0.72),
                    font * 0.44,
                    FontWeight::NORMAL,
                    ro,
                ))
            })
            .when(double, |d| {
                d.when_some(next_text, |d, next| {
                    d.child(text_layer(
                        next.clone().into_any_element(),
                        unplayed_color.opacity(0.6),
                        font * 0.72,
                        FontWeight::SEMIBOLD,
                        next,
                    ))
                })
            });

        let st = self.state.clone();
        let toolbar = div()
            .absolute()
            .top(px(6.0))
            .left_0()
            .right_0()
            .flex()
            .justify_center()
            .child(
                div()
                    .id("dl-toolbar")
                    .occlude()
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .py_1()
                    .rounded(px(10.0))
                    .bg(hsla(0.0, 0.0, 0.08, 0.78))
                    .border_1()
                    .border_color(hsla(0.0, 0.0, 1.0, 0.12))
                    .child(toolbar_button("dl-prev", IconName::SkipBack).on_click({
                        let st = st.clone();
                        move |_, _, cx| st.update(cx, |s, cx| s.prev_track(cx))
                    }))
                    .child(
                        toolbar_button(
                            "dl-play",
                            if playing {
                                IconName::Pause
                            } else {
                                IconName::Play
                            },
                        )
                        .on_click({
                            let st = st.clone();
                            move |_, _, cx| st.update(cx, |s, cx| s.toggle_play(cx))
                        }),
                    )
                    .child(toolbar_button("dl-next", IconName::SkipForward).on_click({
                        let st = st.clone();
                        move |_, _, cx| st.update(cx, |s, cx| s.next_track(false, cx))
                    }))
                    .child(
                        div()
                            .w(px(1.0))
                            .h(px(14.0))
                            .mx_1()
                            .bg(hsla(0.0, 0.0, 1.0, 0.2)),
                    )
                    .child(
                        toolbar_button("dl-font-dec", IconName::AArrowDown).on_click(
                            cx.listener(|this, _, window, cx| this.change_font(-2, window, cx)),
                        ),
                    )
                    .child(toolbar_button("dl-font-inc", IconName::AArrowUp).on_click(
                        cx.listener(|this, _, window, cx| this.change_font(2, window, cx)),
                    ))
                    .child(
                        div()
                            .id("dl-layout")
                            .h(px(26.0))
                            .px_1p5()
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if double {
                                played_color
                            } else {
                                hsla(0.0, 0.0, 1.0, 0.78)
                            })
                            .cursor_pointer()
                            .hover(|s| s.bg(hsla(0.0, 0.0, 1.0, 0.18)))
                            .child(if double { "双" } else { "单" })
                            .on_click({
                                let st = st.clone();
                                move |_, _, cx| {
                                    st.update(cx, |s, cx| {
                                        s.update_settings(
                                            |st| {
                                                st.desktop_lyrics_style.layout =
                                                    if st.desktop_lyrics_style.layout == "double" {
                                                        "single".into()
                                                    } else {
                                                        "double".into()
                                                    };
                                            },
                                            cx,
                                        )
                                    })
                                }
                            }),
                    )
                    .child(
                        div()
                            .id("dl-tr")
                            .h(px(26.0))
                            .px_1p5()
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if show_translation {
                                played_color
                            } else {
                                hsla(0.0, 0.0, 1.0, 0.55)
                            })
                            .cursor_pointer()
                            .hover(|s| s.bg(hsla(0.0, 0.0, 1.0, 0.18)))
                            .child("译")
                            .on_click({
                                let st = st.clone();
                                move |_, _, cx| {
                                    st.update(cx, |s, cx| {
                                        s.update_settings(
                                            |st| {
                                                st.desktop_lyrics_style.show_translation =
                                                    !st.desktop_lyrics_style.show_translation;
                                            },
                                            cx,
                                        )
                                    })
                                }
                            }),
                    )
                    .child(
                        div()
                            .id("dl-ro")
                            .h(px(26.0))
                            .px_1p5()
                            .rounded(px(6.0))
                            .flex()
                            .items_center()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if show_romaji {
                                played_color
                            } else {
                                hsla(0.0, 0.0, 1.0, 0.55)
                            })
                            .cursor_pointer()
                            .hover(|s| s.bg(hsla(0.0, 0.0, 1.0, 0.18)))
                            .child("音")
                            .on_click({
                                let st = st.clone();
                                move |_, _, cx| {
                                    st.update(cx, |s, cx| {
                                        s.update_settings(
                                            |st| {
                                                st.desktop_lyrics_style.show_romaji =
                                                    !st.desktop_lyrics_style.show_romaji;
                                            },
                                            cx,
                                        )
                                    })
                                }
                            }),
                    )
                    .child(toolbar_button("dl-lock", IconName::Lock).on_click({
                        let st = st.clone();
                        move |_, _, cx| set_desktop_lyrics_locked(&st, true, cx)
                    }))
                    .child(toolbar_button("dl-close", IconName::X).on_click({
                        let st = st.clone();
                        move |_, _, cx| {
                            let st = st.clone();
                            cx.defer(move |cx| open_or_toggle_desktop_lyrics(st, cx));
                        }
                    })),
            );

        div()
            .id("desktop-lyrics")
            .size_full()
            .relative()
            .flex()
            .flex_col()
            .rounded(px(16.0))
            .when(hover_ui, |d| d.bg(hsla(0.0, 0.0, 0.05, 0.5)))
            .on_hover(cx.listener(|this, hovered: &bool, _, cx| {
                if this.hovered != *hovered {
                    this.hovered = *hovered;
                    cx.notify();
                }
            }))
            .child(content)
            .when(hover_ui, |d| d.child(toolbar))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use biu_core::LyricWord;
    use core::prelude::v1::test;

    fn line(start: f64, text: &str) -> LyricLine {
        LyricLine {
            start_secs: start,
            text: text.into(),
            ..Default::default()
        }
    }

    #[test]
    fn line_progress_uses_next_line_start() {
        let l = line(10.0, "hello");
        assert_eq!(line_progress(&l, Some(14.0), 9.0), 0.0);
        assert!((line_progress(&l, Some(14.0), 12.0) - 0.5).abs() < 1e-9);
        assert_eq!(line_progress(&l, Some(14.0), 20.0), 1.0);
        assert!((line_progress(&l, None, 12.5) - 0.5).abs() < 1e-9);
    }

    #[test]
    fn line_progress_prefers_word_timing() {
        let mut l = line(0.0, "ab cd");
        l.words = vec![
            LyricWord {
                start_secs: 0.0,
                duration_secs: 1.0,
                text: "ab".into(),
            },
            LyricWord {
                start_secs: 1.0,
                duration_secs: 1.0,
                text: "cd".into(),
            },
        ];
        assert!((line_progress(&l, Some(10.0), 1.0) - 0.5).abs() < 1e-9);
        assert!((line_progress(&l, Some(10.0), 1.5) - 0.75).abs() < 1e-9);
    }

    #[test]
    fn byte_index_is_char_boundary_safe() {
        let t = "你好世界";
        assert_eq!(byte_index_at_fraction(t, 0.0), 0);
        assert_eq!(byte_index_at_fraction(t, 0.5), "你好".len());
        assert_eq!(byte_index_at_fraction(t, 1.0), t.len());
        assert_eq!(byte_index_at_fraction(t, 7.0), t.len());
        assert_eq!(byte_index_at_fraction("", 0.5), 0);
        assert!(t.is_char_boundary(byte_index_at_fraction(t, 0.3)));
    }

    #[test]
    fn click_through_style_toggles_transparent_bit() {
        let base: isize = 0x100;
        let locked = click_through_ex_style(base, true);
        assert_eq!(locked & WS_EX_TRANSPARENT_BITS, WS_EX_TRANSPARENT_BITS);
        assert_eq!(locked & WS_EX_LAYERED_BITS, WS_EX_LAYERED_BITS);
        assert_eq!(locked & base, base);
        let unlocked = click_through_ex_style(locked, false);
        assert_eq!(unlocked & WS_EX_TRANSPARENT_BITS, 0);
        assert_eq!(unlocked & base, base);
    }

    #[test]
    fn window_height_grows_with_layout() {
        assert_eq!(
            window_height_for(18, false, false),
            DESKTOP_LYRICS_MIN_HEIGHT
        );
        assert!(window_height_for(36, true, true) > window_height_for(36, false, false));
    }
}
