use super::*;
use crate::pages::compact_icon_btn;
use crate::player_bar::{play_mode_icon, play_mode_short_label};
use biu_core::image_cache::ThumbSize;

impl ModalsHost {
    pub(super) fn queue_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let s = self.state.read(cx);
        let queue_len = s.queue.len();
        let mode = s.queue.mode();
        let active_index = s.queue.current_index();
        let state = self.state.clone();

        let header = div()
            .w_full()
            .pb_2()
            .border_b_1()
            .border_color(p.border)
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_xs()
                    .text_color(p.text_muted)
                    .child(format!("共 {queue_len} 首歌曲")),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .child(
                        compact_icon_btn(
                            "queue-mode",
                            play_mode_icon(mode),
                            play_mode_short_label(mode),
                            p,
                        )
                        .on_click({
                            let st = state.clone();
                            move |_, _, cx| st.update(cx, AppState::toggle_play_mode)
                        }),
                    )
                    .when(queue_len > 0, |bar| {
                        let st = state.clone();
                        bar.child(
                            compact_icon_btn("queue-clear", IconName::Trash, "清空播放列表", p)
                                .on_click(move |_, _, cx| st.update(cx, AppState::clear_queue)),
                        )
                    }),
            );

        if queue_len == 0 {
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(header)
                .child(empty_state("播放列表为空", "点击任意歌曲即可开始播放", p));
        }

        let st_list = state.clone();
        let list = uniform_list("playlist-virtual-list", queue_len, move |range, _, cx| {
            let items = st_list.read(cx).queue.items().to_vec();
            range
                .filter_map(|i| {
                    let track = items.get(i)?;
                    let active = active_index == Some(i);
                    let group: SharedString = format!("qrow-{i}").into();
                    let play_id = track.id.clone();
                    let remove_id = track.id.clone();
                    let st_play = st_list.clone();
                    let st_rm = st_list.clone();
                    Some(
                        div()
                            .id(("queue-item", i))
                            .group(group.clone())
                            .h(px(62.0))
                            .w_full()
                            .px_2()
                            .rounded(px(10.0))
                            .flex()
                            .items_center()
                            .gap_3()
                            .cursor_pointer()
                            .when(active, |d| d.bg(p.accent_soft))
                            .when(!active, |d| d.hover(move |s| s.bg(p.surface_hover)))
                            .on_click(move |_, _, cx| {
                                st_play.update(cx, |s, cx| {
                                    if s.queue
                                        .items()
                                        .get(i)
                                        .is_some_and(|t| t.id == play_id)
                                    {
                                        s.jump_to_queue_index(i, cx);
                                    }
                                })
                            })
                            .child(thumbnail(
                                &st_list,
                                &track.cover,
                                ThumbSize::Tiny,
                                (44., 44.),
                                p,
                                cx,
                            ))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .flex()
                                    .flex_col()
                                    .gap(px(2.0))
                                    .child(
                                        div()
                                            .text_sm()
                                            .font_weight(if active {
                                                FontWeight::SEMIBOLD
                                            } else {
                                                FontWeight::MEDIUM
                                            })
                                            .text_color(if active { p.accent } else { p.text })
                                            .truncate()
                                            .child(track.title.clone()),
                                    )
                                    .child(
                                        div()
                                            .text_xs()
                                            .text_color(p.text_muted)
                                            .truncate()
                                            .child(track.artist.clone()),
                                    ),
                            )
                            .child(
                                div()
                                    .invisible()
                                    .group_hover(group, |s| s.visible())
                                    .child(
                                        compact_icon_btn(
                                            ("q-rm", i),
                                            IconName::X,
                                            "从列表移除",
                                            p,
                                        )
                                        .on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            st_rm.update(cx, |s, cx| {
                                                if s.queue
                                                    .items()
                                                    .get(i)
                                                    .is_some_and(|t| t.id == remove_id)
                                                {
                                                    s.remove_from_queue(i, cx);
                                                }
                                            })
                                        }),
                                    ),
                            ),
                    )
                })
                .collect()
        })
        .flex_1()
        .min_h_0()
        .w_full();

        div()
            .flex_1()
            .min_h_0()
            .flex()
            .flex_col()
            .gap_2()
            .child(header)
            .child(list)
    }
}
