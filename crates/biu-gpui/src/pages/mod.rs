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
use crate::state::{ActiveModal, AppState, Route, Services};
use crate::theme::BiuPalette;
use crate::widgets::{cover_box, empty_state, format_duration};
use biu_core::download::DownloadFormat;
use biu_core::image_cache::ThumbSize;
use biu_core::{Page, TrackItem, TrackSource};
use gpui_kit::component::input::{Input, InputEvent, InputState};
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
    data: PageData,
}

impl PagesHost {
    pub fn new(state: Entity<AppState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        cx.observe(&state, |_, _, cx| cx.notify()).detach();
        Self {
            state,
            route: None,
            account: None,
            epoch: 0,
            mutating: false,
            collecting: None,
            data: PageData::default(),
        }
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
        self.mutating = false;
        self.collecting = None;
        self.account = account;
        self.route = Some(route.clone());

        match route {
            Route::MusicRecommend => self.load_recommend(cx),
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
        let services = self.state.read(cx).services.clone();
        let slot_state = slot(self);
        slot_state.result = None;
        slot_state.task = Some(cx.spawn(async move |this, cx| {
            let runtime = services.rt.handle().clone();
            let result = run_on(&runtime, async move { fetch(services).await }).await;
            let _ = this.update(cx, |this, cx| {
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
                button("play-all", "播放全部", p)
                    .on_click(cx.listener(move |this, _, _, cx| collect(this, false, cx))),
            )
            .child(
                button("append-all", "加入队列", p)
                    .on_click(cx.listener(move |this, _, _, cx| collect(this, true, cx))),
            )
    }

    fn login_hint(&self, p: BiuPalette) -> Div {
        empty_state("请先登录", "登录后可以查看账号内容", p).child(state_button(
            &self.state,
            "login",
            "扫码登录",
            p,
            |s, cx| s.open_modal(ActiveModal::Login, cx),
        ))
    }
}

impl Render for PagesHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let state = self.state.read(cx);
        let p = state.palette(window);
        let route = state.route.clone();
        let account = state.user.as_ref().map(|user| user.mid);
        self.enter(route.clone(), account, window, cx);
        let content = match route {
            Route::MusicRecommend => self.recommend_view(p, cx),
            Route::Search { .. } => self.search_view(p, cx),
            Route::Collection { .. } => self.collection_view(p, cx),
            Route::Later => self.later_view(p, cx),
            Route::History => self.history_view(p, cx),
            Route::FollowList => self.follows_view(p, cx),
            Route::DynamicFeed => self.dynamics_view(p, cx),
            Route::UserProfile { .. } => self.profile_view(p, cx),
            Route::LocalMusic => self.local_view(p, cx),
            Route::DownloadList => self.downloads_view(p, cx),
            Route::Settings => self.settings_view(p, cx),
        };
        div().size_full().min_h_0().text_color(p.text).child(
            div()
                .id(("content-page", self.epoch as usize))
                .size_full()
                .overflow_y_scroll()
                .child(content),
        )
    }
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
            None => empty_state("加载中…", "", p),
            Some(Err(error)) => empty_state("加载失败", error.clone(), p).child(retry),
            Some(Ok(value)) => ready(value),
        }
    }
}

pub(crate) async fn run_on<T: Send + 'static>(
    runtime: &tokio::runtime::Handle,
    future: impl Future<Output = anyhow::Result<T>> + Send + 'static,
) -> Result<T, String> {
    // Dropping a route or modal also cancels its pending network requests.
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
    column().w_full().gap_4().p_6().child(
        div()
            .text_2xl()
            .font_weight(FontWeight::SEMIBOLD)
            .child(title.into()),
    )
}

pub(crate) fn button(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    p: BiuPalette,
) -> Stateful<Div> {
    div()
        .id(id)
        .flex_shrink_0()
        .rounded(p.radius)
        .border_1()
        .border_color(p.border)
        .bg(p.surface)
        .px_3()
        .py_1()
        .text_sm()
        .cursor_pointer()
        .hover(move |style| style.bg(p.surface_hover))
        .active(move |style| style.bg(p.surface_active))
        .child(label.into())
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
    let covers = &state.read(cx).covers;
    let image = covers.get(&key);
    let request = covers.should_fetch(&key);
    let root = cover_box(image, w, h, p.radius, p).relative();
    root.when(request, |root| {
        let state = state.clone();
        let url = url.to_owned();
        root.child(
            canvas(
                move |bounds, window, cx| {
                    if bounds.intersects(&window.content_mask().bounds) {
                        window.defer(cx, move |_, cx| {
                            state.update(cx, |state, cx| {
                                state.ensure_cover(&url, size, false, cx);
                            });
                        });
                    }
                },
                |_, (), _, _| {},
            )
            .absolute()
            .top_0()
            .left_0()
            .size_full(),
        )
    })
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
        bar.child(state_button(
            state,
            "play-all",
            "播放全部",
            p,
            move |s, cx| s.play_queue(play.clone(), 0, cx),
        ))
        .child(state_button(
            state,
            "append-all",
            "加入队列",
            p,
            move |s, cx| s.append_to_queue(append.clone(), cx),
        ))
    })
}

fn track_actions(state: &Entity<AppState>, track: &TrackItem, p: BiuPalette) -> Div {
    let play = track.clone();
    let append = track.clone();
    let fav = track.clone();
    let download = track.clone();
    toolbar()
        .child(state_button(state, "play", "播放", p, move |s, cx| {
            s.play_track(play.clone(), cx)
        }))
        .child(state_button(
            state,
            "append",
            "加入队列",
            p,
            move |s, cx| s.append_to_queue(vec![append.clone()], cx),
        ))
        .when(
            !matches!(&track.source, TrackSource::LocalFile { .. }),
            |bar| {
                bar.child(state_button(
                    state,
                    "favorite",
                    "收藏",
                    p,
                    move |s, cx| {
                        let modal = if s.user.is_some() {
                            ActiveModal::FavSelect { track: fav.clone() }
                        } else {
                            ActiveModal::Login
                        };
                        s.open_modal(modal, cx);
                    },
                ))
                .child(state_button(
                    state,
                    "download",
                    "下载",
                    p,
                    move |s, cx| {
                        // DownloadManager::queue requires an entered Tokio runtime.
                        let runtime = s.services.rt.handle().clone();
                        let _guard = runtime.enter();
                        s.queue_download(download.clone(), DownloadFormat::Native, cx);
                    },
                ))
            },
        )
}

fn track_row(
    state: &Entity<AppState>,
    key: usize,
    track: &TrackItem,
    p: BiuPalette,
    cx: &App,
) -> Stateful<Div> {
    div()
        .id(key)
        .flex()
        .items_center()
        .gap_3()
        .min_w_0()
        .p_2()
        .rounded(p.radius)
        .bg(p.surface)
        .child(thumbnail(
            state,
            &track.cover,
            ThumbSize::Tiny,
            (52., 44.),
            p,
            cx,
        ))
        .child(track_details(track, p))
        .child(track_actions(state, track, p))
}

fn track_details(track: &TrackItem, p: BiuPalette) -> Div {
    column()
        .flex_1()
        .gap_0()
        .overflow_hidden()
        .child(div().truncate().child(track.title.clone()))
        .child(
            div()
                .truncate()
                .text_sm()
                .text_color(p.text_muted)
                .child(format!(
                    "{}  ·  {}",
                    track.artist,
                    format_duration(track.duration_secs)
                )),
        )
}

fn track_list(state: &Entity<AppState>, tracks: &[TrackItem], p: BiuPalette, cx: &App) -> Div {
    if tracks.is_empty() {
        return empty_state("暂无歌曲", "", p);
    }
    column().children(
        tracks
            .iter()
            .enumerate()
            .map(|(i, track)| track_row(state, i, track, p, cx)),
    )
}

fn pager<T>(
    data: &Page<T>,
    page: u64,
    p: BiuPalette,
    go: impl Fn(u64, &mut App) + Clone + 'static,
) -> Div {
    let previous = go.clone();
    toolbar()
        .child(
            div()
                .text_sm()
                .text_color(p.text_muted)
                .child(match data.total {
                    Some(total) => format!("第 {page} 页 · 共 {total} 项"),
                    None => format!("第 {page} 页"),
                }),
        )
        .when(page > 1, |bar| {
            bar.child(
                button("prev-page", "上一页", p).on_click(move |_, _, cx| previous(page - 1, cx)),
            )
        })
        .when(page < u64::MAX && data.has_more == Some(true), |bar| {
            bar.child(button("next-page", "下一页", p).on_click(move |_, _, cx| go(page + 1, cx)))
        })
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
    toolbar()
        .child(div().w(px(280.)).child(Input::new(input)))
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
    toolbar()
        .flex_1()
        .flex_nowrap()
        .min_w_0()
        .child(thumbnail(
            state,
            &user.face,
            ThumbSize::Tiny,
            (44., 44.),
            p,
            cx,
        ))
        .child(
            column()
                .flex_1()
                .gap_0()
                .overflow_hidden()
                .child(div().truncate().child(user.name.clone()))
                .child(
                    div()
                        .truncate()
                        .text_sm()
                        .text_color(p.text_muted)
                        .child(user.sign.clone()),
                ),
        )
        .child(state_button(
            state,
            "profile",
            "查看主页",
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
