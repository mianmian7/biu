use super::*;
use biu_core::{FollowingUser, SearchResult};
use std::time::Duration;

const ORDERS: [(&str, &str); 4] = [
    ("totalrank", "综合排序"),
    ("click", "最多播放"),
    ("pubdate", "最新发布"),
    ("stow", "最多收藏"),
];

enum Results {
    Videos(Page<TrackItem>),
    Users(Page<FollowingUser>),
}

#[derive(Default)]
pub(super) struct Search {
    input: Option<Entity<InputState>>,
    query: String,
    order: usize,
    users: bool,
    page: u64,
    results: Load<Results>,
    suggestions: Load<Vec<String>>,
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
                        InputEvent::Change => this.suggest(query, cx),
                        InputEvent::PressEnter { .. } => this.submit_search(query, window, cx),
                        _ => {}
                    }
                },
            )
            .detach();
            self.data.search.input = Some(input);
        }
        self.load_search(1, cx);
    }

    fn submit_search(&mut self, query: String, window: &mut Window, cx: &mut Context<Self>) {
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
        if query.is_empty() || query == self.data.search.query {
            self.data.search.suggestions = Load::default();
            cx.notify();
            return;
        }
        self.load(
            |h| &mut h.data.search.suggestions,
            move |s| async move {
                tokio::time::sleep(Duration::from_millis(250)).await;
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
        let order = ORDERS[search.order].0;
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
                        s.bili.search_users(&query, page).await?,
                    )))
                } else {
                    Ok(Results::Videos(search_page(
                        s.bili.search_videos(&query, order, page).await?,
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

        let mut root = page("搜索发现");
        if let Some(input) = &search.input {
            let value = input.clone();
            root = root.child(
                div()
                    .flex()
                    .items_center()
                    .gap_2p5()
                    .child(
                        div()
                            .w(px(380.0))
                            .max_w_full()
                            .h(px(40.0))
                            .px_4()
                            .rounded_full()
                            .bg(p.surface)
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
        }

        if let Some(Ok(suggestions)) = &search.suggestions.result
            && !suggestions.is_empty()
        {
            root = root.child(div().id("suggestions").child(chips(suggestions, p, cx)));
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
                        .child(chips(&state.search_history, p, cx)),
                ),
            );
        }

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
                    .when(!search.users, |bar| {
                        bar.child(toolbar().children(ORDERS.iter().enumerate().map(
                            |(index, &(id, title))| {
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
                            },
                        )))
                    }),
            )
            .when(!search.users, |bar| {
                bar.child(display_mode_toggle(
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
                ))
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
                            let route = Route::UserProfile {
                                mid: user.mid,
                                name: user.name.clone(),
                            };
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
                                .child(user_row(&self.state, user, p, cx))
                        }))
                        .child(pager(items, search.page, p, go)),
                }
            },
        ))
    }
}

fn chips(items: &[String], p: BiuPalette, cx: &Context<PagesHost>) -> Div {
    toolbar().children(items.iter().enumerate().map(|(i, term)| {
        let term = term.clone();
        div()
            .id(("chip", i))
            .h(px(28.0))
            .px_3()
            .rounded_full()
            .bg(p.surface)
            .flex()
            .items_center()
            .text_xs()
            .text_color(p.text_muted)
            .cursor_pointer()
            .hover(move |s| s.bg(p.surface_hover).text_color(p.text))
            .child(term.clone())
            .on_click(cx.listener(move |this, _, window, cx| {
                this.submit_search(term.clone(), window, cx)
            }))
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
