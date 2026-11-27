use super::*;
use crate::state::{CollectionKind, Route};
use biu_core::FavFolder;
use gpui_kit::component::switch::Switch;

#[derive(Default)]
pub(super) struct FavFolderEditModal {
    folder: Option<FavFolder>,
    title: Option<Entity<InputState>>,
    intro: Option<Entity<InputState>>,
    private: bool,
    confirm_delete: bool,
}

impl ModalsHost {
    pub(super) fn enter_folder_edit(
        &mut self,
        folder: Option<FavFolder>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        let title = folder.as_ref().map_or("", |folder| folder.title.as_str());
        let intro = folder.as_ref().map_or("", |folder| folder.intro.as_str());
        self.data.edit = FavFolderEditModal {
            title: Some(new_input("收藏夹名称", title.to_owned(), window, cx)),
            intro: Some(new_input("收藏夹简介", intro.to_owned(), window, cx)),
            private: folder.as_ref().is_some_and(|folder| folder.attr & 1 != 0),
            folder,
            confirm_delete: false,
        };
    }

    fn save_folder(&mut self, cx: &mut Context<Self>) {
        let edit = &self.data.edit;
        let (Some(title), Some(intro)) = (&edit.title, &edit.intro) else {
            return;
        };
        let title = normalize_query(&title.read(cx).value());
        let intro = normalize_query(&intro.read(cx).value());
        if title.is_empty() {
            self.error = Some("请输入收藏夹名称".into());
            cx.notify();
            return;
        }
        let folder = edit.folder.clone();
        let private = edit.private;
        let success = if folder.is_some() {
            "收藏夹已保存"
        } else {
            "收藏夹已创建"
        };
        self.submit(move |s| async move {
            if let Some(folder) = folder {
                s.bili.edit_fav_folder(folder.id, &title, &intro, private, (!folder.cover.is_empty()).then_some(folder.cover.as_str())).await
            } else {
                s.bili.create_fav_folder(&title, &intro, private).await
            }
        }, success, |state, folder, cx| {
            if matches!(state.route, Route::Collection { kind: CollectionKind::FavFolder, id, .. } if id == folder.id) {
                state.navigate(Route::Collection { kind: CollectionKind::FavFolder, id: folder.id, mid: folder.mid, title: folder.title }, cx);
            }
        }, cx);
    }

    fn delete_folder(&mut self, cx: &mut Context<Self>) {
        let Some(id) = self.data.edit.folder.as_ref().map(|folder| folder.id) else {
            return;
        };
        self.submit(move |s| async move { s.bili.delete_fav_folder(id).await }, "收藏夹已删除", move |state, (), cx| {
            if matches!(state.route, Route::Collection { kind: CollectionKind::FavFolder, id: current, .. } if current == id) {
                state.navigate(Route::MusicRecommend, cx);
            }
        }, cx);
    }

    pub(super) fn folder_edit_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        if self.state.read(cx).user.is_none() {
            return empty_state("请先登录", "", p);
        }
        let edit = &self.data.edit;
        let (Some(title), Some(intro)) = (&edit.title, &edit.intro) else {
            return div();
        };
        let focus = cx.entity().downgrade();
        let root = column()
            .gap_3()
            .child(column().child("名称").child(Input::new(title)))
            .child(column().child("简介").child(Input::new(intro)))
            .child(
                Switch::new("privacy")
                    .label(if edit.private {
                        "公开收藏夹（当前私密）"
                    } else {
                        "公开收藏夹"
                    })
                    .checked(!edit.private)
                    .on_click(move |public, _, cx| {
                        let _ = focus.update(cx, |this, cx| {
                            if !this.submitting {
                                this.data.edit.private = !public;
                                cx.notify();
                            }
                        });
                    }),
            );
        if self.submitting {
            return root;
        }
        if edit.confirm_delete {
            return root.child(
                toolbar()
                    .child("确定删除这个收藏夹？")
                    .child(
                        button("confirm-delete", "确认删除", p)
                            .text_color(p.danger)
                            .on_click(cx.listener(|this, _, _, cx| this.delete_folder(cx))),
                    )
                    .child(button("cancel-delete", "取消", p).on_click(cx.listener(
                        |this, _, _, cx| {
                            this.data.edit.confirm_delete = false;
                            cx.notify();
                        },
                    ))),
            );
        }
        root.child(
            toolbar()
                .child(
                    button("save", "保存", p)
                        .on_click(cx.listener(|this, _, _, cx| this.save_folder(cx))),
                )
                .when(
                    edit.folder
                        .as_ref()
                        .is_some_and(|folder| ((folder.attr >> 1) & 1) != 0),
                    |bar| {
                        bar.child(
                            button("delete", "删除收藏夹", p)
                                .text_color(p.danger)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.data.edit.confirm_delete = true;
                                    cx.notify();
                                })),
                        )
                    },
                ),
        )
    }
}
