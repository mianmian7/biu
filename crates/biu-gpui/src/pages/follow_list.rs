use super::*;
use biu_core::FollowingUser;

#[derive(Default)]
pub(super) struct FollowList {
    input: Option<Entity<InputState>>,
    keyword: String,
    page: u64,
    results: Load<Page<FollowingUser>>,
}

impl PagesHost {
    pub(super) fn enter_follows(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        if self.account.is_none() {
            return;
        }
        self.data.follows.input =
            Some(self.filter_input("搜索关注的 UP 主…", window, cx, Self::filter_follows));
        self.load_follows(1, cx);
    }

    fn filter_follows(&mut self, keyword: String, cx: &mut Context<Self>) {
        self.data.follows.keyword = keyword;
        self.load_follows(1, cx);
    }

    fn load_follows(&mut self, page: u64, cx: &mut Context<Self>) {
        let Some(mid) = self.account else {
            return;
        };
        self.data.follows.page = page;
        let keyword = self.data.follows.keyword.clone();
        self.load_with(
            |h| &mut h.data.follows.results,
            move |s| async move {
                s.bili
                    .get_followings(mid, page, (!keyword.is_empty()).then_some(keyword.as_str()))
                    .await
            },
            |this, page_data, cx| {
                let covers = page_data
                    .items
                    .iter()
                    .take(40)
                    .filter(|u| !u.face.is_empty())
                    .map(|u| (u.face.clone(), ThumbSize::Avatar))
                    .collect();
                this.prefetch_covers(covers, cx);
            },
            cx,
        );
    }

    pub(super) fn follows_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let follows = &self.data.follows;
        let Some(input) = &follows.input else {
            return page("我的关注").child(self.login_hint(p));
        };
        let state = self.state.clone();

        let header = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child("我的关注"),
            )
            .child(filter_bar(input, "筛选", p, cx, Self::filter_follows));

        page("").child(header).child(follows.results.content(
            p,
            button("retry", "重试", p)
                .on_click(cx.listener(|this, _, _, cx| this.refresh_follows(cx))),
            |result| {
                column()
                    .gap_2()
                    .when(result.items.is_empty(), |c| {
                        c.child(empty_state("没有找到关注的 UP 主", "", p))
                    })
                    .children(result.items.iter().enumerate().map(|(i, user)| {
                        let mid = user.mid;
                        let route = Route::UserProfile {
                            mid,
                            name: user.name.clone(),
                        };
                        let state = state.clone();
                        div()
                            .id(("follow-row", i))
                            .flex()
                            .items_center()
                            .gap_3()
                            .p_3()
                            .rounded(px(12.0))
                            .bg(p.surface)
                            .cursor_pointer()
                            .hover(move |style| style.bg(p.surface_hover))
                            .on_click(move |_, _, cx| {
                                state.update(cx, |s, cx| s.navigate(route.clone(), cx))
                            })
                            .child(user_row(&self.state, user, p, cx))
                            .when(!self.mutating, |row| {
                                row.child(button(("unfollow", i), "取消关注", p).on_click(
                                    cx.listener(move |this, _, _, cx| {
                                        cx.stop_propagation();
                                        this.mutate(
                                            move |s| async move {
                                                s.bili.modify_relation(mid, 2).await
                                            },
                                            "已取消关注",
                                            Self::refresh_follows,
                                            cx,
                                        );
                                    }),
                                ))
                            })
                    }))
                    .child(pager(
                        result,
                        follows.page,
                        p,
                        page_change(cx, Self::load_follows),
                    ))
            },
        ))
    }

    fn refresh_follows(&mut self, cx: &mut Context<Self>) {
        self.load_follows(self.data.follows.page, cx);
    }
}
