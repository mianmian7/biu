use super::*;
use crate::desktop_lyrics::open_or_toggle_desktop_lyrics;
use crate::theme::parse_hex_hsla;
use crate::widgets::section_header;
use biu_core::{AppSettings, AudioQuality, ProxySettings};
use gpui_kit::component::switch::Switch;
use std::path::{Path, PathBuf};

const COLORS: [&str; 6] = [
    "#1ed760", "#fb7299", "#3b82f6", "#a855f7", "#f59e0b", "#ef4444",
];

const UNPLAYED_COLORS: [&str; 6] = [
    "#ffffff", "#e5e7eb", "#cbd5e1", "#fde68a", "#bae6fd", "#fecdd3",
];

const MENU_TOGGLE_ITEMS: [(&str, &str); 8] = [
    ("music-recommend", "推荐音乐"),
    ("search", "搜索发现"),
    ("follow-list", "我的关注"),
    ("dynamic-feed", "关注动态"),
    ("later", "稍后再看"),
    ("history", "历史记录"),
    ("local-music", "本地音乐"),
    ("download-list", "下载记录"),
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

    fn pick_ffmpeg_file(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let runtime = self.state.read(cx).services.rt.handle().clone();
        let epoch = self.epoch;
        let window_handle = window.window_handle();
        cx.spawn(async move |this, cx| {
            let path = runtime
                .spawn_blocking(|| {
                    rfd::FileDialog::new()
                        .set_title("选择 FFmpeg 可执行文件")
                        .pick_file()
                })
                .await
                .ok()
                .flatten()
                .map(|p| p.to_string_lossy().into_owned());
            if let Some(path) = path {
                let _ = this.update(cx, |this, cx| {
                    if this.epoch != epoch {
                        return;
                    }
                    if let Some(inputs) = &this.data.settings.inputs {
                        let input = inputs.ffmpeg.clone();
                        let val = path.clone();
                        let _ = window_handle.update(cx, |_, window, cx| {
                            input.update(cx, |i, cx| i.set_value(val, window, cx));
                        });
                    }
                    this.state.update(cx, |s, cx| {
                        s.update_settings(|st| st.ffmpeg_path = path, cx)
                    });
                    this.detect_ffmpeg(cx);
                });
            }
        })
        .detach();
    }

    fn export_settings(&self, cx: &mut Context<Self>) {
        let runtime = self.state.read(cx).services.rt.handle().clone();
        let state = self.state.clone();
        let Ok(payload) = serde_json::to_string_pretty(&self.state.read(cx).settings) else {
            return;
        };
        cx.spawn(async move |_, cx| {
            let res = runtime
                .spawn_blocking(move || -> anyhow::Result<Option<PathBuf>> {
                    let Some(path) = rfd::FileDialog::new()
                        .set_title("导出 Biu 配置")
                        .set_file_name("biu-settings.json")
                        .add_filter("JSON", &["json"])
                        .save_file()
                    else {
                        return Ok(None);
                    };
                    std::fs::write(&path, payload)?;
                    Ok(Some(path))
                })
                .await;
            state.update(cx, |s, cx| match res {
                Ok(Ok(Some(path))) => {
                    s.show_toast(format!("已导出配置至 {}", path.display()), false, cx)
                }
                Ok(Ok(None)) => {}
                Ok(Err(e)) => s.show_toast(format!("导出失败: {e}"), true, cx),
                Err(e) => s.show_toast(format!("导出异常: {e}"), true, cx),
            });
        })
        .detach();
    }

    fn import_settings(&mut self, cx: &mut Context<Self>) {
        let runtime = self.state.read(cx).services.rt.handle().clone();
        let state = self.state.clone();
        cx.spawn(async move |_, cx| {
            let res = runtime
                .spawn_blocking(|| -> anyhow::Result<Option<AppSettings>> {
                    let Some(path) = rfd::FileDialog::new()
                        .set_title("导入 Biu 配置")
                        .add_filter("JSON", &["json"])
                        .pick_file()
                    else {
                        return Ok(None);
                    };
                    let raw = std::fs::read_to_string(&path)?;
                    let parsed: AppSettings = serde_json::from_str(&raw)?;
                    Ok(Some(parsed))
                })
                .await;
            state.update(cx, |s, cx| match res {
                Ok(Ok(Some(imported))) => {
                    s.update_settings(|st| *st = imported, cx);
                    s.show_toast("配置已导入并生效", false, cx);
                }
                Ok(Ok(None)) => {}
                Ok(Err(e)) => s.show_toast(format!("导入失败: {e}"), true, cx),
                Err(e) => s.show_toast(format!("导入异常: {e}"), true, cx),
            });
        })
        .detach();
    }

    pub(super) fn settings_view(&self, p: BiuPalette, cx: &Context<Self>) -> Div {
        let state = self.state.read(cx);
        let s = &state.settings;
        let entity = &self.state;

        let appearance = group("外观", p)
            .child(options(
                entity,
                "主题模式",
                "选择浅色、深色或跟随系统",
                s.theme_mode.as_str(),
                &[("system", "跟随系统"), ("dark", "深色"), ("light", "浅色")],
                p,
                |s, value| s.theme_mode = value.into(),
            ))
            .child(swatches(
                entity,
                "主题主色",
                "更改应用强调色与高亮色",
                &s.primary_color,
                p,
                |s, value| s.primary_color = value.into(),
            ))
            .child(options(
                entity,
                "圆角风格",
                "控制封面与卡片的圆角大小",
                s.border_radius,
                &[(6, "紧凑"), (10, "标准"), (14, "柔和")],
                p,
                |s, value| s.border_radius = value,
            ))
            .child(options(
                entity,
                "默认视图",
                "选择推荐、歌单与搜索的默认显示样式",
                if s.display_mode == "grid" {
                    "card"
                } else {
                    s.display_mode.as_str()
                },
                &[("card", "卡片网格"), ("list", "精致列表")],
                p,
                |s, value| s.display_mode = value.into(),
            ))
            .child(toggle(
                entity,
                "显示搜索历史",
                "在顶部搜索下拉框与搜索页展示最近搜索记录",
                s.show_search_history,
                p,
                |s, value| s.show_search_history = value,
            ));

        let menu_visibility = group("侧边栏菜单显隐", p).child(
            toolbar().children(MENU_TOGGLE_ITEMS.into_iter().enumerate().map(
                |(i, (key, label))| {
                    let visible = !s.hidden_menu_keys.iter().any(|h| h == key);
                    let st = entity.clone();
                    div()
                        .id(("menu-vis", i))
                        .h(px(30.0))
                        .px_3()
                        .rounded_full()
                        .flex()
                        .items_center()
                        .gap_1p5()
                        .text_xs()
                        .font_weight(if visible {
                            FontWeight::SEMIBOLD
                        } else {
                            FontWeight::NORMAL
                        })
                        .bg(if visible {
                            p.accent_soft
                        } else {
                            p.surface_elevated
                        })
                        .text_color(if visible { p.accent } else { p.text_dim })
                        .cursor_pointer()
                        .hover(move |s| s.opacity(0.88))
                        .child(icon(
                            if visible {
                                IconName::Eye
                            } else {
                                IconName::EyeOff
                            },
                            12.0,
                            if visible { p.accent } else { p.text_dim },
                        ))
                        .child(label)
                        .on_click(move |_, _, cx| {
                            st.update(cx, |s, cx| {
                                s.update_settings(
                                    |st| {
                                        if let Some(pos) =
                                            st.hidden_menu_keys.iter().position(|h| h == key)
                                        {
                                            st.hidden_menu_keys.remove(pos);
                                        } else {
                                            st.hidden_menu_keys.push(key.to_string());
                                        }
                                    },
                                    cx,
                                );
                            });
                        })
                },
            )),
        );

        let cache = &self.data.settings.cache;
        let playback = group("播放与音频", p)
            .child(options(
                entity,
                "默认音质",
                "优先尝试播放所选音质",
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
                "关闭窗口时",
                "控制点击窗口关闭按钮的行为",
                if s.close_window_option == "quit" {
                    "exit"
                } else {
                    s.close_window_option.as_str()
                },
                &[("hide", "隐藏到托盘"), ("exit", "直接退出")],
                p,
                |s, value| s.close_window_option = value.into(),
            ))
            .child(toggle(
                entity,
                "上报播放历史",
                "播放 B 站视频时同步历史记录",
                s.report_play_history,
                p,
                |s, value| s.report_play_history = value,
            ))
            .child(match &cache.result {
                None => setting_row("音频缓存", "本地磁盘流媒体缓存占用", p).child(
                    div().text_xs().text_color(p.text_dim).child("正在读取…"),
                ),
                Some(Err(error)) => setting_row("音频缓存", error.clone(), p).child(
                    button("retry-cache", "重试", p)
                        .on_click(cx.listener(|this, _, _, cx| this.load_cache(false, cx))),
                ),
                Some(Ok(bytes)) => setting_row(
                    "音频缓存",
                    format!("当前已占用 {:.1} MiB", *bytes as f64 / 1_048_576.),
                    p,
                )
                .child(
                    button("clear-cache", "清理缓存", p)
                        .on_click(cx.listener(|this, _, _, cx| this.load_cache(true, cx))),
                ),
            });

        let style = &s.desktop_lyrics_style;
        let lyrics_open = state.windows.desktop_lyrics.is_some();
        let st_dl_toggle = entity.clone();
        let lyrics = group("歌词与桌面歌词", p)
            .child(
                setting_row("桌面歌词窗口", "独立置顶的透明悬浮歌词窗口", p).child(
                    Switch::new("settings-desktop-lyrics-open")
                        .checked(lyrics_open)
                        .on_click(move |_, _, cx| {
                            let st = st_dl_toggle.clone();
                            cx.defer(move |cx| open_or_toggle_desktop_lyrics(st, cx));
                        }),
                ),
            )
            .child(toggle(
                entity,
                "顶部栏歌词",
                "在主窗口顶部栏居中显示当前歌词",
                s.show_top_lyrics,
                p,
                |s, v| s.show_top_lyrics = v,
            ))
            .child(toggle(
                entity,
                "顶部歌词翻译",
                "同时显示外语歌词的中文翻译",
                s.show_top_lyrics_translation,
                p,
                |s, v| s.show_top_lyrics_translation = v,
            ))
            .child(font_size(
                entity,
                "全屏歌词字号",
                "沉浸全屏播放界面的歌词大小",
                s.lyrics_font_size,
                p,
                |s| &mut s.lyrics_font_size,
            ))
            .child(toggle(
                entity,
                "锁定桌面歌词",
                "开启后桌面歌词鼠标穿透，可在托盘菜单解锁",
                s.desktop_lyrics_locked,
                p,
                |s, v| s.desktop_lyrics_locked = v,
            ))
            .child(options(
                entity,
                "桌面歌词行数",
                "切换单行或双行滚动显示",
                style.layout.as_str(),
                &[("double", "双行"), ("single", "单行")],
                p,
                |s, value| s.desktop_lyrics_style.layout = value.into(),
            ))
            .child(options(
                entity,
                "桌面歌词对齐",
                "控制桌面歌词的水平对齐方式",
                style.align.as_str(),
                &[("left", "左对齐"), ("center", "居中"), ("right", "右对齐")],
                p,
                |s, value| s.desktop_lyrics_style.align = value.into(),
            ))
            .child(toggle(
                entity,
                "显示翻译",
                "在桌面歌词中展示译文",
                style.show_translation,
                p,
                |s, v| s.desktop_lyrics_style.show_translation = v,
            ))
            .child(toggle(
                entity,
                "显示罗马音",
                "在桌面歌词中展示注音",
                style.show_romaji,
                p,
                |s, v| s.desktop_lyrics_style.show_romaji = v,
            ))
            .child(toggle(
                entity,
                "卡拉 OK 逐字高亮",
                "按播放进度平滑推进歌词染色",
                style.karaoke,
                p,
                |s, v| s.desktop_lyrics_style.karaoke = v,
            ))
            .child(font_size(
                entity,
                "桌面歌词字号",
                "浮动歌词窗口的字体大小",
                s.desktop_lyrics_font_size,
                p,
                |s| &mut s.desktop_lyrics_font_size,
            ))
            .child(swatches(
                entity,
                "已唱歌词颜色",
                "桌面歌词卡拉 OK 高亮色",
                &style.played_color,
                p,
                |s, value| s.desktop_lyrics_style.played_color = value.into(),
            ))
            .child(swatches_from(
                entity,
                "未唱歌词颜色",
                "桌面歌词默认文本颜色",
                &style.unplayed_color,
                &UNPLAYED_COLORS,
                p,
                |s, value| s.desktop_lyrics_style.unplayed_color = value.into(),
            ));

        let mut local = group("下载与本地", p).child(
            setting_row(
                "下载目录",
                if s.download_path.is_empty() {
                    "未设置目录".to_string()
                } else {
                    format!("{}（更改后重启生效）", s.download_path)
                },
                p,
            )
            .child(
                icon_pill_button(
                    "pick-download",
                    IconName::FolderOpen,
                    if self.data.settings.choosing_directory {
                        "正在选择…"
                    } else {
                        "选择文件夹"
                    },
                    p,
                )
                .on_click(cx.listener(|this, _, _, cx| this.pick_download_dir(cx))),
            ),
        );

        let mut proxy = group("网络代理", p);
        if let Some(inputs) = &self.data.settings.inputs {
            let ffmpeg = inputs.ffmpeg.clone();
            let ffmpeg_desc = match &self.data.settings.ffmpeg.result {
                None => "正在检测 FFmpeg…".into(),
                Some(Ok(Some(path))) => format!("已检测到 {}", path.display()),
                Some(Ok(None)) => "未检测到 FFmpeg（仅影响转码与声纹对齐）".into(),
                Some(Err(error)) => format!("检测失败：{error}"),
            };
            local = local.child(
                setting_row("FFmpeg 路径", ffmpeg_desc, p).child(
                    div()
                        .flex()
                        .items_center()
                        .gap_2()
                        .child(
                            div()
                                .w(px(220.0))
                                .h(px(34.0))
                                .px_3()
                                .rounded(px(8.0))
                                .bg(p.surface_elevated)
                                .flex()
                                .items_center()
                                .text_sm()
                                .child(Input::new(&ffmpeg).appearance(false)),
                        )
                        .child(
                            icon_pill_button("pick-ffmpeg", IconName::FolderOpen, "选择", p)
                                .on_click(cx.listener(|this, _, window, cx| {
                                    this.pick_ffmpeg_file(window, cx)
                                })),
                        )
                        .child(button("save-ffmpeg", "应用", p).on_click(cx.listener(
                            move |this, _, _, cx| {
                                let path = normalize_query(&ffmpeg.read(cx).value());
                                this.state.update(cx, |s, cx| {
                                    s.update_settings(|s| s.ffmpeg_path = path, cx)
                                });
                                this.detect_ffmpeg(cx);
                            },
                        ))),
                ),
            );
            proxy = proxy
                .child(
                    setting_row("代理类型", "为 B 站 API 与媒体流配置网络代理", p).child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .p(px(3.0))
                            .rounded_full()
                            .bg(p.surface_elevated)
                            .children(
                                [
                                    ("none", "不使用"),
                                    ("http", "HTTP"),
                                    ("socks4", "SOCKS4"),
                                    ("socks5", "SOCKS5"),
                                ]
                                .into_iter()
                                .map(|(id, title)| {
                                    let active = self.data.settings.proxy_type == id;
                                    div()
                                        .id(id)
                                        .h(px(26.0))
                                        .px_3()
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
                                        .child(title)
                                        .on_click(cx.listener(move |this, _, _, cx| {
                                            this.data.settings.proxy_type = id.into();
                                            cx.notify();
                                        }))
                                }),
                            ),
                    ),
                )
                .when(self.data.settings.proxy_type != "none", |group| {
                    group.children(
                        [
                            ("主机地址", &inputs.host),
                            ("端口", &inputs.port),
                            ("用户名", &inputs.username),
                            ("密码", &inputs.password),
                        ]
                        .into_iter()
                        .map(|(label, input)| {
                            setting_row(label, "", p).child(
                                div()
                                    .w(px(260.0))
                                    .h(px(34.0))
                                    .px_3()
                                    .rounded(px(8.0))
                                    .bg(p.surface_elevated)
                                    .flex()
                                    .items_center()
                                    .text_sm()
                                    .child(Input::new(input).appearance(false)),
                            )
                        }),
                    )
                })
                .child(
                    div().pt_1().flex().justify_end().child(
                        primary_button("apply-proxy", IconName::Check, "应用代理设置", p)
                            .on_click(cx.listener(|this, _, _, cx| this.apply_proxy(cx))),
                    ),
                );
        }

        let backup = group("配置备份与恢复", p).child(
            setting_row(
                "导出 / 导入配置",
                "以 JSON 文件备份或恢复外观、歌词、代理与目录配置",
                p,
            )
            .child(
                toolbar()
                    .child(
                        icon_pill_button("export-cfg", IconName::Download, "导出配置", p)
                            .on_click(cx.listener(|this, _, _, cx| this.export_settings(cx))),
                    )
                    .child(
                        icon_pill_button("import-cfg", IconName::Upload, "导入配置", p)
                            .on_click(cx.listener(|this, _, _, cx| this.import_settings(cx))),
                    ),
            ),
        );

        let account = if let Some(user) = &state.user {
            group("账号", p).child(
                div()
                    .flex()
                    .items_center()
                    .gap_3()
                    .child(thumbnail(
                        entity,
                        &user.face,
                        ThumbSize::Avatar,
                        (48., 48.),
                        BiuPalette {
                            radius: px(24.0),
                            ..p
                        },
                        cx,
                    ))
                    .child(
                        column()
                            .flex_1()
                            .gap(px(2.0))
                            .child(div().font_weight(FontWeight::SEMIBOLD).child(user.name.clone()))
                            .child(
                                div()
                                    .text_xs()
                                    .text_color(p.text_muted)
                                    .child(format!("UID {}", user.mid)),
                            ),
                    )
                    .when(!self.mutating, |bar| {
                        bar.child(
                            button("logout", "退出登录", p)
                                .text_color(p.danger)
                                .on_click(cx.listener(|this, _, _, cx| {
                                    this.mutate(
                                        |s| async move { s.bili.logout().await },
                                        "已退出登录",
                                        |this, cx| {
                                            this.state.update(cx, |s, cx| {
                                                s.user = None;
                                                s.created_folders.clear();
                                                s.collected_folders.clear();
                                                s.refresh_user_and_folders(cx);
                                                s.notify_ui(cx);
                                            });
                                        },
                                        cx,
                                    );
                                })),
                        )
                    }),
            )
        } else {
            group("账号", p).child(
                setting_row("未登录", "使用哔哩哔哩 App 扫码登录以同步收藏夹", p).child(
                    primary_button("login", IconName::User, "扫码登录", p).on_click({
                        let st = entity.clone();
                        move |_, _, cx| st.update(cx, |s, cx| s.open_modal(ActiveModal::Login, cx))
                    }),
                ),
            )
        };

        page("设置")
            .max_w(px(860.0))
            .child(appearance)
            .child(menu_visibility)
            .child(playback)
            .child(lyrics)
            .child(local)
            .child(proxy)
            .child(backup)
            .child(account)
    }
}

fn group(title: &'static str, p: BiuPalette) -> Div {
    div()
        .w_full()
        .rounded(px(14.0))
        .bg(p.surface)
        .p_5()
        .flex()
        .flex_col()
        .gap_3()
        .child(section_header(title, p).mb_1())
}

fn setting_row(
    label: &'static str,
    desc: impl Into<SharedString>,
    p: BiuPalette,
) -> Stateful<Div> {
    let desc: SharedString = desc.into();
    let has_desc = !desc.is_empty();
    div()
        .id(label)
        .w_full()
        .py_1p5()
        .flex()
        .flex_wrap()
        .items_center()
        .justify_between()
        .gap_3()
        .child(
            div()
                .flex()
                .flex_col()
                .gap(px(2.0))
                .child(div().text_sm().font_weight(FontWeight::MEDIUM).child(label))
                .when(has_desc, |d| {
                    d.child(div().text_xs().text_color(p.text_dim).child(desc))
                }),
        )
}

fn options<T: Copy + PartialEq<S> + 'static, S>(
    entity: &Entity<AppState>,
    label: &'static str,
    desc: &'static str,
    selected: S,
    values: &[(T, &'static str)],
    p: BiuPalette,
    set: fn(&mut AppSettings, T),
) -> Stateful<Div> {
    setting_row(label, desc, p).child(
        div()
            .flex()
            .items_center()
            .gap_1()
            .p(px(3.0))
            .rounded_full()
            .bg(p.surface_elevated)
            .children(values.iter().enumerate().map(|(i, &(value, title))| {
                let active = value == selected;
                let st = entity.clone();
                div()
                    .id((label, i))
                    .h(px(26.0))
                    .px_3()
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
                    .child(title)
                    .on_click(move |_, _, cx| {
                        st.update(cx, |s, cx| s.update_settings(|s| set(s, value), cx))
                    })
            })),
    )
}

fn swatches(
    entity: &Entity<AppState>,
    label: &'static str,
    desc: &'static str,
    selected: &str,
    p: BiuPalette,
    set: fn(&mut AppSettings, &'static str),
) -> Stateful<Div> {
    swatches_from(entity, label, desc, selected, &COLORS, p, set)
}

fn swatches_from(
    entity: &Entity<AppState>,
    label: &'static str,
    desc: &'static str,
    selected: &str,
    palette_colors: &'static [&'static str],
    p: BiuPalette,
    set: fn(&mut AppSettings, &'static str),
) -> Stateful<Div> {
    setting_row(label, desc, p).child(
        div()
            .flex()
            .items_center()
            .gap_2()
            .children(palette_colors.iter().enumerate().filter_map(|(idx, &hex)| {
                let color = parse_hex_hsla(hex)?;
                let active = selected.eq_ignore_ascii_case(hex);
                let st = entity.clone();
                Some(
                    div()
                        .id((label, idx))
                        .size(px(26.0))
                        .rounded_full()
                        .bg(color)
                        .cursor_pointer()
                        .flex()
                        .items_center()
                        .justify_center()
                        .when(active, |d| {
                            d.border_2()
                                .border_color(p.text)
                                .child(icon(IconName::Check, 13.0, rgb(0x0b0c0e).into()))
                        })
                        .on_click(move |_, _, cx| {
                            st.update(cx, |s, cx| s.update_settings(|s| set(s, hex), cx))
                        }),
                )
            })),
    )
}

fn toggle(
    entity: &Entity<AppState>,
    label: &'static str,
    desc: &'static str,
    checked: bool,
    p: BiuPalette,
    set: fn(&mut AppSettings, bool),
) -> Stateful<Div> {
    let entity = entity.clone();
    setting_row(label, desc, p).child(
        Switch::new(label)
            .checked(checked)
            .on_click(move |value, _, cx| {
                entity.update(cx, |s, cx| s.update_settings(|s| set(s, *value), cx));
            }),
    )
}

fn font_size(
    entity: &Entity<AppState>,
    label: &'static str,
    desc: &'static str,
    current: u32,
    p: BiuPalette,
    access: fn(&mut AppSettings) -> &mut u32,
) -> Stateful<Div> {
    setting_row(label, desc, p).child(
        div()
            .flex()
            .items_center()
            .gap_2()
            .child(state_button(entity, "smaller", "−", p, move |s, cx| {
                s.update_settings(
                    |s| {
                        let value = access(s);
                        *value = value.saturating_sub(1).clamp(10, 96);
                    },
                    cx,
                );
            }))
            .child(
                div()
                    .min_w(px(44.0))
                    .text_center()
                    .text_sm()
                    .font_weight(FontWeight::MEDIUM)
                    .child(format!("{current} px")),
            )
            .child(state_button(entity, "larger", "+", p, move |s, cx| {
                s.update_settings(
                    |s| {
                        let value = access(s);
                        *value = value.saturating_add(1).clamp(10, 96);
                    },
                    cx,
                );
            })),
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn all_color_presets_parse() {
        for hex in super::COLORS.iter().chain(super::UNPLAYED_COLORS.iter()) {
            assert!(super::parse_hex_hsla(hex).is_some(), "{hex}");
        }
    }
}
