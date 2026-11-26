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
        let dirs = &self.state.read(cx).settings.local_music_dirs;
        page("本地音乐")
            .child(
                toolbar()
                    .child(
                        button(
                            "add-folder",
                            if self.data.local.picking {
                                "正在选择…"
                            } else {
                                "添加文件夹"
                            },
                            p,
                        )
                        .on_click(cx.listener(|this, _, _, cx| this.add_local_folder(cx))),
                    )
                    .child(
                        button("scan", "重新扫描", p)
                            .on_click(cx.listener(|this, _, _, cx| this.scan_local(cx))),
                    ),
            )
            .child(toolbar().children(dirs.iter().enumerate().map(|(i, path)| {
                let remove = path.clone();
                div()
                    .id(i)
                    .flex()
                    .gap_2()
                    .items_center()
                    .p_1()
                    .pl_2()
                    .bg(p.surface)
                    .rounded(p.radius)
                    .child(div().text_sm().child(path.clone()))
                    .child(button("remove-dir", "移除", p).on_click(cx.listener(
                        move |this, _, _, cx| {
                            this.state.update(cx, |s, cx| {
                                s.update_settings(
                                    |settings| {
                                        settings.local_music_dirs.retain(|dir| dir != &remove)
                                    },
                                    cx,
                                )
                            });
                            this.scan_local(cx);
                        },
                    )))
            })))
            .child(
                self.data.local.tracks.content(
                    p,
                    button("retry", "重试", p)
                        .on_click(cx.listener(|this, _, _, cx| this.scan_local(cx))),
                    |tracks| {
                        column()
                            .child(play_all(&self.state, tracks, p))
                            .when(tracks.is_empty(), |c| {
                                c.child(empty_state(
                                    "暂无本地音乐",
                                    "添加含有音频文件的文件夹开始扫描",
                                    p,
                                ))
                            })
                            .children(tracks.iter().enumerate().map(|(i, track)| {
                                let TrackSource::LocalFile { format, .. } = &track.source else {
                                    return track_row(&self.state, i, track, p, cx);
                                };
                                track_row(&self.state, i, track, p, cx).child(
                                    div()
                                        .rounded(p.radius)
                                        .bg(p.surface_active)
                                        .text_sm()
                                        .px_2()
                                        .py_1()
                                        .child(format.to_ascii_uppercase()),
                                )
                            }))
                    },
                ),
            )
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
