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
        self.load(
            |h| &mut h.data.later.results,
            move |s| async move {
                if page == 1 {
                    s.bili.get_later_list().await
                } else {
                    s.bili.get_later_list_page(page).await
                }
            },
            cx,
        );
    }

    pub(super) fn later_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let root = page("稍后再看");
        if self.account.is_none() {
            return root.child(self.login_hint(p));
        }
        let reload = || cx.listener(|this, _, _, cx| this.refresh_later(cx));
        let data = &self.data.later;
        root.child(toolbar().child(button("refresh", "刷新", p).on_click(reload())))
            .child(data.results.content(
                p,
                button("retry", "重试", p).on_click(reload()),
                |result| {
                    column()
                        .child(self.paginated_actions(result, data.page, p, Self::all_later, cx))
                        .when(result.items.is_empty(), |c| {
                            c.child(empty_state(
                                "稍后再看为空",
                                "在哔哩哔哩添加的视频会显示在这里",
                                p,
                            ))
                        })
                        .children(result.items.iter().enumerate().map(|(i, track)| {
                            track_row(&self.state, i, track, p, cx).when(!self.mutating, |row| {
                                row.when_some(
                                    playable_id(track).filter(|(_, kind)| *kind == 2),
                                    |row, (aid, _)| {
                                        row.child(button("remove", "移出", p).on_click(
                                            cx.listener(move |this, _, _, cx| {
                                                this.mutate(
                                                    move |s| async move {
                                                        s.bili.delete_later(aid).await
                                                    },
                                                    "已移出稍后再看",
                                                    Self::refresh_later,
                                                    cx,
                                                );
                                            }),
                                        ))
                                    },
                                )
                            })
                        }))
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
