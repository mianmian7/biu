use super::*;
use crate::state::CollectionKind;

#[derive(Default)]
pub(super) struct Collection {
    input: Option<Entity<InputState>>,
    keyword: String,
    page: u64,
    results: Load<Page<TrackItem>>,
}

impl PagesHost {
    pub(super) fn enter_collection(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if matches!(
            self.route,
            Some(Route::Collection {
                kind: CollectionKind::FavFolder,
                ..
            })
        ) {
            self.data.collection.input =
                Some(self.filter_input("在歌单内搜索…", window, cx, Self::filter_collection));
        }
        self.load_collection(1, cx);
    }

    fn filter_collection(&mut self, keyword: String, cx: &mut Context<Self>) {
        self.data.collection.keyword = keyword;
        self.load_collection(1, cx);
    }

    fn load_collection(&mut self, page: u64, cx: &mut Context<Self>) {
        let Some(Route::Collection { kind, id, mid, .. }) = self.route else {
            return;
        };
        self.data.collection.page = page;
        let keyword = self.data.collection.keyword.clone();
        self.load_with(
            |h| &mut h.data.collection.results,
            move |s| collection_page(s, kind, id, mid, page, keyword),
            |this, page_data, cx| {
                this.prefetch_tracks(&page_data.items, cx);
                if let Some(first) = page_data.items.first()
                    && !first.cover.is_empty()
                {
                    this.prefetch_covers(vec![(first.cover.clone(), ThumbSize::Card)], cx);
                }
            },
            cx,
        );
    }

    fn all_collection(&mut self, append: bool, cx: &mut Context<Self>) {
        let Some(Route::Collection { kind, id, mid, .. }) = self.route else {
            return;
        };
        let keyword = self.data.collection.keyword.clone();
        self.queue_all(
            move |s, page| collection_page(s, kind, id, mid, page, keyword.clone()),
            append,
            cx,
        );
    }

    pub(super) fn collection_view(
        &self,
        p: BiuPalette,
        content_width: f32,
        cx: &Context<Self>,
    ) -> Div {
        let Some(Route::Collection {
            kind, id, title, ..
        }) = self.route.as_ref()
        else {
            return div();
        };
        let data = &self.data.collection;
        let state = self.state.read(cx);
        let grid = is_grid_mode(state);
        let owned = (*kind == CollectionKind::FavFolder)
            .then(|| state.created_folders.iter().find(|folder| folder.id == *id))
            .flatten()
            .cloned();
        let display_title = owned
            .as_ref()
            .map_or_else(|| title.clone(), |folder| folder.title.clone());
        let kind_label = match kind {
            CollectionKind::FavFolder => "收藏夹",
            CollectionKind::Season => "合集",
            CollectionKind::Series => "系列",
        };
        let total_count = data
            .results
            .result
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .and_then(|res| res.total)
            .or_else(|| owned.as_ref().map(|f| f.media_count));
        let hero_cover_url = owned
            .as_ref()
            .map(|f| f.cover.as_str())
            .filter(|c| !c.is_empty())
            .or_else(|| {
                data.results
                    .result
                    .as_ref()
                    .and_then(|r| r.as_ref().ok())
                    .and_then(|res| res.items.first())
                    .map(|t| t.cover.as_str())
            })
            .unwrap_or("");
        let hero_cover = if hero_cover_url.is_empty() {
            None
        } else {
            state
                .covers
                .get(&CoverKey::thumb(hero_cover_url.to_string(), ThumbSize::Card))
                .or_else(|| {
                    state
                        .covers
                        .get(&CoverKey::thumb(hero_cover_url.to_string(), ThumbSize::Tiny))
                })
        };

        // Hero Banner matching `screenshots/main.png`
        let hero = div()
            .w_full()
            .flex()
            .items_end()
            .gap_6()
            .pb_2()
            .child(
                div()
                    .rounded(px(14.0))
                    .shadow_xl()
                    .child(cover_box(hero_cover, px(152.0), px(152.0), px(14.0), p)),
            )
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_3xl()
                            .font_weight(FontWeight::BOLD)
                            .text_color(p.text)
                            .truncate()
                            .child(display_title),
                    )
                    .child(
                        div()
                            .text_sm()
                            .text_color(p.text_muted)
                            .child(match total_count {
                                Some(n) => format!("{kind_label}  ·  {n} 条内容"),
                                None => kind_label.to_string(),
                            }),
                    )
                    .when_some(
                        owned.as_ref().filter(|f| !f.intro.trim().is_empty()),
                        |col, f| {
                            col.child(
                                div()
                                    .text_xs()
                                    .text_color(p.text_dim)
                                    .truncate()
                                    .child(f.intro.clone()),
                            )
                        },
                    )
                    .child(
                        div()
                            .pt_2()
                            .flex()
                            .flex_wrap()
                            .items_center()
                            .justify_between()
                            .gap_3()
                            .child(
                                toolbar()
                                    .when_some(
                                        data.results.result.as_ref().and_then(|r| r.as_ref().ok()),
                                        |bar, result| {
                                            bar.child(self.paginated_actions(
                                                result,
                                                data.page,
                                                p,
                                                Self::all_collection,
                                                cx,
                                            ))
                                        },
                                    )
                                    .when_some(owned.clone(), |bar, folder| {
                                        let st = self.state.clone();
                                        bar.child(
                                            icon_pill_button(
                                                "edit-folder",
                                                IconName::Pencil,
                                                "修改信息",
                                                p,
                                            )
                                            .on_click(move |_, _, cx| {
                                                st.update(cx, |s, cx| {
                                                    s.open_modal(
                                                        ActiveModal::FavFolderEdit {
                                                            folder: Some(folder.clone()),
                                                        },
                                                        cx,
                                                    )
                                                });
                                            }),
                                        )
                                    }),
                            )
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .gap_2()
                                    .when_some(data.input.as_ref(), |bar, input| {
                                        bar.child(filter_bar(
                                            input,
                                            "筛选",
                                            p,
                                            cx,
                                            Self::filter_collection,
                                        ))
                                    })
                                    .child(display_mode_toggle(
                                        &self.state,
                                        grid,
                                        p,
                                        |this, _, cx| {
                                            if let Some(Ok(res)) =
                                                this.data.collection.results.result.as_ref()
                                            {
                                                let items = res.items.clone();
                                                this.prefetch_tracks(&items, cx);
                                            }
                                        },
                                        cx,
                                    )),
                            ),
                    ),
            );

        let folder_id = *id;
        page("").child(hero).child(data.results.content(
            p,
            button("retry", "重试", p).on_click(
                cx.listener(|this, _, _, cx| this.load_collection(this.data.collection.page, cx)),
            ),
            |result| {
                if result.items.is_empty() {
                    return empty_state("歌单为空", "收藏的视频或音频会显示在这里", p);
                }
                column()
                    .gap_4()
                    .child(if grid {
                        track_grid(&self.state, &result.items, content_width, p, cx)
                    } else {
                        div()
                            .w_full()
                            .flex()
                            .flex_col()
                            .gap(px(2.0))
                            .child(track_list_header(p))
                            .children(result.items.iter().enumerate().map(|(i, track)| {
                                let resource = playable_id(track);
                                track_row(&self.state, i, track, p, cx).when(
                                    owned.is_some() && !self.mutating,
                                    |row| {
                                        row.when_some(resource, |row, resource| {
                                            row.child(
                                                compact_icon_btn(
                                                    ("rm-fav", i),
                                                    IconName::Trash,
                                                    "移出收藏夹",
                                                    p,
                                                )
                                                .on_click(cx.listener(
                                                    move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.mutate(
                                                            move |s| async move {
                                                                s.bili
                                                                    .batch_del_fav_resources(
                                                                        folder_id,
                                                                        &[resource],
                                                                    )
                                                                    .await
                                                            },
                                                            "已移出收藏夹",
                                                            Self::refresh_collection,
                                                            cx,
                                                        );
                                                    },
                                                )),
                                            )
                                        })
                                    },
                                )
                            }))
                    })
                    .child(pager(
                        result,
                        data.page,
                        p,
                        page_change(cx, Self::load_collection),
                    ))
            },
        ))
    }

    fn refresh_collection(&mut self, cx: &mut Context<Self>) {
        self.state.update(cx, AppState::refresh_user_and_folders);
        self.load_collection(self.data.collection.page, cx);
    }
}

async fn collection_page(
    services: Arc<Services>,
    kind: CollectionKind,
    id: u64,
    mid: u64,
    page: u64,
    keyword: String,
) -> anyhow::Result<Page<TrackItem>> {
    let keyword = (!keyword.is_empty()).then_some(keyword.as_str());
    let bili = &services.bili;
    match kind {
        CollectionKind::FavFolder => bili.get_fav_folder_resources(id, page, keyword).await,
        CollectionKind::Season => bili.get_season_archives(mid, id, page).await,
        CollectionKind::Series => bili.get_series_archives(mid, id, page).await,
    }
}
