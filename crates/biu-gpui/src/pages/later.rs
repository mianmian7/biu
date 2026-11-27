use super::*;

#[derive(Default)]
pub(super) struct Later {
    input: Option<Entity<InputState>>,
    keyword: String,
    confirm_clear_viewed: bool,
    results: Load<Page<TrackItem>>,
    page: u64,
}

impl PagesHost {
    pub(super) fn enter_later(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.account.is_none() {
            return;
        }
        self.data.later.input =
            Some(self.filter_input("搜索稍后再看…", window, cx, Self::filter_later));
        self.load_later(1, cx);
    }

    fn filter_later(&mut self, keyword: String, cx: &mut Context<Self>) {
        self.data.later.keyword = keyword;
        self.load_later(1, cx);
    }

    pub(super) fn load_later(&mut self, page: u64, cx: &mut Context<Self>) {
        if self.account.is_none() {
            return;
        }
        self.data.later.page = page;
        self.data.later.confirm_clear_viewed = false;
        let keyword = self.data.later.keyword.clone();
        self.load_with(
            |h| &mut h.data.later.results,
            move |s| async move {
                s.bili
                    .get_later_list_filtered(page, (!keyword.is_empty()).then_some(keyword.as_str()))
                    .await
            },
            |this, page_data, cx| this.prefetch_tracks(&page_data.items, cx),
            cx,
        );
    }

    pub(super) fn later_view(&self, p: BiuPalette, content_width: f32, cx: &Context<Self>) -> Div {
        if self.account.is_none() {
            return page("稍后再看").child(self.login_hint(p));
        }
        let grid = is_grid_mode(self.state.read(cx));
        let reload = || cx.listener(|this, _, _, cx| this.refresh_later(cx));
        let data = &self.data.later;

        let clear_viewed = if self.mutating {
            toolbar()
        } else if data.confirm_clear_viewed {
            toolbar()
                .child(div().text_xs().text_color(p.danger).child("清除已看完？"))
                .child(
                    button("confirm-clear-viewed", "确认", p)
                        .text_color(p.danger)
                        .on_click(cx.listener(|this, _, _, cx| {
                            this.data.later.confirm_clear_viewed = false;
                            this.mutate(
                                |s| async move { s.bili.clear_viewed_later().await },
                                "已清除已看完的视频",
                                Self::refresh_later,
                                cx,
                            );
                        })),
                )
                .child(button("cancel-clear-viewed", "取消", p).on_click(cx.listener(
                    |this, _, _, cx| {
                        this.data.later.confirm_clear_viewed = false;
                        cx.notify();
                    },
                )))
        } else {
            toolbar().child(
                icon_pill_button("clear-viewed", IconName::Trash, "清除已看完", p).on_click(
                    cx.listener(|this, _, _, cx| {
                        this.data.later.confirm_clear_viewed = true;
                        cx.notify();
                    }),
                ),
            )
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
                    .child("稍后再看"),
            )
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .items_center()
                    .gap_2()
                    .when_some(data.input.as_ref(), |bar, input| {
                        bar.child(filter_bar(input, "搜索", p, cx, Self::filter_later))
                    })
                    .child(clear_viewed)
                    .child(display_mode_toggle(
                        &self.state,
                        grid,
                        p,
                        |this, _, cx| {
                            if let Some(Ok(res)) = this.data.later.results.result.as_ref() {
                                let items = res.items.clone();
                                this.prefetch_tracks(&items, cx);
                            }
                        },
                        cx,
                    ))
                    .child(
                        icon_pill_button("refresh", IconName::RotateCw, "刷新", p)
                            .on_click(reload()),
                    ),
            );

        page("").child(header).child(data.results.content(
            p,
            button("retry", "重试", p).on_click(reload()),
            |result| {
                if result.items.is_empty() {
                    return empty_state("稍后再看为空", "在哔哩哔哩添加的视频会显示在这里", p);
                }
                column()
                    .gap_4()
                    .child(self.paginated_actions(result, data.page, p, Self::all_later, cx))
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
                                track_row(&self.state, i, track, p, cx).when(!self.mutating, |row| {
                                    row.when_some(
                                        playable_id(track).filter(|(_, kind)| *kind == 2),
                                        |row, (aid, _)| {
                                            row.child(
                                                compact_icon_btn(
                                                    ("rm-later", i),
                                                    IconName::Trash,
                                                    "移出稍后再看",
                                                    p,
                                                )
                                                .on_click(cx.listener(
                                                    move |this, _, _, cx| {
                                                        cx.stop_propagation();
                                                        this.mutate(
                                                            move |s| async move {
                                                                s.bili.delete_later(aid).await
                                                            },
                                                            "已移出稍后再看",
                                                            Self::refresh_later,
                                                            cx,
                                                        );
                                                    },
                                                )),
                                            )
                                        },
                                    )
                                })
                            }))
                    })
                    .child(pager(
                        result,
                        data.page,
                        p,
                        page_change(cx, Self::load_later),
                    ))
            },
        ))
    }

    fn all_later(&mut self, append: bool, cx: &mut Context<Self>) {
        let keyword = self.data.later.keyword.clone();
        self.queue_all(
            move |s, page| {
                let kw = keyword.clone();
                async move {
                    s.bili
                        .get_later_list_filtered(page, (!kw.is_empty()).then_some(kw.as_str()))
                        .await
                }
            },
            append,
            cx,
        );
    }

    fn refresh_later(&mut self, cx: &mut Context<Self>) {
        self.load_later(self.data.later.page, cx);
    }
}
