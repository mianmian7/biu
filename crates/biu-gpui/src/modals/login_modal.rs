use super::*;
use crate::image_loader::render_qr_image;
use biu_core::QrPollStatus;
use std::time::Duration;

#[derive(Default)]
pub(super) struct LoginModal {
    image: Option<Arc<RenderImage>>,
    status: QrStatus,
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
                    let runtime = this.state.read(cx).services.rt.handle().clone();
                    this.task = Some(cx.spawn(async move |this, cx| {
                        loop {
                            cx.background_executor().timer(Duration::from_secs(2)).await;
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
        self.data.login.status = match result {
            Ok(QrPollStatus::WaitingForScan) => QrStatus::Scan,
            Ok(QrPollStatus::WaitingForConfirmation) => QrStatus::Confirm,
            Ok(QrPollStatus::Expired) => QrStatus::Expired,
            Err(error) => QrStatus::Failed(error),
            Ok(QrPollStatus::LoggedIn { .. }) => {
                self.state.update(cx, |s, cx| {
                    s.refresh_user_and_folders(cx);
                    s.close_modal(cx);
                    s.show_toast("登录成功", false, cx);
                });
                return false;
            }
        };
        cx.notify();
        matches!(self.data.login.status, QrStatus::Scan | QrStatus::Confirm)
    }

    pub(super) fn login_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let login = &self.data.login;
        let status = match &login.status {
            QrStatus::Loading => "正在生成二维码…",
            QrStatus::Scan => "请使用哔哩哔哩 App 扫码",
            QrStatus::Confirm => "已扫码，请在手机上确认",
            QrStatus::Expired => "二维码已过期",
            QrStatus::Failed(error) => error,
        };
        column()
            .items_center()
            .gap_3()
            .pb_4()
            .when_some(login.image.clone(), |c, image| {
                let size = image.size(0);
                c.child(
                    img(image)
                        .w(px(size.width.0 as f32))
                        .h(px(size.height.0 as f32)),
                )
            })
            .child(div().text_color(p.text_muted).child(status.to_owned()))
            .when(
                matches!(login.status, QrStatus::Expired | QrStatus::Failed(_)),
                |c| {
                    c.child(
                        button("refresh-qr", "刷新二维码", p)
                            .on_click(cx.listener(|this, _, _, cx| this.load_qr(cx))),
                    )
                },
            )
    }
}
