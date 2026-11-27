//! Persistent bottom player bar (isolated Entity so 100ms playback ticks only
//! repaint this 82px bar and never rebuild the scrollable page tree), plus
//! shared player primitives reused by the full-screen player, mini-player and
//! desktop-lyrics window.

use crate::image_loader::CoverKey;
use crate::state::{ActiveModal, AppState, Route};
use crate::theme::BiuPalette;
use crate::widgets::{cover_box, format_duration};
use biu_audio::PlaybackState;
use biu_core::download::DownloadFormat;
use biu_core::image_cache::ThumbSize;
use biu_core::{PlayMode, TrackSource};
use gpui_kit::assets::IconName;
use gpui_kit::component::tooltip::Tooltip;
use gpui_kit::prelude::*;
use gpui_kit::*;
use std::rc::Rc;

pub const PLAYER_BAR_HEIGHT: f32 = 82.0;
pub const PLAYBACK_RATES: [f32; 5] = [0.75, 1.0, 1.25, 1.5, 2.0];

// ---------------------------------------------------------------------------
// Pure helpers
// ---------------------------------------------------------------------------

/// Map a pointer x-coordinate onto a 0..=1 ratio across a track.
pub fn ratio_from_position(x: f32, left: f32, width: f32) -> f32 {
    if !width.is_finite() || width <= 0.0 || !x.is_finite() {
        return 0.0;
    }
    ((x - left) / width).clamp(0.0, 1.0)
}

/// Current playback progress as a 0..=1 ratio.
pub fn progress_ratio(position: f64, duration: f64) -> f32 {
    if !duration.is_finite() || duration <= 0.0 || !position.is_finite() {
        return 0.0;
    }
    (position / duration).clamp(0.0, 1.0) as f32
}

/// Next playback rate in the `0.75x → 1.0x → … → 2.0x → 0.75x` cycle.
pub fn next_playback_rate(current: f32) -> f32 {
    let idx = PLAYBACK_RATES
        .iter()
        .position(|r| (r - current).abs() < 0.01);
    match idx {
        Some(i) => PLAYBACK_RATES[(i + 1) % PLAYBACK_RATES.len()],
        None => 1.0,
    }
}

pub fn format_rate(rate: f32) -> String {
    if (rate - rate.round()).abs() < 0.001 {
        format!("{rate:.1}x")
    } else {
        format!("{}x", format!("{rate:.2}").trim_end_matches('0'))
    }
}

pub fn play_mode_short_label(mode: PlayMode) -> &'static str {
    match mode {
        PlayMode::Loop => "列表循环",
        PlayMode::Order => "顺序播放",
        PlayMode::Single => "单曲循环",
        PlayMode::Shuffle => "随机播放",
    }
}

pub fn play_mode_icon(mode: PlayMode) -> IconName {
    match mode {
        PlayMode::Loop => IconName::Repeat,
        PlayMode::Order => IconName::ListOrdered,
        PlayMode::Single => IconName::Repeat1,
        PlayMode::Shuffle => IconName::Shuffle,
    }
}

/// Effective duration: engine duration when known, otherwise the track metadata.
pub fn effective_duration(engine_secs: f64, track_secs: f64) -> f64 {
    if engine_secs.is_finite() && engine_secs > 0.0 {
        engine_secs
    } else if track_secs.is_finite() && track_secs > 0.0 {
        track_secs
    } else {
        0.0
    }
}

fn quality_tag(source: &TrackSource) -> Option<String> {
    match source {
        TrackSource::LocalFile { format, .. } if !format.is_empty() => {
            Some(format.to_ascii_uppercase())
        }
        TrackSource::BiliAudio { .. } => Some("音频".into()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Shared visual primitives
// ---------------------------------------------------------------------------

pub fn icon(name: IconName, size: f32, color: Hsla) -> Svg {
    svg()
        .path(name.path())
        .flex_shrink_0()
        .size(px(size))
        .text_color(color)
}

/// Square, rounded ghost icon button with subtle hover state.
pub fn icon_button(
    id: impl Into<ElementId>,
    name: IconName,
    icon_size: f32,
    palette: BiuPalette,
    active: bool,
) -> Stateful<Div> {
    let color = if active {
        palette.accent
    } else {
        palette.text_muted
    };
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(icon_size + 16.0))
        .rounded_full()
        .cursor_pointer()
        .hover(move |s| s.bg(palette.surface_hover))
        .active(move |s| s.bg(palette.surface_active))
        .child(icon(name, icon_size, color))
}

/// High-contrast circular play/pause button (white disc in dark mode, matching original Biu).
pub fn play_button(
    id: impl Into<ElementId>,
    playing: bool,
    diameter: f32,
    palette: BiuPalette,
) -> Stateful<Div> {
    let glyph = if playing {
        IconName::Pause
    } else {
        IconName::Play
    };
    div()
        .id(id)
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(diameter))
        .rounded_full()
        .bg(palette.play_btn_bg)
        .shadow_md()
        .cursor_pointer()
        .hover(|s| s.opacity(0.90))
        .active(|s| s.opacity(0.78))
        .child(
            div()
                .when(!playing, |d| d.pl(px(1.5)))
                .child(icon(glyph, diameter * 0.44, palette.play_btn_fg)),
        )
}

/// Accent circular play button used on card hover overlays & action bars.
pub fn accent_play_circle(diameter: f32, palette: BiuPalette) -> Div {
    div()
        .flex()
        .flex_shrink_0()
        .items_center()
        .justify_center()
        .size(px(diameter))
        .rounded_full()
        .bg(palette.accent)
        .shadow_lg()
        .child(
            div()
                .pl(px(2.0))
                .child(icon(IconName::Play, diameter * 0.45, palette.accent_fg)),
        )
}

/// Build a click handler that runs `f` against `AppState`.
pub fn on_state<E: 'static>(
    state: &Entity<AppState>,
    f: impl Fn(&mut AppState, &mut Context<AppState>) + 'static,
) -> impl Fn(&E, &mut Window, &mut App) + 'static {
    let state = state.clone();
    move |_, _, cx| state.update(cx, |s, cx| f(s, cx))
}

/// `.tip("…")` — themed gpui-component tooltip on any stateful element.
pub trait TooltipExt: StatefulInteractiveElement + Sized {
    fn tip(self, text: impl Into<SharedString>) -> Self {
        let text: SharedString = text.into();
        self.tooltip(move |window, cx| Tooltip::new(text.clone()).build(window, cx))
    }
}

impl<T: StatefulInteractiveElement> TooltipExt for T {}

pub fn is_playing(state: PlaybackState) -> bool {
    matches!(state, PlaybackState::Playing | PlaybackState::Loading)
}

// ---------------------------------------------------------------------------
// Slider (seek / volume)
// ---------------------------------------------------------------------------

type SliderCallback = Rc<dyn Fn(f32, bool, &mut Window, &mut App)>;

#[derive(Default)]
struct SliderDrag {
    dragging: bool,
    preview: f32,
}

/// Draggable horizontal slider. The callback receives `(ratio, committed)`:
/// `committed == false` for live drag updates (only when `live` is set) and
/// `true` once on mouse release (or on a single click).
#[derive(IntoElement)]
pub struct Slider {
    id: ElementId,
    value: f32,
    palette: BiuPalette,
    thickness: f32,
    fill: Option<Hsla>,
    live: bool,
    on_change: Option<SliderCallback>,
}

impl Slider {
    pub fn new(id: impl Into<ElementId>, value: f32, palette: BiuPalette) -> Self {
        Self {
            id: id.into(),
            value,
            palette,
            thickness: 4.0,
            fill: None,
            live: false,
            on_change: None,
        }
    }

    pub fn thickness(mut self, px_: f32) -> Self {
        self.thickness = px_;
        self
    }

    pub fn fill(mut self, color: Hsla) -> Self {
        self.fill = Some(color);
        self
    }

    pub fn live(mut self, live: bool) -> Self {
        self.live = live;
        self
    }

    pub fn on_change(mut self, f: impl Fn(f32, bool, &mut Window, &mut App) + 'static) -> Self {
        self.on_change = Some(Rc::new(f));
        self
    }
}

impl RenderOnce for Slider {
    fn render(self, window: &mut Window, cx: &mut App) -> impl IntoElement {
        let drag = window.use_keyed_state(self.id.clone(), cx, |_, _| SliderDrag::default());
        let (dragging, preview) = {
            let d = drag.read(cx);
            (d.dragging, d.preview)
        };
        let shown = if dragging { preview } else { self.value }.clamp(0.0, 1.0);
        let fill = self.fill.unwrap_or(self.palette.text);
        let hover_fill = self.palette.accent;
        let track = if self.palette.dark {
            hsla(0.0, 0.0, 1.0, 0.14)
        } else {
            hsla(0.0, 0.0, 0.0, 0.12)
        };
        let group: SharedString = format!("slider-{:?}", self.id).into();
        let thumb = 12.0_f32;
        let on_change = self.on_change.clone();
        let live = self.live;

        div()
            .id(self.id.clone())
            .group(group.clone())
            .relative()
            .w_full()
            .h(px(16.0))
            .flex()
            .items_center()
            .cursor_pointer()
            .child(
                div()
                    .relative()
                    .w_full()
                    .h(px(self.thickness))
                    .rounded_full()
                    .bg(track)
                    .overflow_hidden()
                    .child(
                        div()
                            .absolute()
                            .left_0()
                            .top_0()
                            .h_full()
                            .w(relative(shown))
                            .rounded_full()
                            .bg(if dragging { hover_fill } else { fill })
                            .group_hover(group.clone(), move |s| s.bg(hover_fill)),
                    ),
            )
            .child(
                div()
                    .absolute()
                    .top(px((16.0 - thumb) / 2.0))
                    .left(relative(shown))
                    .ml(px(-thumb / 2.0))
                    .size(px(thumb))
                    .rounded_full()
                    .bg(gpui_kit::white())
                    .shadow_md()
                    .when(!dragging, |s| s.invisible())
                    .group_hover(group.clone(), |s| s.visible()),
            )
            .child(
                canvas(
                    |bounds, window, _| window.insert_hitbox(bounds, HitboxBehavior::Normal),
                    move |bounds, hitbox, window, _| {
                        let Some(on_change) = on_change else {
                            return;
                        };
                        let left = f32::from(bounds.origin.x);
                        let width = f32::from(bounds.size.width);

                        let drag_down = drag.clone();
                        let cb_down = on_change.clone();
                        window.on_mouse_event(move |ev: &MouseDownEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble
                                || ev.button != MouseButton::Left
                                || !hitbox.is_hovered(window)
                            {
                                return;
                            }
                            let r = ratio_from_position(f32::from(ev.position.x), left, width);
                            drag_down.update(cx, |d, cx| {
                                d.dragging = true;
                                d.preview = r;
                                cx.notify();
                            });
                            if live {
                                cb_down(r, false, window, cx);
                            }
                            cx.stop_propagation();
                        });

                        let drag_move = drag.clone();
                        let cb_move = on_change.clone();
                        window.on_mouse_event(move |ev: &MouseMoveEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble || !drag_move.read(cx).dragging {
                                return;
                            }
                            if ev.pressed_button != Some(MouseButton::Left) {
                                drag_move.update(cx, |d, cx| {
                                    d.dragging = false;
                                    cx.notify();
                                });
                                return;
                            }
                            let r = ratio_from_position(f32::from(ev.position.x), left, width);
                            drag_move.update(cx, |d, cx| {
                                d.preview = r;
                                cx.notify();
                            });
                            if live {
                                cb_move(r, false, window, cx);
                            }
                        });

                        let drag_up = drag.clone();
                        let cb_up = on_change;
                        window.on_mouse_event(move |ev: &MouseUpEvent, phase, window, cx| {
                            if phase != DispatchPhase::Bubble
                                || ev.button != MouseButton::Left
                                || !drag_up.read(cx).dragging
                            {
                                return;
                            }
                            let r = ratio_from_position(f32::from(ev.position.x), left, width);
                            drag_up.update(cx, |d, cx| {
                                d.dragging = false;
                                d.preview = r;
                                cx.notify();
                            });
                            cb_up(r, true, window, cx);
                        });
                    },
                )
                .absolute()
                .inset_0(),
            )
    }
}

/// Seek slider bound to the current track position.
pub fn seek_slider(
    id: impl Into<ElementId>,
    state: &Entity<AppState>,
    position: f64,
    duration: f64,
    palette: BiuPalette,
) -> Slider {
    let st = state.clone();
    Slider::new(id, progress_ratio(position, duration), palette).on_change(
        move |ratio, committed, _, cx| {
            if committed && duration > 0.0 {
                st.update(cx, |s, cx| s.seek(ratio as f64 * duration, cx));
            }
        },
    )
}

// ---------------------------------------------------------------------------
// Isolated PlayerBarView Entity
// ---------------------------------------------------------------------------

pub struct PlayerBarView {
    state: Entity<AppState>,
    last_playback_ver: u64,
    last_ui_ver: u64,
    last_cover_present: bool,
    _observe: Subscription,
}

impl PlayerBarView {
    pub fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |this, state, cx| {
            let s = state.read(cx);
            let cover_present = s
                .current_track()
                .filter(|t| !t.cover.is_empty())
                .is_some_and(|t| {
                    s.covers
                        .get(&CoverKey::thumb(t.cover.clone(), ThumbSize::Tiny))
                        .is_some()
                });
            if this.last_playback_ver != s.playback_version
                || this.last_ui_ver != s.ui_version
                || this.last_cover_present != cover_present
            {
                this.last_playback_ver = s.playback_version;
                this.last_ui_ver = s.ui_version;
                this.last_cover_present = cover_present;
                cx.notify();
            }
        });
        let s = state.read(cx);
        Self {
            last_playback_ver: s.playback_version,
            last_ui_ver: s.ui_version,
            last_cover_present: false,
            state,
            _observe: observe,
        }
    }
}

impl Render for PlayerBarView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let palette = self.state.read(cx).palette(window);
        render_player_bar(&self.state, palette, window, cx)
    }
}

pub fn render_player_bar(
    state: &Entity<AppState>,
    palette: BiuPalette,
    window: &mut Window,
    cx: &mut App,
) -> AnyElement {
    let (
        track,
        cover,
        missing_cover,
        snap,
        mode,
        queue_len,
        full_screen,
        audio_badge,
        pages_count,
        liked,
        logged_in,
    ) = {
        let s = state.read(cx);
        let track = s.current_track().cloned();
        let cover_url = track.as_ref().map(|t| t.cover.clone()).unwrap_or_default();
        let mut missing = None;
        let cover = if cover_url.is_empty() {
            None
        } else {
            let key = CoverKey::thumb(cover_url.clone(), ThumbSize::Tiny);
            let img = s.covers.get(&key);
            if img.is_none() && s.covers.should_fetch(&key) {
                missing = Some(cover_url);
            }
            img
        };
        (
            track,
            cover,
            missing,
            s.snapshot.clone(),
            s.queue.mode(),
            s.queue.len(),
            s.is_full_screen,
            s.current_audio_badge.clone(),
            s.current_video_pages.len(),
            s.current_track_liked,
            s.user.is_some(),
        )
    };
    if let Some(url) = missing_cover {
        let st = state.clone();
        window.defer(cx, move |_, cx| {
            st.update(cx, |s, cx| {
                s.ensure_cover(&url, ThumbSize::Tiny, false, cx);
            });
        });
    }
    let duration = effective_duration(
        snap.duration_secs,
        track.as_ref().map(|t| t.duration_secs).unwrap_or(0.0),
    );
    let position = snap
        .position_secs
        .min(if duration > 0.0 { duration } else { f64::MAX });
    let playing = is_playing(snap.state);
    let has_track = track.is_some();

    // -- Left: album art + title + quality badge + artist ------------------
    let display_title = match track.as_ref() {
        Some(t) => match &t.source {
            TrackSource::BiliVideo {
                part_title: Some(part),
                ..
            } if !part.is_empty() && pages_count > 1 => format!("{part} · {}", t.title),
            _ => t.title.clone(),
        },
        None => "Biu 音乐".into(),
    };
    let title: SharedString = display_title.into();
    let artist: SharedString = track
        .as_ref()
        .map(|t| t.artist.clone())
        .unwrap_or_else(|| "挑选一首歌开始聆听".into())
        .into();
    let artist_mid = track.as_ref().and_then(|t| t.artist_mid);
    let badge = audio_badge.or_else(|| track.as_ref().and_then(|t| quality_tag(&t.source)));
    let is_bili_track = track
        .as_ref()
        .is_some_and(|t| !matches!(t.source, TrackSource::LocalFile { .. }));
    let is_bili_video = track
        .as_ref()
        .is_some_and(|t| matches!(t.source, TrackSource::BiliVideo { .. }));

    let mut title_el = div()
        .id("pb-title")
        .flex_1()
        .min_w_0()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(palette.text)
        .truncate()
        .child(title);
    if is_bili_track
        && let Some(t) = track.clone()
    {
        title_el = title_el
            .cursor_pointer()
            .hover(move |s| s.text_color(palette.accent))
            .tip("在 B 站打开")
            .on_click(on_state(state, move |s, cx| {
                s.open_track_in_bilibili(&t, cx)
            }));
    }

    let mut artist_el = div()
        .id("pb-artist")
        .text_xs()
        .text_color(palette.text_muted)
        .truncate()
        .child(artist.clone());
    if let Some(mid) = artist_mid {
        let name = artist.to_string();
        artist_el = artist_el
            .cursor_pointer()
            .hover(move |s| s.text_color(palette.text))
            .on_click(on_state(state, move |s, cx| {
                s.navigate(
                    Route::UserProfile {
                        mid,
                        name: name.clone(),
                    },
                    cx,
                )
            }));
    }

    let left = div()
        .w(px(300.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_3()
        .child(
            div()
                .id("pb-cover")
                .group("pb-cover-grp")
                .relative()
                .cursor_pointer()
                .rounded(px(10.0))
                .shadow_md()
                .on_click(on_state(state, |s, cx| {
                    if s.current_track().is_some() {
                        s.toggle_full_screen(cx)
                    }
                }))
                .child(cover_box(cover, px(52.0), px(52.0), px(10.0), palette))
                .when(has_track, |d| {
                    d.child(
                        div()
                            .absolute()
                            .inset_0()
                            .rounded(px(10.0))
                            .bg(hsla(0.0, 0.0, 0.0, 0.45))
                            .flex()
                            .items_center()
                            .justify_center()
                            .invisible()
                            .group_hover("pb-cover-grp", |s| s.visible())
                            .child(icon(IconName::Maximize2, 16.0, gpui_kit::white())),
                    )
                }),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(3.0))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .min_w_0()
                        .child(title_el)
                        .when_some(badge, |row, tag| {
                            row.child(
                                div()
                                    .flex_shrink_0()
                                    .px_1p5()
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .bg(palette.accent_soft)
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(palette.accent)
                                    .child(tag),
                            )
                        }),
                )
                .child(artist_el),
        )
        .when(pages_count > 1, |row| {
            row.child(
                div()
                    .id("pb-parts")
                    .h(px(26.0))
                    .px_2()
                    .flex_shrink_0()
                    .rounded_full()
                    .bg(palette.surface)
                    .flex()
                    .items_center()
                    .gap_1()
                    .text_xs()
                    .text_color(palette.accent)
                    .cursor_pointer()
                    .hover(move |s| s.bg(palette.surface_hover))
                    .tip(format!("切换视频分集（共 {pages_count}P）"))
                    .on_click(on_state(state, |s, cx| {
                        s.open_modal(ActiveModal::PageList, cx)
                    }))
                    .child(icon(IconName::ListVideo, 13.0, palette.accent))
                    .child(format!("{pages_count}P")),
            )
        });

    // -- Center: transport + sleek seek bar --------------------------------
    let center = div()
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap(px(4.0))
        .px_6()
        .child(
            div()
                .flex()
                .items_center()
                .gap_3()
                .child(
                    icon_button("pb-prev", IconName::SkipBack, 17.0, palette, false)
                        .tip("上一首")
                        .on_click(on_state(state, |s, cx| s.prev_track(cx))),
                )
                .child(
                    play_button("pb-play", playing, 38.0, palette)
                        .on_click(on_state(state, |s, cx| s.toggle_play(cx))),
                )
                .child(
                    icon_button("pb-next", IconName::SkipForward, 17.0, palette, false)
                        .tip("下一首")
                        .on_click(on_state(state, |s, cx| s.next_track(false, cx))),
                ),
        )
        .child(
            div()
                .w_full()
                .max_w(px(620.0))
                .flex()
                .items_center()
                .gap_2p5()
                .child(time_label(position, palette))
                .child(
                    div()
                        .flex_1()
                        .child(seek_slider("pb-seek", state, position, duration, palette)),
                )
                .child(time_label(duration, palette)),
        );

    // -- Right: mode / fav / download / queue / volume / rate / expand -----
    let vol_icon = if snap.muted || snap.volume <= 0.001 {
        IconName::VolumeX
    } else if snap.volume < 0.5 {
        IconName::Volume1
    } else {
        IconName::Volume2
    };

    let right = div()
        .w(px(310.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_end()
        .gap_0p5()
        .child(
            icon_button(
                "pb-mode",
                play_mode_icon(mode),
                15.0,
                palette,
                mode != PlayMode::Order,
            )
            .tip(play_mode_short_label(mode))
            .on_click(on_state(state, |s, cx| {
                s.toggle_play_mode(cx);
                let label = s.queue.mode().label();
                s.show_toast(label, false, cx);
            })),
        )
        .when(is_bili_video && logged_in, |row| {
            row.child(
                icon_button("pb-like", IconName::ThumbsUp, 15.0, palette, liked)
                    .tip(if liked { "取消点赞" } else { "点赞" })
                    .on_click(on_state(state, |s, cx| s.toggle_like_current_track(cx))),
            )
        })
        .when_some(track.clone().filter(|_| is_bili_track), |row, t| {
            let fav_track = t.clone();
            let dl_track = t;
            row.child(
                icon_button("pb-fav", IconName::Star, 15.0, palette, false)
                    .tip("收藏到收藏夹")
                    .on_click(on_state(state, move |s, cx| {
                        if s.user.is_none() {
                            s.open_modal(ActiveModal::Login, cx);
                        } else {
                            s.open_modal(
                                ActiveModal::FavSelect {
                                    track: fav_track.clone(),
                                },
                                cx,
                            );
                        }
                    })),
            )
            .child(
                icon_button("pb-download", IconName::Download, 15.0, palette, false)
                    .tip("下载音频")
                    .on_click(on_state(state, move |s, cx| {
                        let runtime = s.services.rt.handle().clone();
                        let _guard = runtime.enter();
                        s.queue_download(dl_track.clone(), DownloadFormat::Native, cx)
                    })),
            )
        })
        .child(queue_button("pb-queue", state, queue_len, palette))
        .child(
            icon_button("pb-mute", vol_icon, 15.0, palette, false)
                .tip(if snap.muted { "取消静音" } else { "静音" })
                .on_click(on_state(state, |s, cx| s.toggle_mute(cx))),
        )
        .child(div().w(px(64.0)).mx_0p5().child(volume_slider_with(
            "pb-volume",
            state,
            snap.volume,
            snap.muted,
            palette,
        )))
        .child(rate_chip("pb-rate", state, snap.rate, palette))
        .child(
            icon_button(
                "pb-expand",
                if full_screen {
                    IconName::Minimize2
                } else {
                    IconName::Maximize2
                },
                14.0,
                palette,
                full_screen,
            )
            .tip("全屏歌词")
            .when(!has_track, |b| b.opacity(0.35))
            .on_click(on_state(state, |s, cx| {
                if s.current_track().is_some() || s.is_full_screen {
                    s.toggle_full_screen(cx)
                }
            })),
        );

    div()
        .id("player-bar")
        .h(px(PLAYER_BAR_HEIGHT))
        .w_full()
        .flex_shrink_0()
        .flex()
        .items_center()
        .px_4()
        .gap_2()
        .bg(palette.bar_bg)
        .border_t_1()
        .border_color(palette.border)
        .child(left)
        .child(center)
        .child(right)
        .into_any_element()
}

pub fn time_label(secs: f64, palette: BiuPalette) -> Div {
    div()
        .flex_shrink_0()
        .min_w(px(38.0))
        .text_xs()
        .text_color(palette.text_dim)
        .text_center()
        .child(format_duration(secs))
}

/// Volume slider with explicit values (avoids re-borrowing `App`).
pub fn volume_slider_with(
    id: impl Into<ElementId>,
    state: &Entity<AppState>,
    volume: f32,
    muted: bool,
    palette: BiuPalette,
) -> Slider {
    let value = if muted { 0.0 } else { volume };
    let st = state.clone();
    Slider::new(id, value, palette)
        .live(true)
        .fill(palette.text_muted)
        .on_change(move |ratio, _, _, cx| {
            st.update(cx, |s, cx| {
                if s.snapshot.muted && ratio > 0.0 {
                    s.toggle_mute(cx);
                }
                s.set_volume(ratio, cx);
            });
        })
}

pub fn rate_chip(
    id: impl Into<ElementId>,
    state: &Entity<AppState>,
    rate: f32,
    palette: BiuPalette,
) -> Stateful<Div> {
    let non_default = (rate - 1.0).abs() > 0.01;
    div()
        .id(id)
        .flex_shrink_0()
        .h(px(24.0))
        .min_w(px(34.0))
        .px_1p5()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .text_xs()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(if non_default {
            palette.accent
        } else {
            palette.text_muted
        })
        .cursor_pointer()
        .hover(move |s| s.bg(palette.surface_hover))
        .child(format_rate(rate))
        .on_click(on_state(state, |s, cx| {
            let next = next_playback_rate(s.snapshot.rate);
            s.set_rate(next, cx);
        }))
}

pub fn queue_button(
    id: impl Into<ElementId>,
    state: &Entity<AppState>,
    count: usize,
    palette: BiuPalette,
) -> Stateful<Div> {
    let badge = if count > 99 {
        "99+".to_string()
    } else {
        count.to_string()
    };
    icon_button(id, IconName::ListMusic, 15.0, palette, false)
        .tip("播放列表")
        .relative()
        .when(count > 0, |b| {
            b.child(
                div()
                    .absolute()
                    .top(px(1.0))
                    .right(px(0.0))
                    .min_w(px(15.0))
                    .h(px(13.0))
                    .px(px(3.0))
                    .rounded_full()
                    .bg(palette.accent)
                    .text_color(palette.accent_fg)
                    .text_size(px(9.0))
                    .font_weight(FontWeight::BOLD)
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(badge),
            )
        })
        .on_click(on_state(state, |s, cx| {
            if matches!(s.modal, ActiveModal::PlaylistDrawer) {
                s.close_modal(cx);
            } else {
                s.open_modal(ActiveModal::PlaylistDrawer, cx);
            }
        }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn ratio_clamps_to_track() {
        assert_eq!(ratio_from_position(50.0, 0.0, 100.0), 0.5);
        assert_eq!(ratio_from_position(-20.0, 0.0, 100.0), 0.0);
        assert_eq!(ratio_from_position(300.0, 0.0, 100.0), 1.0);
        assert_eq!(ratio_from_position(10.0, 0.0, 0.0), 0.0);
        assert_eq!(ratio_from_position(f32::NAN, 0.0, 10.0), 0.0);
        assert!((ratio_from_position(125.0, 100.0, 50.0) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn progress_ratio_handles_unknown_duration() {
        assert_eq!(progress_ratio(10.0, 0.0), 0.0);
        assert_eq!(progress_ratio(10.0, f64::NAN), 0.0);
        assert_eq!(progress_ratio(30.0, 60.0), 0.5);
        assert_eq!(progress_ratio(90.0, 60.0), 1.0);
    }

    #[test]
    fn playback_rate_cycles() {
        assert_eq!(next_playback_rate(0.75), 1.0);
        assert_eq!(next_playback_rate(1.0), 1.25);
        assert_eq!(next_playback_rate(2.0), 0.75);
        assert_eq!(next_playback_rate(1.7), 1.0);
        assert_eq!(format_rate(1.0), "1.0x");
        assert_eq!(format_rate(1.25), "1.25x");
        assert_eq!(format_rate(1.5), "1.5x");
        assert_eq!(format_rate(0.75), "0.75x");
    }
}
