use super::*;
use biu_core::{DownloadStatus, DownloadTask};
use std::{path::Path, time::Duration};

#[derive(Default)]
pub(super) struct DownloadList {
    tasks: Option<Result<Vec<DownloadTask>, String>>,
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
        let root = page("下载列表").child(
            toolbar().child(
                button("refresh", "刷新", p)
                    .on_click(cx.listener(|this, _, _, cx| this.load_downloads(cx))),
            ),
        );
        let tasks = match &self.data.downloads.tasks {
            None => return root.child(empty_state("正在读取下载任务…", "", p)),
            Some(Err(error)) => {
                return root.child(empty_state("读取下载任务失败", error.clone(), p));
            }
            Some(Ok(tasks)) => tasks,
        };
        if tasks.is_empty() {
            return root.child(empty_state("暂无下载", "", p));
        }
        root.children(
            tasks
                .iter()
                .rev()
                .enumerate()
                .map(|(i, task)| self.download_row(i, task, p, cx)),
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
        let status = match task.status {
            DownloadStatus::Queued => "排队中",
            DownloadStatus::Downloading => "下载中",
            DownloadStatus::Merging => "合并中",
            DownloadStatus::Converting => "转换中",
            DownloadStatus::Paused => "已暂停",
            DownloadStatus::Done => "已完成",
            DownloadStatus::Failed => "失败",
            DownloadStatus::Cancelled => "已取消",
            DownloadStatus::Other => "未知状态",
        };
        let mut actions = toolbar();
        if let (DownloadStatus::Done, Some(path)) = (&task.status, file) {
            if let Some(dir) = path.parent().map(Path::to_path_buf) {
                actions = actions.child(button("open-folder", "打开文件夹", p).on_click(
                    cx.listener(move |this, _, _, cx| {
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
                    }),
                ));
            }
            let path = path.to_string_lossy().into_owned();
            let track = TrackItem {
                id: format!(
                    "local:{}",
                    biu_core::local_music::md5_hex_lower(&path.to_lowercase())
                ),
                title: task.title.clone(),
                artist: String::new(),
                artist_mid: None,
                cover: task.cover.clone().unwrap_or_default(),
                duration_secs: 0.,
                source: TrackSource::LocalFile {
                    path,
                    format: format.to_ascii_lowercase(),
                },
            };
            actions = actions.child(state_button(
                &self.state,
                "play",
                "播放文件",
                p,
                move |s, cx| s.play_track(track.clone(), cx),
            ));
        }
        if is_active(&task.status) && !self.mutating {
            actions = actions.child(button("cancel", "取消", p).on_click(cx.listener(
                move |this, _, _, cx| {
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
                },
            )));
        }
        if !self.mutating && !is_active(&task.status) {
            actions = actions.child(button("delete", "删除记录", p).on_click(cx.listener(
                move |this, _, _, cx| {
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
                },
            )));
        }
        div().id(index).rounded(p.radius).bg(p.surface).p_3().child(
            column()
                .child(
                    toolbar()
                        .child(div().flex_1().child(task.title.clone()))
                        .child(
                            div()
                                .bg(p.surface_active)
                                .rounded(p.radius)
                                .px_2()
                                .text_sm()
                                .child(format),
                        )
                        .child(
                            div()
                                .text_color(p.text_muted)
                                .text_sm()
                                .child(format!("{status} · {:.0}%", progress * 100.)),
                        ),
                )
                .child(
                    div()
                        .h(px(5.))
                        .bg(p.surface_active)
                        .rounded(p.radius)
                        .overflow_hidden()
                        .child(div().h_full().w(relative(progress as f32)).bg(p.accent)),
                )
                .when_some(task.error.clone(), |c, error| {
                    c.child(div().text_color(p.danger).text_sm().child(error))
                })
                .child(actions),
        )
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
