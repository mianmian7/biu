mod collection;
mod download_list;
mod dynamic_feed;
mod follow_list;
mod history;
mod later;
mod local_music;
mod music_recommend;
mod search;
mod settings;
mod user_profile;

use crate::image_loader::CoverKey;
use crate::player_bar::{TooltipExt, accent_play_circle, icon, is_playing};
use crate::state::{ActiveModal, AppState, Route, Services};
use crate::theme::BiuPalette;
use crate::widgets::{cover_box, empty_state, fluid_cover, format_duration};
use biu_core::download::DownloadFormat;
use biu_core::image_cache::ThumbSize;
use biu_core::{Page, TrackItem, TrackSource};
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::scroll::ScrollableElement;
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::future::Future;
use std::sync::Arc;

#[derive(Default)]
struct PageData {
    recommend: music_recommend::MusicRecommend,
    search: search::Search,
    collection: collection::Collection,
    later: later::Later,
    history: history::History,
    follows: follow_list::FollowList,
    dynamics: dynamic_feed::DynamicFeed,
    profile: user_profile::UserProfile,
    local: local_music::LocalMusic,
    downloads: download_list::DownloadList,
    settings: settings::Settings,
}

pub struct PagesHost {
    pub state: Entity<AppState>,
    route: Option<Route>,
    account: Option<u64>,
    epoch: u64,
    mutating: bool,
    collecting: Option<Task<()>>,
    scroll_handle: ScrollHandle,
    last_ui_ver: u64,
    last_covers_ver: u64,
    data: PageData,
}

impl PagesHost {
    pub fn new(state: Entity<AppState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (init_ui, init_covers) = {
            let s = state.read(cx);
            (s.ui_version, s.covers_version())
        };
        // Only repaint PagesHost when UI state or cover cache changes;
        // completely ignore 100ms playback progress ticks so scrolling stays 120 FPS!
        cx.observe(&state, |this, state, cx| {
            let s = state.read(cx);
            let ui_ver = s.ui_version;
            let covers_ver = s.covers_version();
            if this.last_ui_ver != ui_ver || this.last_covers_ver != covers_ver {
                this.last_ui_ver = ui_ver;
                this.last_covers_ver = covers_ver;
                cx.notify();
            }
        })
        .detach();
        Self {
            state,
            route: None,
            account: None,
            epoch: 0,
            mutating: false,
            collecting: None,
            scroll_handle: ScrollHandle::new(),
            last_ui_ver: init_ui,
            last_covers_ver: init_covers,
            data: PageData::default(),
        }
    }

    pub(super) fn prefetch_covers(
        &self,
        items: Vec<(String, ThumbSize)>,
        cx: &mut Context<Self>,
    ) {
        if items.is_empty() {
            return;
        }
        self.state
            .update(cx, |s, cx| s.prefetch_covers(items, cx));
    }

    pub(super) fn prefetch_tracks(&self, tracks: &[TrackItem], cx: &mut Context<Self>) {
        let size = if is_grid_mode(self.state.read(cx)) {
            ThumbSize::Card
        } else {
            ThumbSize::Tiny
        };
        let items = tracks
            .iter()
            .take(48)
            .filter(|t| !t.cover.is_empty())
            .map(|t| (t.cover.clone(), size))
            .collect();
        self.prefetch_covers(items, cx);
    }

    fn enter(
        &mut self,
        route: Route,
        account: Option<u64>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        if self.route.as_ref() == Some(&route)
            && (self.account == account
                || matches!(
                    route,
                    Route::Settings | Route::LocalMusic | Route::DownloadList
                ))
        {
            return;
        }
        let keep_search = matches!(
            (&self.route, &route),
            (Some(Route::Search { .. }), Route::Search { .. })
        );
        if !keep_search {
            self.data = PageData::default();
        }
        self.epoch = self.epoch.wrapping_add(1);
        self.scroll_handle = ScrollHandle::new();
        self.mutating = false;
        self.collecting = None;
        self.account = account;
        self.route = Some(route.clone());

        match route {
            Route::MusicRecommend => self.enter_recommend(cx),
            Route::Search { query } => self.enter_search(&query, window, cx),
            Route::Collection { .. } => self.enter_collection(window, cx),
            Route::Later => self.load_later(1, cx),
            Route::History => self.enter_history(window, cx),
            Route::FollowList => self.enter_follows(window, cx),
            Route::DynamicFeed => self.load_dynamics(false, cx),
            Route::UserProfile { mid, .. } => self.enter_profile(mid, window, cx),
            Route::LocalMusic => self.scan_local(cx),
            Route::DownloadList => self.load_downloads(cx),
            Route::Settings => self.enter_settings(window, cx),
        }
    }

    fn load<T, F, Fut>(
        &mut self,
        slot: fn(&mut Self) -> &mut Load<T>,
        fetch: F,
        cx: &mut Context<Self>,
    ) where
        T: Send + 'static,
        F: FnOnce(Arc<Services>) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        self.load_with(slot, fetch, |_, _, _| {}, cx);
    }

    fn load_with<T, F, Fut>(
        &mut self,
        slot: fn(&mut Self) -> &mut Load<T>,
        fetch: F,
        on_ready: fn(&mut Self, &T, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) where
        T: Send + 'static,
        F: FnOnce(Arc<Services>) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        let services = self.state.read(cx).services.clone();
        let slot_state = slot(self);
        slot_state.result = None;
        slot_state.task = Some(cx.spawn(async move |this, cx| {
            let runtime = services.rt.handle().clone();
            let result = run_on(&runtime, async move { fetch(services).await }).await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(ref value) = result {
                    on_ready(this, value, cx);
                }
                slot(this).result = Some(result);
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn mutate<F, Fut>(
        &mut self,
        fetch: F,
        success: &'static str,
        after: fn(&mut Self, &mut Context<Self>),
        cx: &mut Context<Self>,
    ) where
        F: FnOnce(Arc<Services>) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<()>> + Send + 'static,
    {
        if self.mutating {
            return;
        }
        self.mutating = true;
        cx.notify();
        let state = self.state.clone();
        let services = state.read(cx).services.clone();
        let epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            let runtime = services.rt.handle().clone();
            let result = run_on(&runtime, async move { fetch(services).await }).await;
            state.update(cx, |state, cx| match &result {
                Ok(()) => state.show_toast(success, false, cx),
                Err(error) => state.show_toast(error.clone(), true, cx),
            });
            let _ = this.update(cx, |this, cx| {
                if this.epoch == epoch {
                    this.mutating = false;
                    if result.is_ok() {
                        after(this, cx);
                    }
                    cx.notify();
                }
            });
        })
        .detach();
    }

    fn filter_input(
        &self,
        placeholder: &'static str,
        window: &mut Window,
        cx: &mut Context<Self>,
        submit: impl Fn(&mut Self, String, &mut Context<Self>) + 'static,
    ) -> Entity<InputState> {
        let input = new_input(placeholder, "", window, cx);
        cx.subscribe_in(
            &input,
            window,
            move |this, input, event: &InputEvent, _, cx| {
                if matches!(event, InputEvent::PressEnter { .. }) {
                    submit(this, normalize_query(&input.read(cx).value()), cx);
                }
            },
        )
        .detach();
        input
    }

    fn queue_all<F, Fut>(&mut self, fetch: F, append: bool, cx: &mut Context<Self>)
    where
        F: Fn(Arc<Services>, u64) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<Page<TrackItem>>> + Send + 'static,
    {
        if self.collecting.is_some() {
            return;
        }
        let services = self.state.read(cx).services.clone();
        self.collecting = Some(cx.spawn(async move |this, cx| {
            let runtime = services.rt.handle().clone();
            let result = run_on(&runtime, async move {
                let mut items = Vec::new();
                let mut seen = std::collections::HashSet::new();
                for page in 1..=u64::MAX {
                    let response = fetch(services.clone(), page).await?;
                    let count_before = items.len();
                    let had_tracks = !response.items.is_empty();
                    items.extend(
                        response
                            .items
                            .into_iter()
                            .filter(|track| seen.insert(track.id.clone())),
                    );
                    if response.has_more != Some(true)
                        || (had_tracks && items.len() == count_before)
                    {
                        break;
                    }
                }
                Ok(items)
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                this.collecting = None;
                this.state.update(cx, |state, cx| match result {
                    Ok(items) if items.is_empty() => {
                        state.show_toast("没有可播放的歌曲", false, cx)
                    }
                    Ok(items) if append => state.append_to_queue(items, cx),
                    Ok(items) => state.play_queue(items, 0, cx),
                    Err(error) => state.show_toast(error, true, cx),
                });
                cx.notify();
            });
        }));
        cx.notify();
    }

    fn paginated_actions(
        &self,
        data: &Page<TrackItem>,
        current: u64,
        p: BiuPalette,
        collect: fn(&mut Self, bool, &mut Context<Self>),
        cx: &Context<Self>,
    ) -> Div {
        if self.collecting.is_some() {
            return toolbar().child(
                div()
                    .text_sm()
                    .text_color(p.text_muted)
                    .child("正在获取全部歌曲…"),
            );
        }
        if current == 1 && data.has_more != Some(true) {
            return play_all(&self.state, &data.items, p);
        }
        toolbar()
            .child(
                primary_button("play-all", IconName::Play, "播放全部", p)
                    .on_click(cx.listener(move |this, _, _, cx| collect(this, false, cx))),
            )
            .child(
                button("append-all", "加入队列", p)
                    .on_click(cx.listener(move |this, _, _, cx| collect(this, true, cx))),
            )
    }

    fn login_hint(&self, p: BiuPalette) -> Div {
        empty_state("请先登录 B 站账号", "登录后即可同步收藏夹、关注与历史记录", p).child(
            div().mt_2().child(
                primary_button("login", IconName::User, "扫码登录", p).on_click({
                    let state = self.state.clone();
                    move |_, _, cx| state.update(cx, |s, cx| s.open_modal(ActiveModal::Login, cx))
                }),
            ),
        )
    }
}

impl Render for PagesHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (p, route, account, sidebar_collapsed) = {
            let state = self.state.read(cx);
            (
                state.palette(window),
                state.route.clone(),
                state.user.as_ref().map(|user| user.mid),
                state.sidebar_collapsed,
            )
        };
        let viewport_w = f32::from(window.viewport_size().width);
        let sidebar_w = if sidebar_collapsed {
            crate::shell::SIDEBAR_COLLAPSED
        } else {
            crate::shell::SIDEBAR_EXPANDED
        };
        let content_width = (viewport_w - sidebar_w - 56.0).max(480.0);

        self.enter(route.clone(), account, window, cx);
        let content = match route {
            Route::MusicRecommend => self.recommend_view(p, content_width, cx),
            Route::Search { .. } => self.search_view(p, content_width, cx),
            Route::Collection { .. } => self.collection_view(p, content_width, cx),
            Route::Later => self.later_view(p, content_width, cx),
            Route::History => self.history_view(p, cx),
            Route::FollowList => self.follows_view(p, cx),
            Route::DynamicFeed => self.dynamics_view(p, content_width, cx),
            Route::UserProfile { .. } => self.profile_view(p, content_width, cx),
            Route::LocalMusic => self.local_view(p, cx),
            Route::DownloadList => self.downloads_view(p, cx),
            Route::Settings => self.settings_view(p, cx),
        };

        div().relative().size_full().min_h_0().text_color(p.text).child(
            div()
                .id(("content-page", self.epoch as usize))
                .size_full()
                .overflow_y_scroll()
                .track_scroll(&self.scroll_handle)
                .child(content)
                .vertical_scrollbar(&self.scroll_handle),
        )
    }
}

pub(super) fn grid_columns_for_width(content_width: f32) -> usize {
    if content_width >= 1180.0 {
        5
    } else if content_width >= 880.0 {
        4
    } else if content_width >= 620.0 {
        3
    } else {
        2
    }
}

pub(super) fn is_grid_mode(state: &AppState) -> bool {
    let mode = state.settings.display_mode.as_str();
    mode == "grid" || mode == "card"
}

struct Load<T> {
    result: Option<Result<T, String>>,
    task: Option<Task<()>>,
}

impl<T> Default for Load<T> {
    fn default() -> Self {
        Self {
            result: None,
            task: None,
        }
    }
}

impl<T> Load<T> {
    fn content(
        &self,
        p: BiuPalette,
        retry: impl IntoElement,
        ready: impl FnOnce(&T) -> Div,
    ) -> Div {
        match &self.result {
            None => loading_state(p),
            Some(Err(error)) => empty_state("加载失败", error.clone(), p).child(retry),
            Some(Ok(value)) => ready(value),
        }
    }
}

fn loading_state(p: BiuPalette) -> Div {
    div()
        .w_full()
        .py_16()
        .flex()
        .flex_col()
        .items_center()
        .justify_center()
        .gap_3()
        .child(icon(IconName::LoaderCircle, 24.0, p.accent))
        .child(
            div()
                .text_sm()
                .text_color(p.text_muted)
                .child("正在加载内容…"),
        )
}

pub(crate) async fn run_on<T: Send + 'static>(
    runtime: &tokio::runtime::Handle,
    future: impl Future<Output = anyhow::Result<T>> + Send + 'static,
) -> Result<T, String> {
    struct Abort(tokio::task::AbortHandle);
    impl Drop for Abort {
        fn drop(&mut self) {
            self.0.abort();
        }
    }

    let task = runtime.spawn(future);
    let _abort = Abort(task.abort_handle());
    task.await
        .map_err(|error| error.to_string())?
        .map_err(|error| error.to_string())
}

pub(crate) fn new_input(
    placeholder: impl Into<SharedString>,
    value: impl Into<SharedString>,
    window: &mut Window,
    cx: &mut App,
) -> Entity<InputState> {
    let placeholder = placeholder.into();
    let value = value.into();
    cx.new(|cx| {
        InputState::new(window, cx)
            .placeholder(placeholder)
            .default_value(value)
    })
}

pub(crate) fn normalize_query(query: &str) -> String {
    query.trim().to_owned()
}

pub(crate) fn column() -> Div {
    div().flex().flex_col().gap_2().min_w_0()
}

pub(crate) fn toolbar() -> Div {
    div().flex().flex_wrap().items_center().gap_2()
}

fn page(title: impl Into<SharedString>) -> Div {
    let title: SharedString = title.into();
    let has_title = !title.is_empty();
    column()
        .w_full()
        .gap_5()
        .px_7()
        .pt_2()
        .pb_8()
        .when(has_title, |d| {
            d.child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child(title),
            )
        })
}

pub(crate) fn primary_button(
    id: impl Into<ElementId>,
    ico: IconName,
    label: impl Into<SharedString>,
    p: BiuPalette,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(34.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_1p5()
        .rounded_full()
        .bg(p.accent)
        .px_4()
        .text_sm()
        .font_weight(FontWeight::SEMIBOLD)
        .text_color(p.accent_fg)
        .cursor_pointer()
        .hover(|s| s.opacity(0.90))
        .active(|s| s.opacity(0.80))
        .child(icon(ico, 15.0, p.accent_fg))
        .child(label.into())
}

pub(crate) fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    p: BiuPalette,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(34.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .justify_center()
        .rounded_full()
        .bg(p.surface)
        .px_3p5()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.text)
        .cursor_pointer()
        .hover(move |style| style.bg(p.surface_hover))
        .active(move |style| style.bg(p.surface_active))
        .child(label.into())
}

pub(crate) fn icon_pill_button(
    id: impl Into<ElementId>,
    ico: IconName,
    label: impl Into<SharedString>,
    p: BiuPalette,
) -> Stateful<Div> {
    div()
        .id(id)
        .h(px(34.0))
        .flex_shrink_0()
        .flex()
        .items_center()
        .gap_1p5()
        .rounded_full()
        .bg(p.surface)
        .px_3p5()
        .text_sm()
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.text)
        .cursor_pointer()
        .hover(move |style| style.bg(p.surface_hover))
        .active(move |style| style.bg(p.surface_active))
        .child(icon(ico, 14.0, p.text_muted))
        .child(label.into())
}

pub(crate) fn compact_icon_btn(
    id: impl Into<ElementId>,
    ico: IconName,
    tip: &'static str,
    p: BiuPalette,
) -> Stateful<Div> {
    div()
        .id(id)
        .size(px(28.0))
        .flex_shrink_0()
        .rounded_full()
        .flex()
        .items_center()
        .justify_center()
        .cursor_pointer()
        .hover(move |s| s.bg(p.surface_active))
        .tip(tip)
        .child(icon(ico, 14.0, p.text_muted))
}

pub(crate) fn state_button(
    state: &Entity<AppState>,
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    p: BiuPalette,
    f: impl Fn(&mut AppState, &mut Context<AppState>) + 'static,
) -> Stateful<Div> {
    let state = state.clone();
    button(id, label, p).on_click(move |_, _, cx| {
        cx.stop_propagation();
        state.update(cx, &f);
    })
}

/// View-mode toggle pill (Grid / List) matching Biu's modern chrome.
pub(super) fn display_mode_toggle(
    state: &Entity<AppState>,
    grid: bool,
    p: BiuPalette,
    on_switch: impl Fn(&mut PagesHost, bool, &mut Context<PagesHost>) + Clone + 'static,
    cx: &Context<PagesHost>,
) -> Div {
    let st1 = state.clone();
    let st2 = state.clone();
    let cb1 = on_switch.clone();
    let cb2 = on_switch;
    div()
        .flex()
        .items_center()
        .p(px(3.0))
        .rounded_full()
        .bg(p.surface)
        .child(
            div()
                .id("mode-card")
                .size(px(28.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(grid, |d| d.bg(p.surface_active))
                .when(!grid, |d| d.hover(move |s| s.bg(p.surface_hover)))
                .tip("卡片网格")
                .child(icon(
                    IconName::LayoutGrid,
                    14.0,
                    if grid { p.accent } else { p.text_muted },
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    st1.update(cx, |s, cx| {
                        s.update_settings(|st| st.display_mode = "card".into(), cx)
                    });
                    cb1(this, true, cx);
                })),
        )
        .child(
            div()
                .id("mode-list")
                .size(px(28.0))
                .rounded_full()
                .flex()
                .items_center()
                .justify_center()
                .cursor_pointer()
                .when(!grid, |d| d.bg(p.surface_active))
                .when(grid, |d| d.hover(move |s| s.bg(p.surface_hover)))
                .tip("精致列表")
                .child(icon(
                    IconName::List,
                    14.0,
                    if !grid { p.accent } else { p.text_muted },
                ))
                .on_click(cx.listener(move |this, _, _, cx| {
                    st2.update(cx, |s, cx| {
                        s.update_settings(|st| st.display_mode = "list".into(), cx)
                    });
                    cb2(this, false, cx);
                })),
        )
}

/// Pure read-only cover box with zero per-frame canvas overhead during scroll.
pub(crate) fn thumbnail(
    state: &Entity<AppState>,
    url: &str,
    size: ThumbSize,
    dimensions: (f32, f32),
    p: BiuPalette,
    cx: &App,
) -> Div {
    let (w, h) = (px(dimensions.0), px(dimensions.1));
    let key = CoverKey::thumb(url, size);
    let image = state.read(cx).covers.get(&key);
    cover_box(image, w, h, p.radius, p)
}

pub(crate) fn playable_id(track: &TrackItem) -> Option<(u64, u32)> {
    match &track.source {
        TrackSource::BiliVideo { aid, .. } if *aid > 0 => Some((*aid, 2)),
        TrackSource::BiliAudio { sid } if *sid > 0 => Some((*sid, 12)),
        _ => None,
    }
}

fn play_all(state: &Entity<AppState>, tracks: &[TrackItem], p: BiuPalette) -> Div {
    toolbar().when(!tracks.is_empty(), |bar| {
        let play = tracks.to_vec();
        let append = play.clone();
        let st1 = state.clone();
        let st2 = state.clone();
        bar.child(
            primary_button("play-all", IconName::Play, "全部播放", p).on_click(move |_, _, cx| {
                st1.update(cx, |s, cx| s.play_queue(play.clone(), 0, cx));
            }),
        )
        .child(
            icon_pill_button("append-all", IconName::Plus, "加入队列", p).on_click(
                move |_, _, cx| {
                    st2.update(cx, |s, cx| s.append_to_queue(append.clone(), cx));
                },
            ),
        )
    })
}

fn quick_track_icons(
    state: &Entity<AppState>,
    track: &TrackItem,
    id_prefix: &'static str,
    idx: usize,
    p: BiuPalette,
) -> Div {
    let append = track.clone();
    let fav = track.clone();
    let download = track.clone();
    let is_local = matches!(&track.source, TrackSource::LocalFile { .. });
    let st_add = state.clone();
    let st_fav = state.clone();
    let st_dl = state.clone();

    div()
        .flex()
        .items_center()
        .gap_0p5()
        .child(
            compact_icon_btn((id_prefix, idx * 4), IconName::Plus, "加入播放队列", p).on_click(
                move |_, _, cx| {
                    cx.stop_propagation();
                    st_add.update(cx, |s, cx| s.append_to_queue(vec![append.clone()], cx));
                },
            ),
        )
        .when(!is_local, |bar| {
            bar.child(
                compact_icon_btn((id_prefix, idx * 4 + 1), IconName::Star, "收藏", p).on_click(
                    move |_, _, cx| {
                        cx.stop_propagation();
                        st_fav.update(cx, |s, cx| {
                            let modal = if s.user.is_some() {
                                ActiveModal::FavSelect { track: fav.clone() }
                            } else {
                                ActiveModal::Login
                            };
                            s.open_modal(modal, cx);
                        });
                    },
                ),
            )
            .child(
                compact_icon_btn((id_prefix, idx * 4 + 2), IconName::Download, "下载音频", p)
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        st_dl.update(cx, |s, cx| {
                            let runtime = s.services.rt.handle().clone();
                            let _guard = runtime.enter();
                            s.queue_download(download.clone(), DownloadFormat::Native, cx);
                        });
                    }),
            )
        })
}

/// High-craft responsive grid of album/video cards matching `screenshots/home.png`.
pub(super) fn track_grid(
    state: &Entity<AppState>,
    tracks: &[TrackItem],
    content_width: f32,
    p: BiuPalette,
    cx: &App,
) -> Div {
    if tracks.is_empty() {
        return empty_state("暂无歌曲", "", p);
    }
    let cols = grid_columns_for_width(content_width);
    let card_cover_h = if cols >= 5 {
        136.0
    } else if cols == 4 {
        152.0
    } else {
        164.0
    };
    let s = state.read(cx);
    let current_id = s.current_track().map(|t| t.id.as_str());

    let mut grid = div().flex().flex_col().gap_4().w_full();
    for (row_idx, chunk) in tracks.chunks(cols).enumerate() {
        let mut row = div().flex().gap_4().w_full();
        for (col_idx, track) in chunk.iter().enumerate() {
            let idx = row_idx * cols + col_idx;
            let active = current_id == Some(track.id.as_str());
            row = row.child(track_card(
                state,
                idx,
                track,
                card_cover_h,
                active,
                p,
                cx,
            ));
        }
        // Pad last row so cards keep exact column width.
        for _ in chunk.len()..cols {
            row = row.child(div().flex_1().min_w_0());
        }
        grid = grid.child(row);
    }
    grid
}

fn track_card(
    state: &Entity<AppState>,
    idx: usize,
    track: &TrackItem,
    cover_h: f32,
    active: bool,
    p: BiuPalette,
    cx: &App,
) -> Stateful<Div> {
    let group: SharedString = format!("tcard-{idx}").into();
    let cover_img = state
        .read(cx)
        .covers
        .get(&CoverKey::thumb(track.cover.clone(), ThumbSize::Card));
    let play_track = track.clone();
    let st_play = state.clone();
    let duration_text = (track.duration_secs > 0.0).then(|| format_duration(track.duration_secs));
    let artist_mid = track.artist_mid;
    let artist_name = track.artist.clone();
    let st_artist = state.clone();

    let mut artist_el = div()
        .id(("card-artist", idx))
        .flex_1()
        .min_w_0()
        .text_xs()
        .text_color(p.text_muted)
        .truncate()
        .child(if track.artist.is_empty() {
            "未知 UP 主".to_string()
        } else {
            track.artist.clone()
        });
    if let Some(mid) = artist_mid {
        artist_el = artist_el
            .cursor_pointer()
            .hover(move |s| s.text_color(p.text))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                st_artist.update(cx, |s, cx| {
                    s.navigate(
                        Route::UserProfile {
                            mid,
                            name: artist_name.clone(),
                        },
                        cx,
                    )
                });
            });
    }

    div()
        .id(("track-card", idx))
        .group(group.clone())
        .flex_1()
        .min_w_0()
        .flex()
        .flex_col()
        .rounded(px(12.0))
        .overflow_hidden()
        .bg(if active { p.surface_elevated } else { p.surface })
        .cursor_pointer()
        .hover(move |s| s.bg(p.surface_hover))
        .on_click(move |_, _, cx| {
            st_play.update(cx, |s, cx| s.play_track(play_track.clone(), cx));
        })
        .child(
            div()
                .relative()
                .w_full()
                .h(px(cover_h))
                .overflow_hidden()
                .child(fluid_cover(cover_img, px(cover_h), px(0.0), p))
                .when_some(duration_text, |d, dur| {
                    d.child(
                        div()
                            .absolute()
                            .left_0()
                            .right_0()
                            .bottom_0()
                            .h(px(36.0))
                            .px_2p5()
                            .pb_1p5()
                            .flex()
                            .items_end()
                            .justify_start()
                            .bg(linear_gradient(
                                180.0,
                                linear_color_stop(hsla(0.0, 0.0, 0.0, 0.0), 0.0),
                                linear_color_stop(hsla(0.0, 0.0, 0.0, 0.78), 1.0),
                            ))
                            .child(
                                div()
                                    .px_1p5()
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .bg(hsla(0.0, 0.0, 0.0, 0.45))
                                    .text_size(px(11.0))
                                    .font_weight(FontWeight::MEDIUM)
                                    .text_color(hsla(0.0, 0.0, 1.0, 0.94))
                                    .child(dur),
                            ),
                    )
                })
                .child(
                    div()
                        .absolute()
                        .right(px(10.0))
                        .bottom(px(10.0))
                        .when(!active, |d| d.invisible())
                        .group_hover(group.clone(), |s| s.visible())
                        .child(accent_play_circle(38.0, p)),
                ),
        )
        .child(
            div()
                .px_3()
                .pt_2p5()
                .pb_2p5()
                .flex()
                .flex_col()
                .gap_1p5()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(if active { p.accent } else { p.text })
                        .truncate()
                        .child(track.title.clone()),
                )
                .child(
                    div()
                        .h(px(24.0))
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_1()
                        .child(artist_el)
                        .child(
                            div()
                                .invisible()
                                .group_hover(group, |s| s.visible())
                                .child(quick_track_icons(state, track, "cact", idx, p)),
                        ),
                ),
        )
}

fn track_list_header(p: BiuPalette) -> Div {
    div()
        .w_full()
        .h(px(32.0))
        .px_3()
        .mb_1()
        .flex()
        .items_center()
        .gap_3()
        .border_b_1()
        .border_color(p.border)
        .text_xs()
        .font_weight(FontWeight::MEDIUM)
        .text_color(p.text_dim)
        .child(div().w(px(28.0)).text_center().child("#"))
        .child(div().flex_1().min_w_0().child("歌曲 / 视频"))
        .child(div().w(px(56.0)).text_right().child("时长"))
        .child(div().w(px(104.0)).text_right().child("操作"))
}

pub(super) fn track_row(
    state: &Entity<AppState>,
    key: usize,
    track: &TrackItem,
    p: BiuPalette,
    cx: &App,
) -> Stateful<Div> {
    let s = state.read(cx);
    let active = s.current_track().is_some_and(|t| t.id == track.id);
    let playing = active && is_playing(s.snapshot.state);
    let group: SharedString = format!("trow-{key}").into();
    let play_track = track.clone();
    let st_play = state.clone();
    let st_artist = state.clone();
    let artist_mid = track.artist_mid;
    let artist_name = track.artist.clone();

    let mut artist_el = div()
        .id(("row-artist", key))
        .text_xs()
        .text_color(p.text_muted)
        .truncate()
        .child(if track.artist.is_empty() {
            "未知 UP 主".to_string()
        } else {
            track.artist.clone()
        });
    if let Some(mid) = artist_mid {
        artist_el = artist_el
            .cursor_pointer()
            .hover(move |s| s.text_color(p.text))
            .on_click(move |_, _, cx| {
                cx.stop_propagation();
                st_artist.update(cx, |s, cx| {
                    s.navigate(
                        Route::UserProfile {
                            mid,
                            name: artist_name.clone(),
                        },
                        cx,
                    )
                });
            });
    }

    div()
        .id(("track-row", key))
        .group(group.clone())
        .h(px(60.0))
        .w_full()
        .flex()
        .items_center()
        .gap_3()
        .px_3()
        .rounded(px(10.0))
        .cursor_pointer()
        .when(active, |d| d.bg(p.accent_soft))
        .when(!active, |d| d.hover(move |s| s.bg(p.surface_hover)))
        .on_click(move |_, _, cx| {
            st_play.update(cx, |s, cx| s.play_track(play_track.clone(), cx));
        })
        .child(
            div()
                .w(px(28.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_center()
                .text_xs()
                .text_color(if active { p.accent } else { p.text_dim })
                .child(if playing {
                    icon(IconName::AudioLines, 14.0, p.accent).into_any_element()
                } else {
                    format!("{}", key + 1).into_any_element()
                }),
        )
        .child(
            div()
                .relative()
                .size(px(44.0))
                .flex_shrink_0()
                .child(thumbnail(
                    state,
                    &track.cover,
                    ThumbSize::Tiny,
                    (44., 44.),
                    p,
                    cx,
                ))
                .child(
                    div()
                        .absolute()
                        .inset_0()
                        .rounded(p.radius)
                        .bg(hsla(0.0, 0.0, 0.0, 0.48))
                        .flex()
                        .items_center()
                        .justify_center()
                        .invisible()
                        .group_hover(group.clone(), |s| s.visible())
                        .child(icon(IconName::Play, 15.0, gpui_kit::white())),
                ),
        )
        .child(
            div()
                .flex_1()
                .min_w_0()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(
                    div()
                        .text_sm()
                        .font_weight(if active {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::MEDIUM
                        })
                        .text_color(if active { p.accent } else { p.text })
                        .truncate()
                        .child(track.title.clone()),
                )
                .child(artist_el),
        )
        .child(
            div()
                .w(px(56.0))
                .flex_shrink_0()
                .text_right()
                .text_xs()
                .text_color(p.text_dim)
                .child(if track.duration_secs > 0.0 {
                    format_duration(track.duration_secs)
                } else {
                    "--:--".into()
                }),
        )
        .child(
            div()
                .w(px(96.0))
                .flex_shrink_0()
                .flex()
                .items_center()
                .justify_end()
                .child(quick_track_icons(state, track, "ract", key, p)),
        )
}

pub(super) fn track_list(
    state: &Entity<AppState>,
    tracks: &[TrackItem],
    p: BiuPalette,
    cx: &App,
) -> Div {
    if tracks.is_empty() {
        return empty_state("暂无歌曲", "", p);
    }
    div()
        .w_full()
        .flex()
        .flex_col()
        .gap(px(2.0))
        .child(track_list_header(p))
        .children(
            tracks
                .iter()
                .enumerate()
                .map(|(i, track)| track_row(state, i, track, p, cx)),
        )
}

/// Renders tracks in either responsive card grid or clean table list depending on user preference.
pub(super) fn track_collection(
    state: &Entity<AppState>,
    tracks: &[TrackItem],
    content_width: f32,
    p: BiuPalette,
    cx: &App,
) -> Div {
    if is_grid_mode(state.read(cx)) {
        track_grid(state, tracks, content_width, p, cx)
    } else {
        track_list(state, tracks, p, cx)
    }
}

fn pager<T>(
    data: &Page<T>,
    page: u64,
    p: BiuPalette,
    go: impl Fn(u64, &mut App) + Clone + 'static,
) -> Div {
    let previous = go.clone();
    div()
        .w_full()
        .pt_3()
        .flex()
        .items_center()
        .justify_between()
        .child(
            div()
                .text_xs()
                .text_color(p.text_dim)
                .child(match data.total {
                    Some(total) => format!("第 {page} 页 · 共 {total} 项"),
                    None => format!("第 {page} 页"),
                }),
        )
        .child(
            toolbar()
                .when(page > 1, |bar| {
                    bar.child(
                        icon_pill_button("prev-page", IconName::ChevronLeft, "上一页", p)
                            .on_click(move |_, _, cx| previous(page - 1, cx)),
                    )
                })
                .when(page < u64::MAX && data.has_more == Some(true), |bar| {
                    bar.child(
                        icon_pill_button("next-page", IconName::ChevronRight, "下一页", p)
                            .on_click(move |_, _, cx| go(page + 1, cx)),
                    )
                }),
        )
}

fn page_change(
    cx: &Context<PagesHost>,
    change: fn(&mut PagesHost, u64, &mut Context<PagesHost>),
) -> impl Fn(u64, &mut App) + Clone + 'static {
    let host = cx.entity().downgrade();
    move |page, cx| {
        let _ = host.update(cx, |host, cx| change(host, page, cx));
    }
}

fn filter_bar<T: 'static>(
    input: &Entity<InputState>,
    label: &'static str,
    p: BiuPalette,
    cx: &Context<T>,
    submit: impl Fn(&mut T, String, &mut Context<T>) + 'static,
) -> Div {
    let value = input.clone();
    div()
        .flex()
        .items_center()
        .gap_2()
        .child(
            div()
                .w(px(260.0))
                .h(px(34.0))
                .px_3()
                .rounded_full()
                .bg(p.surface)
                .flex()
                .items_center()
                .gap_2()
                .child(icon(IconName::Search, 14.0, p.text_dim))
                .child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .text_sm()
                        .child(Input::new(input).appearance(false).cleanable(true)),
                ),
        )
        .child(
            button("submit-filter", label, p).on_click(cx.listener(move |this, _, _, cx| {
                submit(this, normalize_query(&value.read(cx).value()), cx);
            })),
        )
}

fn user_row(
    state: &Entity<AppState>,
    user: &biu_core::FollowingUser,
    p: BiuPalette,
    cx: &App,
) -> Div {
    let route = Route::UserProfile {
        mid: user.mid,
        name: user.name.clone(),
    };
    div()
        .flex_1()
        .min_w_0()
        .flex()
        .items_center()
        .gap_3()
        .child(thumbnail(
            state,
            &user.face,
            ThumbSize::Tiny,
            (44., 44.),
            BiuPalette {
                radius: px(22.0),
                ..p
            },
            cx,
        ))
        .child(
            column()
                .flex_1()
                .gap(px(2.0))
                .overflow_hidden()
                .child(
                    div()
                        .text_sm()
                        .font_weight(FontWeight::SEMIBOLD)
                        .truncate()
                        .child(user.name.clone()),
                )
                .child(
                    div()
                        .truncate()
                        .text_xs()
                        .text_color(p.text_muted)
                        .child(if user.sign.is_empty() {
                            "这个人很懒，什么都没有写".into()
                        } else {
                            user.sign.clone()
                        }),
                ),
        )
        .child(state_button(
            state,
            "profile",
            "主页",
            p,
            move |s, cx| s.navigate(route.clone(), cx),
        ))
}

#[cfg(test)]
mod tests {
    #[test]
    fn normalizes_search_without_changing_inner_spacing() {
        assert_eq!(super::normalize_query("  林  宥嘉\n"), "林  宥嘉");
        assert!(super::normalize_query(" \t").is_empty());
    }
}
