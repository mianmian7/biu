use super::*;
use crate::widgets::section_header;
use biu_core::{BannerItem, RankItem};

#[derive(Default)]
pub(super) struct MusicRecommend {
    banners: Load<Vec<BannerItem>>,
    rank: Load<Vec<RankItem>>,
    new_music: Load<Vec<RankItem>>,
    region: Load<Vec<TrackItem>>,
}

impl PagesHost {
    pub(super) fn load_recommend(&mut self, cx: &mut Context<Self>) {
        self.load(
            |h| &mut h.data.recommend.banners,
            |s| async move { s.bili.get_music_banners().await },
            cx,
        );
        self.load(
            |h| &mut h.data.recommend.rank,
            |s| async move { s.bili.get_music_rank().await },
            cx,
        );
        self.load(
            |h| &mut h.data.recommend.new_music,
            |s| async move { s.bili.get_new_music().await },
            cx,
        );
        self.load(
            |h| &mut h.data.recommend.region,
            |s| async move { s.bili.get_region_feed_rcmd(1).await },
            cx,
        );
    }

    pub(super) fn recommend_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let mode = self.state.read(cx).settings.display_mode.as_str();
        let grid = mode == "grid" || mode == "card";
        let retry = || {
            button("retry", "重试", p)
                .on_click(cx.listener(|this, _, _, cx| this.load_recommend(cx)))
        };
        let data = &self.data.recommend;
        let track_section =
            |id, title, load: &Load<Vec<RankItem>>| {
                div()
                    .id(id)
                    .child(section_header(title, p))
                    .child(load.content(p, retry(), |items| {
                        let tracks: Vec<_> =
                            items.iter().filter_map(|item| item.track.clone()).collect();
                        column()
                            .child(play_all(&self.state, &tracks, p))
                            .child(recommend_tracks(&self.state, &tracks, grid, p, cx))
                            .children(items.iter().filter(|item| item.track.is_none()).map(
                                |item| {
                                    toolbar()
                                        .child(thumbnail(
                                            &self.state,
                                            &item.cover,
                                            ThumbSize::Tiny,
                                            (52., 44.),
                                            p,
                                            cx,
                                        ))
                                        .child(format!("{} · {}", item.title, item.author))
                                },
                            ))
                    }))
            };

        page("音乐推荐")
            .child(
                toolbar()
                    .child(
                        button("refresh", "刷新", p)
                            .on_click(cx.listener(|this, _, _, cx| this.load_recommend(cx))),
                    )
                    .child(state_button(
                        &self.state,
                        "display-mode",
                        if grid { "切换列表" } else { "切换网格" },
                        p,
                        move |s, cx| {
                            s.update_settings(
                                |settings| {
                                    settings.display_mode =
                                        if grid { "list" } else { "grid" }.into()
                                },
                                cx,
                            );
                        },
                    )),
            )
            .child(
                div()
                    .id("music-banners")
                    .child(data.banners.content(p, retry(), |banners| {
                        if banners.is_empty() {
                            return empty_state("暂无推荐横幅", "", p);
                        }
                        div().flex().gap_3().overflow_hidden().child(
                            div()
                                .id("banner-strip")
                                .flex()
                                .gap_3()
                                .overflow_x_scroll()
                                .children(
                                    banners.iter().enumerate().map(|(i, banner)| {
                                        banner_card(&self.state, i, banner, p, cx)
                                    }),
                                ),
                        )
                    })),
            )
            .child(track_section("rank", "音乐综合榜", &data.rank))
            .child(track_section("new-music", "新歌推荐", &data.new_music))
            .child(
                div()
                    .id("region")
                    .child(section_header("音乐分区推荐", p))
                    .child(data.region.content(p, retry(), |tracks| {
                        column()
                            .child(play_all(&self.state, tracks, p))
                            .child(recommend_tracks(&self.state, tracks, grid, p, cx))
                    })),
            )
    }
}

fn banner_card(
    state: &Entity<AppState>,
    index: usize,
    banner: &BannerItem,
    p: BiuPalette,
    cx: &App,
) -> Stateful<Div> {
    let card = div()
        .id(index)
        .w(px(224.))
        .flex_shrink_0()
        .rounded(p.radius)
        .bg(p.surface)
        .child(thumbnail(
            state,
            &banner.cover,
            ThumbSize::Card,
            (224., 112.),
            p,
            cx,
        ))
        .child(div().px_2().py_1().truncate().child(banner.title.clone()));
    if banner.bvid.is_empty() {
        return card;
    }
    let state = state.clone();
    let track = TrackItem {
        id: format!("{}:{}", banner.bvid, banner.cid),
        title: banner.title.clone(),
        artist: banner.author.clone(),
        artist_mid: None,
        cover: banner.cover.clone(),
        duration_secs: 0.,
        source: TrackSource::BiliVideo {
            bvid: banner.bvid.clone(),
            aid: banner.aid,
            cid: banner.cid,
            part_title: None,
        },
    };
    card.cursor_pointer()
        .hover(move |s| s.bg(p.surface_hover))
        .on_click(move |_, _, cx| {
            state.update(cx, |s, cx| s.play_track(track.clone(), cx));
        })
}

fn recommend_tracks(
    state: &Entity<AppState>,
    tracks: &[TrackItem],
    grid: bool,
    p: BiuPalette,
    cx: &App,
) -> Div {
    if !grid || tracks.is_empty() {
        return track_list(state, tracks, p, cx);
    }
    toolbar()
        .items_start()
        .children(tracks.iter().enumerate().map(|(i, track)| {
            div()
                .id(i)
                .w(px(264.))
                .rounded(p.radius)
                .bg(p.surface)
                .p_2()
                .child(
                    column()
                        .child(thumbnail(
                            state,
                            &track.cover,
                            ThumbSize::Card,
                            (248., 128.),
                            p,
                            cx,
                        ))
                        .child(track_details(track, p))
                        .child(track_actions(state, track, p)),
                )
        }))
}
