use super::*;
use crate::pages::compact_icon_btn;
use crate::player_bar::{icon, play_mode_icon, play_mode_short_label};
use biu_core::TrackSource;
use biu_core::image_cache::ThumbSize;

#[derive(Default)]
pub(super) struct PlaylistDrawerState {
    filter: Option<Entity<InputState>>,
    scroll: UniformListScrollHandle,
}

impl ModalsHost {
    pub(super) fn enter_queue(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = new_input("搜索播放列表…", "", window, cx);
        cx.subscribe_in(&input, window, |_, _, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change) {
                cx.notify();
            }
        })
        .detach();
        self.data.queue = PlaylistDrawerState {
            filter: Some(input),
            scroll: UniformListScrollHandle::new(),
        };
    }

    pub(super) fn queue_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let s = self.state.read(cx);
        let all_items = s.queue.items().to_vec();
        let queue_len = all_items.len();
        let mode = s.queue.mode();
        let active_index = s.queue.current_index();
        let state = self.state.clone();

        let query = self
            .data
            .queue
            .filter
            .as_ref()
            .map(|i| i.read(cx).value().trim().to_lowercase())
            .unwrap_or_default();

        let matched_indices: Vec<usize> = all_items
            .iter()
            .enumerate()
            .filter(|(_, t)| {
                query.is_empty()
                    || t.title.to_lowercase().contains(&query)
                    || t.artist.to_lowercase().contains(&query)
            })
            .map(|(idx, _)| idx)
            .collect();

        let active_pos_in_filtered =
            active_index.and_then(|ai| matched_indices.iter().position(|&idx| idx == ai));
        let scroll_handle = self.data.queue.scroll.clone();

        let header = div()
            .w_full()
            .pb_2()
            .border_b_1()
            .border_color(p.border)
            .flex()
            .flex_col()
            .gap_2()
            .child(
                div()
                    .w_full()
                    .flex()
                    .items_center()
                    .justify_between()
                    .child(
                        div()
                            .text_xs()
                            .text_color(p.text_muted)
                            .child(if query.is_empty() {
                                format!("共 {queue_len} 首歌曲")
                            } else {
                                format!("匹配 {} / {queue_len} 首", matched_indices.len())
                            }),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .when_some(active_pos_in_filtered, |bar, pos| {
                                let handle = scroll_handle.clone();
                                bar.child(
                                    compact_icon_btn(
                                        "queue-locate",
                                        IconName::LocateFixed,
                                        "定位当前播放",
                                        p,
                                    )
                                    .on_click(move |_, _, _| {
                                        handle.scroll_to_item(pos, ScrollStrategy::Center);
                                    }),
                                )
                            })
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
                                    compact_icon_btn(
                                        "queue-clear",
                                        IconName::Trash,
                                        "清空播放列表",
                                        p,
                                    )
                                    .on_click(move |_, _, cx| st.update(cx, AppState::clear_queue)),
                                )
                            }),
                    ),
            )
            .when_some(
                self.data.queue.filter.as_ref().filter(|_| queue_len > 0),
                |col, input| {
                    col.child(
                        div()
                            .w_full()
                            .h(px(32.0))
                            .px_3()
                            .rounded_full()
                            .bg(p.surface)
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(icon(IconName::Search, 13.0, p.text_dim))
                            .child(
                                div()
                                    .flex_1()
                                    .min_w_0()
                                    .text_xs()
                                    .child(Input::new(input).appearance(false).cleanable(true)),
                            ),
                    )
                },
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

        if matched_indices.is_empty() {
            return div()
                .flex_1()
                .min_h_0()
                .flex()
                .flex_col()
                .child(header)
                .child(empty_state("没有匹配的歌曲", "", p));
        }

        let st_list = state.clone();
        let filtered_len = matched_indices.len();
        let list = uniform_list("playlist-virtual-list", filtered_len, move |range, _, cx| {
            let items = st_list.read(cx).queue.items().to_vec();
            range
                .filter_map(|row_i| {
                    let orig_i = *matched_indices.get(row_i)?;
                    let track = items.get(orig_i)?;
                    let active = active_index == Some(orig_i);
                    let group: SharedString = format!("qrow-{orig_i}").into();
                    let play_id = track.id.clone();
                    let remove_id = track.id.clone();
                    let fav_track = track.clone();
                    let open_track = track.clone();
                    let is_local = matches!(&track.source, TrackSource::LocalFile { .. });
                    let st_play = st_list.clone();
                    let st_rm = st_list.clone();
                    let st_fav = st_list.clone();
                    let st_open = st_list.clone();
                    Some(
                        div()
                            .id(("queue-item", orig_i))
                            .group(group.clone())
                            .h(px(62.0))
                            .w_full()
                            .px_2()
                            .rounded(px(10.0))
                            .flex()
                            .items_center()
                            .gap_2p5()
                            .cursor_pointer()
                            .when(active, |d| d.bg(p.accent_soft))
                            .when(!active, |d| d.hover(move |s| s.bg(p.surface_hover)))
                            .on_click(move |_, _, cx| {
                                st_play.update(cx, |s, cx| {
                                    if s.queue
                                        .items()
                                        .get(orig_i)
                                        .is_some_and(|t| t.id == play_id)
                                    {
                                        s.jump_to_queue_index(orig_i, cx);
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
                                    .flex()
                                    .items_center()
                                    .gap_0p5()
                                    .when(!is_local, |bar| {
                                        bar.child(
                                            compact_icon_btn(
                                                ("q-fav", orig_i),
                                                IconName::Star,
                                                "收藏",
                                                p,
                                            )
                                            .on_click(move |_, _, cx| {
                                                cx.stop_propagation();
                                                st_fav.update(cx, |s, cx| {
                                                    let modal = if s.user.is_some() {
                                                        ActiveModal::FavSelect {
                                                            track: fav_track.clone(),
                                                        }
                                                    } else {
                                                        ActiveModal::Login
                                                    };
                                                    s.open_modal(modal, cx);
                                                });
                                            }),
                                        )
                                        .child(
                                            compact_icon_btn(
                                                ("q-open", orig_i),
                                                IconName::ExternalLink,
                                                "在 B 站打开",
                                                p,
                                            )
                                            .on_click(move |_, _, cx| {
                                                cx.stop_propagation();
                                                st_open.update(cx, |s, cx| {
                                                    s.open_track_in_bilibili(&open_track, cx);
                                                });
                                            }),
                                        )
                                    })
                                    .child(
                                        compact_icon_btn(
                                            ("q-rm", orig_i),
                                            IconName::X,
                                            "从列表移除",
                                            p,
                                        )
                                        .on_click(move |_, _, cx| {
                                            cx.stop_propagation();
                                            st_rm.update(cx, |s, cx| {
                                                if s.queue
                                                    .items()
                                                    .get(orig_i)
                                                    .is_some_and(|t| t.id == remove_id)
                                                {
                                                    s.remove_from_queue(orig_i, cx);
                                                }
                                            })
                                        }),
                                    ),
                            ),
                    )
                })
                .collect()
        })
        .track_scroll(&scroll_handle)
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
