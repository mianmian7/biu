//! Main window shell: sidebar, header, content host, player bar and overlays
//! (full-screen player, modals, toast), plus window-level shortcuts and the
//! close-to-tray lifecycle.

use crate::desktop_lyrics::open_or_toggle_desktop_lyrics;
use crate::full_screen::FullScreenPlayer;
use crate::mini_player::open_or_toggle_mini_player;
use crate::modals::ModalsHost;
use crate::pages::PagesHost;
use crate::player_bar::{TooltipExt, icon, icon_button, is_playing, on_state, render_player_bar};
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

pub const SIDEBAR_EXPANDED: f32 = 216.0;
pub const SIDEBAR_COLLAPSED: f32 = 64.0;
pub const HEADER_HEIGHT: f32 = 52.0;

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
        label: "音乐推荐",
        icon: IconName::Sparkles,
        key: "music-recommend",
    },
    NavItem {
        label: "搜索",
        icon: IconName::Search,
        key: "search",
    },
    NavItem {
        label: "稍后再看",
        icon: IconName::Clock,
        key: "later",
    },
    NavItem {
        label: "播放历史",
        icon: IconName::RotateCcwClock,
        key: "history",
    },
    NavItem {
        label: "我的关注",
        icon: IconName::Users,
        key: "follow-list",
    },
    NavItem {
        label: "动态",
        icon: IconName::Rss,
        key: "dynamic-feed",
    },
    NavItem {
        label: "本地音乐",
        icon: IconName::HardDrive,
        key: "local-music",
    },
    NavItem {
        label: "下载列表",
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

pub struct MainShell {
    pub state: Entity<AppState>,
    pub pages: Entity<PagesHost>,
    pub modals: Entity<ModalsHost>,
    full_screen: Entity<FullScreenPlayer>,
    search: Entity<InputState>,
    focus: FocusHandle,
    user_menu_open: bool,
    _subscriptions: Vec<Subscription>,
}

impl MainShell {
    pub fn new(state: Entity<AppState>, window: &mut Window, cx: &mut Context<Self>) -> Self {
        let pages = cx.new(|cx| PagesHost::new(state.clone(), window, cx));
        let modals = cx.new(|cx| ModalsHost::new(state.clone(), window, cx));
        let full_screen = cx.new(|cx| FullScreenPlayer::new(state.clone(), window, cx));
        let search = cx.new(|cx| InputState::new(window, cx).placeholder("搜索歌曲、视频、UP 主"));
        let focus = cx.focus_handle();

        let mut subs = Vec::new();
        subs.push(cx.observe_in(&state, window, |this, _, window, cx| {
            this.sync_component_theme(window, cx);
            cx.notify();
        }));
        subs.push(cx.observe_window_appearance(window, |this, window, cx| {
            this.sync_component_theme(window, cx);
            cx.notify();
        }));
        subs.push(
            cx.subscribe_in(&search, window, |this, input, ev: &InputEvent, _, cx| {
                if let InputEvent::PressEnter { .. } = ev {
                    let query = input.read(cx).value().trim().to_string();
                    if !query.is_empty() {
                        this.state
                            .update(cx, |s, cx| s.navigate(Route::Search { query }, cx));
                    }
                }
            }),
        );

        // Close-to-tray: let the window (and its GPU surface) be destroyed while
        // the app and AudioEngine keep running in the background.
        let st = state.clone();
        window.on_window_should_close(cx, move |window, cx| {
            if system::is_quitting() {
                return true;
            }
            let option = st.read(cx).settings.close_window_option.clone();
            if option == "minimize" {
                window.minimize_window();
                return false;
            }
            st.update(cx, |s, cx| {
                s.windows.main = None;
                s.covers.clear();
                if s.is_full_screen {
                    s.is_full_screen = false;
                    s.services.audio.set_waveform_enabled(false);
                }
                cx.notify();
            });
            if option != "hide" {
                cx.defer(system::request_quit);
            }
            true
        });

        window.focus(&focus, cx);

        let mut this = Self {
            state,
            pages,
            modals,
            full_screen,
            search,
            focus,
            user_menu_open: false,
            _subscriptions: subs,
        };
        this.sync_component_theme(window, cx);
        this
    }

    /// Keep gpui-component widgets (inputs, popovers) in step with Biu's theme.
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
        // Anything other than the shell root holding focus is treated as a text field.
        let text_focused = window.focused(cx).is_some_and(|f| f != self.focus);

        if key == "escape" {
            let s = self.state.read(cx);
            let action = escape_action(
                !matches!(s.modal, ActiveModal::None),
                s.is_full_screen,
                self.user_menu_open,
            );
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
                // Arrow chords are text-navigation inside inputs; leave them alone there.
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
                state.update(cx, |s, cx| {
                    match res {
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
                    }
                    cx.notify();
                })
            });
        })
        .detach();
        cx.notify();
    }

    // -- Sidebar -----------------------------------------------------------

    fn render_sidebar(&self, p: BiuPalette, cx: &mut Context<Self>) -> AnyElement {
        let s = self.state.read(cx);
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
            .h(px(44.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .when(collapsed, |d| d.justify_center())
            .when(!collapsed, |d| d.justify_between().px_3())
            .when(!collapsed, |d| {
                d.child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .size(px(26.0))
                                .rounded(px(8.0))
                                .bg(p.accent)
                                .flex()
                                .items_center()
                                .justify_center()
                                .child(icon(IconName::Music, 15.0, p.accent_fg)),
                        )
                        .child(
                            div()
                                .text_lg()
                                .font_weight(FontWeight::BOLD)
                                .text_color(p.text)
                                .child("Biu"),
                        ),
                )
            })
            .child(
                icon_button(
                    "sidebar-toggle",
                    if collapsed {
                        IconName::PanelLeftOpen
                    } else {
                        IconName::PanelLeftClose
                    },
                    16.0,
                    p,
                    false,
                )
                .tip(if collapsed {
                    "展开侧边栏"
                } else {
                    "收起侧边栏"
                })
                .on_click(on_state(&self.state, |s, cx| s.toggle_sidebar(cx))),
            );

        let nav = NAV_ITEMS
            .iter()
            .filter(|item| !hidden.iter().any(|h| h == item.key))
            .map(|item| {
                let active = nav_active(item.key, &route);
                let target = nav_route(item.key, &current_query);
                let label = item.label;
                div()
                    .id(item.key)
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .gap_3()
                    .rounded(px(8.0))
                    .cursor_pointer()
                    .when(collapsed, |d| d.justify_center().tip(label))
                    .when(!collapsed, |d| d.px_3())
                    .when(active, |d| d.bg(p.surface_active))
                    .when(!active, |d| d.hover(|s| s.bg(p.surface_hover)))
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
                                .text_color(if active { p.text } else { p.text_muted })
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
                .mt_3()
                .px_3()
                .flex()
                .items_center()
                .justify_between()
                .rounded(px(6.0))
                .cursor_pointer()
                .hover(|s| s.bg(p.surface_hover))
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
                        .gap_1()
                        .child(icon(
                            if is_folded {
                                IconName::ChevronRight
                            } else {
                                IconName::ChevronDown
                            },
                            12.0,
                            p.text_dim,
                        ))
                        .child(
                            div()
                                .text_xs()
                                .font_weight(FontWeight::SEMIBOLD)
                                .text_color(p.text_dim)
                                .child(title),
                        ),
                )
                .when(with_add, |d| {
                    d.child(
                        div()
                            .id(("sec-add", 0usize))
                            .size(px(20.0))
                            .flex()
                            .items_center()
                            .justify_center()
                            .rounded(px(4.0))
                            .hover(|s| s.bg(p.surface_active))
                            .on_click(on_state(&self.state, |s, cx| {
                                cx.stop_propagation();
                                s.open_modal(ActiveModal::FavFolderEdit { folder: None }, cx)
                            }))
                            .child(icon(IconName::Plus, 13.0, p.text_muted)),
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
                            .h(px(32.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .gap_2()
                            .rounded(px(8.0))
                            .cursor_pointer()
                            .when(active, |d| d.bg(p.surface_active))
                            .when(!active, |d| d.hover(|s| s.bg(p.surface_hover)))
                            .child(icon(
                                if with_add {
                                    IconName::FolderHeart
                                } else {
                                    IconName::Star
                                },
                                14.0,
                                if active { p.accent } else { p.text_dim },
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .truncate()
                                    .text_color(if active { p.text } else { p.text_muted })
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
            div().flex().flex_col().child(header).children(rows)
        };

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
                    .px_2()
                    .pb_3()
                    .flex()
                    .flex_col()
                    .gap_0p5()
                    .children(nav)
                    .when(logged_in && !collapsed, |d| {
                        d.child(folder_section(
                            "sec-created",
                            "创建的收藏夹",
                            created,
                            folded.created,
                            true,
                        ))
                        .child(folder_section(
                            "sec-collected",
                            "收藏的歌单",
                            collected,
                            folded.collected,
                            false,
                        ))
                    }),
            )
            .into_any_element()
    }

    // -- Header ------------------------------------------------------------

    fn render_header(
        &self,
        p: BiuPalette,
        avatar: Option<std::sync::Arc<RenderImage>>,
        cx: &mut Context<Self>,
    ) -> AnyElement {
        let s = self.state.read(cx);
        let mac = cfg!(target_os = "macos");
        let theme_mode = s.settings.theme_mode.clone();
        let lyrics_open = s.windows.desktop_lyrics.is_some();
        let mini_open = s.windows.mini_player.is_some();
        let user = s.user.clone();
        let playing = is_playing(s.snapshot.state);

        let ticker = if s.settings.show_top_lyrics && playing && !s.is_full_screen {
            active_line_index(&s.lyrics.lines, s.snapshot.position_secs)
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

        let drag = |d: Stateful<Div>| {
            d.on_mouse_down(MouseButton::Left, |ev, window, _| {
                if ev.click_count >= 2 {
                    window.titlebar_double_click();
                } else {
                    window.start_window_move();
                }
            })
        };

        let search = div()
            .occlude()
            .w(px(280.0))
            .h(px(32.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_2()
            .px_3()
            .rounded_full()
            .bg(p.surface)
            .border_1()
            .border_color(p.border)
            .child(icon(IconName::Search, 14.0, p.text_dim))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .text_sm()
                    .child(Input::new(&self.search).appearance(false).cleanable(true)),
            );

        let center = drag(
            div()
                .id("header-drag")
                .flex_1()
                .min_w_0()
                .h_full()
                .flex()
                .items_center()
                .justify_center(),
        )
        .when_some(ticker, |d, (text, tr)| {
            d.child(
                div()
                    .id("top-lyrics")
                    .occlude()
                    .max_w(px(520.0))
                    .px_3()
                    .py_1()
                    .rounded(px(8.0))
                    .flex()
                    .flex_col()
                    .items_center()
                    .cursor_pointer()
                    .hover(|s| s.bg(p.surface_hover))
                    .on_click(on_state(&self.state, |s, cx| s.toggle_full_screen(cx)))
                    .child(
                        div()
                            .max_w_full()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(p.accent)
                            .truncate()
                            .child(text),
                    )
                    .when_some(tr, |d, t| {
                        d.child(
                            div()
                                .max_w_full()
                                .text_xs()
                                .text_color(p.text_dim)
                                .truncate()
                                .child(t),
                        )
                    }),
            )
        });

        let user_button = match &user {
            Some(u) => {
                let name = u.name.clone();
                div()
                    .id("user-button")
                    .h(px(32.0))
                    .pl_1()
                    .pr_2()
                    .flex()
                    .items_center()
                    .gap_2()
                    .rounded_full()
                    .cursor_pointer()
                    .hover(|s| s.bg(p.surface_hover))
                    .child(crate::widgets::cover_box(
                        avatar,
                        px(26.0),
                        px(26.0),
                        px(13.0),
                        p,
                    ))
                    .child(
                        div()
                            .max_w(px(96.0))
                            .text_sm()
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
                .h(px(30.0))
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
                .hover(|s| s.opacity(0.88))
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
            .child(div().w(px(1.0)).h(px(18.0)).mx_1().bg(p.border))
            .child(user_button);

        drag(div().id("header"))
            .h(px(HEADER_HEIGHT))
            .w_full()
            .flex_shrink_0()
            .flex()
            .items_center()
            .gap_3()
            .pl(px(if mac { 84.0 } else { 16.0 }))
            .pr_4()
            .bg(p.bg)
            .border_b_1()
            .border_color(p.border)
            .child(search)
            .child(center)
            .child(right)
            .into_any_element()
    }

    fn render_user_menu(&self, p: BiuPalette, cx: &mut Context<Self>) -> Option<AnyElement> {
        if !self.user_menu_open {
            return None;
        }
        let user = self.state.read(cx).user.clone()?;
        let item = |id: &'static str, name: IconName, label: &'static str, danger: bool| {
            div()
                .id(id)
                .h(px(34.0))
                .px_3()
                .flex()
                .items_center()
                .gap_2()
                .rounded(px(6.0))
                .cursor_pointer()
                .text_sm()
                .text_color(if danger { p.danger } else { p.text })
                .hover(|s| s.bg(p.surface_hover))
                .child(icon(
                    name,
                    14.0,
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
                        .right(px(14.0))
                        .w(px(180.0))
                        .p_1()
                        .rounded(px(10.0))
                        .bg(p.surface)
                        .border_1()
                        .border_color(p.border)
                        .shadow_lg()
                        .occlude()
                        .flex()
                        .flex_col()
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
            (p.danger, p.danger.opacity(0.35))
        } else {
            (p.accent, p.accent.opacity(0.35))
        };
        Some(
            div()
                .absolute()
                .top(px(HEADER_HEIGHT + 12.0))
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
                        .bg(p.surface)
                        .border_1()
                        .border_color(ring)
                        .shadow_lg()
                        .text_sm()
                        .text_color(p.text)
                        .cursor_pointer()
                        .on_click(on_state(&self.state, |s, cx| {
                            s.toast = None;
                            cx.notify();
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
        let face = self
            .state
            .read(cx)
            .user
            .as_ref()
            .map(|u| u.face.clone())
            .unwrap_or_default();
        let avatar = if face.is_empty() {
            None
        } else {
            self.state.update(cx, |s, cx| {
                s.ensure_cover(&face, ThumbSize::Tiny, false, cx)
            })
        };

        let p = self.state.read(cx).palette(window);
        let (full_screen, modal_open) = {
            let s = self.state.read(cx);
            (s.is_full_screen, !matches!(s.modal, ActiveModal::None))
        };

        let sidebar = self.render_sidebar(p, cx);
        let header = self.render_header(p, avatar, cx);
        let player_bar = render_player_bar(&self.state, p, window, cx);
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
            .child(header)
            .child(
                div().flex_1().min_h_0().flex().child(sidebar).child(
                    div()
                        .flex_1()
                        .min_w_0()
                        .min_h_0()
                        .overflow_hidden()
                        .child(self.pages.clone()),
                ),
            )
            .child(player_bar)
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
