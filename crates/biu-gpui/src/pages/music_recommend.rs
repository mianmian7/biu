use super::*;
use crate::widgets::section_header;
use biu_core::TrackItem;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum RecommendTab {
    #[default]
    Music,
    Guichu,
    Pop,
}

impl RecommendTab {
    const ALL: [(Self, &'static str, &'static str); 3] = [
        (Self::Music, "tab-music", "音乐"),
        (Self::Guichu, "tab-guichu", "鬼畜"),
        (Self::Pop, "tab-pop", "流行"),
    ];
}

#[derive(Default)]
pub(super) struct MusicRecommend {
    tab: RecommendTab,
    page: u64,
    prior: Vec<TrackItem>,
    tracks: Load<Vec<TrackItem>>,
    new_music: Load<Vec<TrackItem>>,
    new_music_page: usize,
    has_more: bool,
}

impl PagesHost {
    pub(super) fn enter_recommend(&mut self, cx: &mut Context<Self>) {
        self.data.recommend = MusicRecommend::default();
        self.load_recommend_tab(RecommendTab::Music, false, cx);
    }

    fn switch_recommend_tab(&mut self, tab: RecommendTab, cx: &mut Context<Self>) {
        if self.data.recommend.tab == tab {
            return;
        }
        self.data.recommend.tab = tab;
        self.data.recommend.prior.clear();
        self.data.recommend.page = 0;
        self.load_recommend_tab(tab, false, cx);
        if tab == RecommendTab::Pop && self.data.recommend.new_music.result.is_none() {
            self.load_new_music_strip(cx);
        }
    }

    fn load_new_music_strip(&mut self, cx: &mut Context<Self>) {
        self.load_with(
            |h| &mut h.data.recommend.new_music,
            |s| async move {
                let (banners, new_list) =
                    tokio::join!(s.bili.get_music_banners(), s.bili.get_new_music());
                let mut seen = std::collections::HashSet::new();
                let mut out = Vec::new();
                if let Ok(banners) = banners {
                    for b in banners {
                        if b.bvid.is_empty() {
                            continue;
                        }
                        let id = format!("{}:{}", b.bvid, b.cid);
                        if seen.insert(id.clone()) {
                            out.push(TrackItem {
                                id,
                                title: b.title,
                                artist: b.author,
                                artist_mid: None,
                                cover: b.cover,
                                duration_secs: 0.0,
                                source: TrackSource::BiliVideo {
                                    bvid: b.bvid,
                                    aid: b.aid,
                                    cid: b.cid,
                                    part_title: None,
                                },
                            });
                        }
                    }
                }
                if let Ok(items) = new_list {
                    for item in items {
                        if let Some(track) = item.track
                            && seen.insert(track.id.clone())
                        {
                            out.push(track);
                        }
                    }
                }
                Ok(out)
            },
            |this, tracks, cx| {
                let covers = tracks
                    .iter()
                    .take(12)
                    .filter(|t| !t.cover.is_empty())
                    .map(|t| (t.cover.clone(), ThumbSize::Card))
                    .collect();
                this.prefetch_covers(covers, cx);
            },
            cx,
        );
    }

    fn load_recommend_tab(&mut self, tab: RecommendTab, append: bool, cx: &mut Context<Self>) {
        let next_page = if append {
            self.data.recommend.page.saturating_add(1)
        } else {
            self.data.recommend.prior.clear();
            1
        };
        if append
            && let Some(Ok(existing)) = self.data.recommend.tracks.result.take()
        {
            self.data.recommend.prior = existing;
        }
        self.data.recommend.page = next_page;
        let prior = self.data.recommend.prior.clone();

        self.load_with(
            |h| &mut h.data.recommend.tracks,
            move |s| async move {
                let fresh: Vec<TrackItem> = match tab {
                    RecommendTab::Music => s.bili.get_region_feed_rcmd_for(1003, next_page).await?,
                    RecommendTab::Guichu => {
                        s.bili.get_region_feed_rcmd_for(1007, next_page).await?
                    }
                    RecommendTab::Pop => s
                        .bili
                        .get_music_rank_page(next_page)
                        .await?
                        .into_iter()
                        .filter_map(|item| item.track)
                        .collect(),
                };
                let mut merged = prior;
                let mut seen: std::collections::HashSet<String> =
                    merged.iter().map(|t| t.id.clone()).collect();
                for track in fresh {
                    if seen.insert(track.id.clone()) {
                        merged.push(track);
                    }
                }
                Ok(merged)
            },
            |this, tracks, cx| {
                this.data.recommend.has_more = !tracks.is_empty();
                this.prefetch_tracks(tracks, cx);
            },
            cx,
        );
    }

    pub(super) fn recommend_view(
        &self,
        p: BiuPalette,
        content_width: f32,
        cx: &Context<Self>,
    ) -> Div {
        let grid = is_grid_mode(self.state.read(cx));
        let rec = &self.data.recommend;
        let tab = rec.tab;

        let tracks_for_play = rec
            .tracks
            .result
            .as_ref()
            .and_then(|r| r.as_ref().ok())
            .cloned()
            .unwrap_or_default();

        let tabs = div()
            .flex()
            .items_center()
            .gap_1()
            .p(px(4.0))
            .rounded(px(12.0))
            .bg(p.surface)
            .children(RecommendTab::ALL.into_iter().map(|(key, id, label)| {
                let active = tab == key;
                div()
                    .id(id)
                    .h(px(32.0))
                    .px_4()
                    .rounded(px(9.0))
                    .flex()
                    .items_center()
                    .justify_center()
                    .text_sm()
                    .font_weight(if active {
                        FontWeight::SEMIBOLD
                    } else {
                        FontWeight::MEDIUM
                    })
                    .text_color(if active { p.text } else { p.text_muted })
                    .cursor_pointer()
                    .when(active, |d| d.bg(p.surface_active).shadow_sm())
                    .when(!active, |d| d.hover(move |s| s.text_color(p.text)))
                    .child(label)
                    .on_click(cx.listener(move |this, _, _, cx| {
                        this.switch_recommend_tab(key, cx);
                    }))
            }));

        let right_controls = div()
            .flex()
            .items_center()
            .gap_2()
            .child(display_mode_toggle(
                &self.state,
                grid,
                p,
                |this, _, cx| {
                    if let Some(Ok(tracks)) = this.data.recommend.tracks.result.as_ref() {
                        let cloned = tracks.clone();
                        this.prefetch_tracks(&cloned, cx);
                    }
                },
                cx,
            ))
            .child(
                icon_pill_button("rec-refresh", IconName::RotateCw, "换一批", p).on_click(
                    cx.listener(move |this, _, _, cx| this.load_recommend_tab(tab, false, cx)),
                ),
            )
            .when(!tracks_for_play.is_empty(), |bar| {
                let st = self.state.clone();
                let list = tracks_for_play.clone();
                bar.child(
                    primary_button("rec-play-all", IconName::Play, "全部播放", p).on_click(
                        move |_, _, cx| {
                            st.update(cx, |s, cx| s.play_queue(list.clone(), 0, cx));
                        },
                    ),
                )
            });

        let header_bar = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .gap_3()
            .child(tabs)
            .child(right_controls);

        let retry = button("retry", "重试", p)
            .on_click(cx.listener(move |this, _, _, cx| this.load_recommend_tab(tab, false, cx)));

        let new_music_section = if tab == RecommendTab::Pop {
            rec.new_music
                .result
                .as_ref()
                .and_then(|r| r.as_ref().ok())
                .filter(|items| !items.is_empty())
                .map(|items| {
                    let cols = grid_columns_for_width(content_width).max(4);
                    let page_size = cols;
                    let total_pages = items.len().div_ceil(page_size).max(1);
                    let current_page = rec.new_music_page.min(total_pages - 1);
                    let start = current_page * page_size;
                    let slice = &items[start..(start + page_size).min(items.len())];

                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_3()
                        .child(
                            div()
                                .flex()
                                .items_center()
                                .justify_between()
                                .child(section_header("新歌速递", p).mb_0())
                                .child(
                                    div()
                                        .flex()
                                        .items_center()
                                        .gap_1p5()
                                        .child(
                                            compact_icon_btn(
                                                "nm-prev",
                                                IconName::ChevronLeft,
                                                "上一组",
                                                p,
                                            )
                                            .when(current_page == 0, |d| d.opacity(0.35))
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if this.data.recommend.new_music_page > 0 {
                                                    this.data.recommend.new_music_page -= 1;
                                                    if let Some(Ok(all)) =
                                                        this.data.recommend.new_music.result.as_ref()
                                                    {
                                                        let s = this.data.recommend.new_music_page
                                                            * page_size;
                                                        let covers = all[s..(s + page_size)
                                                            .min(all.len())]
                                                            .iter()
                                                            .map(|t| {
                                                                (t.cover.clone(), ThumbSize::Card)
                                                            })
                                                            .collect();
                                                        this.prefetch_covers(covers, cx);
                                                    }
                                                    cx.notify();
                                                }
                                            })),
                                        )
                                        .child(
                                            div()
                                                .text_xs()
                                                .text_color(p.text_muted)
                                                .child(format!(
                                                    "{} / {}",
                                                    current_page + 1,
                                                    total_pages
                                                )),
                                        )
                                        .child(
                                            compact_icon_btn(
                                                "nm-next",
                                                IconName::ChevronRight,
                                                "下一组",
                                                p,
                                            )
                                            .when(current_page + 1 >= total_pages, |d| {
                                                d.opacity(0.35)
                                            })
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                if this.data.recommend.new_music_page + 1
                                                    < total_pages
                                                {
                                                    this.data.recommend.new_music_page += 1;
                                                    if let Some(Ok(all)) =
                                                        this.data.recommend.new_music.result.as_ref()
                                                    {
                                                        let s = this.data.recommend.new_music_page
                                                            * page_size;
                                                        let covers = all[s..(s + page_size)
                                                            .min(all.len())]
                                                            .iter()
                                                            .map(|t| {
                                                                (t.cover.clone(), ThumbSize::Card)
                                                            })
                                                            .collect();
                                                        this.prefetch_covers(covers, cx);
                                                    }
                                                    cx.notify();
                                                }
                                            })),
                                        ),
                                ),
                        )
                        .child(track_grid(&self.state, slice, content_width, p, cx))
                })
        } else {
            None
        };

        let section_title = match tab {
            RecommendTab::Music => "热歌精选",
            RecommendTab::Guichu => "鬼畜精选",
            RecommendTab::Pop => "综合热榜",
        };

        page("")
            .child(header_bar)
            .when_some(new_music_section, |d, sec| d.child(sec))
            .child(
                div()
                    .w_full()
                    .flex()
                    .flex_col()
                    .gap_3()
                    .child(section_header(section_title, p).mb_0())
                    .child(rec.tracks.content(p, retry, |tracks| {
                        column()
                            .gap_4()
                            .child(track_collection(&self.state, tracks, content_width, p, cx))
                            .when(rec.has_more && !tracks.is_empty(), |c| {
                                c.child(
                                    div()
                                        .w_full()
                                        .pt_2()
                                        .flex()
                                        .justify_center()
                                        .child(
                                            icon_pill_button(
                                                "rec-load-more",
                                                IconName::Plus,
                                                "加载更多推荐",
                                                p,
                                            )
                                            .on_click(cx.listener(move |this, _, _, cx| {
                                                this.load_recommend_tab(tab, true, cx);
                                            })),
                                        ),
                                )
                            })
                    })),
            )
    }
}
