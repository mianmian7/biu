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
            Some(self.filter_input("搜索播放历史", window, cx, Self::filter_history));
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
        self.load(
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
            cx,
        );
    }

    pub(super) fn history_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let mut root = page("播放历史");
        let history = &self.data.history;
        let Some(input) = &history.input else {
            return root.child(self.login_hint(p));
        };
        root = root.child(filter_bar(input, "搜索", p, cx, Self::filter_history));
        if !self.mutating {
            root =
                if history.confirm_clear {
                    root.child(
                        toolbar()
                            .child("清空哔哩哔哩账号的全部历史记录？")
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
                            ))),
                    )
                } else {
                    root.child(toolbar().child(button("clear", "清空历史", p).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.data.history.confirm_clear = true;
                            cx.notify();
                        }),
                    )))
                };
        }
        root.child(
            history.results.content(
                p,
                button("retry", "重试", p)
                    .on_click(cx.listener(|this, _, _, cx| this.load_history(cx))),
                |result| {
                    column()
                        .when(result.items.is_empty(), |c| {
                            c.child(empty_state("没有历史记录", "", p))
                        })
                        .children(result.items.iter().enumerate().map(|(i, item)| {
                            let row = if let Some(track) = &item.track {
                                track_row(&self.state, i, track, p, cx)
                            } else {
                                div()
                                    .id(i)
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .p_2()
                                    .bg(p.surface)
                                    .rounded(p.radius)
                                    .child(item.title.clone())
                            };
                            let kid = item.kid.clone();
                            row.when(!kid.is_empty() && !self.mutating, |row| {
                                row.child(button("delete", "删除", p).on_click(cx.listener(
                                    move |this, _, _, cx| {
                                        let kid = kid.clone();
                                        this.mutate(
                                                move |s| async move {
                                                    s.bili.delete_history(&kid).await
                                                },
                                                "已删除历史记录",
                                                Self::reset_history,
                                                cx,
                                            );
                                    },
                                )))
                            })
                        }))
                        .when(result.next_offset.is_some(), |c| {
                            c.child(toolbar().child(
                                button("more", "加载更多", p).on_click(
                                    cx.listener(|this, _, _, cx| this.append_history(cx)),
                                ),
                            ))
                        })
                },
            ),
        )
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
