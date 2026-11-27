use super::*;
use biu_core::{DynamicItem, FollowingUser};

#[derive(Default)]
pub(super) struct DynamicFeed {
    prior: Vec<TrackItem>,
    offset: Option<String>,
    selected_mid: Option<u64>,
    authors: Load<Vec<FollowingUser>>,
    results: Load<Page<TrackItem>>,
}

impl PagesHost {
    pub(super) fn enter_dynamics(&mut self, cx: &mut Context<Self>) {
        if let Some(mid) = self.account {
            self.load_with(
                |h| &mut h.data.dynamics.authors,
                move |s| async move {
                    let page = s.bili.get_followings(mid, 1, None).await?;
                    Ok(page.items)
                },
                |this, users, cx| {
                    let covers = users
                        .iter()
                        .take(24)
                        .filter(|u| !u.face.is_empty())
                        .map(|u| (u.face.clone(), ThumbSize::Avatar))
                        .collect();
                    this.prefetch_covers(covers, cx);
                },
                cx,
            );
        }
        self.load_dynamics(false, cx);
    }

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
        let selected_mid = dynamics.selected_mid;
        let mut items = dynamics.prior.clone();
        self.load_with(
            |h| &mut h.data.dynamics.results,
            move |s| async move {
                let response = if let Some(host_mid) = selected_mid {
                    s.bili
                        .get_dynamic_feed_space(host_mid, offset.as_deref())
                        .await?
                } else {
                    s.bili.get_dynamic_feed(offset.as_deref()).await?
                };
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
        let selected_mid = self.data.dynamics.selected_mid;
        let loaded_tracks: Vec<TrackItem> = self
            .data
            .dynamics
            .results
            .result
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .map(|r| r.items.clone())
            .unwrap_or_default();

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
                    .child("关注动态"),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .when(!loaded_tracks.is_empty(), |bar| {
                        bar.child(play_all(&self.state, &loaded_tracks, p))
                    })
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

        let author_strip = self
            .data
            .dynamics
            .authors
            .result
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .filter(|users| !users.is_empty())
            .map(|users| {
                let all_active = selected_mid.is_none();
                toolbar()
                    .child(
                        div()
                            .id("dyn-author-all")
                            .h(px(32.0))
                            .px_3p5()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .text_xs()
                            .font_weight(if all_active {
                                FontWeight::SEMIBOLD
                            } else {
                                FontWeight::MEDIUM
                            })
                            .bg(if all_active { p.accent_soft } else { p.surface })
                            .text_color(if all_active { p.accent } else { p.text_muted })
                            .cursor_pointer()
                            .hover(move |s| s.bg(p.surface_hover))
                            .child("全部关注")
                            .on_click(cx.listener(|this, _, _, cx| {
                                if this.data.dynamics.selected_mid.is_some() {
                                    this.data.dynamics.selected_mid = None;
                                    this.load_dynamics(false, cx);
                                }
                            })),
                    )
                    .children(users.iter().take(18).enumerate().map(|(i, u)| {
                        let mid = u.mid;
                        let active = selected_mid == Some(mid);
                        div()
                            .id(("dyn-author", i))
                            .h(px(32.0))
                            .pl_1()
                            .pr_3()
                            .rounded_full()
                            .flex()
                            .items_center()
                            .gap_1p5()
                            .bg(if active { p.accent_soft } else { p.surface })
                            .cursor_pointer()
                            .hover(move |s| s.bg(p.surface_hover))
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.data.dynamics.selected_mid =
                                    if this.data.dynamics.selected_mid == Some(mid) {
                                        None
                                    } else {
                                        Some(mid)
                                    };
                                this.load_dynamics(false, cx);
                            }))
                            .child(thumbnail(
                                &self.state,
                                &u.face,
                                ThumbSize::Avatar,
                                (24., 24.),
                                BiuPalette {
                                    radius: px(12.0),
                                    ..p
                                },
                                cx,
                            ))
                            .child(
                                div()
                                    .max_w(px(96.0))
                                    .text_xs()
                                    .font_weight(if active {
                                        FontWeight::SEMIBOLD
                                    } else {
                                        FontWeight::NORMAL
                                    })
                                    .text_color(if active { p.accent } else { p.text_muted })
                                    .truncate()
                                    .child(u.name.clone()),
                            )
                    }))
            });

        page("")
            .child(header)
            .when_some(author_strip, |d, strip| d.child(strip))
            .child(self.data.dynamics.results.content(
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
                                    icon_pill_button("more", IconName::Plus, "加载更多", p)
                                        .on_click(cx.listener(|this, _, _, cx| {
                                            this.load_dynamics(true, cx)
                                        })),
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
