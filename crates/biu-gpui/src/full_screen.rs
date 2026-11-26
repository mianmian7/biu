//! Immersive full-screen player overlay: blurred ambient backdrop, large cover,
//! live 64-bar spectrum and auto-centering synchronized lyrics.

use crate::desktop_lyrics::open_or_toggle_desktop_lyrics;
use crate::player_bar::{
    TooltipExt, effective_duration, icon, icon_button, is_playing, play_button, play_mode_icon,
    play_mode_short_label, queue_button, rate_chip, seek_slider, time_label, volume_slider_with,
};
use crate::state::{ActiveModal, AppState};
use crate::theme::BiuPalette;
use crate::widgets::cover_box;
use biu_audio::SPECTRUM_BARS;
use biu_core::image_cache::ThumbSize;
use biu_core::lyrics::active_line_index;
use biu_core::{PlayMode, TrackSource};
use gpui_kit::assets::IconName;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::time::{Duration, Instant};

/// After a manual wheel scroll, auto-centering pauses for this long.
const USER_SCROLL_HOLD: Duration = Duration::from_secs(3);

/// Scroll offset (negative-down, GPUI convention) that centers an item.
pub fn centered_scroll_offset(
    item_top_in_content: f32,
    item_height: f32,
    viewport_height: f32,
    max_scroll: f32,
) -> f32 {
    let target = -(item_top_in_content + item_height / 2.0 - viewport_height / 2.0);
    target.clamp(-max_scroll.max(0.0), 0.0)
}

/// One step of exponential smoothing toward `target`; snaps when close.
pub fn smooth_step(current: f32, target: f32, factor: f32) -> (f32, bool) {
    let delta = target - current;
    if delta.abs() < 0.5 {
        (target, true)
    } else {
        (current + delta * factor.clamp(0.0, 1.0), false)
    }
}

pub fn source_badge(source: &TrackSource) -> String {
    match source {
        TrackSource::BiliVideo { .. } => "B站视频".into(),
        TrackSource::BiliAudio { .. } => "B站音频".into(),
        TrackSource::LocalFile { format, .. } => {
            if format.is_empty() {
                "本地".into()
            } else {
                format!("本地 · {}", format.to_uppercase())
            }
        }
    }
}

/// Dark-glass palette used regardless of the app theme.
fn glass_palette(base: BiuPalette) -> BiuPalette {
    let mut p = base;
    p.dark = true;
    p.text = hsla(0.0, 0.0, 1.0, 0.96);
    p.text_muted = hsla(0.0, 0.0, 1.0, 0.70);
    p.text_dim = hsla(0.0, 0.0, 1.0, 0.45);
    p.surface = hsla(0.0, 0.0, 1.0, 0.06);
    p.surface_hover = hsla(0.0, 0.0, 1.0, 0.12);
    p.surface_active = hsla(0.0, 0.0, 1.0, 0.18);
    p.border = hsla(0.0, 0.0, 1.0, 0.12);
    p
}

fn pill_button(
    id: &'static str,
    name: IconName,
    label: impl Into<SharedString>,
    p: BiuPalette,
    disabled: bool,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(30.0))
        .px_3()
        .flex()
        .flex_shrink_0()
        .items_center()
        .gap_1p5()
        .rounded_full()
        .bg(p.surface)
        .border_1()
        .border_color(p.border)
        .text_xs()
        .text_color(if disabled { p.text_dim } else { p.text })
        .when(!disabled, |d| {
            d.cursor_pointer()
                .hover(|s| s.bg(p.surface_hover))
                .active(|s| s.bg(p.surface_active))
        })
        .child(icon(
            name,
            13.0,
            if disabled { p.text_dim } else { p.text_muted },
        ))
        .child(label.into())
}

pub struct FullScreenPlayer {
    state: Entity<AppState>,
    lyrics_scroll: ScrollHandle,
    last_active: Option<usize>,
    user_scroll_at: Option<Instant>,
}

impl FullScreenPlayer {
    pub fn new(state: Entity<AppState>, _window: &mut Window, _cx: &mut Context<Self>) -> Self {
        Self {
            state,
            lyrics_scroll: ScrollHandle::new(),
            last_active: None,
            user_scroll_at: None,
        }
    }

    fn auto_center(&mut self, active: Option<usize>, window: &mut Window) {
        if active != self.last_active {
            self.last_active = active;
        }
        if self
            .user_scroll_at
            .is_some_and(|t| t.elapsed() < USER_SCROLL_HOLD)
        {
            return;
        }
        let Some(ix) = active else {
            return;
        };
        // Child 0 is the top spacer.
        let Some(item) = self.lyrics_scroll.bounds_for_item(ix + 1) else {
            return;
        };
        let viewport = self.lyrics_scroll.bounds();
        let vh = f32::from(viewport.size.height);
        if vh <= 0.0 {
            return;
        }
        let item_top = f32::from(item.origin.y - viewport.origin.y);
        let target = centered_scroll_offset(
            item_top,
            f32::from(item.size.height),
            vh,
            f32::from(self.lyrics_scroll.max_offset().y),
        );
        let current = self.lyrics_scroll.offset();
        let (next, done) = smooth_step(f32::from(current.y), target, 0.22);
        if (next - f32::from(current.y)).abs() > 0.01 {
            self.lyrics_scroll.set_offset(point(current.x, px(next)));
        }
        if !done {
            window.request_animation_frame();
        }
    }

    fn render_lyrics(
        &mut self,
        p: BiuPalette,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = self.state.read(cx);
        let lines = s.lyrics.lines.clone();
        let loading = s.lyrics_loading;
        let auto_running = s.auto_lyrics_running;
        let font = s.settings.lyrics_font_size.clamp(14, 40) as f32;
        let pos = s.snapshot.position_secs;
        let active = active_line_index(&lines, pos);

        if lines.is_empty() {
            let st = self.state.clone();
            let st2 = self.state.clone();
            return div()
                .size_full()
                .flex()
                .flex_col()
                .items_center()
                .justify_center()
                .gap_3()
                .child(icon(IconName::MicVocal, 36.0, p.text_dim))
                .child(
                    div()
                        .text_lg()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.text_muted)
                        .child(if loading {
                            "正在查找歌词…"
                        } else {
                            "暂无歌词"
                        }),
                )
                .when(!loading, |d| {
                    d.child(
                        div()
                            .text_sm()
                            .text_color(p.text_dim)
                            .child("可以手动搜索，或用声纹自动对齐"),
                    )
                    .child(
                        div()
                            .flex()
                            .gap_2()
                            .mt_2()
                            .child(
                                pill_button(
                                    "fs-empty-search",
                                    IconName::TextSearch,
                                    "搜索歌词",
                                    p,
                                    false,
                                )
                                .on_click(move |_, _, cx| {
                                    st.update(cx, |s, cx| {
                                        s.open_modal(ActiveModal::LyricsSearch, cx)
                                    })
                                }),
                            )
                            .child(
                                pill_button(
                                    "fs-empty-auto",
                                    IconName::AudioLines,
                                    if auto_running {
                                        "对齐中…"
                                    } else {
                                        "声纹自动对齐"
                                    },
                                    p,
                                    auto_running,
                                )
                                .on_click(move |_, _, cx| {
                                    st2.update(cx, |s, cx| s.trigger_auto_lyrics(cx))
                                }),
                            ),
                    )
                })
                .into_any_element();
        }

        self.auto_center(active, window);
        let pad = (f32::from(self.lyrics_scroll.bounds().size.height) / 2.0).max(120.0);

        let items = lines.iter().enumerate().map(|(i, line)| {
            let is_active = Some(i) == active;
            let is_past = active.is_some_and(|a| i < a);
            let main_color = if is_active {
                p.text
            } else if is_past {
                hsla(0.0, 0.0, 1.0, 0.32)
            } else {
                hsla(0.0, 0.0, 1.0, 0.52)
            };
            let sub_color = if is_active {
                p.accent
            } else {
                main_color.opacity(0.8)
            };
            let start = line.start_secs;
            let st = self.state.clone();
            div()
                .id(("fs-lyric", i))
                .w_full()
                .py(px(font * 0.45))
                .px_3()
                .rounded(px(10.0))
                .cursor_pointer()
                .hover(|s| s.bg(hsla(0.0, 0.0, 1.0, 0.06)))
                .on_click(cx.listener(move |this, _, _, cx| {
                    this.user_scroll_at = None;
                    st.update(cx, |s, cx| s.seek(start, cx));
                }))
                .child(
                    div()
                        .text_size(px(if is_active { font * 1.12 } else { font }))
                        .line_height(relative(1.35))
                        .font_weight(if is_active {
                            FontWeight::BOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(main_color)
                        .child(if line.text.trim().is_empty() {
                            "♪".to_string()
                        } else {
                            line.text.clone()
                        }),
                )
                .when_some(
                    line.translation.clone().filter(|t| !t.trim().is_empty()),
                    |d, t| {
                        d.child(
                            div()
                                .mt_0p5()
                                .text_size(px(font * 0.7))
                                .text_color(sub_color)
                                .child(t),
                        )
                    },
                )
                .when_some(
                    line.romaji.clone().filter(|t| !t.trim().is_empty()),
                    |d, t| {
                        d.child(
                            div()
                                .mt_0p5()
                                .text_size(px(font * 0.6))
                                .text_color(main_color.opacity(0.75))
                                .child(t),
                        )
                    },
                )
        });

        div()
            .id("fs-lyrics-scroll")
            .size_full()
            .flex()
            .flex_col()
            .overflow_y_scroll()
            .track_scroll(&self.lyrics_scroll)
            .on_scroll(cx.listener(|this, _, _, _| {
                this.user_scroll_at = Some(Instant::now());
            }))
            .child(div().flex_shrink_0().h(px(pad)))
            .children(items)
            .child(div().flex_shrink_0().h(px(pad)))
            .into_any_element()
    }
}

fn waveform(spectrum: &[f32; SPECTRUM_BARS], p: BiuPalette, height: f32) -> Div {
    let top = p.accent;
    let bottom = p.accent.opacity(0.35);
    div()
        .w_full()
        .h(px(height))
        .flex()
        .items_end()
        .gap(px(2.0))
        .children(spectrum.iter().map(|v| {
            let v = if v.is_finite() {
                v.clamp(0.0, 1.0)
            } else {
                0.0
            };
            div()
                .flex_1()
                .h(px((v * height).max(2.0)))
                .rounded_t(px(2.0))
                .bg(linear_gradient(
                    180.0,
                    linear_color_stop(top, 0.0),
                    linear_color_stop(bottom, 1.0),
                ))
        }))
}

impl Render for FullScreenPlayer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let cover_url = self
            .state
            .read(cx)
            .current_track()
            .map(|t| t.cover.clone())
            .unwrap_or_default();
        let (cover, backdrop) = if cover_url.is_empty() {
            (None, None)
        } else {
            self.state.update(cx, |s, cx| {
                (
                    s.ensure_cover(&cover_url, ThumbSize::Cover, false, cx),
                    s.ensure_cover(&cover_url, ThumbSize::BlurBg, true, cx),
                )
            })
        };

        let lyrics_el = {
            let base = self.state.read(cx).palette(window);
            self.render_lyrics(glass_palette(base), window, cx)
        };

        let s = self.state.read(cx);
        let p = glass_palette(s.palette(window));
        let track = s.current_track().cloned();
        let snap = s.snapshot.clone();
        let duration = effective_duration(
            snap.duration_secs,
            track.as_ref().map(|t| t.duration_secs).unwrap_or(0.0),
        );
        let position = snap.position_secs;
        let playing = is_playing(snap.state);
        let mode = s.queue.mode();
        let queue_len = s.queue.len();
        let spectrum = s.spectrum;
        let auto_running = s.auto_lyrics_running;
        let lyrics_open = s.windows.desktop_lyrics.is_some();
        let title = track.as_ref().map(|t| t.title.clone()).unwrap_or_default();
        let artist = track.as_ref().map(|t| t.artist.clone()).unwrap_or_default();
        let badge = track.as_ref().map(|t| source_badge(&t.source));
        let st = self.state.clone();
        let mac_pad = if cfg!(target_os = "macos") {
            84.0
        } else {
            20.0
        };

        let top_bar = div()
            .h(px(56.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .pl(px(mac_pad))
            .pr_5()
            .child(
                div()
                    .id("fs-drag")
                    .flex_1()
                    .min_w_0()
                    .h_full()
                    .flex()
                    .items_center()
                    .gap_2()
                    .on_mouse_down(MouseButton::Left, |ev, window, _| {
                        if ev.click_count >= 2 {
                            window.titlebar_double_click();
                        } else {
                            window.start_window_move();
                        }
                    })
                    .when_some(badge, |d, b| {
                        d.child(
                            div()
                                .flex_shrink_0()
                                .px_2()
                                .py_0p5()
                                .rounded(px(6.0))
                                .bg(p.accent.opacity(0.18))
                                .text_color(p.accent)
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .child(b),
                        )
                    })
                    .child(
                        div()
                            .text_sm()
                            .text_color(p.text_muted)
                            .truncate()
                            .child(title.clone()),
                    ),
            )
            .child(
                pill_button(
                    "fs-search-lyrics",
                    IconName::TextSearch,
                    "搜索歌词",
                    p,
                    false,
                )
                .on_click({
                    let st = st.clone();
                    move |_, _, cx| {
                        st.update(cx, |s, cx| s.open_modal(ActiveModal::LyricsSearch, cx))
                    }
                }),
            )
            .child(
                pill_button(
                    "fs-auto-lyrics",
                    if auto_running {
                        IconName::LoaderCircle
                    } else {
                        IconName::AudioLines
                    },
                    if auto_running {
                        "对齐中…"
                    } else {
                        "声纹对齐"
                    },
                    p,
                    auto_running,
                )
                .on_click({
                    let st = st.clone();
                    move |_, _, cx| st.update(cx, |s, cx| s.trigger_auto_lyrics(cx))
                }),
            )
            .child(
                pill_button(
                    "fs-desktop-lyrics",
                    IconName::Captions,
                    if lyrics_open {
                        "关闭桌面歌词"
                    } else {
                        "桌面歌词"
                    },
                    p,
                    false,
                )
                .on_click({
                    let st = st.clone();
                    move |_, _, cx| open_or_toggle_desktop_lyrics(st.clone(), cx)
                }),
            )
            .child(
                icon_button("fs-close", IconName::ChevronDown, 20.0, p, false).on_click({
                    let st = st.clone();
                    move |_, _, cx| st.update(cx, |s, cx| s.toggle_full_screen(cx))
                }),
            );

        let left_pane = div()
            .w(relative(0.42))
            .min_w(px(320.0))
            .h_full()
            .flex()
            .flex_col()
            .items_center()
            .justify_center()
            .gap_5()
            .px_8()
            .child(div().rounded(px(18.0)).shadow_2xl().child(cover_box(
                cover,
                px(280.0),
                px(280.0),
                px(18.0),
                p,
            )))
            .child(
                div()
                    .w(px(320.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .gap_1()
                    .child(
                        div()
                            .w_full()
                            .text_xl()
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.text)
                            .text_center()
                            .truncate()
                            .child(title),
                    )
                    .child(
                        div()
                            .w_full()
                            .text_sm()
                            .text_color(p.text_muted)
                            .text_center()
                            .truncate()
                            .child(artist),
                    ),
            )
            .child(div().w(px(320.0)).child(waveform(&spectrum, p, 56.0)));

        let right_pane = div()
            .flex_1()
            .min_w_0()
            .h_full()
            .py_6()
            .pr_10()
            .child(lyrics_el);

        let bottom = div()
            .flex_shrink_0()
            .px_10()
            .pb_6()
            .pt_2()
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(time_label(position, p))
                    .child(
                        div()
                            .flex_1()
                            .child(seek_slider("fs-seek", &st, position, duration, p)),
                    )
                    .child(time_label(duration, p)),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                icon_button(
                                    "fs-mode",
                                    play_mode_icon(mode),
                                    16.0,
                                    p,
                                    mode != PlayMode::Order,
                                )
                                .tip(play_mode_short_label(mode))
                                .on_click({
                                    let st = st.clone();
                                    move |_, _, cx| st.update(cx, |s, cx| s.toggle_play_mode(cx))
                                }),
                            )
                            .child(rate_chip("fs-rate", &st, snap.rate, p)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_4()
                            .child(
                                icon_button("fs-prev", IconName::SkipBack, 22.0, p, false)
                                    .on_click({
                                        let st = st.clone();
                                        move |_, _, cx| st.update(cx, |s, cx| s.prev_track(cx))
                                    }),
                            )
                            .child(play_button("fs-play", playing, 52.0, p).on_click({
                                let st = st.clone();
                                move |_, _, cx| st.update(cx, |s, cx| s.toggle_play(cx))
                            }))
                            .child(
                                icon_button("fs-next", IconName::SkipForward, 22.0, p, false)
                                    .on_click({
                                        let st = st.clone();
                                        move |_, _, cx| {
                                            st.update(cx, |s, cx| s.next_track(false, cx))
                                        }
                                    }),
                            ),
                    )
                    .child(
                        div()
                            .flex_1()
                            .flex()
                            .items_center()
                            .justify_end()
                            .gap_1()
                            .child(
                                icon_button(
                                    "fs-mute",
                                    if snap.muted || snap.volume <= 0.001 {
                                        IconName::VolumeX
                                    } else {
                                        IconName::Volume2
                                    },
                                    16.0,
                                    p,
                                    false,
                                )
                                .on_click({
                                    let st = st.clone();
                                    move |_, _, cx| st.update(cx, |s, cx| s.toggle_mute(cx))
                                }),
                            )
                            .child(div().w(px(110.0)).child(volume_slider_with(
                                "fs-volume",
                                &st,
                                snap.volume,
                                snap.muted,
                                p,
                            )))
                            .child(queue_button("fs-queue", &st, queue_len, p)),
                    ),
            );

        div()
            .id("full-screen-player")
            .absolute()
            .inset_0()
            .occlude()
            .overflow_hidden()
            .bg(hsla(0.0, 0.0, 0.04, 1.0))
            .text_color(p.text)
            .when_some(backdrop, |d, img_arc| {
                d.child(
                    div()
                        .absolute()
                        .inset_0()
                        .child(img(img_arc).size_full().object_fit(ObjectFit::Cover)),
                )
            })
            .child(div().absolute().inset_0().bg(hsla(0.0, 0.0, 0.06, 0.72)))
            .child(
                div()
                    .absolute()
                    .inset_0()
                    .flex()
                    .flex_col()
                    .child(top_bar)
                    .child(
                        div()
                            .flex_1()
                            .min_h_0()
                            .flex()
                            .items_center()
                            .child(left_pane)
                            .child(right_pane),
                    )
                    .child(bottom),
            )
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn centering_offset_is_clamped() {
        // Item at content y=500 (h=40) in a 400px viewport, plenty of scroll room.
        assert_eq!(centered_scroll_offset(500.0, 40.0, 400.0, 2000.0), -320.0);
        // Never scrolls above the top.
        assert_eq!(centered_scroll_offset(10.0, 40.0, 400.0, 2000.0), 0.0);
        // Never beyond max scroll.
        assert_eq!(centered_scroll_offset(5000.0, 40.0, 400.0, 1000.0), -1000.0);
    }

    #[test]
    fn smoothing_converges_and_snaps() {
        let (v, done) = smooth_step(0.0, -100.0, 0.25);
        assert_eq!(v, -25.0);
        assert!(!done);
        let (v, done) = smooth_step(-99.8, -100.0, 0.25);
        assert_eq!(v, -100.0);
        assert!(done);
    }

    #[test]
    fn badges_describe_sources() {
        assert_eq!(
            source_badge(&TrackSource::LocalFile {
                path: "/a.flac".into(),
                format: "flac".into()
            }),
            "本地 · FLAC"
        );
        assert_eq!(source_badge(&TrackSource::BiliAudio { sid: 1 }), "B站音频");
    }
}
