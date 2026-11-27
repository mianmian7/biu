use super::*;
use crate::image_loader::render_qr_image;
use crate::player_bar::icon;
use biu_core::QrPollStatus;
use std::time::Duration;

const MAX_TRANSIENT_POLL_ERRORS: u8 = 3;

#[derive(Default)]
pub(super) struct LoginModal {
    image: Option<Arc<RenderImage>>,
    status: QrStatus,
    poll_errors: u8,
}

#[derive(Default)]
enum QrStatus {
    #[default]
    Loading,
    Scan,
    Confirm,
    Expired,
    Failed(String),
}

impl ModalsHost {
    pub(super) fn load_qr(&mut self, cx: &mut Context<Self>) {
        self.data.login = LoginModal::default();
        let bili = self.state.read(cx).services.bili.clone();
        self.with_task(
            move |s| async move { s.bili.generate_qrcode().await },
            move |this, result, cx| match result.and_then(|session| {
                let image = render_qr_image(&session.url).ok_or("无法生成二维码")?;
                Ok((image, session.qrcode_key))
            }) {
                Ok((image, key)) => {
                    this.data.login.image = Some(image);
                    this.data.login.status = QrStatus::Scan;
                    this.data.login.poll_errors = 0;
                    let runtime = this.state.read(cx).services.rt.handle().clone();
                    this.task = Some(cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor()
                                .timer(Duration::from_millis(1500))
                                .await;
                            let client = bili.clone();
                            let key = key.clone();
                            let result =
                                run_on(&runtime, async move { client.poll_qrcode(&key).await })
                                    .await;
                            let again = this.update(cx, |this, cx| {
                                if !is_current(
                                    &ModalKey::Login,
                                    modal_key(this.state.read(cx)).as_ref(),
                                ) {
                                    return false;
                                }
                                this.receive_qr_status(result, cx)
                            });
                            if !matches!(again, Ok(true)) {
                                break;
                            }
                        }
                    }));
                }
                Err(error) => this.data.login.status = QrStatus::Failed(error),
            },
            cx,
        );
    }

    fn receive_qr_status(
        &mut self,
        result: Result<QrPollStatus, String>,
        cx: &mut Context<Self>,
    ) -> bool {
        match result {
            Ok(QrPollStatus::WaitingForScan) => {
                self.data.login.poll_errors = 0;
                self.data.login.status = QrStatus::Scan;
            }
            Ok(QrPollStatus::WaitingForConfirmation) => {
                self.data.login.poll_errors = 0;
                self.data.login.status = QrStatus::Confirm;
            }
            Ok(QrPollStatus::Expired) => {
                self.data.login.status = QrStatus::Expired;
            }
            Err(error) => {
                self.data.login.poll_errors = self.data.login.poll_errors.saturating_add(1);
                if self.data.login.poll_errors < MAX_TRANSIENT_POLL_ERRORS {
                    // Tolerate transient network hiccups without aborting QR polling.
                    return true;
                }
                self.data.login.status = QrStatus::Failed(error);
            }
            Ok(QrPollStatus::LoggedIn { .. }) => {
                self.state.update(cx, |s, cx| {
                    s.refresh_user_and_folders(cx);
                    s.close_modal(cx);
                    s.show_toast("登录成功", false, cx);
                });
                return false;
            }
        }
        cx.notify();
        matches!(self.data.login.status, QrStatus::Scan | QrStatus::Confirm)
    }

    pub(super) fn login_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let login = &self.data.login;
        let (status, status_color) = match &login.status {
            QrStatus::Loading => ("正在生成二维码…", p.text_muted),
            QrStatus::Scan => ("请使用哔哩哔哩手机客户端扫码登录", p.text_muted),
            QrStatus::Confirm => ("已扫码，请在手机上点击确认登录", p.accent),
            QrStatus::Expired => ("二维码已过期，请点击刷新", p.warning),
            QrStatus::Failed(error) => (error.as_str(), p.danger),
        };
        let expired = matches!(login.status, QrStatus::Expired | QrStatus::Failed(_));

        column()
            .w_full()
            .items_center()
            .gap_4()
            .py_4()
            .child(
                div()
                    .relative()
                    .size(px(216.0))
                    .p_3()
                    .rounded(px(16.0))
                    .bg(gpui_kit::white())
                    .shadow_lg()
                    .flex()
                    .items_center()
                    .justify_center()
                    .when_some(login.image.clone(), |box_el, image| {
                        box_el.child(img(image).size(px(192.0)))
                    })
                    .when(login.image.is_none(), |box_el| {
                        box_el.child(icon(IconName::LoaderCircle, 28.0, p.accent))
                    })
                    .when(expired, |box_el| {
                        box_el.child(
                            div()
                                .id("qr-expired-mask")
                                .absolute()
                                .inset_0()
                                .rounded(px(16.0))
                                .bg(hsla(0.0, 0.0, 0.0, 0.72))
                                .flex()
                                .flex_col()
                                .items_center()
                                .justify_center()
                                .gap_2()
                                .cursor_pointer()
                                .on_click(cx.listener(|this, _, _, cx| this.load_qr(cx)))
                                .child(
                                    div()
                                        .size(px(40.0))
                                        .rounded_full()
                                        .bg(p.accent)
                                        .flex()
                                        .items_center()
                                        .justify_center()
                                        .child(icon(IconName::RotateCw, 18.0, p.accent_fg)),
                                )
                                .child(
                                    div()
                                        .text_xs()
                                        .font_weight(FontWeight::SEMIBOLD)
                                        .text_color(gpui_kit::white())
                                        .child("点击刷新二维码"),
                                ),
                        )
                    }),
            )
            .child(
                div()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .text_color(status_color)
                    .child(status.to_owned()),
            )
            .child(
                button("refresh-qr", "刷新二维码", p)
                    .on_click(cx.listener(|this, _, _, cx| this.load_qr(cx))),
            )
    }
}
