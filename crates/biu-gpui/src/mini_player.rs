//! Compact always-on-top floating mini player.

use crate::desktop_lyrics::open_or_toggle_desktop_lyrics;
use crate::image_loader::CoverKey;
use crate::player_bar::{
    TooltipExt, effective_duration, icon_button, is_playing, play_button, play_mode_icon,
    play_mode_short_label, seek_slider, time_label,
};
use crate::state::AppState;
use crate::system::open_or_focus_main_window;
use crate::widgets::cover_box;
use biu_core::image_cache::ThumbSize;
use gpui_kit::assets::IconName;
use gpui_kit::prelude::*;
use gpui_kit::*;

pub const MINI_WIDTH: f32 = 360.0;
pub const MINI_HEIGHT: f32 = 118.0;

pub fn open_or_toggle_mini_player(state: Entity<AppState>, cx: &mut App) {
    if let Some(handle) = state.read(cx).windows.mini_player {
        let _ = handle.update(cx, |_, window, _| window.remove_window());
        state.update(cx, |s, cx| {
            s.windows.mini_player = None;
            cx.notify();
        });
        return;
    }

    let display = cx.primary_display();
    let bounds = match display.as_ref() {
        Some(d) => {
            let db = d.bounds();
            let x = db.origin.x + db.size.width - px(MINI_WIDTH) - px(32.0);
            let y = db.origin.y + px(72.0);
            Bounds::new(point(x, y), size(px(MINI_WIDTH), px(MINI_HEIGHT)))
        }
        None => Bounds::centered(None, size(px(MINI_WIDTH), px(MINI_HEIGHT)), cx),
    };
    let opts = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: None,
        kind: WindowKind::PopUp,
        focus: true,
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
        window.set_window_title("Biu Mini");
        cx.new(|cx| MiniPlayerWindow::new(st, window, cx))
    }) {
        Ok((handle, _)) => state.update(cx, |s, cx| {
            s.windows.mini_player = Some(handle);
            cx.notify();
        }),
        Err(err) => tracing::error!("failed to open mini player: {err:#}"),
    }
}

pub struct MiniPlayerWindow {
    state: Entity<AppState>,
    _observe: Subscription,
}

impl MiniPlayerWindow {
    pub fn new(state: Entity<AppState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |_, _, cx| cx.notify());
        Self {
            state,
            _observe: observe,
        }
    }
}

impl Render for MiniPlayerWindow {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (cover, missing_cover) = {
            let s = self.state.read(cx);
            let cover_url = s
                .current_track()
                .map(|t| t.cover.clone())
                .unwrap_or_default();
            if cover_url.is_empty() {
                (None, None)
            } else {
                let key = CoverKey::thumb(cover_url.clone(), ThumbSize::Card);
                let img = s.covers.get(&key);
                let miss = (img.is_none() && s.covers.should_fetch(&key)).then_some(cover_url);
                (img, miss)
            }
        };
        if let Some(url) = missing_cover {
            let st = self.state.clone();
            window.defer(cx, move |_, cx| {
                st.update(cx, |s, cx| {
                    s.ensure_cover(&url, ThumbSize::Card, false, cx);
                });
            });
        }

        let s = self.state.read(cx);
        let palette = s.palette(window);
        let track = s.current_track().cloned();
        let snap = s.snapshot.clone();
        let duration = effective_duration(
            snap.duration_secs,
            track.as_ref().map(|t| t.duration_secs).unwrap_or(0.0),
        );
        let position = snap.position_secs;
        let playing = is_playing(snap.state);
        let mode = s.queue.mode();
        let lyrics_open = s.windows.desktop_lyrics.is_some();
        let title = track
            .as_ref()
            .map(|t| t.title.clone())
            .unwrap_or_else(|| "Biu 音乐".into());
        let artist = track
            .as_ref()
            .map(|t| t.artist.clone())
            .unwrap_or_else(|| "未在播放".into());
        let st = self.state.clone();

        let chrome = div()
            .occlude()
            .flex()
            .items_center()
            .gap_0p5()
            .child(
                icon_button(
                    "mini-lyrics",
                    IconName::Captions,
                    13.0,
                    palette,
                    lyrics_open,
                )
                .on_click({
                    let st = st.clone();
                    move |_, _, cx| open_or_toggle_desktop_lyrics(st.clone(), cx)
                }),
            )
            .child(
                icon_button("mini-restore", IconName::AppWindow, 13.0, palette, false).on_click({
                    let st = st.clone();
                    move |_, _, cx| open_or_focus_main_window(st.clone(), cx)
                }),
            )
            .child(
                icon_button("mini-close", IconName::X, 13.0, palette, false).on_click({
                    let st = st.clone();
                    move |_, _, cx| {
                        let st = st.clone();
                        cx.defer(move |cx| open_or_toggle_mini_player(st, cx));
                    }
                }),
            );

        let controls = div()
            .occlude()
            .flex()
            .items_center()
            .gap_1()
            .child(
                icon_button(
                    "mini-mode",
                    play_mode_icon(mode),
                    13.0,
                    palette,
                    mode != biu_core::PlayMode::Order,
                )
                .tip(play_mode_short_label(mode))
                .on_click({
                    let st = st.clone();
                    move |_, _, cx| st.update(cx, |s, cx| s.toggle_play_mode(cx))
                }),
            )
            .child(
                icon_button("mini-prev", IconName::SkipBack, 15.0, palette, false).on_click({
                    let st = st.clone();
                    move |_, _, cx| st.update(cx, |s, cx| s.prev_track(cx))
                }),
            )
            .child(play_button("mini-play", playing, 30.0, palette).on_click({
                let st = st.clone();
                move |_, _, cx| st.update(cx, |s, cx| s.toggle_play(cx))
            }))
            .child(
                icon_button("mini-next", IconName::SkipForward, 15.0, palette, false).on_click({
                    let st = st.clone();
                    move |_, _, cx| st.update(cx, |s, cx| s.next_track(false, cx))
                }),
            );

        div().size_full().p(px(4.0)).child(
            div()
                .id("mini-player")
                .size_full()
                .flex()
                .items_center()
                .gap_3()
                .p_3()
                .rounded(px(16.0))
                .bg(palette.surface)
                .border_1()
                .border_color(palette.border)
                .shadow_lg()
                .text_color(palette.text)
                .on_mouse_down(MouseButton::Left, |_, window, _| window.start_window_move())
                .child(cover_box(cover, px(64.0), px(64.0), px(10.0), palette))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .items_start()
                                .gap_1()
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .flex()
                                        .flex_col()
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .truncate()
                                                .child(title),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(palette.text_muted)
                                                .truncate()
                                                .child(artist),
                                        ),
                                )
                                .child(chrome),
                        )
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(controls)
                                .child(
                                    div()
                                        .text_xs()
                                        .text_color(palette.text_dim)
                                        .flex()
                                        .gap_1()
                                        .child(time_label(position, palette).min_w_0())
                                        .child("/")
                                        .child(time_label(duration, palette).min_w_0()),
                                ),
                        )
                        .child(
                            div().occlude().child(
                                seek_slider("mini-seek", &st, position, duration, palette)
                                    .thickness(3.0),
                            ),
                        ),
                ),
        )
    }
}
