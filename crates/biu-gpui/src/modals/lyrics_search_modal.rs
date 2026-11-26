use super::*;
use crate::widgets::format_duration;
use biu_core::{LyricsCandidate, LyricsProvider};

#[derive(Default)]
pub(super) struct LyricsSearchModal {
    input: Option<Entity<InputState>>,
    results: Option<Result<SearchResults, String>>,
    query: String,
}

struct SearchResults {
    items: Vec<LyricsCandidate>,
    errors: Vec<String>,
}

impl ModalsHost {
    pub(super) fn enter_lyrics(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let Some(track) = self.state.read(cx).current_track() else {
            return;
        };
        let title = track.title.clone();
        let input = new_input("歌曲名或歌手", title.clone(), window, cx);
        cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::PressEnter { .. }) {
                this.search_lyrics(normalize_query(&input.read(cx).value()), cx);
            }
        })
        .detach();
        self.data.lyrics.input = Some(input);
        self.search_lyrics(normalize_query(&title), cx);
    }

    fn search_lyrics(&mut self, query: String, cx: &mut Context<Self>) {
        self.data.lyrics.query = query.clone();
        self.data.lyrics.results = None;
        if query.is_empty() {
            self.task = None;
            cx.notify();
            return;
        }
        self.with_task(
            move |s| async move {
                let (netease, lrclib) = tokio::join!(
                    s.lyrics.search_netease(&query, 20),
                    s.lyrics.search_lrclib(&query)
                );
                let mut items = Vec::new();
                let mut errors = Vec::new();
                for (name, result) in [("网易云", netease), ("LRCLIB", lrclib)] {
                    match result {
                        Ok(mut candidates) => items.append(&mut candidates),
                        Err(error) => errors.push(format!("{name}：{error}")),
                    }
                }
                anyhow::ensure!(errors.len() < 2, "{}", errors.join("\n"));
                Ok(SearchResults { items, errors })
            },
            |this, result, _| this.data.lyrics.results = Some(result),
            cx,
        );
    }

    pub(super) fn lyrics_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let lyrics = &self.data.lyrics;
        let Some(input) = &lyrics.input else {
            return empty_state("未选择歌曲", "播放歌曲后可搜索歌词", p);
        };
        let value = input.clone();
        let state = self.state.read(cx);
        let mut root = column()
            .child(
                toolbar()
                    .child(div().flex_1().child(Input::new(input)))
                    .child(button("search", "搜索", p).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.search_lyrics(normalize_query(&value.read(cx).value()), cx);
                        },
                    ))),
            )
            .child(
                toolbar()
                    .when(!state.auto_lyrics_running, |bar| {
                        bar.child(state_button(
                            &self.state,
                            "auto-lyrics",
                            "声纹自动对齐 (Auto-Lyrics)",
                            p,
                            AppState::trigger_auto_lyrics,
                        ))
                    })
                    .when(state.auto_lyrics_running, |bar| bar.child("正在自动对齐…")),
            );

        if lyrics.query.is_empty() {
            return root.child(empty_state("请输入搜索词", "", p));
        }
        let retry = || {
            button("retry", "重试", p).on_click(cx.listener(|this, _, _, cx| {
                this.search_lyrics(this.data.lyrics.query.clone(), cx);
            }))
        };
        match &lyrics.results {
            None => root.child(empty_state("正在搜索歌词…", "", p)),
            Some(Err(error)) => {
                root.child(empty_state("搜索失败", error.clone(), p).child(retry()))
            }
            Some(Ok(result)) => {
                if !result.errors.is_empty() {
                    root = root.child(
                        toolbar()
                            .child(div().text_color(p.warning).child(result.errors.join(" · ")))
                            .child(retry()),
                    );
                }
                root.when(result.items.is_empty(), |c| {
                    c.child(empty_state("没有找到歌词", "", p))
                })
                .children(result.items.iter().enumerate().map(|(i, candidate)| {
                    let selection = candidate.clone();
                    let entity = self.state.clone();
                    let track = state.current_track().map(|track| track.id.clone());
                    let provider = match candidate.provider {
                        LyricsProvider::NetEase => "网易云",
                        LyricsProvider::Lrclib => "LRCLIB",
                    };
                    div()
                        .id(i)
                        .flex()
                        .items_center()
                        .gap_2()
                        .px_2()
                        .py_1()
                        .rounded(p.radius)
                        .cursor_pointer()
                        .hover(move |style| style.bg(p.surface_hover))
                        .on_click(move |_, _, cx| {
                            entity.update(cx, |s, cx| {
                                if matches!(s.modal, ActiveModal::LyricsSearch)
                                    && s.current_track().map(|track| &track.id) == track.as_ref()
                                {
                                    s.apply_manual_lyrics(selection.clone(), cx);
                                    s.close_modal(cx);
                                }
                            })
                        })
                        .child(
                            div()
                                .text_xs()
                                .px_2()
                                .bg(p.surface_active)
                                .rounded(p.radius)
                                .child(provider),
                        )
                        .child(
                            column()
                                .flex_1()
                                .gap_0()
                                .child(div().truncate().child(candidate.title.clone()))
                                .child(
                                    div().truncate().text_sm().text_color(p.text_muted).child(
                                        format!("{} · {}", candidate.artist, candidate.album),
                                    ),
                                ),
                        )
                        .child(
                            div().text_sm().child(
                                candidate
                                    .duration_secs
                                    .map_or_else(|| "—".into(), format_duration),
                            ),
                        )
                }))
            }
        }
    }
}
