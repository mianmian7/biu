use super::*;

#[derive(Default)]
pub(super) struct Later {
    results: Load<Page<TrackItem>>,
    page: u64,
}

impl PagesHost {
    pub(super) fn load_later(&mut self, page: u64, cx: &mut Context<Self>) {
        if self.account.is_none() {
            return;
        }
        self.data.later.page = page;
        self.load_with(
            |h| &mut h.data.later.results,
            move |s| async move {
                if page == 1 {
                    s.bili.get_later_list().await
                } else {
                    s.bili.get_later_list_page(page).await
                }
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

        let header = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child("稍后再看"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
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
        self.queue_all(
            |s, page| async move { s.bili.get_later_list_page(page).await },
            append,
            cx,
        );
    }

    fn refresh_later(&mut self, cx: &mut Context<Self>) {
        self.load_later(self.data.later.page, cx);
    }
}
