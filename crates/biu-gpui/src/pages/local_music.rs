use super::*;
use biu_core::local_music::scan_local_music_dirs;

#[derive(Default)]
pub(super) struct LocalMusic {
    tracks: Load<Vec<TrackItem>>,
    picking: bool,
}

impl PagesHost {
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

        let header = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child("本地音乐"),
            )
            .child(
                toolbar()
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
                col.child(toolbar().children(dirs.into_iter().enumerate().map(|(i, path)| {
                    let remove = path.clone();
                    div()
                        .id(("local-dir", i))
                        .h(px(30.0))
                        .flex()
                        .gap_2()
                        .items_center()
                        .pl_3()
                        .pr_1p5()
                        .bg(p.surface)
                        .rounded_full()
                        .child(icon(IconName::Folder, 13.0, p.accent))
                        .child(div().text_xs().text_color(p.text_muted).child(path))
                        .child(
                            compact_icon_btn(("rm-dir", i), IconName::X, "移除目录", p).on_click(
                                cx.listener(move |this, _, _, cx| {
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
                                }),
                            ),
                        )
                })))
            })
            .child(self.data.local.tracks.content(
                p,
                button("retry", "重试", p)
                    .on_click(cx.listener(|this, _, _, cx| this.scan_local(cx))),
                |tracks| {
                    if tracks.is_empty() {
                        return empty_state(
                            "暂无本地音乐",
                            "点击右上角「添加目录」扫描本地 FLAC / MP3 / M4A 文件",
                            p,
                        );
                    }
                    column()
                        .gap_3()
                        .child(play_all(&self.state, tracks, p))
                        .child(
                            div()
                                .w_full()
                                .flex()
                                .flex_col()
                                .gap(px(2.0))
                                .child(track_list_header(p))
                                .children(tracks.iter().enumerate().map(|(i, track)| {
                                    let fmt = match &track.source {
                                        TrackSource::LocalFile { format, .. }
                                            if !format.is_empty() =>
                                        {
                                            Some(format.to_ascii_uppercase())
                                        }
                                        _ => None,
                                    };
                                    track_row(&self.state, i, track, p, cx).when_some(
                                        fmt,
                                        |row, tag| {
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
                                        },
                                    )
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
