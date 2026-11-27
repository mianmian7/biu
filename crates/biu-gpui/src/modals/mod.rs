mod fav_folder_edit_modal;
mod fav_select_modal;
mod login_modal;
mod lyrics_search_modal;
mod page_list_modal;
mod playlist_drawer;

use crate::pages::{
    button, column, new_input, normalize_query, run_on, state_button, thumbnail, toolbar,
};
use crate::player_bar::{icon_button, TooltipExt};
use crate::state::{ActiveModal, AppState, Services};
use crate::theme::BiuPalette;
use crate::widgets::empty_state;
use biu_core::image_cache::ThumbSize;
use gpui_kit::assets::IconName;
use gpui_kit::component::input::{Input, InputEvent, InputState};
use gpui_kit::prelude::FluentBuilder;
use gpui_kit::*;
use std::{future::Future, sync::Arc};

#[derive(Debug, Clone, PartialEq, Eq)]
enum ModalKey {
    Login,
    Lyrics(Option<String>),
    Favorite(String, Option<u64>),
    EditFolder(Option<u64>, Option<u64>),
    Queue,
    PageList(Option<String>),
}

fn modal_key(state: &AppState) -> Option<ModalKey> {
    let mid = state.user.as_ref().map(|user| user.mid);
    match &state.modal {
        ActiveModal::None => None,
        ActiveModal::Login => Some(ModalKey::Login),
        ActiveModal::LyricsSearch => Some(ModalKey::Lyrics(
            state.current_track().map(|track| track.id.clone()),
        )),
        ActiveModal::FavSelect { track } => Some(ModalKey::Favorite(track.id.clone(), mid)),
        ActiveModal::FavFolderEdit { folder } => Some(ModalKey::EditFolder(
            folder.as_ref().map(|folder| folder.id),
            mid,
        )),
        ActiveModal::PlaylistDrawer => Some(ModalKey::Queue),
        ActiveModal::PageList => Some(ModalKey::PageList(
            state.current_track().map(|track| track.id.clone()),
        )),
    }
}

fn is_current(expected: &ModalKey, current: Option<&ModalKey>) -> bool {
    current == Some(expected)
}

#[derive(Default)]
struct ModalData {
    login: login_modal::LoginModal,
    lyrics: lyrics_search_modal::LyricsSearchModal,
    favorites: fav_select_modal::FavSelectModal,
    edit: fav_folder_edit_modal::FavFolderEditModal,
    page_list: page_list_modal::PageListModal,
    queue: playlist_drawer::PlaylistDrawerState,
}

pub struct ModalsHost {
    pub state: Entity<AppState>,
    key: Option<ModalKey>,
    data: ModalData,
    focus: FocusHandle,
    previous_focus: Option<FocusHandle>,
    task: Option<Task<()>>,
    submitting: bool,
    error: Option<String>,
    last_ui_ver: u64,
    last_covers_ver: u64,
}

impl ModalsHost {
    pub fn new(state: Entity<AppState>, _window: &mut Window, cx: &mut Context<Self>) -> Self {
        let (init_ui, init_covers) = {
            let s = state.read(cx);
            (s.ui_version, s.covers_version())
        };
        cx.observe(&state, |this, state, cx| {
            let s = state.read(cx);
            let modal_open = !matches!(s.modal, ActiveModal::None) || this.key.is_some();
            if !modal_open {
                return;
            }
            let ui_ver = s.ui_version;
            let covers_ver = s.covers_version();
            if this.last_ui_ver != ui_ver || this.last_covers_ver != covers_ver {
                this.last_ui_ver = ui_ver;
                this.last_covers_ver = covers_ver;
                cx.notify();
            }
        })
        .detach();
        Self {
            state,
            key: None,
            data: ModalData::default(),
            focus: cx.focus_handle(),
            previous_focus: None,
            task: None,
            submitting: false,
            error: None,
            last_ui_ver: init_ui,
            last_covers_ver: init_covers,
        }
    }

    fn sync_modal(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let state = self.state.read(cx);
        let key = modal_key(state);
        if key == self.key {
            return;
        }
        if self.key.is_none() {
            self.previous_focus = window.focused(cx);
        }
        self.key = key;
        self.task = None;
        self.submitting = false;
        self.error = None;
        self.data = ModalData::default();
        let active = state.modal.clone();
        match active {
            ActiveModal::None => {
                if let Some(previous) = self.previous_focus.take() {
                    previous.focus(window, cx);
                }
                return;
            }
            ActiveModal::Login => self.load_qr(cx),
            ActiveModal::LyricsSearch => self.enter_lyrics(window, cx),
            ActiveModal::FavSelect { track } => self.enter_favorites(track, cx),
            ActiveModal::FavFolderEdit { folder } => self.enter_folder_edit(folder, window, cx),
            ActiveModal::PlaylistDrawer => {
                let covers: Vec<(String, ThumbSize)> = state
                    .queue
                    .items()
                    .iter()
                    .take(40)
                    .filter(|t| !t.cover.is_empty())
                    .map(|t| (t.cover.clone(), ThumbSize::Tiny))
                    .collect();
                self.enter_queue(window, cx);
                if !covers.is_empty() {
                    let st = self.state.clone();
                    window.defer(cx, move |_, cx| {
                        st.update(cx, |s, cx| s.prefetch_covers(covers, cx));
                    });
                }
            }
            ActiveModal::PageList => self.enter_page_list(window, cx),
        }
        self.focus.focus(window, cx);
    }

    fn close(&mut self, cx: &mut Context<Self>) {
        if !self.submitting {
            self.task = None;
            self.state.update(cx, AppState::close_modal);
        }
    }

    fn with_task<T, F, Fut>(
        &mut self,
        fetch: F,
        finish: impl FnOnce(&mut Self, Result<T, String>, &mut Context<Self>) + 'static,
        cx: &mut Context<Self>,
    ) where
        T: Send + 'static,
        F: FnOnce(Arc<Services>) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        let Some(key) = self.key.clone() else {
            return;
        };
        let services = self.state.read(cx).services.clone();
        self.task = Some(cx.spawn(async move |this, cx| {
            let runtime = services.rt.handle().clone();
            let result = run_on(&runtime, async move { fetch(services).await }).await;
            let _ = this.update(cx, |this, cx| {
                if is_current(&key, modal_key(this.state.read(cx)).as_ref()) {
                    finish(this, result, cx);
                    cx.notify();
                }
            });
        }));
        cx.notify();
    }

    fn submit<T, F, Fut>(
        &mut self,
        fetch: F,
        success: &'static str,
        after: impl FnOnce(&mut AppState, T, &mut Context<AppState>) + 'static,
        cx: &mut Context<Self>,
    ) where
        T: Send + 'static,
        F: FnOnce(Arc<Services>) -> Fut + Send + 'static,
        Fut: Future<Output = anyhow::Result<T>> + Send + 'static,
    {
        if self.submitting {
            return;
        }
        self.error = None;
        self.submitting = true;
        self.with_task(
            fetch,
            move |this, result, cx| {
                this.submitting = false;
                match result {
                    Ok(value) => this.state.update(cx, |s, cx| {
                        after(s, value, cx);
                        s.refresh_user_and_folders(cx);
                        s.close_modal(cx);
                        s.show_toast(success, false, cx);
                    }),
                    Err(error) => {
                        this.state
                            .update(cx, |s, cx| s.show_toast(error.clone(), true, cx));
                        this.error = Some(error);
                    }
                }
            },
            cx,
        );
    }
}

impl Render for ModalsHost {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        self.sync_modal(window, cx);
        let p = self.state.read(cx).palette(window);
        let Some(key) = &self.key else {
            return div();
        };
        let drawer = matches!(key, ModalKey::Queue);
        let (title, content) = match key {
            ModalKey::Login => ("扫码登录", self.login_view(p, cx).into_any_element()),
            ModalKey::Lyrics(_) => ("搜索歌词", self.lyrics_view(p, cx).into_any_element()),
            ModalKey::Favorite(..) => ("收藏歌曲", self.favorites_view(p, cx).into_any_element()),
            ModalKey::EditFolder(folder, _) => (
                if folder.is_some() {
                    "编辑收藏夹"
                } else {
                    "新建收藏夹"
                },
                self.folder_edit_view(p, cx).into_any_element(),
            ),
            ModalKey::Queue => ("播放列表", self.queue_view(p, cx).into_any_element()),
            ModalKey::PageList(_) => ("视频分集", self.page_list_view(p, cx).into_any_element()),
        };

        div().absolute().top_0().left_0().size_full().child(
            deferred(
                div()
                    .id("modal-overlay")
                    .size_full()
                    .relative()
                    .occlude()
                    .track_focus(&self.focus)
                    .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                        if event.keystroke.key == "escape" {
                            this.close(cx);
                            cx.stop_propagation();
                        }
                    }))
                    .flex()
                    .when(drawer, |overlay| overlay.justify_end())
                    .when(!drawer, |overlay| overlay.items_center().justify_center())
                    .child(
                        div()
                            .id("backdrop")
                            .absolute()
                            .top_0()
                            .left_0()
                            .size_full()
                            .bg(rgba(0x00000099))
                            .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                    )
                    .child(
                        column()
                            .relative()
                            .occlude()
                            .bg(p.surface_elevated)
                            .text_color(p.text)
                            .border_1()
                            .border_color(p.border_strong)
                            .shadow_2xl()
                            .p_5()
                            .gap_3()
                            .when(drawer, |panel| {
                                panel
                                    .w(px(380.0))
                                    .h_full()
                                    .rounded_l(px(16.0))
                                    .border_r_0()
                            })
                            .when(!drawer, |panel| {
                                panel
                                    .w(px(520.0))
                                    .max_w(relative(0.92))
                                    .max_h(relative(0.86))
                                    .rounded(px(16.0))
                            })
                            .child(
                                div()
                                    .flex()
                                    .items_center()
                                    .justify_between()
                                    .child(
                                        div()
                                            .text_lg()
                                            .font_weight(FontWeight::BOLD)
                                            .child(title),
                                    )
                                    .when(!self.submitting, |bar| {
                                        bar.child(
                                            icon_button("close", IconName::X, 16.0, p, false)
                                                .tip("关闭")
                                                .on_click(cx.listener(|this, _, _, cx| {
                                                    this.close(cx)
                                                })),
                                        )
                                    }),
                            )
                            .when_some(self.error.clone(), |panel, error| {
                                panel.child(div().text_color(p.danger).text_sm().child(error))
                            })
                            .when(self.submitting, |panel| {
                                panel.child(
                                    div().text_sm().text_color(p.text_muted).child("正在保存…"),
                                )
                            })
                            .child(if drawer {
                                div()
                                    .flex_1()
                                    .min_h_0()
                                    .flex()
                                    .flex_col()
                                    .child(content)
                                    .into_any_element()
                            } else if matches!(key, ModalKey::Login) {
                                div().w_full().child(content).into_any_element()
                            } else {
                                div()
                                    .id("modal-content")
                                    .min_h_0()
                                    .overflow_y_scroll()
                                    .child(content)
                                    .into_any_element()
                            }),
                    ),
            )
            .with_priority(10),
        )
    }
}

#[cfg(test)]
mod tests {
    use super::{ModalKey, is_current};

    #[test]
    fn stale_callback_rejects_closed_or_replaced_modal() {
        assert!(is_current(&ModalKey::Login, Some(&ModalKey::Login)));
        assert!(!is_current(&ModalKey::Login, None));
        assert!(!is_current(&ModalKey::Login, Some(&ModalKey::Queue)));
        let modal = ModalKey::Favorite("BV1:1".into(), Some(42));
        assert!(!is_current(
            &modal,
            Some(&ModalKey::Favorite("BV1:1".into(), Some(43)))
        ));
    }
}
