use super::*;
use biu_core::{DownloadStatus, DownloadTask};
use std::{path::Path, time::Duration};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
enum DownloadFilter {
    #[default]
    All,
    Active,
    Done,
}

#[derive(Default)]
pub(super) struct DownloadList {
    tasks: Option<Result<Vec<DownloadTask>, String>>,
    filter: DownloadFilter,
    poll: Option<Task<()>>,
}

impl PagesHost {
    pub(super) fn load_downloads(&mut self, cx: &mut Context<Self>) {
        let services = self.state.read(cx).services.clone();
        self.data.downloads.poll = Some(cx.spawn(async move |this, cx| {
            loop {
                let manager = services.downloads.clone();
                let tasks = services
                    .rt
                    .spawn_blocking(move || manager.tasks())
                    .await
                    .map_err(|error| error.to_string());
                let busy = tasks
                    .as_ref()
                    .is_ok_and(|tasks| tasks.iter().any(|task| is_active(&task.status)));
                if this
                    .update(cx, |this, cx| {
                        if let Ok(ref list) = tasks {
                            let covers = list
                                .iter()
                                .rev()
                                .take(40)
                                .filter_map(|t| t.cover.as_deref())
                                .filter(|c| !c.is_empty())
                                .map(|c| (c.to_string(), ThumbSize::Tiny))
                                .collect();
                            this.prefetch_covers(covers, cx);
                        }
                        this.data.downloads.tasks = Some(tasks);
                        cx.notify();
                    })
                    .is_err()
                    || !busy
                {
                    break;
                }
                cx.background_executor()
                    .timer(Duration::from_millis(500))
                    .await;
            }
        }));
    }

    fn refresh_downloads(&mut self, cx: &mut Context<Self>) {
        self.load_downloads(cx);
    }

    pub(super) fn downloads_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let download_dir = self.state.read(cx).settings.download_path.clone();
        let filter = self.data.downloads.filter;

        let top_header = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .text_2xl()
                    .font_weight(FontWeight::BOLD)
                    .child("下载记录"),
            )
            .when(!download_dir.is_empty(), |row| {
                let dir_open = download_dir.clone();
                row.child(
                    icon_pill_button("open-dl-root", IconName::FolderOpen, download_dir, p)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let dir = dir_open.clone();
                            this.mutate(
                                move |_| async move {
                                    tokio::task::spawn_blocking(move || open::that(dir)).await??;
                                    Ok(())
                                },
                                "已打开下载目录",
                                Self::refresh_downloads,
                                cx,
                            );
                        })),
                )
            });

        let filter_bar = div()
            .w_full()
            .flex()
            .items_center()
            .justify_between()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1()
                    .p(px(3.0))
                    .rounded_full()
                    .bg(p.surface_elevated)
                    .children(
                        [
                            (DownloadFilter::All, "dl-all", "全部"),
                            (DownloadFilter::Active, "dl-active", "进行中"),
                            (DownloadFilter::Done, "dl-done", "已完成"),
                        ]
                        .into_iter()
                        .map(|(f, id, label)| {
                            let active = filter == f;
                            div()
                                .id(id)
                                .h(px(28.0))
                                .px_3p5()
                                .rounded_full()
                                .flex()
                                .items_center()
                                .text_xs()
                                .font_weight(if active {
                                    FontWeight::SEMIBOLD
                                } else {
                                    FontWeight::MEDIUM
                                })
                                .text_color(if active { p.accent } else { p.text_muted })
                                .cursor_pointer()
                                .when(active, |d| d.bg(p.surface_active))
                                .when(!active, |d| d.hover(move |s| s.text_color(p.text)))
                                .child(label)
                                .on_click(cx.listener(move |this, _, _, cx| {
                                    this.data.downloads.filter = f;
                                    cx.notify();
                                }))
                        }),
                    ),
            )
            .child(
                icon_pill_button("dl-refresh", IconName::RotateCw, "刷新", p)
                    .on_click(cx.listener(|this, _, _, cx| this.load_downloads(cx))),
            );

        let body = match &self.data.downloads.tasks {
            None => loading_state(p),
            Some(Err(error)) => empty_state("读取下载任务失败", error.clone(), p),
            Some(Ok(tasks)) => {
                let filtered: Vec<&DownloadTask> = tasks
                    .iter()
                    .rev()
                    .filter(|t| match filter {
                        DownloadFilter::All => true,
                        DownloadFilter::Active => is_active(&t.status),
                        DownloadFilter::Done => t.status == DownloadStatus::Done,
                    })
                    .collect();
                if filtered.is_empty() {
                    empty_state("暂无下载记录", "在歌曲上点击下载图标即可离线保存音频", p)
                } else {
                    div()
                        .w_full()
                        .flex()
                        .flex_col()
                        .gap_1()
                        .child(
                            div()
                                .h(px(34.0))
                                .px_3()
                                .rounded(px(8.0))
                                .bg(p.surface_elevated)
                                .flex()
                                .items_center()
                                .gap_3()
                                .text_xs()
                                .font_weight(FontWeight::MEDIUM)
                                .text_color(p.text_dim)
                                .child(div().flex_1().min_w_0().child("文件"))
                                .child(div().w(px(160.0)).child("状态"))
                                .child(div().w(px(110.0)).text_right().child("操作")),
                        )
                        .children(
                            filtered
                                .into_iter()
                                .enumerate()
                                .map(|(i, task)| self.download_row(i, task, p, cx)),
                        )
                }
            }
        };

        page("").child(top_header).child(
            div()
                .w_full()
                .rounded(px(14.0))
                .bg(p.surface)
                .p_4()
                .flex()
                .flex_col()
                .gap_4()
                .child(filter_bar)
                .child(body),
        )
    }

    fn download_row(
        &self,
        index: usize,
        task: &DownloadTask,
        p: BiuPalette,
        cx: &Context<Self>,
    ) -> Stateful<Div> {
        let file = task
            .save_path
            .as_deref()
            .filter(|path| !path.is_empty())
            .map(Path::new);
        let format = file
            .and_then(|path| path.extension())
            .and_then(|ext| ext.to_str())
            .or_else(|| {
                task.extra
                    .get("requestedFormat")
                    .and_then(serde_json::Value::as_str)
            })
            .unwrap_or(&task.output_file_type)
            .to_ascii_uppercase();
        let progress = task.progress();
        let cancel = task.id.clone();
        let delete = task.id.clone();
        let (status_label, status_color) = match task.status {
            DownloadStatus::Queued => ("排队中", p.text_muted),
            DownloadStatus::Downloading => ("下载中", p.accent),
            DownloadStatus::Merging => ("合并中", p.accent),
            DownloadStatus::Converting => ("转换中", p.accent),
            DownloadStatus::Paused => ("已暂停", p.warning),
            DownloadStatus::Done => ("下载完成", p.accent),
            DownloadStatus::Failed => ("失败", p.danger),
            DownloadStatus::Cancelled => ("已取消", p.text_dim),
            DownloadStatus::Other => ("未知状态", p.text_dim),
        };

        let mut actions = div()
            .w(px(110.0))
            .flex_shrink_0()
            .flex()
            .items_center()
            .justify_end()
            .gap_1();

        if let (DownloadStatus::Done, Some(path)) = (&task.status, file) {
            let path_str = path.to_string_lossy().into_owned();
            let track = TrackItem {
                id: format!(
                    "local:{}",
                    biu_core::local_music::md5_hex_lower(&path_str.to_lowercase())
                ),
                title: task.title.clone(),
                artist: String::new(),
                artist_mid: None,
                cover: task.cover.clone().unwrap_or_default(),
                duration_secs: 0.,
                source: TrackSource::LocalFile {
                    path: path_str,
                    format: format.to_ascii_lowercase(),
                },
            };
            let st = self.state.clone();
            actions = actions.child(
                compact_icon_btn(("dl-play", index), IconName::Play, "播放本地文件", p).on_click(
                    move |_, _, cx| {
                        st.update(cx, |s, cx| s.play_track(track.clone(), cx));
                    },
                ),
            );
            if let Some(dir) = path.parent().map(Path::to_path_buf) {
                actions = actions.child(
                    compact_icon_btn(("dl-dir", index), IconName::FolderOpen, "打开所在文件夹", p)
                        .on_click(cx.listener(move |this, _, _, cx| {
                            let dir = dir.clone();
                            this.mutate(
                                move |_| async move {
                                    tokio::task::spawn_blocking(move || open::that(dir)).await??;
                                    Ok(())
                                },
                                "已打开文件夹",
                                Self::refresh_downloads,
                                cx,
                            );
                        })),
                );
            }
        }
        if is_active(&task.status) && !self.mutating {
            actions = actions.child(
                compact_icon_btn(("dl-cancel", index), IconName::X, "取消下载", p).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let id = cancel.clone();
                        this.mutate(
                            move |s| async move {
                                anyhow::ensure!(s.downloads.cancel(&id)?, "任务已经结束");
                                Ok(())
                            },
                            "已取消下载",
                            Self::refresh_downloads,
                            cx,
                        );
                    }),
                ),
            );
        } else if matches!(
            task.status,
            DownloadStatus::Paused | DownloadStatus::Failed | DownloadStatus::Cancelled
        ) && !self.mutating
        {
            let retry_id = task.id.clone();
            actions = actions.child(
                compact_icon_btn(("dl-retry", index), IconName::RotateCw, "重试下载", p).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let id = retry_id.clone();
                        this.mutate(
                            move |s| async move {
                                anyhow::ensure!(s.downloads.retry(&id)?, "无法重试该任务");
                                Ok(())
                            },
                            "已重新加入下载队列",
                            Self::refresh_downloads,
                            cx,
                        );
                    }),
                ),
            );
        }
        if !self.mutating && !is_active(&task.status) {
            actions = actions.child(
                compact_icon_btn(("dl-del", index), IconName::Trash, "删除记录", p).on_click(
                    cx.listener(move |this, _, _, cx| {
                        let id = delete.clone();
                        this.mutate(
                            move |s| async move {
                                s.downloads.delete(&id)?;
                                Ok(())
                            },
                            "已删除下载记录",
                            Self::refresh_downloads,
                            cx,
                        );
                    }),
                ),
            );
        }

        let status_col = div()
            .w(px(160.0))
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_1()
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_1p5()
                    .when(task.status == DownloadStatus::Done, |d| {
                        d.child(icon(IconName::CircleCheck, 14.0, p.accent))
                    })
                    .child(
                        div()
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(status_color)
                            .child(if is_active(&task.status) {
                                format!("{status_label} · {:.0}%", progress * 100.0)
                            } else {
                                status_label.to_string()
                            }),
                    ),
            )
            .when(is_active(&task.status), |col| {
                col.child(
                    div()
                        .w_full()
                        .h(px(4.0))
                        .rounded_full()
                        .bg(p.surface_active)
                        .overflow_hidden()
                        .child(div().h_full().w(relative(progress as f32)).bg(p.accent)),
                )
            })
            .when_some(task.error.clone(), |col, err| {
                col.child(div().text_size(px(11.0)).text_color(p.danger).truncate().child(err))
            });

        div()
            .id(("dl-row", index))
            .w_full()
            .px_3()
            .py_2p5()
            .rounded(px(10.0))
            .hover(move |s| s.bg(p.surface_hover))
            .flex()
            .items_center()
            .gap_3()
            .child(thumbnail(
                &self.state,
                task.cover.as_deref().unwrap_or(""),
                ThumbSize::Tiny,
                (44., 44.),
                p,
                cx,
            ))
            .child(
                div()
                    .flex_1()
                    .min_w_0()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(
                        div()
                            .text_sm()
                            .font_weight(FontWeight::MEDIUM)
                            .truncate()
                            .child(task.title.clone()),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                div()
                                    .px_1p5()
                                    .py(px(1.0))
                                    .rounded(px(4.0))
                                    .bg(p.surface_elevated)
                                    .text_size(px(10.0))
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(p.text_muted)
                                    .child(format),
                            ),
                    ),
            )
            .child(status_col)
            .child(actions)
    }
}

fn is_active(status: &DownloadStatus) -> bool {
    matches!(
        status,
        DownloadStatus::Queued
            | DownloadStatus::Downloading
            | DownloadStatus::Converting
            | DownloadStatus::Merging
    )
}
