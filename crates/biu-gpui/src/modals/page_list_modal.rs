use super::*;
use crate::pages::primary_button;
use crate::widgets::format_duration;
use biu_core::TrackSource;

#[derive(Default)]
pub(super) struct PageListModal {
    filter: Option<Entity<InputState>>,
}

impl ModalsHost {
    pub(super) fn enter_page_list(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = new_input("搜索分集标题…", "", window, cx);
        cx.subscribe_in(&input, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        self.data.page_list = PageListModal {
            filter: Some(input),
        };
    }

    pub(super) fn page_list_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let s = self.state.read(cx);
        let pages = s.current_video_pages.clone();
        let active_cid = match s.current_track().map(|t| &t.source) {
            Some(TrackSource::BiliVideo { cid, .. }) => *cid,
            _ => 0,
        };
        if pages.is_empty() {
            return empty_state("当前视频没有分集信息", "", p);
        }
        let query = self
            .data
            .page_list
            .filter
            .as_ref()
            .map(|i| i.read(cx).value().trim().to_lowercase())
            .unwrap_or_default();
        let filtered: Vec<_> = pages
            .iter()
            .filter(|pg| {
                query.is_empty()
                    || pg.part.to_lowercase().contains(&query)
                    || format!("p{}", pg.page).contains(&query)
            })
            .cloned()
            .collect();

        let state = self.state.clone();
        let top = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_2()
            .when_some(self.data.page_list.filter.as_ref(), |bar, input| {
                bar.child(
                    div()
                        .flex_1()
                        .h(px(34.0))
                        .px_3()
                        .rounded_full()
                        .bg(p.surface)
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(crate::player_bar::icon(IconName::Search, 14.0, p.text_dim))
                        .child(
                            div()
                                .flex_1()
                                .min_w_0()
                                .text_sm()
                                .child(Input::new(input).appearance(false).cleanable(true)),
                        ),
                )
            })
            .child(
                primary_button("queue-all-parts", IconName::Plus, "全部加入队列", p).on_click({
                    let st = state.clone();
                    move |_, _, cx| {
                        st.update(cx, |s, cx| {
                            s.queue_all_video_parts(cx);
                            s.close_modal(cx);
                        });
                    }
                }),
            );

        column()
            .gap_3()
            .child(top)
            .child(
                div()
                    .text_xs()
                    .text_color(p.text_dim)
                    .child(format!("共 {} 个分集（显示 {} 个）", pages.len(), filtered.len())),
            )
            .when(filtered.is_empty(), |col| {
                col.child(empty_state("没有匹配的分集", "", p))
            })
            .children(filtered.into_iter().enumerate().map(|(i, pg)| {
                let active = active_cid > 0 && pg.cid == active_cid;
                let st = state.clone();
                let part_num = pg.page;
                let part_title = if pg.part.is_empty() {
                    format!("P{part_num}")
                } else {
                    pg.part.clone()
                };
                let dur = (pg.duration > 0.0).then(|| format_duration(pg.duration));
                div()
                    .id(("video-part", i))
                    .h(px(44.0))
                    .w_full()
                    .px_3()
                    .rounded(px(10.0))
                    .flex()
                    .items_center()
                    .gap_3()
                    .cursor_pointer()
                    .when(active, |d| d.bg(p.accent_soft))
                    .when(!active, |d| d.hover(move |s| s.bg(p.surface_hover)))
                    .on_click(move |_, _, cx| {
                        let chosen = pg.clone();
                        st.update(cx, |s, cx| {
                            s.switch_video_part(&chosen, cx);
                            s.close_modal(cx);
                        });
                    })
                    .child(
                        div()
                            .w(px(36.0))
                            .flex_shrink_0()
                            .text_xs()
                            .font_weight(FontWeight::SEMIBOLD)
                            .text_color(if active { p.accent } else { p.text_dim })
                            .child(format!("P{part_num}")),
                    )
                    .child(
                        div()
                            .flex_1()
                            .min_w_0()
                            .text_sm()
                            .font_weight(if active {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::NORMAL
                            })
                            .text_color(if active { p.accent } else { p.text })
                            .truncate()
                            .child(part_title),
                    )
                    .when_some(dur, |row, d| {
                        row.child(div().text_xs().text_color(p.text_dim).child(d))
                    })
            }))
    }
}
