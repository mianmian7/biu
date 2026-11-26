use super::*;
use crate::pages::playable_id;
use biu_core::{FavFolder, TrackItem, TrackSource};
use gpui_kit::component::checkbox::Checkbox;
use std::collections::BTreeSet;

#[derive(Default)]
pub(super) struct FavSelectModal {
    track: Option<TrackItem>,
    membership: Option<Result<Membership, String>>,
    selected: BTreeSet<u64>,
}

struct Membership {
    resource: (u64, u32),
    folders: Vec<FavFolder>,
    initial: BTreeSet<u64>,
}

impl ModalsHost {
    pub(super) fn enter_favorites(&mut self, track: TrackItem, cx: &mut Context<Self>) {
        self.data.favorites.track = Some(track.clone());
        self.data.favorites.membership = None;
        let state = self.state.read(cx);
        let Some(mid) = state.user.as_ref().map(|user| user.mid) else {
            return;
        };
        self.with_task(
            move |s| async move {
                let resource = match (&track.source, playable_id(&track)) {
                    (_, Some(resource)) => Some(resource),
                    (TrackSource::BiliVideo { bvid, .. }, None) => {
                        playable_id(&s.bili.get_video_view(bvid).await?.track)
                    }
                    _ => None,
                }
                .ok_or_else(|| anyhow::anyhow!("无法识别这首歌曲的收藏 ID"))?;
                let rows = s
                    .bili
                    .get_created_fav_folders_for_resource(mid, resource.0)
                    .await?;
                let mut folders = Vec::with_capacity(rows.len());
                let mut initial = BTreeSet::new();
                for (folder, favored) in rows {
                    if favored {
                        initial.insert(folder.id);
                    }
                    folders.push(folder);
                }
                Ok(Membership {
                    resource,
                    folders,
                    initial,
                })
            },
            |this, result, _| {
                if let Ok(membership) = &result {
                    this.data.favorites.selected = membership.initial.clone();
                }
                this.data.favorites.membership = Some(result);
            },
            cx,
        );
    }

    fn save_favorites(&mut self, cx: &mut Context<Self>) {
        let data = &self.data.favorites;
        let Some(Ok(membership)) = &data.membership else {
            return;
        };
        let (add, remove) = changes(&membership.initial, &data.selected);
        if add.is_empty() && remove.is_empty() {
            self.close(cx);
            return;
        }
        let (rid, kind) = membership.resource;
        self.submit(
            move |s| async move {
                if kind == 2 {
                    s.bili.deal_fav_resource(rid, &add, &remove).await
                } else {
                    s.bili
                        .deal_fav_resource_with_type(rid, kind, &add, &remove)
                        .await
                }
            },
            "收藏已更新",
            |_, (), _| {},
            cx,
        );
    }

    pub(super) fn favorites_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let data = &self.data.favorites;
        if self.state.read(cx).user.is_none() {
            return empty_state("请先登录", "", p).child(state_button(
                &self.state,
                "login",
                "扫码登录",
                p,
                |s, cx| {
                    s.open_modal(ActiveModal::Login, cx);
                },
            ));
        }
        let root = column().child(
            div().text_color(p.text_muted).child(
                data.track
                    .as_ref()
                    .map(|track| track.title.clone())
                    .unwrap_or_default(),
            ),
        );
        match &data.membership {
            None => root.child(empty_state("正在读取收藏夹…", "", p)),
            Some(Err(error)) => root.child(empty_state("收藏夹加载失败", error.clone(), p).child(
                button("retry", "重试", p).on_click(cx.listener(|this, _, _, cx| {
                    if let Some(track) = this.data.favorites.track.clone() {
                        this.enter_favorites(track, cx);
                    }
                })),
            )),
            Some(Ok(membership)) if membership.folders.is_empty() => root
                .child(empty_state("暂无收藏夹", "", p))
                .child(toolbar().child(state_button(
                    &self.state,
                    "create",
                    "新建收藏夹",
                    p,
                    |s, cx| {
                        s.open_modal(ActiveModal::FavFolderEdit { folder: None }, cx);
                    },
                ))),
            Some(Ok(membership)) => root
                .children(membership.folders.iter().enumerate().map(|(i, folder)| {
                    let id = folder.id;
                    let host = cx.entity().downgrade();
                    div()
                        .id(i)
                        .p_2()
                        .rounded(p.radius)
                        .bg(p.surface_active)
                        .child(
                            Checkbox::new("folder")
                                .label(folder.title.clone())
                                .checked(data.selected.contains(&id))
                                .on_click(move |checked, _, cx| {
                                    let _ = host.update(cx, |this, cx| {
                                        if this.submitting {
                                            return;
                                        }
                                        if *checked {
                                            this.data.favorites.selected.insert(id);
                                        } else {
                                            this.data.favorites.selected.remove(&id);
                                        }
                                        cx.notify();
                                    });
                                }),
                        )
                }))
                .when(!self.submitting, |c| {
                    c.child(
                        toolbar().child(
                            button("save", "保存", p)
                                .on_click(cx.listener(|this, _, _, cx| this.save_favorites(cx))),
                        ),
                    )
                }),
        }
    }
}

fn changes(initial: &BTreeSet<u64>, selected: &BTreeSet<u64>) -> (Vec<u64>, Vec<u64>) {
    (
        selected.difference(initial).copied().collect(),
        initial.difference(selected).copied().collect(),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn saves_only_favorite_changes() {
        assert_eq!(
            super::changes(&[2, 5].into(), &[2, 9].into()),
            (vec![9], vec![5])
        );
        assert_eq!(super::changes(&[2].into(), &[2].into()), (vec![], vec![]));
    }
}
