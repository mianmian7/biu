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
        self.load(
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
            cx,
        );
    }

    pub(super) fn dynamics_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let root = page("关注动态");
        if self.account.is_none() {
            return root.child(self.login_hint(p));
        }
        root.child(
            toolbar().child(
                button("refresh", "刷新", p)
                    .on_click(cx.listener(|this, _, _, cx| this.load_dynamics(false, cx))),
            ),
        )
        .child(
            self.data.dynamics.results.content(
                p,
                button("retry", "重试", p)
                    .on_click(cx.listener(|this, _, _, cx| this.load_dynamics(true, cx))),
                |result| {
                    column()
                        .child(track_list(&self.state, &result.items, p, cx))
                        .when(result.next_offset.is_some(), |c| {
                            c.child(toolbar().child(button("more", "加载更多", p).on_click(
                                cx.listener(|this, _, _, cx| this.load_dynamics(true, cx)),
                            )))
                        })
                },
            ),
        )
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
