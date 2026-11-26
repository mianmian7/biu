use super::*;
use biu_core::HistoryItem;

#[derive(Default)]
pub(super) struct History {
    input: Option<Entity<InputState>>,
    keyword: String,
    prior: Vec<HistoryItem>,
    cursor: Option<String>,
    results: Load<Page<HistoryItem>>,
    confirm_clear: bool,
}

impl PagesHost {
    pub(super) fn enter_history(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.account.is_none() {
            return;
        }
        self.data.history.input =
            Some(self.filter_input("搜索播放历史…", window, cx, Self::filter_history));
        self.reset_history(cx);
    }

    fn filter_history(&mut self, keyword: String, cx: &mut Context<Self>) {
        self.data.history.keyword = keyword;
        self.reset_history(cx);
    }

    fn reset_history(&mut self, cx: &mut Context<Self>) {
        let history = &mut self.data.history;
        history.prior.clear();
        history.cursor = None;
        history.confirm_clear = false;
        self.load_history(cx);
    }

    fn append_history(&mut self, cx: &mut Context<Self>) {
        let history = &mut self.data.history;
        if let Some(Ok(result)) = history.results.result.take() {
            history.prior = result.items;
            history.cursor = result.next_offset;
            self.load_history(cx);
        }
    }

    fn load_history(&mut self, cx: &mut Context<Self>) {
        let history = &self.data.history;
        let keyword = history.keyword.clone();
        let requested = history.cursor.clone();
        let prior = history.prior.clone();
        self.load_with(
            |h| &mut h.data.history.results,
            move |s| async move {
                let mut response = if keyword.is_empty() {
                    let (max, view_at) = requested
                        .as_deref()
                        .and_then(parse_cursor)
                        .unwrap_or_default();
                    s.bili.get_history(max, view_at, None).await?
                } else {
                    let page = requested
                        .as_deref()
                        .and_then(|cursor| cursor.parse().ok())
                        .unwrap_or(1);
                    s.bili.search_history(&keyword, page, 0).await?
                };
                if response.next_offset == requested {
                    response.next_offset = None;
                }
                let mut items = prior;
                items.append(&mut response.items);
                let mut seen = std::collections::HashSet::new();
                items.retain(|item| item.kid.is_empty() || seen.insert(item.kid.clone()));
                response.items = items;
                Ok(response)
            },
            |this, page_data, cx| {
                let covers = page_data
                    .items
                    .iter()
                    .take(48)
                    .filter_map(|item| item.track.as_ref())
                    .filter(|t| !t.cover.is_empty())
                    .map(|t| (t.cover.clone(), ThumbSize::Tiny))
                    .collect();
                this.prefetch_covers(covers, cx);
            },
            cx,
        );
    }

    pub(super) fn history_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let history = &self.data.history;
        let Some(input) = &history.input else {
            return page("历史记录").child(self.login_hint(p));
        };

        let clear_control = if !self.mutating {
            if history.confirm_clear {
                toolbar()
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.danger)
                            .child("确认清空全部历史？"),
                    )
                    .child(
                        button("confirm-clear", "确认清空", p)
                            .text_color(p.danger)
                            .on_click(cx.listener(|this, _, _, cx| {
                                this.data.history.confirm_clear = false;
                                this.mutate(
                                    |s| async move { s.bili.clear_history().await },
                                    "已清空历史",
                                    Self::reset_history,
                                    cx,
                                );
                            })),
                    )
                    .child(button("cancel-clear", "取消", p).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.data.history.confirm_clear = false;
                            cx.notify();
                        },
                    )))
            } else {
                toolbar().child(
                    icon_pill_button("clear", IconName::Trash, "清空历史", p).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.data.history.confirm_clear = true;
                            cx.notify();
                        }),
                    ),
                )
            }
        } else {
            toolbar()
        };

        let header = div()
            .w_full()
            .flex()
            .flex_wrap()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child("历史记录"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .child(filter_bar(input, "搜索", p, cx, Self::filter_history))
                    .child(clear_control),
            );

        page("").child(header).child(history.results.content(
            p,
            button("retry", "重试", p)
                .on_click(cx.listener(|this, _, _, cx| this.load_history(cx))),
            |result| {
                if result.items.is_empty() {
                    return empty_state("没有历史记录", "", p);
                }
                column()
                    .gap(px(2.0))
                    .child(track_list_header(p))
                    .children(result.items.iter().enumerate().map(|(i, item)| {
                        let row = if let Some(track) = &item.track {
                            track_row(&self.state, i, track, p, cx)
                        } else {
                            div()
                                .id(("hist-plain", i))
                                .h(px(52.0))
                                .px_3()
                                .flex()
                                .items_center()
                                .justify_between()
                                .rounded(px(10.0))
                                .hover(move |s| s.bg(p.surface_hover))
                                .child(div().text_sm().truncate().child(item.title.clone()))
                        };
                        let kid = item.kid.clone();
                        row.when(!kid.is_empty() && !self.mutating, |row| {
                            row.child(
                                compact_icon_btn(("del-hist", i), IconName::Trash, "删除记录", p)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        let kid = kid.clone();
                                        this.mutate(
                                            move |s| async move {
                                                s.bili.delete_history(&kid).await
                                            },
                                            "已删除历史记录",
                                            Self::reset_history,
                                            cx,
                                        );
                                    })),
                            )
                        })
                    }))
                    .when(result.next_offset.is_some(), |c| {
                        c.child(
                            div().w_full().pt_4().flex().justify_center().child(
                                icon_pill_button("more", IconName::Plus, "加载更多", p)
                                    .on_click(cx.listener(|this, _, _, cx| {
                                        this.append_history(cx)
                                    })),
                            ),
                        )
                    })
            },
        ))
    }
}

fn parse_cursor(cursor: &str) -> Option<(u64, u64)> {
    let (max, view_at) = cursor.split_once(':')?;
    Some((max.parse().ok()?, view_at.parse().ok()?))
}

#[cfg(test)]
mod tests {
    #[test]
    fn decodes_history_cursor_and_rejects_invalid_input() {
        assert_eq!(super::parse_cursor("15:200"), Some((15, 200)));
        assert_eq!(super::parse_cursor("1"), None);
        assert_eq!(super::parse_cursor("15:x"), None);
    }
}
