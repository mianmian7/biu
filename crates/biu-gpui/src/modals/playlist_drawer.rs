use super::*;
use biu_core::image_cache::ThumbSize;

impl ModalsHost {
    pub(super) fn queue_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let queue = &self.state.read(cx).queue;
        let state = &self.state;
        let active_index = queue.current_index();
        column()
            .gap_3()
            .child(
                toolbar()
                    .child(
                        div()
                            .text_sm()
                            .text_color(p.text_muted)
                            .child(format!("{} 首", queue.len())),
                    )
                    .child(state_button(
                        state,
                        "play-mode",
                        queue.mode().label(),
                        p,
                        AppState::toggle_play_mode,
                    ))
                    .when(!queue.is_empty(), |bar| {
                        bar.child(state_button(
                            state,
                            "clear",
                            "清空列表",
                            p,
                            AppState::clear_queue,
                        ))
                    }),
            )
            .when(queue.is_empty(), |c| {
                c.child(empty_state("队列为空", "", p))
            })
            .children(queue.items().iter().enumerate().map(|(i, track)| {
                let state = state.clone();
                let remove_id = track.id.clone();
                let play_id = track.id.clone();
                let active = active_index == Some(i);
                div()
                    .id(i)
                    .flex()
                    .gap_2()
                    .items_center()
                    .p_2()
                    .rounded(p.radius)
                    .bg(if active { p.surface_active } else { p.surface })
                    .text_color(if active { p.accent } else { p.text })
                    .cursor_pointer()
                    .hover(move |style| style.bg(p.surface_hover))
                    .child(thumbnail(
                        &state,
                        &track.cover,
                        ThumbSize::Tiny,
                        (44., 44.),
                        p,
                        cx,
                    ))
                    .child(
                        column()
                            .flex_1()
                            .gap_0()
                            .overflow_hidden()
                            .child(div().truncate().child(track.title.clone()))
                            .child(
                                div()
                                    .text_sm()
                                    .truncate()
                                    .text_color(p.text_muted)
                                    .child(track.artist.clone()),
                            ),
                    )
                    .child(state_button(&state, "remove", "移除", p, move |s, cx| {
                        if s.queue
                            .items()
                            .get(i)
                            .is_some_and(|track| track.id == remove_id)
                        {
                            s.remove_from_queue(i, cx);
                        }
                    }))
                    .on_click(move |_, _, cx| {
                        state.update(cx, |s, cx| {
                            if s.queue
                                .items()
                                .get(i)
                                .is_some_and(|track| track.id == play_id)
                            {
                                s.jump_to_queue_index(i, cx);
                            }
                        })
                    })
            }))
    }
}
