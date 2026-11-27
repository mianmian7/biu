use super::*;
use biu_core::local_music::scan_local_music_dirs;
use std::path::Path;

#[derive(Default)]
pub(super) struct LocalMusic {
    input: Option<Entity<InputState>>,
    keyword: String,
    selected_dir: Option<String>,
    tracks: Load<Vec<TrackItem>>,
    picking: bool,
}

impl PagesHost {
    pub(super) fn enter_local(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let input = new_input("搜索本地歌曲…", "", window, cx);
        cx.subscribe_in(&input, window, |this, input, event: &InputEvent, _, cx| {
            if matches!(event, InputEvent::Change | InputEvent::PressEnter { .. }) {
                this.data.local.keyword = normalize_query(&input.read(cx).value());
                cx.notify();
            }
        })
        .detach();
        self.data.local.input = Some(input);
        self.scan_local(cx);
    }

    pub(super) fn scan_local(&mut self, cx: &mut Context<Self>) {
        let dirs = self.state.read(cx).settings.local_music_dirs.clone();
        self.load(
            |h| &mut h.data.local.tracks,
            move |_| async move {
                Ok(tokio::task::spawn_blocking(move || {
                    scan_local_music_dirs(&dirs)
                        .into_iter()
                        .map(TrackItem::from)
                        .collect()
                })
                .await?)
            },
            cx,
        );
    }

    fn add_local_folder(&mut self, cx: &mut Context<Self>) {
        if self.data.local.picking {
            return;
        }
        self.data.local.picking = true;
        self.pick_folder(
            |this, path, cx| {
                this.data.local.picking = false;
                if let Some(path) = path {
                    this.state.update(cx, |s, cx| {
                        s.update_settings(
                            |settings| {
                                if !settings.local_music_dirs.contains(&path) {
                                    settings.local_music_dirs.push(path);
                                }
                            },
                            cx,
                        )
                    });
                    this.scan_local(cx);
                }
            },
            cx,
        );
    }

    pub(super) fn local_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let dirs = self.state.read(cx).settings.local_music_dirs.clone();
        let selected_dir = self.data.local.selected_dir.clone();
        let kw = self.data.local.keyword.to_lowercase();

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
                    .child("本地音乐"),
            )
            .child(
                toolbar()
                    .when_some(self.data.local.input.as_ref(), |bar, input| {
                        bar.child(
                            div()
                                .w(px(220.0))
                                .h(px(34.0))
                                .px_3()
                                .rounded_full()
                                .bg(p.surface)
                                .flex()
                                .items_center()
                                .gap_2()
                                .child(icon(IconName::Search, 14.0, p.text_dim))
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
                        primary_button(
                            "add-folder",
                            IconName::FolderPlus,
                            if self.data.local.picking {
                                "正在选择…"
                            } else {
                                "添加目录"
                            },
                            p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.add_local_folder(cx))),
                    )
                    .child(
                        icon_pill_button("scan", IconName::RotateCw, "重新扫描", p)
                            .on_click(cx.listener(|this, _, _, cx| this.scan_local(cx))),
                    ),
            );

        page("")
            .child(header)
            .when(!dirs.is_empty(), |col| {
                let all_active = selected_dir.is_none();
                col.child(
                    toolbar()
                        .child(
                            div()
                                .id("local-dir-all")
                                .h(px(30.0))
                                .px_3()
                                .rounded_full()
                                .flex()
                                .items_center()
                                .text_xs()
                                .font_weight(if all_active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::NORMAL
                                })
                                .bg(if all_active { p.accent_soft } else { p.surface })
                                .text_color(if all_active { p.accent } else { p.text_muted })
                                .cursor_pointer()
                                .hover(move |s| s.bg(p.surface_hover))
                                .child("全部目录")
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.data.local.selected_dir = None;
                                    cx.notify();
                                })),
                        )
                        .children(dirs.into_iter().enumerate().map(|(i, path)| {
                            let active = selected_dir.as_deref() == Some(path.as_str());
                            let select_path = path.clone();
                            let remove = path.clone();
                            let display_name = Path::new(&path)
                                .file_name()
                                .and_then(|n| n.to_str())
                                .unwrap_or(&path)
                                .to_string();
                            div()
                                .id(("local-dir", i))
                                .h(px(30.0))
                                .flex()
                                .gap_1p5()
                                .items_center()
                                .pl_3()
                                .pr_1p5()
                                .bg(if active { p.accent_soft } else { p.surface })
                                .rounded_full()
                                .cursor_pointer()
                                .hover(move |s| s.bg(p.surface_hover))
                                .tip(path.clone())
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.data.local.selected_dir =
                                        if this.data.local.selected_dir.as_deref()
                                            == Some(select_path.as_str())
                                        {
                                            None
                                        } else {
                                            Some(select_path.clone())
                                        };
                                    cx.notify();
                                }))
                                .child(icon(IconName::Folder, 13.0, p.accent))
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(if active {
                                            FontWeight::SEMIBOLD
                                        } else {
                                            FontWeight::NORMAL
                                        })
                                        .text_color(if active { p.accent } else { p.text_muted })
                                        .child(display_name),
                                )
                                .child(
                                    compact_icon_btn(("rm-dir", i), IconName::X, "移除目录", p)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            cx.stop_propagation();
                                            if this.data.local.selected_dir.as_deref()
                                                == Some(remove.as_str())
                                            {
                                                this.data.local.selected_dir = None;
                                            }
                                            this.state.update(cx, |s, cx| {
                                                s.update_settings(
                                                    |settings| {
                                                        settings
                                                            .local_music_dirs
                                                            .retain(|dir| dir != &remove)
                                                    },
                                                    cx,
                                                )
                                            });
                                            this.scan_local(cx);
                                        })),
                                )
                        })),
                )
            })
            .child(self.data.local.tracks.content(
                p,
                button("retry", "重试", p)
                    .on_click(cx.listener(|this, _, _, cx| this.scan_local(cx))),
                |tracks| {
                    let filtered: Vec<TrackItem> = tracks
                        .iter()
                        .filter(|t| {
                            let dir_ok = match (&selected_dir, &t.source) {
                                (Some(dir), TrackSource::LocalFile { path, .. }) => {
                                    Path::new(path).starts_with(Path::new(dir))
                                }
                                _ => true,
                            };
                            let kw_ok = kw.is_empty()
                                || t.title.to_lowercase().contains(&kw)
                                || t.artist.to_lowercase().contains(&kw);
                            dir_ok && kw_ok
                        })
                        .cloned()
                        .collect();
                    if filtered.is_empty() {
                        return empty_state(
                            "暂无本地音乐",
                            if tracks.is_empty() {
                                "点击右上角「添加目录」扫描本地 FLAC / MP3 / M4A 文件"
                            } else {
                                "当前筛选条件下没有匹配的歌曲"
                            },
                            p,
                        );
                    }
                    column()
                        .gap_3()
                        .child(play_all(&self.state, &filtered, p))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(track_list_header(p))
                                .children(filtered.iter().enumerate().map(|(i, track)| {
                                    let (fmt, parent_dir) = match &track.source {
                                        TrackSource::LocalFile { path, format } => (
                                            (!format.is_empty())
                                                .then(|| format.to_ascii_uppercase()),
                                            Path::new(path).parent().map(Path::to_path_buf),
                                        ),
                                        _ => (None, None),
                                    };
                                    track_row(&self.state, i, track, p, cx)
                                        .when_some(fmt, |row, tag| {
                                            row.child(
                                                div()
                                                    .px_1p5()
                                                    .py(px(2.0))
                                                    .rounded(px(4.0))
                                                    .bg(p.surface_elevated)
                                                    .text_size(px(10.0))
                                                    .font_weight(FontWeight::SEMIBOLD)
                                                    .text_color(p.text_muted)
                                                    .child(tag),
                                            )
                                        })
                                        .when_some(parent_dir, |row, dir| {
                                            row.child(
                                                compact_icon_btn(
                                                    ("local-open", i),
                                                    IconName::FolderOpen,
                                                    "打开所在文件夹",
                                                    p,
                                                )
                                                .on_click(move |_, _, cx| {
                                                    cx.stop_propagation();
                                                    let _ = open::that_detached(&dir);
                                                }),
                                            )
                                        })
                                })),
                        )
                },
            ))
    }

    pub(super) fn pick_folder(
        &self,
        finish: impl FnOnce(&mut Self, Option<String>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) {
        let runtime = self.state.read(cx).services.rt.handle().clone();
        let epoch = self.epoch;
        cx.spawn(async move |this, cx| {
            let path = runtime
                .spawn_blocking(|| rfd::FileDialog::new().pick_folder())
                .await
                .ok()
                .flatten()
                .map(|path| path.to_string_lossy().into_owned());
            let _ = this.update(cx, |this, cx| {
                if this.epoch == epoch {
                    finish(this, path, cx);
                    cx.notify();
                }
            });
        })
        .detach();
        cx.notify();
    }
}
