use super::*;
use crate::theme::parse_hex_hsla;
use crate::widgets::section_header;
use biu_core::{AppSettings, AudioQuality, ProxySettings};
use gpui_kit::component::switch::Switch;
use std::path::{Path, PathBuf};

const COLORS: [&str; 6] = [
    "#1ed760", "#fb7299", "#3b82f6", "#a855f7", "#f59e0b", "#ef4444",
];

struct SettingsInputs {
    ffmpeg: Entity<InputState>,
    host: Entity<InputState>,
    port: Entity<InputState>,
    username: Entity<InputState>,
    password: Entity<InputState>,
}

#[derive(Default)]
pub(super) struct Settings {
    inputs: Option<SettingsInputs>,
    proxy_type: String,
    cache: Load<u64>,
    ffmpeg: Load<Option<PathBuf>>,
    choosing_directory: bool,
}

impl PagesHost {
    pub(super) fn enter_settings(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let settings = self.state.read(cx).settings.clone();
        let proxy = settings.proxy_settings;
        self.data.settings.proxy_type = proxy.proxy_type;
        self.data.settings.inputs = Some(SettingsInputs {
            ffmpeg: new_input("留空时自动查找", settings.ffmpeg_path, window, cx),
            host: new_input("主机", proxy.host.unwrap_or_default(), window, cx),
            port: new_input(
                "端口",
                proxy.port.map(|p| p.to_string()).unwrap_or_default(),
                window,
                cx,
            ),
            username: new_input(
                "用户名（可选）",
                proxy.username.unwrap_or_default(),
                window,
                cx,
            ),
            password: cx.new(|cx| {
                InputState::new(window, cx)
                    .placeholder("密码（可选）")
                    .default_value(proxy.password.unwrap_or_default())
                    .masked(true)
            }),
        });
        self.load_cache(false, cx);
        self.detect_ffmpeg(cx);
    }

    fn load_cache(&mut self, clear: bool, cx: &mut Context<Self>) {
        self.load(
            |h| &mut h.data.settings.cache,
            move |s| async move {
                if clear {
                    s.media_cache.clear_cache().await?;
                }
                Ok(s.media_cache.cache_size_bytes().await?)
            },
            cx,
        );
    }

    fn detect_ffmpeg(&mut self, cx: &mut Context<Self>) {
        let setting = self.state.read(cx).settings.ffmpeg_path.trim().to_owned();
        self.load(
            |h| &mut h.data.settings.ffmpeg,
            move |_| async move {
                Ok(tokio::task::spawn_blocking(move || {
                    biu_audio::decoder::resolve_ffmpeg(
                        (!setting.is_empty()).then(|| Path::new(&setting)),
                    )
                })
                .await?)
            },
            cx,
        );
    }

    fn apply_proxy(&mut self, cx: &mut Context<Self>) {
        let Some(inputs) = &self.data.settings.inputs else {
            return;
        };
        let proxy_type = self.data.settings.proxy_type.clone();
        let field = |input: &Entity<InputState>| {
            let value = normalize_query(&input.read(cx).value());
            (!value.is_empty()).then_some(value)
        };
        let host = field(&inputs.host);
        let port = field(&inputs.port)
            .and_then(|value| value.parse::<u16>().ok())
            .filter(|port| *port > 0);
        let password = inputs.password.read(cx).value().to_string();
        let proxy = ProxySettings {
            proxy_type,
            host,
            port,
            username: field(&inputs.username),
            password: (!password.is_empty()).then_some(password),
        };
        self.state.update(cx, |s, cx| {
            if proxy.proxy_type != "none"
                && (proxy.port.is_none()
                    || !proxy
                        .host
                        .as_deref()
                        .is_some_and(|host| url::Host::parse(host).is_ok()))
            {
                s.show_toast("请输入有效的代理主机和端口", true, cx);
                return;
            }
            match s.services.bili.update_proxy(Some(proxy.clone())) {
                Ok(()) => {
                    s.update_settings(|settings| settings.proxy_settings = proxy, cx);
                    s.show_toast("代理设置已应用", false, cx);
                }
                Err(error) => s.show_toast(format!("代理设置失败：{error}"), true, cx),
            }
        });
    }

    fn pick_download_dir(&mut self, cx: &mut Context<Self>) {
        if self.data.settings.choosing_directory {
            return;
        }
        self.data.settings.choosing_directory = true;
        self.pick_folder(
            |this, path, cx| {
                this.data.settings.choosing_directory = false;
                if let Some(path) = path {
                    this.state.update(cx, |s, cx| {
                        s.update_settings(|settings| settings.download_path = path, cx)
                    });
                }
            },
            cx,
        );
    }

    pub(super) fn settings_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let state = self.state.read(cx);
        let s = &state.settings;
        let entity = &self.state;
        let appearance = group("外观", p)
            .child(options(
                entity,
                "主题",
                s.theme_mode.as_str(),
                &[("system", "跟随系统"), ("dark", "深色"), ("light", "浅色")],
                p,
                |s, value| s.theme_mode = value.into(),
            ))
            .child(swatches(
                entity,
                "主题色",
                &s.primary_color,
                p,
                |s, value| s.primary_color = value.into(),
            ))
            .child(options(
                entity,
                "窗口圆角",
                s.border_radius,
                &[(4, "4"), (8, "8"), (12, "12")],
                p,
                |s, value| s.border_radius = value,
            ))
            .child(options(
                entity,
                "内容布局",
                s.display_mode.as_str(),
                &[("list", "列表"), ("grid", "网格")],
                p,
                |s, value| s.display_mode = value.into(),
            ));

        let cache = &self.data.settings.cache;
        let playback = group("播放与音频", p)
            .child(options(
                entity,
                "音质",
                s.audio_quality,
                &[
                    (AudioQuality::Auto, "自动"),
                    (AudioQuality::Lossless, "无损"),
                    (AudioQuality::High, "高"),
                    (AudioQuality::Medium, "中"),
                    (AudioQuality::Low, "低"),
                ],
                p,
                |s, quality| s.audio_quality = quality,
            ))
            .child(options(
                entity,
                "关闭窗口",
                s.close_window_option.as_str(),
                &[("hide", "最小化到托盘"), ("quit", "直接退出")],
                p,
                |s, value| s.close_window_option = value.into(),
            ))
            .child(toggle(
                entity,
                "上报播放历史",
                s.report_play_history,
                |s, value| s.report_play_history = value,
            ))
            .child(match &cache.result {
                None => row("音频缓存").child("正在读取…"),
                Some(Err(error)) => row("音频缓存").child(error.clone()).child(
                    button("retry-cache", "重试", p)
                        .on_click(cx.listener(|this, _, _, cx| this.load_cache(false, cx))),
                ),
                Some(Ok(bytes)) => row("音频缓存")
                    .child(format!("{:.1} MiB", *bytes as f64 / 1_048_576.))
                    .child(
                        button("clear-cache", "清理音频缓存", p)
                            .on_click(cx.listener(|this, _, _, cx| this.load_cache(true, cx))),
                    ),
            });

        let style = &s.desktop_lyrics_style;
        let lyrics = group("歌词与桌面歌词", p)
            .child(toggle(entity, "顶部歌词", s.show_top_lyrics, |s, v| {
                s.show_top_lyrics = v
            }))
            .child(toggle(
                entity,
                "顶部歌词翻译",
                s.show_top_lyrics_translation,
                |s, v| s.show_top_lyrics_translation = v,
            ))
            .child(font_size(
                entity,
                "全屏歌词字号",
                s.lyrics_font_size,
                p,
                |s| &mut s.lyrics_font_size,
            ))
            .child(toggle(
                entity,
                "锁定桌面歌词",
                s.desktop_lyrics_locked,
                |s, v| s.desktop_lyrics_locked = v,
            ))
            .child(options(
                entity,
                "桌面歌词布局",
                style.layout.as_str(),
                &[("double", "双行"), ("single", "单行")],
                p,
                |s, value| s.desktop_lyrics_style.layout = value.into(),
            ))
            .child(options(
                entity,
                "桌面歌词对齐",
                style.align.as_str(),
                &[("left", "左对齐"), ("center", "居中"), ("right", "右对齐")],
                p,
                |s, value| s.desktop_lyrics_style.align = value.into(),
            ))
            .child(toggle(
                entity,
                "显示翻译",
                style.show_translation,
                |s, v| s.desktop_lyrics_style.show_translation = v,
            ))
            .child(toggle(
                entity,
                "显示罗马音",
                style.show_romaji,
                |s, v| s.desktop_lyrics_style.show_romaji = v,
            ))
            .child(toggle(entity, "卡拉 OK", style.karaoke, |s, v| {
                s.desktop_lyrics_style.karaoke = v
            }))
            .child(font_size(
                entity,
                "桌面歌词字号",
                s.desktop_lyrics_font_size,
                p,
                |s| &mut s.desktop_lyrics_font_size,
            ))
            .child(swatches(
                entity,
                "已唱歌词颜色",
                &style.played_color,
                p,
                |s, value| s.desktop_lyrics_style.played_color = value.into(),
            ));

        let mut local = group("下载与本地", p).child(
            row("下载目录")
                .child(s.download_path.clone())
                .child(
                    button(
                        "pick-download",
                        if self.data.settings.choosing_directory {
                            "正在选择…"
                        } else {
                            "选择文件夹"
                        },
                        p,
                    )
                    .on_click(cx.listener(|this, _, _, cx| this.pick_download_dir(cx))),
                )
                .child(
                    div()
                        .text_sm()
                        .text_color(p.text_muted)
                        .child("更改目录后重启生效"),
                ),
        );
        let mut proxy = group("网络代理", p);
        if let Some(inputs) = &self.data.settings.inputs {
            let ffmpeg = inputs.ffmpeg.clone();
            local = local.child(
                row("FFmpeg")
                    .child(div().w(px(280.)).child(Input::new(&ffmpeg)))
                    .child(button("save-ffmpeg", "应用", p).on_click(cx.listener(
                        move |this, _, _, cx| {
                            let path = normalize_query(&ffmpeg.read(cx).value());
                            this.state.update(cx, |s, cx| {
                                s.update_settings(|s| s.ffmpeg_path = path, cx)
                            });
                            this.detect_ffmpeg(cx);
                        },
                    )))
                    .child(div().text_sm().text_color(p.text_muted).child(
                        match &self.data.settings.ffmpeg.result {
                            None => "正在检测…".into(),
                            Some(Ok(Some(path))) => format!("已检测到 {}", path.display()),
                            Some(Ok(None)) => "未检测到 FFmpeg".into(),
                            Some(Err(error)) => format!("检测失败：{error}"),
                        },
                    )),
            );
            proxy = proxy
                .child(
                    row("代理类型").children(
                        [("none", "无"), ("http", "HTTP"), ("socks5", "SOCKS5")]
                            .into_iter()
                            .map(|(id, title)| {
                                button(id, title, p)
                                    .when(self.data.settings.proxy_type == id, |b| {
                                        b.border_color(p.accent)
                                    })
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.data.settings.proxy_type = id.into();
                                        cx.notify();
                                    }))
                            }),
                    ),
                )
                .when(self.data.settings.proxy_type != "none", |group| {
                    group.children(
                        [
                            ("主机", &inputs.host),
                            ("端口", &inputs.port),
                            ("用户名", &inputs.username),
                            ("密码", &inputs.password),
                        ]
                        .into_iter()
                        .map(|(label, input)| {
                            row(label).child(div().w(px(280.)).child(Input::new(input)))
                        }),
                    )
                })
                .child(
                    toolbar().child(
                        button("apply-proxy", "应用", p)
                            .on_click(cx.listener(|this, _, _, cx| this.apply_proxy(cx))),
                    ),
                );
        }

        let account = if let Some(user) = &state.user {
            group("账号", p).child(
                toolbar()
                    .child(thumbnail(
                        entity,
                        &user.face,
                        ThumbSize::Tiny,
                        (48., 48.),
                        p,
                        cx,
                    ))
                    .child(div().flex_1().child(user.name.clone()))
                    .when(!self.mutating, |bar| {
                        bar.child(button("logout", "退出登录", p).on_click(cx.listener(
                            |this, _, _, cx| {
                                this.mutate(
                                    |s| async move { s.bili.logout().await },
                                    "已退出登录",
                                    |this, cx| {
                                        this.state.update(cx, |s, cx| {
                                            s.user = None;
                                            s.created_folders.clear();
                                            s.collected_folders.clear();
                                            s.refresh_user_and_folders(cx);
                                            cx.notify();
                                        });
                                    },
                                    cx,
                                );
                            },
                        )))
                    }),
            )
        } else {
            group("账号", p).child(toolbar().child(state_button(
                entity,
                "login",
                "扫码登录",
                p,
                |s, cx| s.open_modal(ActiveModal::Login, cx),
            )))
        };
        page("设置")
            .child(appearance)
            .child(playback)
            .child(lyrics)
            .child(local)
            .child(proxy)
            .child(account)
    }
}

fn group(title: &'static str, p: BiuPalette) -> Div {
    column()
        .bg(p.surface)
        .rounded(p.radius)
        .p_4()
        .child(section_header(title, p))
}

fn row(label: &'static str) -> Stateful<Div> {
    div()
        .id(label)
        .child(toolbar().child(div().w(px(140.)).flex_shrink_0().child(label)))
        .flex()
        .flex_wrap()
        .items_center()
        .gap_2()
        .py_1()
}

fn options<T: Copy + PartialEq<S> + 'static, S>(
    entity: &Entity<AppState>,
    label: &'static str,
    selected: S,
    values: &[(T, &'static str)],
    p: BiuPalette,
    set: fn(&mut AppSettings, T),
) -> Stateful<Div> {
    row(label).children(values.iter().enumerate().map(|(i, &(value, title))| {
        state_button(entity, i, title, p, move |s, cx| {
            s.update_settings(|s| set(s, value), cx)
        })
        .when(value == selected, |b| {
            b.border_color(p.accent).bg(p.surface_active)
        })
    }))
}

fn swatches(
    entity: &Entity<AppState>,
    label: &'static str,
    selected: &str,
    p: BiuPalette,
    set: fn(&mut AppSettings, &'static str),
) -> Stateful<Div> {
    row(label).children(COLORS.into_iter().filter_map(|hex| {
        let color = parse_hex_hsla(hex)?;
        Some(
            state_button(
                entity,
                hex,
                if selected.eq_ignore_ascii_case(hex) {
                    "✓"
                } else {
                    ""
                },
                BiuPalette {
                    surface: color,
                    surface_hover: color,
                    surface_active: color,
                    ..p
                },
                move |s, cx| s.update_settings(|s| set(s, hex), cx),
            )
            .size(px(32.))
            .p_0()
            .flex()
            .items_center()
            .justify_center()
            .text_color(rgb(0x101215)),
        )
    }))
}

fn toggle(
    entity: &Entity<AppState>,
    label: &'static str,
    checked: bool,
    set: fn(&mut AppSettings, bool),
) -> Stateful<Div> {
    let entity = entity.clone();
    row(label).child(
        Switch::new("switch")
            .checked(checked)
            .on_click(move |value, _, cx| {
                entity.update(cx, |s, cx| s.update_settings(|s| set(s, *value), cx));
            }),
    )
}

fn font_size(
    entity: &Entity<AppState>,
    label: &'static str,
    current: u32,
    p: BiuPalette,
    access: fn(&mut AppSettings) -> &mut u32,
) -> Stateful<Div> {
    row(label)
        .child(state_button(entity, "smaller", "−", p, move |s, cx| {
            s.update_settings(
                |s| {
                    let value = access(s);
                    *value = value.saturating_sub(1).clamp(10, 96);
                },
                cx,
            );
        }))
        .child(format!("{current} px"))
        .child(state_button(entity, "larger", "+", p, move |s, cx| {
            s.update_settings(
                |s| {
                    let value = access(s);
                    *value = value.saturating_add(1).clamp(10, 96);
                },
                cx,
            );
        }))
}

#[cfg(test)]
mod tests {
    #[test]
    fn all_color_presets_parse() {
        for hex in super::COLORS {
            assert!(super::parse_hex_hsla(hex).is_some(), "{hex}");
        }
    }
}
