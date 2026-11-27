//! Main window shell: full-height branded sidebar (matching original Biu),
//! right-pane top navbar with back/forward + pill search + isolated TopLyrics
//! entity, scrollable PagesHost, isolated PlayerBarView, and overlays.

use crate::BIU_LOGO_SVG;
use crate::desktop_lyrics::open_or_toggle_desktop_lyrics;
use crate::full_screen::FullScreenPlayer;
use crate::image_loader::CoverKey;
use crate::mini_player::open_or_toggle_mini_player;
use crate::modals::ModalsHost;
use crate::pages::PagesHost;
use crate::player_bar::{PlayerBarView, TooltipExt, icon, icon_button, is_playing, on_state};
use crate::state::{ActiveModal, AppState, CollectionKind, Route};
use crate::system;
use crate::theme::BiuPalette;
use biu_core::FavFolder;
use biu_core::image_cache::ThumbSize;
use biu_core::lyrics::active_line_index;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::component::theme::{Theme, ThemeMode};
use gpui_kit::prelude::*;
use gpui_kit::*;

pub const SIDEBAR_EXPANDED: f32 = 220.0;
pub const SIDEBAR_COLLAPSED: f32 = 70.0;
pub const HEADER_HEIGHT: f32 = 58.0;

/// Bilibili `type` for collected UGC seasons (合集) in the collected-folders list.
const BILI_FOLDER_TYPE_SEASON: u64 = 21;

#[derive(Clone, Copy)]
struct NavItem {
    label: &'static str,
    icon: IconName,
    key: &'static str,
}

const NAV_ITEMS: [NavItem; 9] = [
    NavItem {
        label: "推荐音乐",
        icon: IconName::Disc3,
        key: "music-recommend",
    },
    NavItem {
        label: "搜索发现",
        icon: IconName::Search,
        key: "search",
    },
    NavItem {
        label: "我的关注",
        icon: IconName::Users,
        key: "follow-list",
    },
    NavItem {
        label: "关注动态",
        icon: IconName::Rss,
        key: "dynamic-feed",
    },
    NavItem {
        label: "稍后再看",
        icon: IconName::Clock,
        key: "later",
    },
    NavItem {
        label: "历史记录",
        icon: IconName::RotateCcwClock,
        key: "history",
    },
    NavItem {
        label: "本地音乐",
        icon: IconName::HardDrive,
        key: "local-music",
    },
    NavItem {
        label: "下载记录",
        icon: IconName::Download,
        key: "download-list",
    },
    NavItem {
        label: "设置",
        icon: IconName::Settings,
        key: "settings",
    },
];

fn nav_route(key: &str, search_query: &str) -> Route {
    match key {
        "search" => Route::Search {
            query: search_query.to_string(),
        },
        "later" => Route::Later,
        "history" => Route::History,
        "follow-list" => Route::FollowList,
        "dynamic-feed" => Route::DynamicFeed,
        "local-music" => Route::LocalMusic,
        "download-list" => Route::DownloadList,
        "settings" => Route::Settings,
        _ => Route::MusicRecommend,
    }
}

fn nav_active(key: &str, route: &Route) -> bool {
    matches!(
        (key, route),
        ("music-recommend", Route::MusicRecommend)
            | ("search", Route::Search { .. })
            | ("later", Route::Later)
            | ("history", Route::History)
            | ("follow-list", Route::FollowList)
            | ("dynamic-feed", Route::DynamicFeed)
            | ("local-music", Route::LocalMusic)
            | ("download-list", Route::DownloadList)
            | ("settings", Route::Settings)
    )
}

/// `system → dark → light → system` theme cycle.
pub fn next_theme_mode(mode: &str) -> &'static str {
    match mode {
        "dark" => "light",
        "light" => "system",
        _ => "dark",
    }
}

fn theme_mode_icon(mode: &str) -> IconName {
    match mode {
        "dark" => IconName::Moon,
        "light" => IconName::Sun,
        _ => IconName::SunMoon,
    }
}

fn folder_route(folder: &FavFolder) -> Route {
    let kind = if folder.folder_type == BILI_FOLDER_TYPE_SEASON {
        CollectionKind::Season
    } else {
        CollectionKind::FavFolder
    };
    Route::Collection {
        kind,
        id: folder.id,
        mid: folder.mid,
        title: folder.title.clone(),
    }
}

/// What `Escape` should do given the overlay stack (modal first, then full screen).
#[derive(Debug, PartialEq, Eq)]
pub enum EscapeAction {
    CloseModal,
    ExitFullScreen,
    CloseUserMenu,
    None,
}

pub fn escape_action(modal_open: bool, full_screen: bool, user_menu_open: bool) -> EscapeAction {
    if modal_open {
        EscapeAction::CloseModal
    } else if user_menu_open {
        EscapeAction::CloseUserMenu
    } else if full_screen {
        EscapeAction::ExitFullScreen
    } else {
        EscapeAction::None
    }
}

// ---------------------------------------------------------------------------
// Isolated TopLyricsView (subscribes to playback ticks independently)
// ---------------------------------------------------------------------------

struct TopLyricsView {
    state: Entity<AppState>,
    last_playback_ver: u64,
    _observe: Subscription,
}

impl TopLyricsView {
    fn new(state: Entity<AppState>, cx: &mut Context<Self>) -> Self {
        let observe = cx.observe(&state, |this, state, cx| {
            let s = state.read(cx);
            if this.last_playback_ver != s.playback_version {
                this.last_playback_ver = s.playback_version;
                cx.notify();
            }
        });
        let ver = state.read(cx).playback_version;
        Self {
            state,
            last_playback_ver: ver,
            _observe: observe,
        }
    }
}

impl Render for TopLyricsView {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let s = self.state.read(cx);
        let p = s.palette(window);
        let playing = is_playing(s.snapshot.state);
        let ticker = if s.settings.show_top_lyrics && playing && !s.is_full_screen {
            active_line_index(
                &s.lyrics.lines,
                (s.snapshot.position_secs + s.lyrics_offset_secs).max(0.0),
            )
                .and_then(|i| s.lyrics.lines.get(i))
                .filter(|l| !l.text.trim().is_empty())
                .map(|l| {
                    let tr = if s.settings.show_top_lyrics_translation {
                        l.translation.clone().filter(|t| !t.trim().is_empty())
                    } else {
                        None
                    };
                    (l.text.clone(), tr)
                })
        } else {
            None
        };

        div()
            .id("header-drag")
            .flex_1()
            .min_w_0()
            .h_full()
            .flex()
            .items_center()
            .justify_center()
            .on_mouse_down(MouseButton::Left, |ev, window, _| {
                if ev.click_count >= 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
            .when_some(ticker, |d, (text, tr)| {
                d.child(
                    div()
                        .id("top-lyrics")
                        .occlude()
                        .max_w(px(460.0))
                        .px_3()
                        .py_1()
                        .rounded_full()
                        .flex()
                        .flex_col()
                        .items_center()
                        .cursor_pointer()
                        .hover(move |s| s.bg(p.surface_hover))
                        .on_click(on_state(&self.state, |s, cx| s.toggle_full_screen(cx)))
                        .child(
                            div()
                                .max_w_full()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(p.accent)
                                .truncate()
                                .child(text),
                        )
                        .when_some(tr, |d, t| {
                            d.child(
                                div()
                                    .max_w_full()
                                    .text_size(px(11.0))
                                    .text_color(p.text_dim)
                                    .truncate()
                                    .child(t),
                            )
                        }),
                )
            })
    }
}

// ---------------------------------------------------------------------------
// MainShell (ignores 100ms playback ticks completely!)
// ---------------------------------------------------------------------------

pub struct MainShell {
    pub state: Entity<AppState>,
    pub pages: Entity<PagesHost>,
    pub modals: Entity<ModalsHost>,
    player_bar: Entity<PlayerBarView>,
    top_lyrics: Entity<TopLyricsView>,
    full_screen: Entity<FullScreenPlayer>,
    search: Entity<InputState>,
    search_open: bool,
    search_suggestions: Vec<String>,
    search_suggest_task: Option<Task<()>>,
    search_active_idx: Option<usize>,
    syncing_search: bool,
    last_synced_search_query: String,
    focus: FocusHandle,
    user_menu_open: bool,
    last_ui_ver: u64,
    last_avatar_loaded: bool,
    _subscriptions: Vec<Subscription>,
}

impl MainShell {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pages = cx.new(|cx| PagesHost::new(state.clone(), window, cx));
        let modals = cx.new(|cx| ModalsHost::new(state.clone(), window, cx));
        let player_bar = cx.new(|cx| PlayerBarView::new(state.clone(), cx));
        let top_lyrics = cx.new(|cx| TopLyricsView::new(state.clone(), cx));
        let full_screen = cx.new(|cx| FullScreenPlayer::new(state.clone(), window, cx));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索歌曲、视频、UP 主"));
        let focus = cx.focus_handle();

        let mut subs = Vec::new();
        subs.push(cx.observe_in(&state, window, |this, state, window, cx| {
            let (ui_ver, avatar_loaded, route_query) = {
                let s = state.read(cx);
                let avatar_loaded = s
                    .user
                    .as_ref()
                    .filter(|u| !u.face.is_empty())
                    .is_some_and(|u| {
                        s.covers
                            .get(&CoverKey::thumb(u.face.clone(), ThumbSize::Avatar))
                            .is_some()
                    });
                let route_query = match &s.route {
                    Route::Search { query } => Some(query.clone()),
                    _ => None,
                };
                (s.ui_version, avatar_loaded, route_query)
            };
            if this.last_ui_ver != ui_ver || this.last_avatar_loaded != avatar_loaded {
                this.last_ui_ver = ui_ver;
                this.last_avatar_loaded = avatar_loaded;
                if let Some(q) = route_query
                    && q != this.last_synced_search_query
                {
                    this.last_synced_search_query = q.clone();
                    if this.search.read(cx).value().as_ref() != q {
                        this.syncing_search = true;
                        this.search.update(cx, |input, cx| {
                            input.set_value(q, window, cx);
                        });
                        this.syncing_search = false;
                    }
                    this.search_open = false;
                }
                this.sync_component_theme(window, cx);
                cx.notify();
            }
        }));
        subs.push(cx.observe_window_appearance(window, |this, window, cx| {
            this.sync_component_theme(window, cx);
            cx.notify();
        }));
        subs.push(
            cx.subscribe_in(&search, window, |this, input, ev: &InputEvent, window, cx| {
                let raw = input.read(cx).value().trim().to_string();
                match ev {
                    InputEvent::Focus => {
                        this.user_menu_open = false;
                        this.search_open = true;
                        if !raw.is_empty() && this.search_suggestions.is_empty() {
                            this.fetch_search_suggestions(raw, cx);
                        }
                        cx.notify();
                    }
                    InputEvent::Change => {
                        if this.syncing_search {
                            return;
                        }
                        this.search_open = true;
                        this.search_active_idx = None;
                        this.fetch_search_suggestions(raw, cx);
                        cx.notify();
                    }
                    InputEvent::PressEnter { .. } => {
                        let chosen = this
                            .search_active_idx
                            .and_then(|i| this.search_suggestions.get(i).cloned())
                            .unwrap_or(raw);
                        this.submit_top_search(chosen, window, cx);
                    }
                    InputEvent::Blur => {}
                }
            }),
        );

        let st = state.clone();
        window.on_window_should_close(cx, move |window, cx| {
            if system::is_quitting() {
                return true;
            }
            let (option, has_mini) = {
                let s = st.read(cx);
                (
                    s.settings.close_window_option.clone(),
                    s.windows.mini_player.is_some(),
                )
            };
            if option == "minimize"
                || (option == "hide" && !system::can_restore_from_background(has_mini))
            {
                window.minimize_window();
                return false;
            }
            st.update(cx, |s, cx| {
                s.windows.main = None;
                s.clear_covers(cx);
                if s.is_full_screen {
                    s.is_full_screen = false;
                    s.services.audio.set_waveform_enabled(false);
                }
                s.notify_ui(cx);
            });
            if option != "hide" {
                cx.defer(system::request_quit);
            }
            true
        });

        window.focus(&focus, cx);

        let initial_ver = state.read(cx).ui_version;
        let mut this = Self {
            state,
            pages,
            modals,
            player_bar,
            top_lyrics,
            full_screen,
            search,
            search_open: false,
            search_suggestions: Vec::new(),
            search_suggest_task: None,
            search_active_idx: None,
            syncing_search: false,
            last_synced_search_query: String::new(),
            focus,
            user_menu_open: false,
            last_ui_ver: initial_ver,
            last_avatar_loaded: false,
            _subscriptions: subs,
        };
        this.sync_component_theme(window, cx);
        this
    }

    fn submit_top_search(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        let query = query.trim().to_string();
        if query.is_empty() {
            return;
        }
        self.search_open = false;
        self.search_active_idx = None;
        self.last_synced_search_query = query.clone();
        if self.search.read(cx).value().as_ref() != query {
            self.syncing_search = true;
            self.search.update(cx, |input, cx| {
                input.set_value(query.clone(), window, cx);
            });
            self.syncing_search = false;
        }
        window.focus(&self.focus, cx);
        self.state
            .update(cx, |s, cx| s.navigate(Route::Search { query }, cx));
        cx.notify();
    }

    fn fetch_search_suggestions(&mut self, term: String, cx: &mut Context<Self>) {
        if term.is_empty() {
            self.search_suggestions.clear();
            self.search_active_idx = None;
            self.search_suggest_task = None;
            return;
        }
        let services = self.state.read(cx).services.clone();
        self.search_suggest_task = Some(cx.spawn(async move |this, cx| {
            let runtime = services.rt.handle().clone();
            let res = crate::pages::run_on(&runtime, async move {
                tokio::time::sleep(std::time::Duration::from_millis(220)).await;
                services.bili.search_suggest(&term).await
            })
            .await;
            let _ = this.update(cx, |this, cx| {
                if let Ok(items) = res {
                    this.search_suggestions = items;
                    this.search_active_idx = None;
                    cx.notify();
                }
            });
        }));
    }

    fn sync_component_theme(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let dark = self.state.read(cx).palette(window).dark;
        let current = cx.try_global::<Theme>().map(|t| t.is_dark());
        if current != Some(dark) {
            Theme::change(
                if dark {
                    ThemeMode::Dark
                } else {
                    ThemeMode::Light
                },
                Some(window),
                cx,
            );
        }
    }

    fn on_key_down(&mut self, ev: &KeyDownEvent, window: &mut Window, cx: &mut Context<Self>) {
        let key = ev.keystroke.key.as_str();
        let mods = &ev.keystroke.modifiers;
        let text_focused = window.focused(cx).is_some_and(|f| f != self.focus);

        if self.search_open && !mods.modified() {
            let count = self.search_suggestions.len();
            if key == "down" && count > 0 {
                self.search_active_idx = Some(match self.search_active_idx {
                    Some(i) => (i + 1) % count,
                    None => 0,
                });
                cx.notify();
                cx.stop_propagation();
                return;
            }
            if key == "up" && count > 0 {
                self.search_active_idx = Some(match self.search_active_idx {
                    Some(0) | None => count - 1,
                    Some(i) => i - 1,
                });
                cx.notify();
                cx.stop_propagation();
                return;
            }
        }

        if key == "escape" {
            let s = self.state.read(cx);
            let modal_open = !matches!(s.modal, ActiveModal::None);
            let action = escape_action(modal_open, s.is_full_screen, self.user_menu_open);
            if !modal_open && self.search_open {
                self.search_open = false;
                self.search_active_idx = None;
                window.focus(&self.focus, cx);
                cx.notify();
                cx.stop_propagation();
                return;
            }
            match action {
                EscapeAction::CloseModal => self.state.update(cx, |s, cx| s.close_modal(cx)),
                EscapeAction::CloseUserMenu => {
                    self.user_menu_open = false;
                    cx.notify();
                }
                EscapeAction::ExitFullScreen => {
                    self.state.update(cx, |s, cx| s.toggle_full_screen(cx))
                }
                EscapeAction::None => {
                    if text_focused {
                        window.focus(&self.focus, cx);
                    }
                    return;
                }
            }
            cx.stop_propagation();
            return;
        }

        let primary = mods.secondary(cx) && !mods.alt && !mods.shift;
        if primary {
            let handled = match key {
                "p" => {
                    self.state.update(cx, |s, cx| s.toggle_play(cx));
                    true
                }
                "m" => {
                    let st = self.state.clone();
                    cx.defer(move |cx| open_or_toggle_mini_player(st, cx));
                    true
                }
                "left" if !text_focused => {
                    self.state.update(cx, |s, cx| s.prev_track(cx));
                    true
                }
                "right" if !text_focused => {
                    self.state.update(cx, |s, cx| s.next_track(false, cx));
                    true
                }
                "up" if !text_focused => {
                    self.state.update(cx, |s, cx| s.adjust_volume(0.05, cx));
                    true
                }
                "down" if !text_focused => {
                    self.state.update(cx, |s, cx| s.adjust_volume(-0.05, cx));
                    true
                }
                _ => false,
            };
            if handled {
                cx.stop_propagation();
            }
            return;
        }

        if key == "space" && !text_focused && !mods.modified() {
            self.state.update(cx, |s, cx| s.toggle_play(cx));
            cx.stop_propagation();
        }
    }

    fn logout(&mut self, cx: &mut Context<Self>) {
        self.user_menu_open = false;
        let services = self.state.read(cx).services.clone();
        let state = self.state.clone();
        cx.spawn(async move |_, cx: &mut AsyncApp| {
            let res = services
                .rt
                .spawn({
                    let bili = services.bili.clone();
                    async move { bili.logout().await }
                })
                .await;
            cx.update(|cx| {
                state.update(cx, |s, cx| match res {
                    Ok(Ok(())) => {
                        s.user = None;
                        s.created_folders.clear();
                        s.collected_folders.clear();
                        if matches!(
                            s.route,
                            Route::UserProfile { .. } | Route::Collection { .. }
                        ) {
                            s.navigate(Route::MusicRecommend, cx);
                        }
                        s.show_toast("已退出登录", false, cx);
                    }
                    Ok(Err(e)) => s.show_toast(format!("退出登录失败: {e}"), true, cx),
                    Err(e) => s.show_toast(format!("退出登录任务异常: {e}"), true, cx),
                })
            });
        })
        .detach();
        cx.notify();
    }

    // -- Full-height branded sidebar (matches screenshots/home.png) --------

    fn render_sidebar(&self, p: BiuPalette, cx: &mut Context<Self>) -> AnyElement {
        let s = self.state.read(cx);
        let mac = cfg!(target_os = "macos");
        let collapsed = s.sidebar_collapsed;
        let route = s.route.clone();
        let logged_in = s.user.is_some();
        let created = s.created_folders.clone();
        let collected = s.collected_folders.clone();
        let folded = s.settings.side_menu_collection_folded.clone();
        let hidden = s.settings.hidden_menu_keys.clone();
        let current_query = match &route {
            Route::Search { query } => query.clone(),
            _ => String::new(),
        };
        let width = if collapsed {
            SIDEBAR_COLLAPSED
        } else {
            SIDEBAR_EXPANDED
        };

        let brand = div()
            .id("sidebar-brand")
            .flex_shrink_0()
            .pt(px(if mac { 38.0 } else { 14.0 }))
            .pb_3()
            .flex()
            .items_center()
            .when(collapsed, |d| d.justify_center())
            .when(!collapsed, |d| d.px_5().gap_2p5())
            .on_mouse_down(MouseButton::Left, |ev, window, _| {
                if ev.click_count >= 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
            .child(
                svg()
                    .path(BIU_LOGO_SVG)
                    .size(px(30.0))
                    .flex_shrink_0()
                    .text_color(p.accent),
            )
            .when(!collapsed, |d| {
                d.child(
                    div()
                        .text_2xl()
                        .font_weight(FontWeight::BOLD)
                        .text_color(p.accent)
                        .child("Biu"),
                )
            });

        let nav = NAV_ITEMS
            .iter()
            .filter(|item| !hidden.iter().any(|h| h == item.key))
            .map(|item| {
                let active = nav_active(item.key, &route);
                let target = nav_route(item.key, &current_query);
                let label = item.label;
                div()
                    .id(item.key)
                    .h(px(38.0))
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded_full()
                    .cursor_pointer()
                    .when(collapsed, |d| d.justify_center().tip(label))
                    .when(!collapsed, |d| d.px_3p5())
                    .when(active, |d| d.bg(p.surface_elevated))
                    .when(!active, |d| d.hover(move |s| s.bg(p.surface_hover)))
                    .child(icon(
                        item.icon,
                        17.0,
                        if active { p.accent } else { p.text_muted },
                    ))
                    .when(!collapsed, |d| {
                        d.child(
                            div()
                                .text_sm()
                                .font_weight(if active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .text_color(if active { p.accent } else { p.text })
                                .child(label),
                        )
                    })
                    .on_click(on_state(&self.state, move |s, cx| {
                        s.navigate(target.clone(), cx)
                    }))
            })
            .collect::<Vec<_>>();

        let folder_section = |id: &'static str,
                              title: &'static str,
                              folders: Vec<FavFolder>,
                              is_folded: bool,
                              with_add: bool| {
            let header = div()
                .id(id)
                .h(px(30.0))
                .mt_4()
                .mb_1()
                .px_3p5()
                .flex()
                .items_center()
                .justify_between()
                .rounded_full()
                .cursor_pointer()
                .hover(move |s| s.bg(p.surface_hover))
                .on_click(on_state(&self.state, move |s, cx| {
                    s.update_settings(
                        |st| {
                            if id == "sec-created" {
                                st.side_menu_collection_folded.created = !is_folded;
                            } else {
                                st.side_menu_collection_folded.collected = !is_folded;
                            }
                        },
                        cx,
                    )
                }))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(p.text_dim)
                                .child(title),
                        )
                        .child(icon(
                            if is_folded {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            },
                            12.0,
                            p.text_dim,
                        )),
                )
                .when(with_add, |d| {
                    d.child(
                        div()
                            .id(("sec-add", 0usize))
                            .size(px(22.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded_full()
                            .hover(move |s| s.bg(p.surface_active))
                            .tip("新建收藏夹")
                            .on_click(on_state(&self.state, |s, cx| {
                                cx.stop_propagation();
                                s.open_modal(ActiveModal::FavFolderEdit { folder: None }, cx)
                            }))
                            .child(icon(IconName::Plus, 14.0, p.text_muted)),
                    )
                });

            let rows = if is_folded {
                Vec::new()
            } else {
                folders
                    .into_iter()
                    .enumerate()
                    .map(|(i, f)| {
                        let r = folder_route(&f);
                        let active = route == r;
                        div()
                            .id((id, i))
                            .h(px(34.0))
                            .px_3p5()
                            .flex()
                            .items_center()
                            .gap_2p5()
                            .rounded_full()
                            .cursor_pointer()
                            .when(active, |d| d.bg(p.surface_elevated))
                            .when(!active, |d| d.hover(move |s| s.bg(p.surface_hover)))
                            .child(icon(
                                if with_add {
                                    IconName::Folder
                                } else {
                                    IconName::Star
                                },
                                15.0,
                                if active { p.accent } else { p.text_dim },
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .truncate()
                                    .font_weight(if active {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .text_color(if active { p.accent } else { p.text_muted })
                                    .child(f.title.clone()),
                            )
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.text_dim)
                                    .child(f.media_count.to_string()),
                            )
                            .on_click(on_state(&self.state, move |s, cx| {
                                s.navigate(r.clone(), cx)
                            }))
                    })
                    .collect::<Vec<_>>()
            };
            div().flex().flex_col().gap_0p5().child(header).children(rows)
        };

        let collapse_bar = div()
            .id("sidebar-toggle")
            .h(px(34.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_center()
            .border_t_1()
            .border_color(p.border)
            .cursor_pointer()
            .hover(move |s| s.bg(p.surface_hover))
            .tip(if collapsed {
                "展开侧边栏"
            } else {
                "收起侧边栏"
            })
            .on_click(on_state(&self.state, |s, cx| s.toggle_sidebar(cx)))
            .child(icon(
                if collapsed {
                    IconName::ChevronsRight
                } else {
                    IconName::ChevronsLeft
                },
                15.0,
                p.text_dim,
            ));

        div()
            .w(px(width))
            .h_full()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .bg(p.sidebar_bg)
            .border_r_1()
            .border_color(p.border)
            .child(brand)
            .child(
                div()
                    .id("sidebar-scroll")
                    .flex_1()
                    .min_h_0()
                    .overflow_y_scroll()
                    .px_3()
                    .pb_4()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .children(nav)
                    .when(logged_in && !collapsed, |d| {
                        d.child(folder_section(
                            "sec-created",
                            "我创建的",
                            created,
                            folded.created,
                            true,
                        ))
                        .child(folder_section(
                            "sec-collected",
                            "我收藏的",
                            collected,
                            folded.collected,
                            false,
                        ))
                    }),
            )
            .child(collapse_bar)
            .into_any_element()
    }

    // -- Right-pane top navbar (matches screenshots/home.png) --------------

    fn render_header(
        &self,
        p: BiuPalette,
        avatar: Option<std::sync::Arc<RenderImage>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = self.state.read(cx);
        let theme_mode = s.settings.theme_mode.clone();
        let lyrics_open = s.windows.desktop_lyrics.is_some();
        let mini_open = s.windows.mini_player.is_some();
        let user = s.user.clone();
        let can_back = s.can_go_back();
        let can_forward = s.can_go_forward();

        let nav_circle = |id: &'static str, name: IconName, enabled: bool| {
            div()
                .id(id)
                .size(px(32.0))
                .flex_shrink_0()
                .rounded_full()
                .bg(p.surface)
                .flex()
                .items_center()
                .justify_center()
                .when(enabled, |d| {
                    d.cursor_pointer()
                        .hover(move |s| s.bg(p.surface_hover))
                        .active(move |s| s.bg(p.surface_active))
                })
                .when(!enabled, |d| d.opacity(0.35))
                .child(icon(name, 16.0, p.text))
        };

        let search = div()
            .id("top-search-box")
            .occlude()
            .w(px(290.0))
            .h(px(36.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .px_3p5()
            .rounded_full()
            .bg(p.surface)
            .border_1()
            .border_color(if self.search_open {
                p.accent.opacity(0.65)
            } else {
                gpui_kit::transparent_black()
            })
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, _, _, cx| {
                    this.user_menu_open = false;
                    if !this.search_open {
                        this.search_open = true;
                        let raw = this.search.read(cx).value().trim().to_string();
                        if !raw.is_empty() && this.search_suggestions.is_empty() {
                            this.fetch_search_suggestions(raw, cx);
                        }
                        cx.notify();
                    }
                }),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .child(Input::new(&self.search).appearance(false).cleanable(true)),
            )
            .child(
                div()
                    .id("top-search-submit")
                    .size(px(24.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .cursor_pointer()
                    .hover(move |s| s.bg(p.surface_hover))
                    .child(icon(IconName::Search, 15.0, p.text_dim))
                    .on_click(cx.listener(|this, _, window, cx| {
                        let q = this.search.read(cx).value().trim().to_string();
                        if q.is_empty() {
                            this.search_open = true;
                            cx.notify();
                        } else {
                            this.submit_top_search(q, window, cx);
                        }
                    })),
            );

        let left = div()
            .occlude()
            .flex()
            .items_center()
            .gap_2()
            .child(
                nav_circle("nav-back", IconName::ChevronLeft, can_back)
                    .tip("返回")
                    .on_click(on_state(&self.state, |s, cx| s.go_back(cx))),
            )
            .child(
                nav_circle("nav-forward", IconName::ChevronRight, can_forward)
                    .tip("前进")
                    .on_click(on_state(&self.state, |s, cx| s.go_forward(cx))),
            )
            .child(search);

        let user_button = match &user {
            Some(u) => {
                let name = u.name.clone();
                div()
                    .id("user-button")
                    .h(px(34.0))
                    .pl_1()
                    .pr_3()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_full()
                    .bg(p.surface)
                    .cursor_pointer()
                    .hover(move |s| s.bg(p.surface_hover))
                    .child(crate::widgets::cover_box(
                        avatar,
                        px(26.0),
                        px(26.0),
                        px(13.0),
                        p,
                    ))
                    .child(
                        div()
                            .max_w(px(100.0))
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.text)
                            .truncate()
                            .child(name),
                    )
                    .child(icon(IconName::ChevronDown, 12.0, p.text_dim))
                    .on_click(cx.listener(|this, _, _, cx| {
                        this.user_menu_open = !this.user_menu_open;
                        cx.notify();
                    }))
            }
            None => div()
                .id("login-button")
                .h(px(34.0))
                .px_4()
                .flex()
                .items_center()
                .gap_1p5()
                .rounded_full()
                .bg(p.accent)
                .text_color(p.accent_fg)
                .text_sm()
                .font_weight(FontWeight::SEMIBOLD)
                .cursor_pointer()
                .hover(|s| s.opacity(0.90))
                .child(icon(IconName::User, 14.0, p.accent_fg))
                .child("登录")
                .on_click(on_state(&self.state, |s, cx| {
                    s.open_modal(ActiveModal::Login, cx)
                })),
        };

        let right = div()
            .occlude()
            .flex()
            .flex_shrink_0()
            .items_center()
            .gap_1()
            .child(
                icon_button("theme-cycle", theme_mode_icon(&theme_mode), 16.0, p, false)
                    .tip(match theme_mode.as_str() {
                        "dark" => "深色模式",
                        "light" => "浅色模式",
                        _ => "跟随系统",
                    })
                    .on_click(on_state(&self.state, |s, cx| {
                        let next = next_theme_mode(&s.settings.theme_mode).to_string();
                        s.update_settings(move |st| st.theme_mode = next, cx);
                    })),
            )
            .child(
                icon_button(
                    "mini-toggle",
                    IconName::PictureInPicture2,
                    16.0,
                    p,
                    mini_open,
                )
                .tip("Mini 播放器")
                .on_click({
                    let st = self.state.clone();
                    move |_, _, cx| {
                        let st = st.clone();
                        cx.defer(move |cx| open_or_toggle_mini_player(st, cx));
                    }
                }),
            )
            .child(
                icon_button("lyrics-toggle", IconName::Captions, 16.0, p, lyrics_open)
                    .tip("桌面歌词")
                    .on_click({
                        let st = self.state.clone();
                        move |_, _, cx| {
                            let st = st.clone();
                            cx.defer(move |cx| open_or_toggle_desktop_lyrics(st, cx));
                        }
                    }),
            )
            .child(div().w(px(6.0)))
            .child(user_button);

        div()
            .id("header")
            .h(px(HEADER_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_3()
            .px_6()
            .bg(p.bg)
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, ev: &MouseDownEvent, window, cx| {
                    if this.search_open {
                        this.search_open = false;
                        this.search_active_idx = None;
                        cx.notify();
                    }
                    if ev.click_count >= 2 {
                        window.titlebar_double_click();
                    } else {
                        window.start_window_move();
                    }
                }),
            )
            .child(left)
            .child(self.top_lyrics.clone())
            .child(right)
            .into_any_element()
    }

    fn render_search_dropdown(
        &self,
        p: BiuPalette,
        cx: &mut Context<Self>,
    ) -> Option<AnyElement> {
        if !self.search_open {
            return None;
        }
        let s = self.state.read(cx);
        let sidebar_w = if s.sidebar_collapsed {
            SIDEBAR_COLLAPSED
        } else {
            SIDEBAR_EXPANDED
        };
        let show_history = s.settings.show_search_history;
        let history: Vec<String> = s.search_history.iter().take(10).cloned().collect();
        let query = self.search.read(cx).value().trim().to_string();
        let suggestions = self.search_suggestions.clone();
        let active_idx = self.search_active_idx;
        let has_history = show_history && !history.is_empty();
        let left_offset = sidebar_w + 24.0 + 72.0;

        let mut panel = div()
            .id("top-search-dropdown")
            .absolute()
            .top(px(HEADER_HEIGHT - 4.0))
            .left(px(left_offset))
            .w(px(360.0))
            .max_h(px(460.0))
            .overflow_y_scroll()
            .p_3()
            .rounded(px(14.0))
            .bg(p.surface_elevated)
            .border_1()
            .border_color(p.border_strong)
            .shadow_xl()
            .occlude()
            .flex()
            .flex_col()
            .gap_2p5();

        if has_history {
            panel = panel
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .px_1()
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.text_dim)
                                .child("搜索历史"),
                        )
                        .child(
                            div()
                                .id("top-clear-history")
                                .text_xs()
                                .text_color(p.text_dim)
                                .cursor_pointer()
                                .hover(move |s| s.text_color(p.text))
                                .child("清除全部")
                                .on_click(on_state(&self.state, |s, cx| {
                                    s.clear_search_history(cx)
                                })),
                        ),
                )
                .child(
                    div()
                        .flex()
                        .flex_wrap()
                        .gap_1p5()
                        .children(history.into_iter().enumerate().map(|(i, term)| {
                            let search_term = term.clone();
                            let del_term = term.clone();
                            let st_del = self.state.clone();
                            div()
                                .id(("top-hist-chip", i))
                                .h(px(26.0))
                                .pl_2p5()
                                .pr_1p5()
                                .max_w(px(160.0))
                                .rounded_full()
                                .bg(p.surface)
                                .flex()
                                .items_center()
                                .gap_1()
                                .cursor_pointer()
                                .hover(move |s| s.bg(p.surface_hover))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.submit_top_search(search_term.clone(), window, cx);
                                }))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_xs()
                                        .text_color(p.text_muted)
                                        .truncate()
                                        .child(term),
                                )
                                .child(
                                    div()
                                        .id(("top-hist-del", i))
                                        .size(px(16.0))
                                        .flex_shrink_0()
                                        .rounded_full()
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .hover(move |s| s.bg(p.surface_active))
                                        .on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            st_del.update(cx, |s, cx| {
                                                s.remove_search_history(&del_term, cx);
                                            });
                                        })
                                        .child(icon(IconName::X, 10.0, p.text_dim)),
                                )
                        })),
                );
        }

        if !suggestions.is_empty() {
            if has_history {
                panel = panel.child(div().h(px(1.0)).w_full().bg(p.border));
            }
            panel = panel
                .child(
                    div()
                        .px_1()
                        .text_xs()
                        .font_weight(FontWeight::SEMIBOLD)
                        .text_color(p.text_dim)
                        .child("搜索建议"),
                )
                .child(
                    div()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .children(suggestions.into_iter().enumerate().map(|(i, term)| {
                            let active = active_idx == Some(i);
                            let chosen = term.clone();
                            div()
                                .id(("top-suggest-item", i))
                                .h(px(34.0))
                                .px_2p5()
                                .rounded(px(8.0))
                                .flex()
                                .items_center()
                                .gap_2()
                                .cursor_pointer()
                                .when(active, |d| d.bg(p.accent_soft))
                                .when(!active, |d| d.hover(move |s| s.bg(p.surface_hover)))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.submit_top_search(chosen.clone(), window, cx);
                                }))
                                .child(icon(
                                    IconName::Search,
                                    13.0,
                                    if active { p.accent } else { p.text_dim },
                                ))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_sm()
                                        .text_color(if active { p.accent } else { p.text })
                                        .truncate()
                                        .child(term),
                                )
                        })),
                );
        } else if !query.is_empty() {
            if has_history {
                panel = panel.child(div().h(px(1.0)).w_full().bg(p.border));
            }
            let direct = query.clone();
            panel = panel.child(
                div()
                    .id("top-suggest-direct")
                    .h(px(36.0))
                    .px_2p5()
                    .rounded(px(8.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .cursor_pointer()
                    .hover(move |s| s.bg(p.surface_hover))
                    .on_click(cx.listener(move |this, _, window, cx| {
                        this.submit_top_search(direct.clone(), window, cx);
                    }))
                    .child(icon(IconName::Search, 14.0, p.accent))
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .text_color(p.text)
                            .truncate()
                            .child(format!("搜索「{query}」")),
                    ),
            );
        } else if !has_history {
            panel = panel.child(
                div()
                    .py_5()
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_xs()
                    .text_color(p.text_dim)
                    .child("输入歌名、歌手、BV 号或 UP 主开始搜索"),
            );
        }

        Some(
            div()
                .absolute()
                .inset_0()
                .child(
                    div()
                        .id("top-search-scrim")
                        .absolute()
                        .top(px(HEADER_HEIGHT))
                        .left_0()
                        .right_0()
                        .bottom_0()
                        .occlude()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(|this, _, _, cx| {
                                this.search_open = false;
                                this.search_active_idx = None;
                                cx.notify();
                            }),
                        ),
                )
                .child(panel)
                .into_any_element(),
        )
    }

    fn render_user_menu(&self, p: BiuPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.user_menu_open {
            return None;
        }
        let user = self.state.read(cx).user.clone()?;
        let item = |id: &'static str, name: IconName, label: &'static str, danger: bool| {
            div()
                .id(id)
                .h(px(36.0))
                .px_3()
                .flex()
                .items_center()
                .gap_2p5()
                .rounded(px(8.0))
                .cursor_pointer()
                .text_sm()
                .text_color(if danger { p.danger } else { p.text })
                .hover(move |s| s.bg(p.surface_hover))
                .child(icon(
                    name,
                    15.0,
                    if danger { p.danger } else { p.text_muted },
                ))
                .child(label)
        };
        let mid = user.mid;
        let uname = user.name.clone();
        Some(
            div()
                .absolute()
                .inset_0()
                .child(
                    div()
                        .id("user-menu-scrim")
                        .absolute()
                        .inset_0()
                        .occlude()
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.user_menu_open = false;
                            cx.notify();
                        })),
                )
                .child(
                    div()
                        .absolute()
                        .top(px(HEADER_HEIGHT - 4.0))
                        .right(px(20.0))
                        .w(px(184.0))
                        .p_1p5()
                        .rounded(px(12.0))
                        .bg(p.surface_elevated)
                        .border_1()
                        .border_color(p.border_strong)
                        .shadow_xl()
                        .occlude()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            item("menu-profile", IconName::UserRound, "个人主页", false).on_click(
                                cx.listener(move |this, _, _, cx| {
                                    this.user_menu_open = false;
                                    let name = uname.clone();
                                    this.state.update(cx, |s, cx| {
                                        s.navigate(Route::UserProfile { mid, name }, cx)
                                    });
                                }),
                            ),
                        )
                        .child(
                            item("menu-logout", IconName::LogOut, "退出登录", true)
                                .on_click(cx.listener(|this, _, _, cx| this.logout(cx))),
                        ),
                )
                .into_any_element(),
        )
    }

    fn render_toast(&self, p: BiuPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        let toast = self.state.read(cx).toast.clone()?;
        let (fg, ring) = if toast.is_error {
            (p.danger, p.danger.opacity(0.4))
        } else {
            (p.accent, p.accent.opacity(0.4))
        };
        Some(
            div()
                .absolute()
                .top(px(HEADER_HEIGHT + 8.0))
                .left_0()
                .right_0()
                .flex()
                .justify_center()
                .child(
                    div()
                        .id("toast")
                        .max_w(px(520.0))
                        .px_4()
                        .py_2()
                        .flex()
                        .items_center()
                        .gap_2()
                        .rounded_full()
                        .bg(p.surface_elevated)
                        .border_1()
                        .border_color(ring)
                        .shadow_xl()
                        .text_sm()
                        .text_color(p.text)
                        .cursor_pointer()
                        .on_click(on_state(&self.state, |s, cx| {
                            s.toast = None;
                            s.notify_ui(cx);
                        }))
                        .child(icon(
                            if toast.is_error {
                                IconName::CircleAlert
                            } else {
                                IconName::CircleCheck
                            },
                            15.0,
                            fg,
                        ))
                        .child(div().truncate().child(toast.text)),
                )
                .into_any_element(),
        )
    }
}

impl Render for MainShell {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let (p, avatar, missing_avatar, full_screen, modal_open) = {
            let s = self.state.read(cx);
            let mut missing = None;
            let avatar = s
                .user
                .as_ref()
                .filter(|u| !u.face.is_empty())
                .and_then(|u| {
                    let key = CoverKey::thumb(u.face.clone(), ThumbSize::Avatar);
                    let img = s.covers.get(&key);
                    if img.is_none() && s.covers.should_fetch(&key) {
                        missing = Some(u.face.clone());
                    }
                    img
                });
            (
                s.palette(window),
                avatar,
                missing,
                s.is_full_screen,
                !matches!(s.modal, ActiveModal::None),
            )
        };
        if let Some(url) = missing_avatar {
            let st = self.state.clone();
            window.defer(cx, move |_, cx| {
                st.update(cx, |s, cx| {
                    s.ensure_cover(&url, ThumbSize::Avatar, false, cx);
                });
            });
        }

        let sidebar = self.render_sidebar(p, cx);
        let header = self.render_header(p, avatar, cx);
        let search_dropdown = self.render_search_dropdown(p, cx);
        let user_menu = self.render_user_menu(p, cx);
        let toast = self.render_toast(p, cx);

        div()
            .id("main-shell")
            .track_focus(&self.focus)
            .key_context("BiuShell")
            .on_key_down(cx.listener(Self::on_key_down))
            .relative()
            .size_full()
            .flex()
            .flex_col()
            .bg(p.bg)
            .text_color(p.text)
            .child(
                div()
                    .flex_1()
                    .min_h_0()
                    .flex()
                    .child(sidebar)
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .min_h_0()
                            .flex()
                            .flex_col()
                            .bg(p.bg)
                            .child(header)
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .min_h_0()
                                    .overflow_hidden()
                                    .child(self.pages.clone()),
                            ),
                    ),
            )
            .child(self.player_bar.clone())
            .when_some(search_dropdown, |d, sd| d.child(sd))
            .when_some(user_menu, |d, m| d.child(m))
            .when(full_screen, |d| d.child(self.full_screen.clone()))
            .when(modal_open, |d| {
                d.child(div().absolute().inset_0().child(self.modals.clone()))
            })
            .when_some(toast, |d, t| d.child(t))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use core::prelude::v1::test;

    #[test]
    fn theme_cycle_visits_all_modes() {
        assert_eq!(next_theme_mode("system"), "dark");
        assert_eq!(next_theme_mode("dark"), "light");
        assert_eq!(next_theme_mode("light"), "system");
        assert_eq!(next_theme_mode("garbage"), "dark");
    }

    #[test]
    fn escape_prefers_modal_then_full_screen() {
        assert_eq!(escape_action(true, true, false), EscapeAction::CloseModal);
        assert_eq!(
            escape_action(false, true, false),
            EscapeAction::ExitFullScreen
        );
        assert_eq!(
            escape_action(false, true, true),
            EscapeAction::CloseUserMenu
        );
        assert_eq!(escape_action(false, false, false), EscapeAction::None);
    }

    #[test]
    fn nav_keys_round_trip_routes() {
        for item in NAV_ITEMS {
            assert!(
                nav_active(item.key, &nav_route(item.key, "")),
                "{}",
                item.key
            );
        }
    }

    #[test]
    fn collected_seasons_map_to_season_routes() {
        let mut f = FavFolder {
            id: 7,
            mid: 9,
            title: "x".into(),
            ..Default::default()
        };
        assert!(matches!(
            folder_route(&f),
            Route::Collection {
                kind: CollectionKind::FavFolder,
                ..
            }
        ));
        f.folder_type = BILI_FOLDER_TYPE_SEASON;
        assert!(matches!(
            folder_route(&f),
            Route::Collection {
                kind: CollectionKind::Season,
                ..
            }
        ));
    }
}
