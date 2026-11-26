use super::*;
use biu_core::{FollowingUser, SearchResult};
use std::time::Duration;

const ORDERS: [(&str, &str); 4] = [
    ("totalrank", "综合"),
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
            let input = new_input("搜索视频或 UP 主", query, window, cx);
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
        self.load(
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
            cx,
        );
    }

    pub(super) fn search_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let search = &self.data.search;
        let mut root = page("搜索");
        if let Some(input) = &search.input {
            let value = input.clone();
            root = root.child(
                toolbar()
                    .child(div().w(px(280.)).child(Input::new(input)))
                    .child(button("submit-search", "搜索", p).on_click(cx.listener(
                        move |this, _, window, cx| {
                            this.submit_search(
                                normalize_query(&value.read(cx).value()),
                                window,
                                cx,
                            );
                        },
                    ))),
            );
        }
        if let Some(Ok(suggestions)) = &search.suggestions.result {
            root = root.child(div().id("suggestions").child(chips(suggestions, p, cx)));
        }
        let state = self.state.read(cx);
        if state.settings.show_search_history && !state.search_history.is_empty() {
            root = root.child(
                div().id("search-history").child(
                    column()
                        .child(toolbar().child("搜索历史").child(state_button(
                            &self.state,
                            "clear-history",
                            "清空历史",
                            p,
                            AppState::clear_search_history,
                        )))
                        .child(chips(&state.search_history, p, cx)),
                ),
            );
        }
        root = root.child(
            toolbar().children([(false, "视频"), (true, "UP 主")].into_iter().map(
                |(users, title)| {
                    button(title, title, p)
                        .when(users == search.users, |b| b.border_color(p.accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.data.search.users = users;
                            this.load_search(1, cx);
                        }))
                },
            )),
        );
        if !search.users {
            root = root.child(toolbar().children(ORDERS.iter().enumerate().map(
                |(index, &(id, title))| {
                    button(id, title, p)
                        .when(index == search.order, |b| b.border_color(p.accent))
                        .on_click(cx.listener(move |this, _, _, cx| {
                            this.data.search.order = index;
                            this.load_search(1, cx);
                        }))
                },
            )));
        }
        if search.query.is_empty() {
            return root.child(empty_state("输入关键词开始搜索", "", p));
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
                        .child(track_list(&self.state, &items.items, p, cx))
                        .child(pager(items, search.page, p, go)),
                    Results::Users(items) => column()
                        .when(items.items.is_empty(), |c| {
                            c.child(empty_state("没有找到 UP 主", "", p))
                        })
                        .children(items.items.iter().enumerate().map(|(i, user)| {
                            div()
                                .id(i)
                                .p_2()
                                .rounded(p.radius)
                                .bg(p.surface)
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
        button(i, term.clone(), p).on_click(
            cx.listener(move |this, _, window, cx| this.submit_search(term.clone(), window, cx)),
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
