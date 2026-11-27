use super::*;
use biu_core::{FollowingUser, SearchResult};
use std::time::Duration;

const VIDEO_ORDERS: [(&str, &str); 5] = [
    ("totalrank", "综合排序"),
    ("click", "最多播放"),
    ("pubdate", "最新发布"),
    ("dm", "最多弹幕"),
    ("stow", "最多收藏"),
];

const USER_ORDERS: [(&str, &str, Option<&str>, Option<u64>); 5] = [
    ("user-default", "默认排序", None, None),
    ("user-fans-desc", "粉丝由高到低", Some("fans"), Some(0)),
    ("user-fans-asc", "粉丝由低到高", Some("fans"), Some(1)),
    ("user-level-desc", "等级由高到低", Some("level"), Some(0)),
    ("user-level-asc", "等级由低到高", Some("level"), Some(1)),
];

const MUSIC_ZONE_TID: u64 = 3;
const SEARCH_PAGE_SIZE: u64 = 24;

enum Results {
    Videos(Page<TrackItem>),
    Users(Page<FollowingUser>),
}

#[derive(Default)]
pub(super) struct Search {
    input: Option<Entity<InputState>>,
    query: String,
    order: usize,
    user_order: usize,
    music_only: bool,
    users: bool,
    page: u64,
    results: Load<Results>,
    suggestions: Load<Vec<String>>,
    suggest_visible: bool,
}

impl PagesHost {
    pub(super) fn enter_search(
        &mut self,
        query: &str,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let query = normalize_query(query);
        self.data.search.query = query.clone();
        self.data.search.suggestions = Load::default();
        self.data.search.suggest_visible = false;
        if let Some(input) = &self.data.search.input {
            if input.read(cx).value().as_ref() != query {
                input.update(cx, |input, cx| input.set_value(query, window, cx));
            }
        } else {
            let input = new_input("搜索视频、音乐或 UP 主…", query, window, cx);
            cx.subscribe_in(
                &input,
                window,
                |this, input, event: &InputEvent, window, cx| {
                    let query = normalize_query(&input.read(cx).value());
                    match event {
                        InputEvent::Focus => {
                            if !query.is_empty() {
                                this.data.search.suggest_visible = true;
                                if this.data.search.suggestions.result.is_none() {
                                    this.suggest(query, cx);
                                } else {
                                    cx.notify();
                                }
                            }
                        }
                        InputEvent::Change => {
                            this.data.search.suggest_visible = !query.is_empty();
                            this.suggest(query, cx);
                        }
                        InputEvent::PressEnter { .. } => this.submit_search(query, window, cx),
                        InputEvent::Blur => {}
                    }
                },
            )
            .detach();
            self.data.search.input = Some(input);
        }
        self.load_search(1, cx);
    }

    fn submit_search(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
        self.data.search.suggest_visible = false;
        self.data.search.suggestions = Load::default();
        self.state.update(cx, |s, cx| {
            s.navigate(
                Route::Search {
                    query: query.clone(),
                },
                cx,
            )
        });
        if self.data.search.query == query {
            self.enter_search(&query, window, cx);
        }
    }

    fn suggest(&mut self, query: String, cx: &mut Context<Self>) {
        if query.is_empty() {
            self.data.search.suggestions = Load::default();
            self.data.search.suggest_visible = false;
            cx.notify();
            return;
        }
        self.load(
            |h| &mut h.data.search.suggestions,
            move |s| async move {
                tokio::time::sleep(Duration::from_millis(220)).await;
                s.bili.search_suggest(&query).await
            },
            cx,
        );
    }

    fn load_search(&mut self, page: u64, cx: &mut Context<Self>) {
        let search = &mut self.data.search;
        search.page = page;
        let query = search.query.clone();
        let users = search.users;
        let order = VIDEO_ORDERS[search.order].0;
        let tids = search.music_only.then_some(MUSIC_ZONE_TID);
        let (_, _, user_order, user_sort) = USER_ORDERS[search.user_order];
        if query.is_empty() {
            search.results = Load::default();
            cx.notify();
            return;
        }
        self.load_with(
            |h| &mut h.data.search.results,
            move |s| async move {
                if users {
                    Ok(Results::Users(search_page(
                        s.bili
                            .search_users_sorted(&query, page, user_order, user_sort)
                            .await?,
                    )))
                } else {
                    Ok(Results::Videos(search_page(
                        s.bili
                            .search_videos_filtered(&query, order, page, SEARCH_PAGE_SIZE, tids)
                            .await?,
                    )))
                }
            },
            |this, res, cx| match res {
                Results::Videos(items) => this.prefetch_tracks(&items.items, cx),
                Results::Users(items) => {
                    let covers = items
                        .items
                        .iter()
                        .take(30)
                        .filter(|u| !u.face.is_empty())
                        .map(|u| (u.face.clone(), ThumbSize::Avatar))
                        .collect();
                    this.prefetch_covers(covers, cx);
                }
            },
            cx,
        );
    }

    pub(super) fn search_view(
        &self,
        p: BiuPalette,
        content_width: f32,
        cx: &Context<Self>,
    ) -> Div {
        let search = &self.data.search;
        let state = self.state.read(cx);
        let grid = is_grid_mode(state);

        let page_tracks: Vec<TrackItem> = match &search.results.result {
            Some(Ok(Results::Videos(v))) => v.items.clone(),
            _ => Vec::new(),
        };

        let title = if search.query.is_empty() {
            "搜索发现".to_string()
        } else {
            format!("搜索「{}」", search.query)
        };
        let mut root = page(title);

        if let Some(input) = &search.input {
            let value = input.clone();
            let suggestions = search
                .suggest_visible
                .then(|| {
                    search
                        .suggestions
                        .result
                        .as_ref()
                        .and_then(|r| r.as_ref().ok())
                        .filter(|items| !items.is_empty())
                })
                .flatten();

            let mut search_box_col = column().gap_2().child(
                div()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .child(
                        div()
                            .w(px(400.0))
                            .max_w_full()
                            .h(px(40.0))
                            .px_4()
                            .rounded_full()
                            .bg(p.surface)
                            .border_1()
                            .border_color(if search.suggest_visible {
                                p.accent.opacity(0.6)
                            } else {
                                gpui_kit::transparent_black()
                            })
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon(IconName::Search, 16.0, p.text_dim))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_sm()
                                    .child(Input::new(input).appearance(false).cleanable(true)),
                            ),
                    )
                    .child(
                        primary_button("submit-search", IconName::Search, "搜索", p).on_click(
                            cx.listener(move |this, _, window, cx| {
                                this.submit_search(
                                    normalize_query(&value.read(cx).value()),
                                    window,
                                    cx,
                                );
                            }),
                        ),
                    ),
            );

            if let Some(suggestions) = suggestions {
                search_box_col = search_box_col.child(
                    div()
                        .id("suggestions")
                        .w(px(480.0))
                        .max_w_full()
                        .p_2()
                        .rounded(px(12.0))
                        .bg(p.surface_elevated)
                        .border_1()
                        .border_color(p.border_strong)
                        .shadow_lg()
                        .flex()
                        .flex_col()
                        .gap_0p5()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .px_2()
                                .pb_1()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(p.text_dim)
                                        .child("搜索建议"),
                                )
                                .child(
                                    div()
                                        .id("close-suggestions")
                                        .text_xs()
                                        .text_color(p.text_dim)
                                        .cursor_pointer()
                                        .hover(move |s| s.text_color(p.text))
                                        .child("收起")
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.data.search.suggest_visible = false;
                                            cx.notify();
                                        })),
                                ),
                        )
                        .children(suggestions.iter().enumerate().map(|(i, term)| {
                            let chosen = term.clone();
                            div()
                                .id(("page-suggest", i))
                                .h(px(32.0))
                                .px_2p5()
                                .rounded(px(8.0))
                                .flex()
                                .items_center()
                                .gap_2()
                                .cursor_pointer()
                                .hover(move |s| s.bg(p.surface_hover))
                                .on_click(cx.listener(move |this, _, window, cx| {
                                    this.submit_search(chosen.clone(), window, cx);
                                }))
                                .child(icon(IconName::Search, 13.0, p.text_dim))
                                .child(
                                    div()
                                        .flex_1()
                                        .min_w_0()
                                        .text_sm()
                                        .text_color(p.text)
                                        .truncate()
                                        .child(term.clone()),
                                )
                        })),
                );
            }

            root = root.child(search_box_col);
        }

        if state.settings.show_search_history && !state.search_history.is_empty() {
            root = root.child(
                div().id("search-history").child(
                    column()
                        .gap_2()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(p.text_dim)
                                        .child("最近搜索"),
                                )
                                .child(
                                    div()
                                        .id("clear-history")
                                        .text_xs()
                                        .text_color(p.text_dim)
                                        .cursor_pointer()
                                        .hover(move |s| s.text_color(p.text))
                                        .child("清空")
                                        .on_click({
                                            let st = self.state.clone();
                                            move |_, _, cx| {
                                                st.update(cx, AppState::clear_search_history)
                                            }
                                        }),
                                ),
                        )
                        .child(history_chips(&self.state, &state.search_history, p, cx)),
                ),
            );
        }

        let sort_bar = if search.users {
            toolbar().children(
                USER_ORDERS
                    .iter()
                    .enumerate()
                    .map(|(index, &(id, title, _, _))| {
                        let active = index == search.user_order;
                        div()
                            .id(id)
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
                            .child(title)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.data.search.user_order = index;
                                this.load_search(1, cx);
                            }))
                    }),
            )
        } else {
            toolbar().children(VIDEO_ORDERS.iter().enumerate().map(|(index, &(id, title))| {
                let active = index == search.order;
                div()
                    .id(id)
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
                    .child(title)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.data.search.order = index;
                        this.load_search(1, cx);
                    }))
            }))
        };

        let controls = div()
            .w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .p(px(3.0))
                            .rounded(px(10.0))
                            .bg(p.surface)
                            .children([(false, "视频"), (true, "UP 主")].into_iter().map(
                                |(users, title)| {
                                    let active = users == search.users;
                                    div()
                                        .id(title)
                                        .h(px(28.0))
                                        .px_3p5()
                                        .rounded(px(7.0))
                                        .flex()
                                        .items_center()
                                        .text_xs()
                                        .font_weight(if active {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::MEDIUM
                                        })
                                        .text_color(if active { p.text } else { p.text_muted })
                                        .cursor_pointer()
                                        .when(active, |d| d.bg(p.surface_active))
                                        .when(!active, |d| {
                                            d.hover(move |s| s.text_color(p.text))
                                        })
                                        .child(title)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.data.search.users = users;
                                            this.load_search(1, cx);
                                        }))
                                },
                            )),
                    )
                    .child(sort_bar),
            )
            .when(!search.users, |bar| {
                let music_only = search.music_only;
                bar.child(
                    div()
                        .flex()
                        .flex_wrap()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .id("search-music-only")
                                .h(px(34.0))
                                .px_3p5()
                                .rounded_full()
                                .flex()
                                .items_center()
                                .gap_1p5()
                                .text_xs()
                                .font_weight(if music_only {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .bg(if music_only { p.accent_soft } else { p.surface })
                                .border_1()
                                .border_color(if music_only {
                                    p.accent.opacity(0.55)
                                } else {
                                    gpui_kit::transparent_black()
                                })
                                .text_color(if music_only { p.accent } else { p.text_muted })
                                .cursor_pointer()
                                .hover(move |s| {
                                    if music_only {
                                        s.opacity(0.9)
                                    } else {
                                        s.bg(p.surface_hover).text_color(p.text)
                                    }
                                })
                                .tip("只筛选 B 站音乐分区视频")
                                .child(icon(
                                    IconName::Music,
                                    13.0,
                                    if music_only { p.accent } else { p.text_muted },
                                ))
                                .child("仅音乐分区")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.data.search.music_only = !this.data.search.music_only;
                                    this.load_search(1, cx);
                                })),
                        )
                        .when(!page_tracks.is_empty(), |right| {
                            let play_list = page_tracks.clone();
                            let append_list = page_tracks.clone();
                            let count = page_tracks.len();
                            let st_play = self.state.clone();
                            let st_append = self.state.clone();
                            right
                                .child(
                                    icon_pill_button(
                                        "search-append-page",
                                        IconName::Plus,
                                        "加入队列",
                                        p,
                                    )
                                    .tip(format!("将本页全部 {count} 首加入播放列表"))
                                    .on_click(move |_, _, cx| {
                                        st_append.update(cx, |s, cx| {
                                            s.append_to_queue(append_list.clone(), cx);
                                        });
                                    }),
                                )
                                .child(
                                    primary_button(
                                        "search-play-page",
                                        IconName::Play,
                                        "播放整页",
                                        p,
                                    )
                                    .tip(format!("立即播放本页全部 {count} 首"))
                                    .on_click(move |_, _, cx| {
                                        st_play.update(cx, |s, cx| {
                                            s.play_queue(play_list.clone(), 0, cx);
                                        });
                                    }),
                                )
                        })
                        .child(display_mode_toggle(
                            &self.state,
                            grid,
                            p,
                            |this, _, cx| {
                                if let Some(Ok(Results::Videos(v))) =
                                    this.data.search.results.result.as_ref()
                                {
                                    let items = v.items.clone();
                                    this.prefetch_tracks(&items, cx);
                                }
                            },
                            cx,
                        )),
                )
            });

        root = root.child(controls);

        if search.query.is_empty() {
            return root.child(empty_state(
                "输入关键词开始探索",
                "支持按歌名、歌手、BV 号或 UP 主搜索",
                p,
            ));
        }

        root.child(search.results.content(
            p,
            button("retry", "重试", p).on_click(cx.listener(|this, _, _, cx| {
                this.load_search(this.data.search.page, cx);
            })),
            |result| {
                let go = page_change(cx, Self::load_search);
                match result {
                    Results::Videos(items) => column()
                        .gap_4()
                        .child(track_collection(
                            &self.state,
                            &items.items,
                            content_width,
                            p,
                            cx,
                        ))
                        .child(pager(items, search.page, p, go)),
                    Results::Users(items) => column()
                        .gap_2()
                        .when(items.items.is_empty(), |c| {
                            c.child(empty_state("没有找到 UP 主", "", p))
                        })
                        .children(items.items.iter().enumerate().map(|(i, user)| {
                            let mid = user.mid;
                            let route = Route::UserProfile {
                                mid,
                                name: user.name.clone(),
                            };
                            let fans = user
                                .extra
                                .get("fans")
                                .and_then(serde_json::Value::as_u64);
                            let level = user
                                .extra
                                .get("level")
                                .and_then(serde_json::Value::as_u64);
                            let st = self.state.clone();
                            div()
                                .id(("search-user", i))
                                .p_3()
                                .rounded(px(12.0))
                                .bg(p.surface)
                                .cursor_pointer()
                                .hover(move |s| s.bg(p.surface_hover))
                                .on_click(move |_, _, cx| {
                                    st.update(cx, |s, cx| s.navigate(route.clone(), cx))
                                })
                                .flex()
                                .items_center()
                                .gap_3()
                                .child(user_row(&self.state, user, p, cx))
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
                                })
                                .when_some(fans, |row, count| {
                                    let text = if count >= 10_000 {
                                        format!("{:.1}万粉丝", count as f64 / 10_000.0)
                                    } else {
                                        format!("{count} 粉丝")
                                    };
                                    row.child(
                                        div()
                                            .flex_shrink_0()
                                            .text_xs()
                                            .text_color(p.text_dim)
                                            .child(text),
                                    )
                                })
                                .when(self.account != Some(mid) && !self.mutating, |row| {
                                    row.child(
                                        icon_pill_button(
                                            ("search-follow", i),
                                            IconName::Plus,
                                            "关注",
                                            p,
                                        )
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            if this.account.is_none() {
                                                this.state.update(cx, |s, cx| {
                                                    s.open_modal(ActiveModal::Login, cx)
                                                });
                                                return;
                                            }
                                            this.mutate(
                                                move |s| async move {
                                                    s.bili.modify_relation(mid, 1).await
                                                },
                                                "已关注",
                                                |_, _| {},
                                                cx,
                                            );
                                        })),
                                    )
                                })
                        }))
                        .child(pager(items, search.page, p, go)),
                }
            },
        ))
    }
}

fn history_chips(
    state: &Entity<AppState>,
    items: &[String],
    p: BiuPalette,
    cx: &Context<PagesHost>,
) -> Div {
    toolbar().children(items.iter().enumerate().map(|(i, term)| {
        let search_term = term.clone();
        let del_term = term.clone();
        let st_del = state.clone();
        div()
            .id(("hist-chip", i))
            .h(px(28.0))
            .pl_3()
            .pr_2()
            .rounded_full()
            .bg(p.surface)
            .flex()
            .items_center()
            .gap_1p5()
            .text_xs()
            .text_color(p.text_muted)
            .cursor_pointer()
            .hover(move |s| s.bg(p.surface_hover).text_color(p.text))
            .on_click(cx.listener(move |this, _, window, cx| {
                this.submit_search(search_term.clone(), window, cx)
            }))
            .child(term.clone())
            .child(
                div()
                    .id(("hist-chip-del", i))
                    .size(px(16.0))
                    .rounded_full()
                    .flex()
                    .items_center()
                    .justify_center()
                    .hover(move |s| s.bg(p.surface_active))
                    .on_click(move |_, _, cx| {
                        cx.stop_propagation();
                        st_del.update(cx, |s, cx| s.remove_search_history(&del_term, cx));
                    })
                    .child(icon(IconName::X, 10.0, p.text_dim)),
            )
    }))
}

fn search_page<T>(result: SearchResult<T>) -> Page<T> {
    Page {
        items: result.items,
        total: Some(result.total),
        has_more: Some(result.page < result.pages),
        next_offset: None,
    }
}

#[cfg(test)]
mod tests {
    use super::search_page;
    use biu_core::SearchResult;

    #[test]
    fn search_pager_stops_at_last_page() {
        let result = |page| SearchResult::<()> {
            items: vec![],
            page,
            page_size: 20,
            total: 21,
            pages: 2,
        };
        assert_eq!(search_page(result(1)).has_more, Some(true));
        assert_eq!(search_page(result(2)).has_more, Some(false));
        assert_eq!(search_page(result(2)).total, Some(21));
    }
}
