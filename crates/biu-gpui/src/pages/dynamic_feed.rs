use super::*;
use biu_core::DynamicItem;

#[derive(Default)]
pub(super) struct DynamicFeed {
    prior: Vec<TrackItem>,
    offset: Option<String>,
    results: Load<Page<TrackItem>>,
}

impl PagesHost {
    pub(super) fn load_dynamics(&mut self, more: bool, cx: &mut Context<Self>) {
        if self.account.is_none() {
            return;
        }
        let dynamics = &mut self.data.dynamics;
        if !more {
            dynamics.prior.clear();
            dynamics.offset = None;
        } else if let Some(Ok(result)) = dynamics.results.result.take() {
            dynamics.prior = result.items;
            dynamics.offset = result.next_offset;
        }
        let offset = dynamics.offset.clone();
        let mut items = dynamics.prior.clone();
        self.load_with(
            |h| &mut h.data.dynamics.results,
            move |s| async move {
                let response = s.bili.get_dynamic_feed(offset.as_deref()).await?;
                items.extend(response.items.into_iter().filter_map(dynamic_track));
                let mut seen = std::collections::HashSet::new();
                items.retain(|track| seen.insert(track.id.clone()));
                Ok(Page {
                    items,
                    total: None,
                    has_more: response.has_more,
                    next_offset: response.next_offset.filter(|next| {
                        offset.as_ref() != Some(next) && response.has_more != Some(false)
                    }),
                })
            },
            |this, page_data, cx| this.prefetch_tracks(&page_data.items, cx),
            cx,
        );
    }

    pub(super) fn dynamics_view(
        &self,
        p: BiuPalette,
        content_width: f32,
        cx: &Context<Self>,
    ) -> Div {
        if self.account.is_none() {
            return page("关注动态").child(self.login_hint(p));
        }
        let grid = is_grid_mode(self.state.read(cx));

        let header = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child("关注动态"),
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
                            if let Some(Ok(res)) = this.data.dynamics.results.result.as_ref() {
                                let items = res.items.clone();
                                this.prefetch_tracks(&items, cx);
                            }
                        },
                        cx,
                    ))
                    .child(
                        icon_pill_button("refresh", IconName::RotateCw, "刷新", p)
                            .on_click(cx.listener(|this, _, _, cx| this.load_dynamics(false, cx))),
                    ),
            );

        page("").child(header).child(self.data.dynamics.results.content(
            p,
            button("retry", "重试", p)
                .on_click(cx.listener(|this, _, _, cx| this.load_dynamics(true, cx))),
            |result| {
                column()
                    .gap_4()
                    .child(track_collection(
                        &self.state,
                        &result.items,
                        content_width,
                        p,
                        cx,
                    ))
                    .when(result.next_offset.is_some(), |c| {
                        c.child(
                            div().w_full().pt_2().flex().justify_center().child(
                                icon_pill_button("more", IconName::Plus, "加载更多", p).on_click(
                                    cx.listener(|this, _, _, cx| this.load_dynamics(true, cx)),
                                ),
                            ),
                        )
                    })
            },
        ))
    }
}

fn dynamic_track(item: DynamicItem) -> Option<TrackItem> {
    if item.track.is_some() {
        return item.track;
    }
    let modules = &item.raw["modules"];
    let music = &modules["module_dynamic"]["major"]["music"];
    let url = music["jump_url"].as_str()?;
    let sid = url
        .split(['/', '?', '#'])
        .find_map(|part| part.strip_prefix("au")?.parse::<u64>().ok())?;
    Some(TrackItem {
        id: format!("au:{sid}"),
        title: music["title"].as_str()?.to_owned(),
        artist: modules["module_author"]["name"]
            .as_str()
            .unwrap_or_default()
            .to_owned(),
        artist_mid: modules["module_author"]["mid"].as_u64(),
        cover: music["cover"].as_str().unwrap_or_default().to_owned(),
        duration_secs: 0.,
        source: TrackSource::BiliAudio { sid },
    })
}
