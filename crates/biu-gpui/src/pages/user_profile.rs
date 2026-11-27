use super::*;
use crate::state::CollectionKind;
use biu_core::{FavFolder, UserInfo};

const PROFILE_VIDEO_ORDERS: [(&str, &str); 3] = [
    ("pubdate", "最新发布"),
    ("click", "最多播放"),
    ("stow", "最多收藏"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum ProfileTab {
    #[default]
    Videos,
    Collections,
    Favorites,
}

#[derive(Default)]
pub(super) struct UserProfile {
    mid: u64,
    input: Option<Entity<InputState>>,
    keyword: String,
    video_order: usize,
    tab: ProfileTab,
    video_page: u64,
    collection_page: u64,
    info: Load<UserInfo>,
    videos: Load<Page<TrackItem>>,
    collections: Load<Page<FavFolder>>,
    favorites: Load<Vec<FavFolder>>,
    favorites_loaded: bool,
}

impl PagesHost {
    pub(super) fn enter_profile(&mut self, mid: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.data.profile.mid = mid;
        self.data.profile.input =
            Some(self.filter_input("搜索投稿视频…", window, cx, Self::filter_profile));
        self.load_profile(cx);
        self.load_profile_videos(1, cx);
    }

    fn load_profile(&mut self, cx: &mut Context<Self>) {
        let mid = self.data.profile.mid;
        self.load_with(
            |h| &mut h.data.profile.info,
            move |s| async move { s.bili.get_user_info(mid).await },
            |this, user, cx| {
                if !user.face.is_empty() {
                    this.prefetch_covers(vec![(user.face.clone(), ThumbSize::Avatar)], cx);
                }
            },
            cx,
        );
    }

    fn filter_profile(&mut self, keyword: String, cx: &mut Context<Self>) {
        self.data.profile.keyword = keyword;
        self.load_profile_videos(1, cx);
    }

    fn load_profile_videos(&mut self, page: u64, cx: &mut Context<Self>) {
        let profile = &mut self.data.profile;
        profile.video_page = page;
        let mid = profile.mid;
        let keyword = profile.keyword.clone();
        let order = PROFILE_VIDEO_ORDERS[profile.video_order].0;
        self.load_with(
            |h| &mut h.data.profile.videos,
            move |s| async move {
                s.bili
                    .get_user_videos_ordered(
                        mid,
                        page,
                        (!keyword.is_empty()).then_some(keyword.as_str()),
                        order,
                    )
                    .await
            },
            |this, page_data, cx| this.prefetch_tracks(&page_data.items, cx),
            cx,
        );
    }

    fn all_profile_videos(&mut self, append: bool, cx: &mut Context<Self>) {
        let mid = self.data.profile.mid;
        let keyword = self.data.profile.keyword.clone();
        let order = PROFILE_VIDEO_ORDERS[self.data.profile.video_order].0;
        self.queue_all(
            move |s, page| {
                let keyword = keyword.clone();
                async move {
                    s.bili
                        .get_user_videos_ordered(
                            mid,
                            page,
                            (!keyword.is_empty()).then_some(keyword.as_str()),
                            order,
                        )
                        .await
                }
            },
            append,
            cx,
        );
    }

    fn load_profile_collections(&mut self, page: u64, cx: &mut Context<Self>) {
        let profile = &mut self.data.profile;
        profile.collection_page = page;
        let mid = profile.mid;
        self.load_with(
            |h| &mut h.data.profile.collections,
            move |s| async move { s.bili.get_user_seasons_series(mid, page).await },
            |this, page_data, cx| {
                let covers = page_data
                    .items
                    .iter()
                    .take(30)
                    .filter(|f| !f.cover.is_empty())
                    .map(|f| (f.cover.clone(), ThumbSize::Tiny))
                    .collect();
                this.prefetch_covers(covers, cx);
            },
            cx,
        );
    }

    fn load_profile_favorites(&mut self, cx: &mut Context<Self>) {
        self.data.profile.favorites_loaded = true;
        let mid = self.data.profile.mid;
        self.load(
            |h| &mut h.data.profile.favorites,
            move |s| async move { s.bili.get_created_fav_folders(mid).await },
            cx,
        );
    }

    pub(super) fn profile_view(
        &self,
        p: BiuPalette,
        content_width: f32,
        cx: &Context<Self>,
    ) -> Div {
        let Some(Route::UserProfile { name, .. }) = &self.route else {
            return div();
        };
        let profile = &self.data.profile;
        let mid = profile.mid;
        let grid = is_grid_mode(self.state.read(cx));
        let retry_header = button("retry-profile", "重试", p)
            .on_click(cx.listener(|this, _, _, cx| this.load_profile(cx)));

        let hero = profile.info.content(p, retry_header, |user| {
            let followed = user
                .extra
                .get("is_followed")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            let level = user
                .extra
                .get("level")
                .and_then(serde_json::Value::as_u64);
            div()
                .w_full()
                .p_5()
                .rounded(px(14.0))
                .bg(p.surface)
                .flex()
                .items_center()
                .gap_4()
                .child(thumbnail(
                    &self.state,
                    &user.face,
                    ThumbSize::Avatar,
                    (72., 72.),
                    BiuPalette {
                        radius: px(36.0),
                        ..p
                    },
                    cx,
                ))
                .child(
                    column()
                        .flex_1()
                        .gap_1()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(
                                    div()
                                        .text_xl()
                                        .font_weight(FontWeight::BOLD)
                                        .child(if user.name.is_empty() {
                                            name.clone()
                                        } else {
                                            user.name.clone()
                                        }),
                                )
                                .when_some(level.filter(|l| *l > 0), |row, lv| {
                                    row.child(
                                        div()
                                            .px_1p5()
                                            .py(px(1.0))
                                            .rounded(px(4.0))
                                            .bg(p.accent_soft)
                                            .text_size(px(11.0))
                                            .font_weight(FontWeight::SEMIBOLD)
                                            .text_color(p.accent)
                                            .child(format!("Lv{lv}")),
                                    )
                                }),
                        )
                        .child(
                            div()
                                .text_sm()
                                .text_color(p.text_muted)
                                .child(if user.sign.is_empty() {
                                    "这个人很懒，什么都没有写".into()
                                } else {
                                    user.sign.clone()
                                }),
                        ),
                )
                .child(
                    icon_pill_button("open-space", IconName::ExternalLink, "B 站主页", p).on_click(
                        move |_, _, _| {
                            let _ = open::that_detached(format!("https://space.bilibili.com/{mid}"));
                        },
                    ),
                )
                .when(self.account != Some(mid) && !self.mutating, |row| {
                    let btn = if followed {
                        button("follow", "已关注", p)
                    } else {
                        primary_button("follow", IconName::Plus, "关注", p)
                    };
                    row.child(btn.on_click(cx.listener(move |this, _, _, cx| {
                        if this.account.is_none() {
                            this.state
                                .update(cx, |s, cx| s.open_modal(ActiveModal::Login, cx));
                            return;
                        }
                        this.mutate(
                            move |s| async move {
                                s.bili
                                    .modify_relation(mid, if followed { 2 } else { 1 })
                                    .await
                            },
                            if followed {
                                "已取消关注"
                            } else {
                                "已关注"
                            },
                            Self::load_profile,
                            cx,
                        );
                    })))
                })
        });

        let tab_bar = div()
            .w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .p(px(3.0))
                    .rounded(px(10.0))
                    .bg(p.surface)
                    .children(
                        [
                            (ProfileTab::Videos, "投稿视频"),
                            (ProfileTab::Collections, "合集与系列"),
                            (ProfileTab::Favorites, "收藏夹"),
                        ]
                        .into_iter()
                        .map(|(tab_key, title)| {
                            let active = profile.tab == tab_key;
                            div()
                                .id(title)
                                .h(px(30.0))
                                .px_4()
                                .rounded(px(8.0))
                                .flex()
                                .items_center()
                                .text_sm()
                                .font_weight(if active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(if active { p.text } else { p.text_muted })
                                .cursor_pointer()
                                .when(active, |d| d.bg(p.surface_active))
                                .when(!active, |d| d.hover(move |s| s.text_color(p.text)))
                                .child(title)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.data.profile.tab = tab_key;
                                    match tab_key {
                                        ProfileTab::Collections
                                            if this.data.profile.collection_page == 0 =>
                                        {
                                            this.load_profile_collections(1, cx);
                                        }
                                        ProfileTab::Favorites
                                            if !this.data.profile.favorites_loaded =>
                                        {
                                            this.load_profile_favorites(cx);
                                        }
                                        _ => cx.notify(),
                                    }
                                }))
                        }),
                    ),
            )
            .when(profile.tab == ProfileTab::Videos, |bar| {
                bar.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(toolbar().children(PROFILE_VIDEO_ORDERS.iter().enumerate().map(
                            |(index, &(_, label))| {
                                let active = index == profile.video_order;
                                div()
                                    .id(("prof-order", index))
                                    .h(px(28.0))
                                    .px_3()
                                    .rounded_full()
                                    .flex()
                                    .items_center()
                                    .text_xs()
                                    .font_weight(if active {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .text_color(if active { p.accent } else { p.text_muted })
                                    .bg(if active {
                                        p.accent_soft
                                    } else {
                                        gpui_kit::transparent_black()
                                    })
                                    .cursor_pointer()
                                    .when(!active, |d| {
                                        d.hover(move |s| s.bg(p.surface_hover).text_color(p.text))
                                    })
                                    .child(label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.data.profile.video_order = index;
                                        this.load_profile_videos(1, cx);
                                    }))
                            },
                        )))
                        .when_some(profile.input.as_ref(), |b, input| {
                            b.child(filter_bar(input, "筛选", p, cx, Self::filter_profile))
                        })
                        .child(display_mode_toggle(
                            &self.state,
                            grid,
                            p,
                            |this, _, cx| {
                                if let Some(Ok(res)) = this.data.profile.videos.result.as_ref() {
                                    let items = res.items.clone();
                                    this.prefetch_tracks(&items, cx);
                                }
                            },
                            cx,
                        )),
                )
            });

        let root = page("").child(hero).child(tab_bar);

        match profile.tab {
            ProfileTab::Favorites => root.child(profile.favorites.content(
                p,
                button("retry-favs", "重试", p)
                    .on_click(cx.listener(|this, _, _, cx| this.load_profile_favorites(cx))),
                |folders| {
                    column()
                        .gap_2()
                        .when(folders.is_empty(), |c| {
                            c.child(empty_state("暂无公开收藏夹", "", p))
                        })
                        .children(folders.iter().enumerate().map(|(i, folder)| {
                            let route = Route::Collection {
                                kind: CollectionKind::FavFolder,
                                id: folder.id,
                                mid,
                                title: folder.title.clone(),
                            };
                            let state = self.state.clone();
                            div()
                                .id(("prof-fav", i))
                                .flex()
                                .items_center()
                                .gap_3()
                                .p_3()
                                .bg(p.surface)
                                .rounded(px(12.0))
                                .cursor_pointer()
                                .hover(move |style| style.bg(p.surface_hover))
                                .on_click(move |_, _, cx| {
                                    state.update(cx, |s, cx| s.navigate(route.clone(), cx))
                                })
                                .child(
                                    div()
                                        .size(px(44.0))
                                        .rounded(px(10.0))
                                        .bg(p.surface_elevated)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(icon(IconName::Folder, 20.0, p.accent)),
                                )
                                .child(
                                    column()
                                        .flex_1()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .truncate()
                                                .child(folder.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(p.text_muted)
                                                .child(format!("{} 个内容", folder.media_count)),
                                        ),
                                )
                        }))
                },
            )),
            ProfileTab::Collections => root.child(profile.collections.content(
                p,
                button("retry-collections", "重试", p).on_click(cx.listener(|this, _, _, cx| {
                    this.load_profile_collections(this.data.profile.collection_page, cx);
                })),
                |result| {
                    column()
                        .gap_2()
                        .when(result.items.is_empty(), |c| {
                            c.child(empty_state("暂无合集与系列", "", p))
                        })
                        .children(result.items.iter().enumerate().map(|(i, folder)| {
                            let route = Route::Collection {
                                kind: if folder.folder_type == 21 {
                                    CollectionKind::Season
                                } else {
                                    CollectionKind::Series
                                },
                                id: folder.id,
                                mid,
                                title: folder.title.clone(),
                            };
                            let state = self.state.clone();
                            div()
                                .id(("prof-col", i))
                                .flex()
                                .items_center()
                                .gap_3()
                                .p_3()
                                .bg(p.surface)
                                .rounded(px(12.0))
                                .cursor_pointer()
                                .hover(move |style| style.bg(p.surface_hover))
                                .on_click(move |_, _, cx| {
                                    state.update(cx, |s, cx| s.navigate(route.clone(), cx))
                                })
                                .child(thumbnail(
                                    &self.state,
                                    &folder.cover,
                                    ThumbSize::Tiny,
                                    (64., 48.),
                                    p,
                                    cx,
                                ))
                                .child(
                                    column()
                                        .flex_1()
                                        .gap(px(2.0))
                                        .child(
                                            div()
                                                .text_sm()
                                                .font_weight(FontWeight::SEMIBOLD)
                                                .truncate()
                                                .child(folder.title.clone()),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(p.text_muted)
                                                .child(format!("{} 个视频", folder.media_count)),
                                        ),
                                )
                        }))
                        .child(pager(
                            result,
                            profile.collection_page,
                            p,
                            page_change(cx, Self::load_profile_collections),
                        ))
                },
            )),
            ProfileTab::Videos => root.child(profile.videos.content(
                p,
                button("retry-videos", "重试", p).on_click(cx.listener(|this, _, _, cx| {
                    this.load_profile_videos(this.data.profile.video_page, cx);
                })),
                |result| {
                    column()
                        .gap_4()
                        .child(self.paginated_actions(
                            result,
                            profile.video_page,
                            p,
                            Self::all_profile_videos,
                            cx,
                        ))
                        .child(track_collection(
                            &self.state,
                            &result.items,
                            content_width,
                            p,
                            cx,
                        ))
                        .child(pager(
                            result,
                            profile.video_page,
                            p,
                            page_change(cx, Self::load_profile_videos),
                        ))
                },
            )),
        }
    }
}
