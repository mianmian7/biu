use super::*;
use crate::state::CollectionKind;
use biu_core::{FavFolder, UserInfo};

#[derive(Default)]
pub(super) struct UserProfile {
    mid: u64,
    input: Option<Entity<InputState>>,
    keyword: String,
    show_collections: bool,
    video_page: u64,
    collection_page: u64,
    info: Load<UserInfo>,
    videos: Load<Page<TrackItem>>,
    collections: Load<Page<FavFolder>>,
}

impl PagesHost {
    pub(super) fn enter_profile(&mut self, mid: u64, window: &mut Window, cx: &mut Context<Self>) {
        self.data.profile.mid = mid;
        self.data.profile.input =
            Some(self.filter_input("搜索投稿视频", window, cx, Self::filter_profile));
        self.load_profile(cx);
        self.load_profile_videos(1, cx);
    }

    fn load_profile(&mut self, cx: &mut Context<Self>) {
        let mid = self.data.profile.mid;
        self.load(
            |h| &mut h.data.profile.info,
            move |s| async move { s.bili.get_user_info(mid).await },
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
        self.load(
            |h| &mut h.data.profile.videos,
            move |s| async move {
                s.bili
                    .get_user_videos(mid, page, (!keyword.is_empty()).then_some(keyword.as_str()))
                    .await
            },
            cx,
        );
    }

    fn all_profile_videos(&mut self, append: bool, cx: &mut Context<Self>) {
        let mid = self.data.profile.mid;
        let keyword = self.data.profile.keyword.clone();
        self.queue_all(
            move |s, page| {
                let keyword = keyword.clone();
                async move {
                    s.bili
                        .get_user_videos(
                            mid,
                            page,
                            (!keyword.is_empty()).then_some(keyword.as_str()),
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
        self.load(
            |h| &mut h.data.profile.collections,
            move |s| async move { s.bili.get_user_seasons_series(mid, page).await },
            cx,
        );
    }

    pub(super) fn profile_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let Some(Route::UserProfile { name, .. }) = &self.route else {
            return div();
        };
        let profile = &self.data.profile;
        let mid = profile.mid;
        let retry_header = button("retry-profile", "重试", p)
            .on_click(cx.listener(|this, _, _, cx| this.load_profile(cx)));
        let mut root = page(if name.is_empty() {
            "UP 主主页"
        } else {
            name
        })
        .child(profile.info.content(p, retry_header, |user| {
            let followed = user
                .extra
                .get("is_followed")
                .and_then(serde_json::Value::as_bool)
                .unwrap_or(false);
            toolbar()
                .child(thumbnail(
                    &self.state,
                    &user.face,
                    ThumbSize::Tiny,
                    (56., 56.),
                    p,
                    cx,
                ))
                .child(
                    column().flex_1().child(user.name.clone()).child(
                        div()
                            .text_sm()
                            .text_color(p.text_muted)
                            .child(user.sign.clone()),
                    ),
                )
                .when(self.account != Some(mid) && !self.mutating, |row| {
                    row.child(
                        button("follow", if followed { "取消关注" } else { "关注" }, p).on_click(
                            cx.listener(move |this, _, _, cx| {
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
                            }),
                        ),
                    )
                })
        }));
        root = root.child(
            toolbar().children([(false, "投稿视频"), (true, "合集与系列")].into_iter().map(
                |(collections, title)| {
                    button(title, title, p)
                        .when(profile.show_collections == collections, |b| {
                            b.border_color(p.accent)
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.data.profile.show_collections = collections;
                            if collections && this.data.profile.collection_page == 0 {
                                this.load_profile_collections(1, cx);
                            }
                            cx.notify();
                        }))
                },
            )),
        );
        if profile.show_collections {
            return root.child(profile.collections.content(
                p,
                button("retry-collections", "重试", p).on_click(cx.listener(|this, _, _, cx| {
                    this.load_profile_collections(this.data.profile.collection_page, cx);
                })),
                |result| {
                    column()
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
                                .id(i)
                                .flex()
                                .items_center()
                                .gap_3()
                                .p_2()
                                .bg(p.surface)
                                .rounded(p.radius)
                                .cursor_pointer()
                                .hover(move |style| style.bg(p.surface_hover))
                                .on_click(move |_, _, cx| {
                                    state.update(cx, |s, cx| s.navigate(route.clone(), cx))
                                })
                                .child(thumbnail(
                                    &self.state,
                                    &folder.cover,
                                    ThumbSize::Tiny,
                                    (56., 44.),
                                    p,
                                    cx,
                                ))
                                .child(
                                    column().flex_1().child(folder.title.clone()).child(
                                        div()
                                            .text_sm()
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
            ));
        }
        if let Some(input) = &profile.input {
            root = root.child(filter_bar(input, "筛选", p, cx, Self::filter_profile));
        }
        root.child(profile.videos.content(
            p,
            button("retry-videos", "重试", p).on_click(cx.listener(|this, _, _, cx| {
                this.load_profile_videos(this.data.profile.video_page, cx);
            })),
            |result| {
                column()
                    .child(self.paginated_actions(
                        result,
                        profile.video_page,
                        p,
                        Self::all_profile_videos,
                        cx,
                    ))
                    .child(track_list(&self.state, &result.items, p, cx))
                    .child(pager(
                        result,
                        profile.video_page,
                        p,
                        page_change(cx, Self::load_profile_videos),
                    ))
            },
        ))
    }
}
