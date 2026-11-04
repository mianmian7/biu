# biu 重写为 Tauri v2 + Rust 后端

owner = Claude（osa-a1）

状态：已批准（2026-11-02 23:54 SGT）。

基线：commit `ff1e6a6`（1.16.1-autolyrics.5）。

## 目标

1. 每个平台的下载体积 ≤ 10 MB（Linux AppImage 除外，见「待用户拍板」D1）。
2. 功能和 1.16.1-autolyrics.5 对齐：播放、登录、下载、本地音乐、歌词（网易云/lrclib/自动歌词/罗马音）、桌面歌词（含卡拉 OK 扫光）、迷你播放器、托盘、全局快捷键、Windows 任务栏按钮、自动更新。
3. 保留现有 React 前端（`src/`、`shared/`）。前端只改平台适配层，页面和组件不重写。
4. 现有 Electron 用户只需手动安装一次，设置保留，尽量免重新登录。

## 非目标

- 不做 iOS/Android。
- 不买 Apple 证书，不做公证；Windows 不做代码签名。
- 不改 UI 设计，不加新功能。
- `feat/seamless-update` 分支和 `docs/plans/seamless-update.md` 方案作废，分支保持原样不删不改。
- `fix/desktop-lyrics-sync`（卡拉 OK）继续在 Electron 代码上进行。它改的是前端（`src/pages/desktop-lyrics` 等）时，合入 master 后自动随前端带进 Tauri 版；改的是 `electron/desktop-lyrics.ts` 时，由 M4 按新行为移植。

## 已作废的决定

| 原决定 | 新处理 |
|---|---|
| Mac 用自签名证书 + Squirrel.Mac 无感更新 | Tauri updater（minisign/Ed25519）。Mac 只做 ad-hoc 签名（`bundle.macOS.signingIdentity = "-"`），updater 直接替换 `.app`，不需要证书 |
| electron-updater 通道过渡（`allowPrerelease=false` 等） | 只在桥接版本 M8 里用到一次；之后由 Tauri updater 的 `latest.json` 取代 |

---

## 1. 目标架构

### 1.1 进程与数据流

```
┌──────────────────────── webview（React，复用 src/ + shared/）────────────────────────┐
│ pages / components / store（zustand）                                                 │
│ src/platform/  ← 新：实现现有 ElectronAPI 类型（invoke / listen），替换 window.electron │
│ src/service/request ← axios 换 adapter：invoke("bili_request")                         │
│ workers/romaji.worker.ts   kuromoji（词典按需下载，经 biu-res:// 读取）               │
│ workers/auto-lyrics.worker.ts  shared/auto-lyrics 对齐（DTW），PCM 由 Rust 提供        │
│ <audio src="biu-media://…">  ← 所有远程/本地音频                                       │
└──────────────┬────────────────────────────── IPC（commands + events）─────────────────┘
               │
┌──────────────┴──────────────────────── Rust：src-tauri（crate biu）───────────────────┐
│ bili::client  reqwest + 持久化 cookie jar + 统一 UA/Referer/Origin + 代理              │
│ media::protocol  biu-media://  Range/206 流代理（远程 CDN + 本地文件）                 │
│ download  队列 / 分块续传 / 合并（见 D2）                                              │
│ lyrics  netease / lrclib                                                               │
│ autolyrics::decode  symphonia 解码 + 重采样 → 22050 Hz mono PCM                        │
│ resources  按需下载（kuromoji 词典、可选 ffmpeg），sha256 校验，多镜像                 │
│ store  tauri-plugin-store，沿用现有 store 名                                           │
│ migrate  导入 Electron 数据                                                            │
│ windows  main / mini / lyrics / tray / thumbar(win) / shortcuts                        │
│ updater  tauri-plugin-updater + manual 模式                                            │
└───────────────────────────────────────────────────────────────────────────────────────┘
```

设计原则：

- **网络全部经过 Rust**。系统 webview 没有 Electron 的 `webRequest`，无法统一改请求头和 Set-Cookie。B 站 API 走 `bili_request`；音频走 `biu-media://`。webview 只直接加载图片（hdslb）和极验脚本（见 S6）。
- **算法留在 TS**。`shared/auto-lyrics`（对齐、识别、LRC）和 kuromoji 已经是 TS/JS，放到 Web Worker 里跑，不改写成 Rust。Rust 只负责 Node 才能做的事：网络、解码、文件、窗口、系统集成。
- **前端适配层保持现有接口**。`src/platform/` 导出与现在 `ElectronAPI` 相同的类型，页面代码不感知换了框架。`window.electron` 在过渡期由适配层赋值。

### 1.2 Rust 模块划分（`src-tauri/src/`）

| 模块 | 职责 | 主要依赖 | 取代 |
|---|---|---|---|
| `lib.rs` | Builder、插件注册、单实例、启动顺序 | `tauri`、`tauri-plugin-single-instance` | `electron/main.ts` |
| `bili/client.rs` | 全局 `reqwest::Client`：默认 UA/Referer/Origin；`reqwest_cookie_store` jar，落盘到 app data（权限 0600）；代理设置变化时重建 client | `reqwest`（rustls）、`reqwest_cookie_store` | `network/interceptor.ts`、`network/cookie.ts`、`network/user-agent.ts`、`ipc/app.ts` 的代理 |
| `bili/request.rs` | `bili_request` command：method/url/params/body/headers → status/headers/body；URL 白名单（`*.bilibili.com`、`*.hdslb.com`、`*.biliapi.net` 等） | — | axios 在 Electron 下的直接请求 |
| `bili/wbi.rs`、`bili/ticket.rs` | WBI 签名、`bili_ticket` | `md5`、`hmac`、`sha2` | `ipc/api/wbi.ts`、`network/web-bili-ticket.ts` |
| `bili/api.rs` | 下载和自动歌词要用的 playurl、audio-stream-url、web-interface/view | `serde` | `ipc/api/*.ts` |
| `media/protocol.rs` | `biu-media://remote?u=<url>`：转发 Range，带 Referer/UA/cookie，返回 206 + `Content-Range` + `Accept-Ranges` + 正确 `Content-Type`，流式输出；`biu-media://local?p=<path>`：本地文件同样支持 Range；远程host仅放行playurl返回的具体host（不得使用`*.akamaized.net`等通配后缀；M0夹具及写死刷新不进入M2） | `tauri` 异步 URI scheme、`http-range` | `<audio>` 直连 CDN、本地音乐的 `file://` |
| `download/` | `queue.rs`（tokio 信号量，并发数沿用设置）、`core.rs`（分块 Range 下载、暂停/续传/重试，URL 过期时重新取）、`persist.rs`（任务存 `media-downloads` store）、`mux.rs`（见 D2）、`events.rs`（`download:sync`） | `tokio`、`futures` | `ipc/download/*` |
| `lyrics/` | netease 搜索/歌词、lrclib 搜索 | — | `ipc/api/netease-lyric.ts`、`lrclib-lyric.ts`、`ipc/lyrics.ts` |
| `autolyrics/` | `sources.rs`（网易云候选、参考音频下载）、`decode.rs`（symphonia：isomp4 + aac + flac → 混成单声道 → 重采样到 22050 Hz → `tauri::ipc::Response` 返回原始 f32 字节） | `symphonia`、`rubato` | `auto-lyrics/decode.ts`、`ffmpeg-resolve.ts`、`sources.ts`、`worker-*.ts` |
| `resources.rs` | 按需资源清单（名称、版本、大小、sha256、镜像 URL 列表）；下载到 app data；进度事件；`biu-res://` 只读协议给 worker 读词典 | `sha2` | 打包进 asar 的 `kuromoji/dict`、`electron/ffmpeg/*` |
| `store.rs` | 薄封装 `tauri-plugin-store`。store 名沿用 `@shared/store` 的 `StoreNameMap` 加 `desktop-lyrics`。`user-login-info` 加密存储（见 §3） | `tauri-plugin-store` | `electron/store.ts`、`ipc/store.ts` |
| `migrate.rs` | 首次启动导入 Electron 数据（见 §3） | `aes`、`cbc`、`pbkdf2` | — |
| `local_music.rs` | 扫描目录、读标签、删除文件 | `walkdir`、`lofty` | `ipc/local-music.ts`（music-metadata） |
| `fonts.rs` | 列系统字体族名 | `font-kit` 或 `fontdb`（S4 选更小的） | `ipc/font.ts`（font-list） |
| `windows/main.rs` | 主窗口（无边框、最小化/最大化/全屏）、关闭行为 | — | `main.ts`、`ipc/window.ts` |
| `windows/mini.rs` | 迷你播放器窗口（透明、置顶） | — | `mini-player.ts` |
| `windows/lyrics.rs` | 桌面歌词窗口：透明、置顶（mac 用 floating 级别）、`set_ignore_cursor_events`、悬停方案（见 D3）、拖动、高度跟随、位置持久化与重置 | `tauri`（`macos-private-api` feature） | `desktop-lyrics.ts`、`ipc/desktop-lyrics.ts` |
| `windows/tray.rs` | 托盘菜单与播放状态 | `tauri` tray | `windows/tray.ts` |
| `windows/thumbar.rs` | Windows 任务栏缩略图按钮（上一首/播放/下一首） | `windows` crate（`ITaskbarList3`） | `windows/thumbar.ts` |
| `shortcuts.rs` | 全局快捷键注册/注销，触发 `shortcut:triggered` | `tauri-plugin-global-shortcut` | `shortcut.ts`、`ipc/shortcut.ts` |
| `updater.rs` | 检查、下载、安装；判断是否支持自动安装（mac 位置可写且未 translocate、Win NSIS、Linux AppImage），否则走 manual 模式（打开 release 页） | `tauri-plugin-updater`、`tauri-plugin-process` | `updater/index.ts` |
| `media_controls.rs`（仅在 S8 不通过时启用） | 系统媒体控制（SMTC / MPNowPlaying / MPRIS） | `souvlaki` | Chromium 内置的 mediaSession 集成 |

其他插件：`tauri-plugin-dialog`、`tauri-plugin-opener`、`tauri-plugin-log`、`tauri-plugin-window-state`（只给主窗口用）。

### 1.3 前端改动范围

| 位置 | 改动 |
|---|---|
| `src/platform/`（新） | 按 `ElectronAPI` 类型实现全部方法（`invoke` / `listen`），启动时赋给 `window.electron` |
| `src/service/request/index.ts` | 6 个 axios 实例改用自定义 adapter → `invoke("bili_request")`；拦截器（WBI、极验）保持不动 |
| 播放相关（`src/store/play-list.ts`、`src/common/utils/audio.ts`、`src/pages/local-music`） | 音频地址经 `toMediaUrl()` 包装；按 `canPlayType` 决定是否选 FLAC/Dolby（见 S1） |
| `src/workers/`（新） | romaji worker（kuromoji）、auto-lyrics worker（从 `electron/auto-lyrics/service.ts` 移来编排逻辑，依赖 `shared/`） |
| `index.html` 及各窗口入口 | `<meta name="referrer" content="no-referrer">`（见 S6） |
| `rsbuild.config.ts`、`plugins/` | 去掉 electron 构建插件，产物交给 `tauri build` |
| 设置页 | ffmpeg 路径设置改为 D2 对应的形式；新增「资源下载」状态（罗马音词典） |

### 1.4 IPC 映射表（54 个 handler）

`cmd:` = Tauri command（Rust）；`plugin:` = 官方插件的 JS API；`前端` = 不再需要后端。

| # | 现有 channel | 类型 | 新实现 |
|---|---|---|---|
| 1 | `store:get` | handle | cmd `store_get`（`store.rs`） |
| 2 | `store:set` | handle | cmd `store_set` |
| 3 | `store:clear` | handle | cmd `store_clear` |
| 4 | `dialog:select-directory` | handle | plugin `dialog.open({ directory: true })` |
| 5 | `dialog:select-file` | handle | plugin `dialog.open()` |
| 6 | `dialog:open-directory` | handle | plugin `opener.openPath` |
| 7 | `dialog:show-file-in-folder` | handle | plugin `opener.revealItemInDir` |
| 8 | `dialog:open-external` | handle | plugin `opener.openUrl`（只允许 http/https） |
| 9 | `font:get-fonts` | handle | cmd `font_list`（`fonts.rs`） |
| 10 | `download:get-list` | handle | cmd `download_list` |
| 11 | `download:add` | handle | cmd `download_add` |
| 12 | `download:add-list` | handle | cmd `download_add_many` |
| 13 | `download:pause` | handle | cmd `download_pause` |
| 14 | `download:resume` | handle | cmd `download_resume` |
| 15 | `download:cancel` | handle | cmd `download_cancel` |
| 16 | `download:retry` | handle | cmd `download_retry` |
| 17 | `download:clear` | handle | cmd `download_clear` |
| 18 | `auto-lyrics:start` | handle | 前端 worker 编排；Rust 提供 cmd `autolyrics_fetch_audio`、`autolyrics_decode`、`netease_*`（见 21–22） |
| 19 | `auto-lyrics:prefetch` | handle | 同上（前端 worker） |
| 20 | `auto-lyrics:cancel` | handle | 前端 worker（`AbortController`）；Rust 端用 `tauri::ipc::Channel` 关闭时取消请求 |
| 21 | `lyrics:netease-search` | handle | cmd `netease_search` |
| 22 | `lyrics:netease-lyrics` | handle | cmd `netease_lyric` |
| 23 | `lyrics:lrclib-search` | handle | cmd `lrclib_search` |
| 24 | `lyrics:romanize` | handle | 前端 romaji worker（kuromoji）；词典经 cmd `resource_ensure("kuromoji-dict")` 准备 |
| 25 | `shortcut:register` | handle | cmd `shortcut_register`（封装 global-shortcut，统一快捷键字符串格式） |
| 26 | `shortcut:unregister` | handle | cmd `shortcut_unregister` |
| 27 | `shortcut:register-all` | handle | cmd `shortcut_register_all` |
| 28 | `shortcut:unregister-all` | handle | cmd `shortcut_unregister_all` |
| 29 | `app:get-version` | handle | plugin `app.getVersion` |
| 30 | `app:check-update` | handle | cmd `update_check`（`updater.rs`，返回是否可自动安装） |
| 31 | `app:download-update` | handle | cmd `update_download`（进度经 `app:update-message` 事件） |
| 32 | `app:quit-and-install` | handle | cmd `update_install`（安装后 `process.relaunch`） |
| 33 | `app:is-dev` | handle | 前端：`import.meta.env.DEV` |
| 34 | `app:set-proxy-settings` | handle | cmd `proxy_set`（重建 reqwest client；webview 代理见 S6） |
| 35 | `cookie:get` | handle | cmd `cookie_get`（读 jar） |
| 36 | `cookie:set` | handle | cmd `cookie_set`（写 jar 并落盘） |
| 37 | `local-music:scan` | handle | cmd `local_music_scan` |
| 38 | `local-music:delete-file` | handle | cmd `local_music_delete`（只允许删除扫描目录下的文件） |
| 39 | `window:toggle-mini` | handle | cmd `window_toggle_mini` |
| 40 | `window:is-maximized` | handle | 前端 `getCurrentWindow().isMaximized()` |
| 41 | `window:is-full-screen` | handle | 前端 `getCurrentWindow().isFullscreen()` |
| 42 | `window:minimize` | on | 前端 `getCurrentWindow().minimize()` |
| 43 | `window:toggle-maximize` | on | 前端 `toggleMaximize()` |
| 44 | `window:close` | on | cmd `window_close`（按设置隐藏到托盘或退出） |
| 45 | `window:toggle-dev-tools` | on | cmd `devtools_toggle`（只在 debug 构建或设置打开时可用） |
| 46 | `desktop-lyrics:get-content` | handle | cmd `lyrics_window_get_content`（Rust 缓存最近一次 sync 内容） |
| 47 | `desktop-lyrics:sync` | on | cmd `lyrics_window_sync`（主窗口 → Rust → `desktop-lyrics:content` 事件发给歌词窗口） |
| 48 | `desktop-lyrics:command` | on | cmd `lyrics_window_command`（转发给主窗口） |
| 49 | `desktop-lyrics:drag-start` | on | 前端 `getCurrentWindow().startDragging()`；Rust 端不再需要 |
| 50 | `desktop-lyrics:drag-end` | on | cmd `lyrics_window_save_bounds`（`Moved` 事件防抖保存也可以覆盖） |
| 51 | `desktop-lyrics:set-ignore-mouse` | on | cmd `lyrics_window_set_ignore`（结合 D3） |
| 52 | `desktop-lyrics:set-content-height` | on | cmd `lyrics_window_set_height` |
| 53 | `desktop-lyrics:reset-position` | on | cmd `lyrics_window_reset` |
| 54 | `player:state` | on | cmd `player_state`（更新托盘、thumbar、可选的媒体控制） |

Rust → 前端事件（沿用 channel 名作为事件名）：`auto-lyrics:progress`（改为 worker 内部消息）、`download:sync`、`desktop-lyrics:content`、`desktop-lyrics:toggle-requested`、`desktop-lyrics:lock-toggle-requested`、`player:prev/next/toggle`、`shortcut:triggered`、`router:navigate`、`app:update-message`、`app:on-update-available`、`window:maximize/unmaximize/enter-full-screen/leave-full-screen`（改用前端窗口事件）。`app:on-before-quit` 改用 `onCloseRequested`。`channel.ts` 里没有 handler 的 `settings:*`、`http:*`、`app:open-installer-directory` 在 M1 确认无调用后删除。

新增 command（Electron 下由 webRequest 隐式完成）：`bili_request`、`resource_ensure`、`resource_status`、`autolyrics_fetch_audio`、`autolyrics_decode`、`migrate_status`。

---

## 2. 早期 spike（M0，所有里程碑之前）

每项 spike 产出一页结果说明（写在本计划对应清单项下），并保留最小复现工程到 `spikes/<name>/`（不进入正式代码）。任何一项不通过且退路也不行时，停下报告，由用户决定是否继续。

| # | 内容 | 机器 | 通过标准 | 不通过时的退路 |
|---|---|---|---|---|
| S1 | Mac WKWebView 经 `biu-media://` 播放 B 站 fMP4 FLAC（`codecs=flac`/`fLaC`）和 Dolby（`ec-3`）各 3 首 | mac（arm64；有条件再测 x64） | `canPlayType` 返回 `probably`/`maybe`；连续播放 ≥ 60 s 无中断；拖动 5 次都能继续播放；`AudioContext` 波形有非零数据 | FLAC 不行：Mac 上 lossless 自动降级为最高码率 AAC，并在音质设置里标注。ec-3 不行：所有平台按 `canPlayType` 跳过 Dolby（与 Electron 现状一致）。两项都不影响继续 |
| S2 | 三个系统上经 `biu-media://` 播放 5 分钟 AAC，拖动进度 | mac、pc（WebView2）、Linux（wsl 的 WebKitGTK + 一台 x64 Linux 实机/VM） | 随机拖动 10 次，每次 ≤ 1.5 s 恢复播放；日志显示每次是 Range 请求，没有整文件重拉；播放 30 分钟内存不持续增长；URL 过期后能经刷新地址继续 | 退路 A：改用 `127.0.0.1` 随机端口的本地 HTTP 代理（每次 app 启动生成一个随机 token，本次进程内的请求共用；退出即失效），接口不变。A 也不行：停下报告 |
| S3 | Linux 上 gst-libav 依赖 | Ubuntu 24.04、Fedora 41+、Arch 的干净 VM（x64；arm64 用 osa-a1 做构建冒烟） | 没装 gst-libav 时，app 能在 3 s 内识别出「无法解码」（`error` 事件或无 `timeupdate`），并提示安装命令；`apt install ./biu.deb` 会自动拉上 `gstreamer1.0-libav`；rpm 声明的依赖在 Fedora 上可解析；装好后 AAC、FLAC 都能播放 | Linux 增加原生播放后端（symphonia + `cpal`，在 Rust 里播放，前端播放器接口加一个实现）。工作量约 +1 个里程碑，需要用户重新批准 |
| S4 | 二进制体积 | CI（三平台） | 带上计划内全部 crate 和插件 + 现在的 `dist/web`，release profile（`opt-level="s"`、`lto=true`、`codegen-units=1`、`strip=true`、`panic="abort"`）：mac arm64 dmg ≤ 8 MB，win x64 NSIS ≤ 7 MB，deb ≤ 8 MB | 按 `cargo bloat` 排行替换大 crate（例如 `fontdb` 换 `font-kit`、去掉 `rubato` 改用简单重采样、`lofty` 只开需要的格式）。仍超 10 MB：停下报告实际数字，由用户决定 |
| S5 | 桌面歌词窗口 | mac、pc、Linux X11、Linux Wayland（GNOME、KDE） | 透明背景无黑边；置顶在普通窗口之上（mac 包括全屏 app 之上，与现状一致）；锁定后点击穿透；D3 方案下悬停 1 s 内出现解锁按钮；卡拉 OK 扫光 60 fps，歌词窗口 CPU < 10%（M 系列 Mac / 中端 Windows） | Wayland 上置顶或穿透不行：与 Electron 现状一样标为已知限制。mac/win 上不行：停下报告 |
| S6 | webview 直接加载的部分 | mac、pc、Linux | `no-referrer` 下 hdslb 封面/头像能加载；二维码登录完整走通；极验验证码能弹出并通过；设置了代理时 webview 里的图片也走代理（`WebviewWindowBuilder::proxy_url`） | 图片不行：图片也走 `biu-media://`（加缓存头）。极验不行：极验页面改由单独的 webview 窗口加载 `bilibili.com` 源。webview 代理不行：只代理 Rust 流量，在设置里写明 |
| S7 | 更新端到端 | mac、pc、Linux（若保留 AppImage） | 用测试仓库发 `0.0.1` → `0.0.2`：后台下载，退出时安装，下次启动是新版本，设置和登录保留。Mac 分别测 `/Applications` 和 `~/Applications`；从 dmg 直接运行（translocated）时进入 manual 模式 | Mac 自动安装不行：Mac 走 manual 模式（下载后提示替换），其他平台照常 |
| S8 | 系统媒体控制（媒体键、mac 正在播放、Windows SMTC、Linux MPRIS） | mac、pc、Linux | 现有 `navigator.mediaSession` 代码在三个 webview 里都能显示标题封面，媒体键可控制 | 用 `souvlaki` 在 Rust 侧实现，由 `player_state` 驱动 |

S1–S3、S5、S8 共用一个最小 Tauri 工程（一个窗口 + `biu-media://` + 一个播放页），先做这个工程，再分发到三台机器。

---

## 3. 现有 Electron 用户迁移

### 3.1 数据位置

| 数据 | Electron（`appData/Biu`，`userData` 下） | Tauri（`appDataDir` = `appData/com.biu.wood3n`） |
|---|---|---|
| 设置 | `app-settings.json`、`shortcut-settings.json`、`desktop-lyrics.json` | 同名 store |
| 下载记录 | `media-downloads.json` | 同名 store；`tempDir`、`chunks` 等未完成状态丢弃，只保留已完成项 |
| 歌词缓存 | `lyrics-cache.json` | 同名 store |
| 登录信息 | `user-login-info.json`（electron-store 加密：aes-256-cbc，key = pbkdf2(encryptionKey, iv, 10000, 32, sha512)，encryptionKey = store 名） | 同名 store |
| B 站 cookie | Chromium `Cookies` SQLite（值经系统钥匙串/DPAPI/libsecret 加密） | `cookies.json`（reqwest cookie jar，0600） |

M1 时核对实际文件名与路径（`StoreNameMap`、`getUserDataPath()`、dev 下的 `biu-dev`），写入本表。

### 3.2 路径

1. **最后一个 Electron 版本（桥接版，`1.16.1-autolyrics.6`）**：旧客户端只认带 `autolyrics` 后缀的版本，所以桥接版必须用这个版本号，通过现有 electron-updater 通道送达（Win/Linux AppImage 自动，Mac 和其他照旧手动）。它做三件事：
   - 启动时把 `.bilibili.com` 的 cookie（`session.cookies.get`）和解密后的登录信息写到 `appData/Biu/migration/export.json`（0600），带 `exportedAt` 和格式版本号。
   - 关闭 electron-updater 的后续检查，避免对 Tauri release 报「找不到 latest*.yml」。
   - 显示一次引导弹窗：说明新版改用 Tauri、体积变化、需要手动安装一次；按钮「下载新版」打开对应平台的 release 资产链接（按 OS/arch 选好），「稍后」可以在设置页再次打开。
2. **Tauri 首次启动**（`migrate.rs`）：
   - 发现 Electron 的 userData 目录就导入：设置类 JSON 原样读入；`user-login-info.json` 用上面的算法解密；有 `export.json` 时导入 cookie，导入后删除该文件。
   - 没有 `export.json`（用户跳过了桥接版）：设置照样导入，cookie 不导入，进入二维码登录。不尝试解密 Chromium 的 cookie 数据库（三平台加密方式各不同，收益不值得）。
   - 导入完成写 `migration.done` 标记，以后不再执行；设置页提供「重新导入」。
   - 任何一步失败只记日志并跳过，不阻塞启动。
3. **旧安装的清理**：Windows 上 Tauri NSIS 安装到新目录。Tauri 首次启动若检测到旧 Electron 版的卸载项（`HKCU\…\Uninstall\<Electron appId GUID>`），提示「卸载旧版本」并调用其卸载程序（旧 NSIS 配置了 `deleteAppDataOnUninstall: true`，所以**必须在导入完成后**才提示）。Mac：提示用户把旧的 `Biu.app` 拖到废纸篓（新旧 bundle id 相同时直接覆盖，见 D5）。Linux deb/rpm：新包 `Conflicts/Replaces` 旧包名。
4. **版本号**：Tauri 首版用 `2.0.0`（见 D5），release 里附 `latest.json`，不再附 `latest*.yml`。

### 3.3 验收

- 单元测试：`migrate.rs` 用三类夹具（完整 export、只有 JSON、损坏文件）覆盖导入结果；解密测试用一份由现有 electron-store 生成的 `user-login-info.json` 夹具。
- 手动：每个平台装 `1.16.1-autolyrics.5` 并登录 → 升级到桥接版 → 按引导装 Tauri 版 → 打开后仍是登录状态，设置、快捷键、桌面歌词位置、歌词缓存一致；Windows 卸载旧版后 Tauri 版数据仍在。

---

## 4. 体积预算

压缩后大小，单位 MB。Rust 部分是 S4 前的估算，S4 后用实测替换。CI 里每个产物 > 10 MB 时构建失败（AppImage 除外）。

| 项 | mac arm64 dmg | mac x64 dmg | win x64 NSIS | win arm64 NSIS | linux deb/rpm（x64/arm64） |
|---|---|---|---|---|---|
| Tauri 核心 + wry + tao | 2.0 | 2.2 | 1.8 | 1.8 | 2.0 |
| reqwest（rustls）+ cookie store + tokio | 1.2 | 1.3 | 1.2 | 1.2 | 1.2 |
| symphonia（isomp4/aac/flac）+ 重采样 | 0.6 | 0.6 | 0.6 | 0.6 | 0.6 |
| lofty + walkdir + 字体枚举 | 0.5 | 0.5 | 0.4 | 0.4 | 0.5 |
| 官方插件（updater/store/dialog/opener/shortcut/single-instance/log/window-state/process） | 0.8 | 0.8 | 0.8 | 0.8 | 0.8 |
| biu 自身 Rust 代码 | 0.5 | 0.5 | 0.5 | 0.5 | 0.5 |
| 前端产物 `dist/web`（2.5 MB，内嵌时压缩） | 0.7 | 0.7 | 0.7 | 0.7 | 0.7 |
| 图标与资源 | 0.3 | 0.3 | 0.1 | 0.1 | 0.2 |
| 安装包格式开销（dmg 镜像 / NSIS 脚本 / deb 控制文件） | 0.4 | 0.4 | 0.3 | 0.3 | 0.1 |
| D2 若选 Rust 重封装（`mp4` 写入） | +0.3 | +0.3 | +0.3 | +0.3 | +0.3 |
| **合计（预算）** | **≈ 7.3** | **≈ 7.6** | **≈ 6.7** | **≈ 6.7** | **≈ 6.9** |
| 硬上限 | 10 | 10 | 10 | 10 | 10 |

不计入安装包、按需下载的部分：

| 资源 | 大小 | 何时下载 |
|---|---|---|
| kuromoji 词典 | 约 14.4 MB（已是 gz） | 第一次遇到没有 `romalrc` 的日文歌词，或用户在设置里点下载 |
| ffmpeg（只有 D2 选按需下载时） | 每平台/架构 30–80 MB | 第一次下载「视频」时 |
| WebView2 Runtime | 系统自带（Win10 21H2+/Win11）；缺失时 NSIS 用 `downloadBootstrapper` 在线安装 | 安装时 |
| WebKitGTK、GStreamer | 由 deb/rpm 依赖拉取 | 安装时 |

其他产物：Linux AppImage 80–100 MB（D1）；Windows 便携版（裸 exe 打 zip，≈ 5 MB，要求系统有 WebView2，不能自动更新）；Mac 只发分架构 dmg，`.app.tar.gz` 只给 updater 用。

---

## 5. 里程碑

每个里程碑结束时：CI 三平台构建通过；本里程碑的单元/集成测试通过；手动验收清单逐条打勾（写明机器和结果）；代码已由非作者模型审查。M0 的结果需要用户确认后才进入 M1。

### M0 — Spike（见 §2）
- [x] 最小 Tauri 工程：一个窗口、`biu-media://`、播放页、歌词窗口原型
  - 结果：**通过（osa-a1，Ubuntu 24.04.5 LTS / aarch64，4 核，Rust 1.99.1，WebKitGTK 2.52.6）**。复现工程在 `spikes/m0/`；正式代码未改。官方插件全部注册，计划内 Linux crate 已纳入，依赖探针实际执行 MP4 读取、标签、字体枚举、FLAC 解码、重采样、加密/摘要等入口；cookie jar 接到 reqwest client，避免只声明依赖而被完全裁掉。
  - 验证：`cargo test --release` **6 passed / 0 failed**（Range/416、字节一致性、Host/Origin限制、测试开关/精确夹具范围、畸形URI返回400且服务继续响应）；Node 语法检查、Python 编译检查、`cargo fmt --check` 通过。现有 `dist/web` 快照内嵌为体积负载，文件合计 **2,481,561 字节（2.481561 MB）**，并未声称已完成正式 React 适配。
  - 证据与边界：`spikes/evidence/` 的环境/前端哈希清单、测试输出；`spikes/README.md` 是复现入口。原始大日志、音频、产物和私钥均忽略提交。音频由 GStreamer 生成原创 440 Hz 正弦波，没有提交或下载版权歌曲。用户另行安排 Claude 审查；本执行线程取消原审查，不声明独立审查通过。全平台 M0 尚未验收，不进入 M1。
- [ ] S1 Mac FLAC / ec-3
  - 结果：**待在 mac（MateBook，arm64）上运行**，未编造样本或测量。Linux `canPlayType` 在 libav 存在时对 AAC、fMP4 FLAC（flac/fLaC）及 ec-3 返回 `probably`，原生 FLAC 返回 `maybe`；这些仅是 Linux 能力声明，不能替代 Mac 的三首/格式实播。
  - 复现：`spikes/m0/` 测试页提供六个用户自备 fMP4 样本入口；每首自动记录连续 60 s、5 次 seek 和波形。文件必须来自用户有权使用的音频，留在忽略的 `fixtures/`，不加入传输包或 Git。
  - 退路：Mac FLAC 不通过则最高码率 AAC 并标注音质降级；ec-3 按 `canPlayType` 跳过。README 的 Mac S1 节给出直接操作步骤及回传字段；Mac x64 仍待有条件补测。
- [ ] S2 三平台拖动进度
  - 结果：**Linux 自定义协议不通过；HTTP退路的播放/seek检查通过；60min内存重测满足本次预设条件（不代表长期无泄漏）。mac / pc / wsl 的 WebKitGTK / Linux x64 实机或 VM 待运行**。WebKitGTK 日志明确为 `biu-media (allowed: no)`；系统 GStreamer 可独立解码同一 AAC，故没有把协议失败误记为 codec 缺失。
  - 退路实测：Rust 随机 loopback 端口 + 每次启动随机临时 token，初始响应 200、实际 Range 响应 206、流式转发；测试上游只允许 `127.0.0.1:18765`。AAC 长度 **300.023333 s**；seed=2401 的 10 次随机 seek，本轮恢复 **33–868 ms**，均 ≤1500 ms；非缓存的 6 次 Range 传输均为 206，没有把每次 seek 改成整文件重拉。缓存命中不产生网络请求，已单独说明。
  - 内存：原1800 s缓存文件循环的结论已撤回并改记**待定**，不再用末段GC回落判通过。按事先提交的方法重测，实际soak **3600.008 s**，**120轮**均使用新URL重新加载并核对代理及上游Range，合计**246次206传输**（含初始seek检查）。排除前**600 s**后，全部样本OLS斜率 **−0.254942 MB/min**，最后30 min **−0.335103 MB/min**，均满足预设≤0条件，故**仅本轮重测通过**。RSS峰值 **690,782,208字节（690.782208 MB）**，末次 **660,664,320字节（660.664320 MB）**；不能据此排除长期泄漏。**359点原始RSS序列**已提交`spikes/evidence/soak-60min-rss.jsonl`；逐轮请求/字节证据在`soak-60min-rounds.json`，方法、斜率和判定在`soak-60min-summary.json`，通过`analyze_soak.py`可复算。
  - URL 过期：夹具注入403，Rust刷新测试地址并继续同一Range。**真实 B 站 CDN / 登录态 / 地址续签没有实测**；HTTP 退路目前只放行夹具上游，生产白名单和续签逻辑不在此原型里。原`*.akamaized.net`等后缀白名单已从spike代码移除；夹具放行及写死403刷新仅在显式`BIU_SPIKE_FIXTURES=1`时启用。**这些规则不得带入M2；M2只能放行playurl返回的具体host，不能沿用Akamai公共后缀白名单**。HTTP退路ACAO只允许app源，Host必须等于实际绑定的`127.0.0.1:<port>`；每次启动一个随机token，进程内共用，并非每请求一次性。Mac/Windows、wsl WebKitGTK、真实CDN、Linux x64实机/VM和物理音频输出均待验证。
  - Mac 崩溃根因（2026-11-04，Claude 在 GitHub macos-26 runner 复现）：`panic = "abort"` 让 wry 无法接住 seek 取消任务后 `didFinish` 抛出的 NSException，进程 SIGABRT。改为 unwind 后 Mac 自定义协议 600 s soak、10 次 seek 全部完成。M0 已删掉该配置；M1 起禁止 macOS 使用 `panic = "abort"`。证据见 `spikes/evidence/ci-mac-s2-panic.md`。3600 s soak 与内存斜率仍待测。
- [ ] S3 Linux gst-libav
  - 结果：**osa-a1 arm64 构建/解码冒烟通过；Ubuntu/Fedora/Arch 干净 x64 VM 矩阵待运行**。独立插件目录链接系统插件但排除 `libgstlibav.so`，同时设置 `GST_PLUGIN_SYSTEM_PATH_1_0`、空 `GST_PLUGIN_PATH_1_0` 和独立 `GST_REGISTRY`；未卸载系统包。仅设置 PLUGIN_PATH 无法隔离系统插件，复现说明已避免此做法。
  - 实测：缺 libav 时 AAC `canPlayType` 为空，**147 ms** 内收到 `MEDIA_ERR_SRC_NOT_SUPPORTED`（code 4），没有 `timeupdate`，页面显示 `sudo apt install gstreamer1.0-libav gstreamer1.0-plugins-good`。正常插件环境中 AAC/FLAC 可播放，AAC 波形最大幅值约 **0.800177**。
  - 打包：已读取 deb 实际 control，其 Depends 包含 `gstreamer1.0-libav` 和 good；`apt-get -s install ./...deb` 解析通过。本机 libav 已安装，**不能据此声称已实测干净 VM 自动下载依赖**。rpm 实际 requires 包含 `gstreamer1-plugin-libav`、`gstreamer1-plugins-good`、WebKitGTK；Fedora 是否需要 RPM Fusion、依赖能否解析仍待 VM，Arch 同待。
  - 退路：若干净 VM 验证确实不通过且不能由依赖解决，按计划另请用户批准 symphonia + cpal 原生播放里程碑；本次未擅自增加原生后端。
- [ ] S4 二进制体积
  - 结果：**Linux aarch64 deb/rpm 通过；Mac 和 Windows 待 CI 或对应机器**。本次完整计划依赖/插件（加 Linux S8 souvlaki 退路）及当前 dist/web 的实测，release profile 为 `opt-level="s"`、`lto=true`、`codegen-units=1`、`strip=true`、`panic="abort"`；MB 统一为1,000,000字节。原型仍不是完整产品代码，后续功能会增加体积。
  - 交付体积：**deb 5,084,262 字节（5.084262 MB）**；**rpm 5,089,701 字节（5.089701 MB）**；**strip 后二进制 10,659,672 字节（10.659672 MB）**；**含媒体插件 AppImage 145,115,656 字节（145.115656 MB）**。deb低于8 MB目标，deb/rpm低于10 MB下载包硬上限；裸二进制不适用压缩下载包门槛。AppImage按D1例外，不受10 MB限制。
  - AppImage处理：默认未打媒体插件的版本解码检测失败；启用 `bundle.linux.appimage.bundleMediaFramework=true` 后成功打包并通过 AAC、FLAC、10次拖动、波形冒烟。linuxdeploy 需要 patchelf，本次只用 `apt-get download` + `dpkg-deb -x` 解包到 worktree，不以sudo安装它。辅助AppImage较预算大，D1仍优先deb/rpm。
  - `cargo bloat --release --crates -n 15`：前几名 **zbus 896.5 KiB、std 866.7 KiB、tauri 820.2 KiB、lofty 432.2 KiB、tokio 382.4 KiB、rustfft 333.6 KiB、rustls 244.5 KiB**；前15名完整输出在 `spikes/evidence/cargo-bloat.txt`。LTO归属是工具估算；bloat重建的未strip分析副本约16.4 MiB，不能当成发布二进制实测值。
  - 证据：`sizes.json`（字节/MB/SHA256）、`frontend-manifest.json`、deb/rpm元数据；以上体积已在审查修复后重测，`acceptance-review.txt`记录非内存断言。Mac dmg与Windows NSIS没有本机实测值，未用预算冒充实测。
- [ ] S5 桌面歌词窗口
  - 结果：**未验证（仅 API 调用无报错）**；完整交互/真实合成器、mac / pc / X11视觉 / Wayland GNOME/KDE仍待验证。创建透明、无边框、置顶窗口并成功调用鼠标穿透；D3每100 ms检查光标，进入左上100×60区域临时取消穿透。悬停轮询已去掉65 s截止，持续到窗口Destroyed事件；本轮日志记录轮询在3600 s仍运行，验证已超过原65 s截止；这仅修复原型生命周期，不代表完整D3交互已通过。
  - 实测：xvfb下60 s扫光记录 **3903帧 / 60004 ms ≈65.046 fps**。该数字是requestAnimationFrame吞吐，**不能证明真实显示器稳定60 fps**。无物理显示器、无真实合成器，libEGL访问/dev/dri被拒，未提升权限，软件渲染继续测试。
  - 缺口：当前解锁按钮为常显原型，完整锁定/悬停显示状态机未实现；不能宣称“1 s内出现解锁按钮”通过。透明无黑边、实体点击穿透、全屏app上方置顶、Mac/Windows CPU<10%均未验证。Wayland限制按原计划记录；Mac/Windows失败仍需报告用户。
- [ ] S6 图片 / 登录 / 极验 / webview 代理
  - 结果：**图片通过；Linux webview 代理不通过（与 S2 退路 A 冲突），采用『只代理 Rust 流量』**；二维码完整登录/极验、mac / pc仍待验证。no-referrer页面实载hdslb默认头像，**naturalWidth=180 px**；`WebviewWindowBuilder::proxy_url`探针图片加载成功，上游记录代理请求且Referer为空。
  - 发现：Linux上代理窗口会影响共享WebKit context，连主窗口loopback媒体请求也进入测试代理，从而播放失败。因此Linux采用只代理Rust流量的退路，图片等webview直连，并在设置明示；正式产品策略作为§7 D6待用户拍板。Mac代理探针已启用tauri的`macos-proxy` feature，需macOS 14+，尚未实机验证。
  - 缺口：当前独立播放器原型没有二维码/极验最小页面，内嵌正式dist不等于已适配正式登录。未发起扫码授权、未自动点击验证码、未使用真实cookie。README明确此格不能靠外部浏览器或占位页验收，需要补最小登录复现页并由用户操作。
- [ ] S7 更新端到端
  - 结果：**Linux AppImage 本地测试更新链通过；mac / pc 待运行**。`python3 -m http.server`仅监听127.0.0.1；Tauri updater后台下载并验签，本地测试AppImage **0.0.1→0.0.2** 安装后退出，再次启动确为0.0.2；测试设置和虚拟登录夹具均保留（两个preserved字段为true）。本轮使用补齐媒体插件后的AppImage重跑成功。
  - 安全：只有spike目录的临时签名钥匙，private目录和所有key忽略Git，命令输出重定向到private日志；未打印钥匙，未建远端仓库/release，未改secrets。Linux更新入口检查APPIMAGE落在private，XDG store/cache也放private；未调用系统apt/rpm安装器。
  - 边界：模拟“退出并安装”命令已经跑通，**正式关闭窗口/退出生命周期尚未接入**；登录保留指虚拟store，不代表B站cookie迁移完成。Mac /Applications、~/Applications、dmg/translocation及Windows NSIS需要对应机器实测，README给出本地签名/HTTP测试步骤；任何GUI授权或权限弹窗由执行者停下报告，不代用户确认。
- [ ] S8 系统媒体控制
  - 结果：**Linux mediaSession未注册MPRIS；计划souvlaki退路的D-Bus自动化检查通过。mac / pc / 实体媒体键待运行**。原生`navigator.mediaSession`存在，但30 min会话D-Bus记录没有MPRIS服务，不能据此声称媒体控制可用。
  - 退路实测：souvlaki注册`org.mpris.MediaPlayer2.biu_m0`；D-Bus GetAll读到标题`Biu M0 native fixture`、艺术家、300 s时长和封面URL；发送Pause/Play，经Rust事件到前端，HTMLAudio分别记录`paused=true/false`。证据在`spikes/evidence/review-controls-dbus.txt`及摘要。
  - 缺口：真实桌面封面展示、物理媒体键、Mac正在播放和Windows SMTC未验证。当前原型元数据/状态是固定夹具，不是完整player_state同步；Mac/Windows souvlaki接线尚未实现，不把Linux退路结果当成跨平台通过。
- 产出：spike 结果汇总，D1–D4 按结果确认或调整。

### M1 — 工程骨架与 CI
- [ ] `src-tauri/` crate、`tauri.conf.json`（identifier `com.biu.wood3n`、三个窗口的配置、CSP、`biu-media`/`biu-res` scheme）
- [ ] rsbuild 产物接入 `tauri build`；移除 Electron 构建插件（`electron/` 目录保留到 M9，供对照）
- [ ] `src/platform/` 骨架：全部 `ElectronAPI` 方法先返回明确的「未实现」错误
- [ ] 主窗口：无边框、窗口按钮、单实例、关闭到托盘的开关（托盘本身在 M4）
- [ ] `store.rs` + 1–3 号 command；日志
- [ ] CI：mac x64/arm64、win x64/arm64、linux x64/arm64 构建；体积报告 job（每个产物大小 + `cargo bloat --crates` 前 20），超 10 MB 失败
- 测试：`cargo test`（store 读写、未知 store 名报错）；`pnpm test` 保持全绿。手动：三平台启动、窗口按钮、二次启动聚焦已有窗口。

### M2 — 网络、cookie、登录
- [ ] `bili/client.rs`、`bili/request.rs`（白名单）、cookie jar 落盘
- [ ] axios adapter；WBI / bili_ticket；代理设置（34）；cookie 命令（35–36）
  - M0审查安全约束：M2仅放行playurl返回的具体CDN host；不得复制spike原`*.akamaized.net`后缀白名单、夹具放行或写死403刷新逻辑。spike测试路径须保持env/cfg显式开关，正式请求不能带测试分支。Linux webview代理策略见§7 D6。
- [ ] 二维码登录、短信/密码登录、极验（按 S6 结论）、退出登录
- 测试：Rust 单元——WBI 签名用现有 TS 实现生成的测试向量对比；Set-Cookie 写入 jar；白名单拒绝非 B 站域名；代理切换后新请求走代理（wiremock / 本地 HTTP 代理）。前端单元——adapter 把 axios 配置正确转成 `bili_request` 参数并还原响应。手动：登录 → 首页推荐、搜索、收藏夹、动态、个人主页各页面能正常显示，重启后仍登录。

### M3 — 播放
- [ ] `media/protocol.rs`（远程 + 本地）、`toMediaUrl()`
- [ ] 音质选择按 `canPlayType` 判断 FLAC/Dolby（S1 结论）
- [ ] 本地音乐播放改用 `biu-media://local`
- [ ] URL 过期刷新；波形组件；mediaSession（或 S8 的退路）
- 测试：Rust 单元——Range 解析（`bytes=0-`、`bytes=100-199`、越界 416）、206 头、白名单、本地路径限制在扫描目录和下载目录内；集成——本地 HTTP 服务器提供测试 fMP4，协议返回的字节与源一致。手动：三平台各播 AAC/FLAC/本地文件各 1 首，拖动、切歌、暂停恢复、播放 1 小时。

### M4 — 窗口与系统集成
- [ ] 托盘（菜单、播放状态）、迷你播放器（39）
- [ ] 全局快捷键（25–28）、Windows thumbar（54 的一部分）
- [ ] 桌面歌词窗口（46–53），悬停按 D3；位置持久化与重置
- [ ] 窗口相关前端调用（40–45）
- 测试：Rust 单元——快捷键字符串转换（覆盖现有 `shortcut-settings` 默认值）；歌词窗口锁定/悬停状态机。手动：S5 的矩阵全部复测；快捷键在 app 失焦时生效；托盘和 thumbar 按钮控制播放。

### M5 — 歌词、罗马音、自动歌词
- [ ] `lyrics/`（21–23）
- [ ] `resources.rs` + `biu-res://`；资源清单与镜像（D4）
- [ ] romaji worker（24）：无词典时先用网易云 `romalrc`，再按需下载词典
- [ ] `autolyrics/`：音频获取、symphonia 解码；auto-lyrics worker（18–20），进度改为 worker 消息
- 测试：Rust 单元——解码同一测试文件，PCM 与现有 ffmpeg 命令（`-f s16le -ac 1 -ar 22050`）输出比较，样本数差 < 0.1%、相关系数 > 0.99；资源下载 sha256 不符时拒绝、镜像回退。前端——现有 `pnpm bench:auto-lyrics` 改为在新解码下运行，各曲 `verifyScore` 与基线差 ≤ 0.02，匹配结果不变。手动：日文歌首次触发词典下载（断网时提示且不卡住）、自动歌词出结果。

### M6 — 下载与本地音乐
- [ ] `download/`（10–17）：分块续传、暂停/恢复/重试/取消、URL 过期重取、重启后恢复列表
- [ ] 音视频合并按 D2
- [ ] 本地音乐扫描/删除（37–38）、字体列表（9）、对话框和打开文件（4–8）
- 测试：Rust 单元——分块规划、续传偏移、取消后清理临时文件；集成——本地 HTTP 服务器（支持 Range、可模拟断线）下载音频和视频，输出文件时长与源一致（用 symphonia 读回校验）。手动：三平台下载音频、FLAC、视频各 1 个，中途暂停/退出 app 后继续，用系统播放器打开结果。

### M7 — 更新与打包
- [ ] `updater.rs`（29–32）、manual 模式判断
- [ ] release workflow：生成 `latest.json`、`.sig`；`TAURI_SIGNING_PRIVATE_KEY` 由用户在 GitHub secret 里创建（执行者不碰 secret，只写 workflow 引用名）
- [ ] 打包：Mac ad-hoc 签名 dmg；Win NSIS（`webviewInstallMode = downloadBootstrapper`）+ 便携 zip；Linux 按 D1，deb `Depends` 含 `libwebkit2gtk-4.1-0`、`gstreamer1.0-libav`、`gstreamer1.0-plugins-good`
- 测试：S7 的流程在正式工程上复测（测试仓库或 draft release）；CI 体积门槛通过。手动：每个平台从上一个测试版本自动更新到下一个。

### M8 — 迁移与桥接版
- [ ] `migrate.rs`（§3.2 第 2 步）、旧版卸载提示（第 3 步）
- [ ] 桥接版 `1.16.1-autolyrics.6`：在 Electron 代码（基于 master）上加 export、关 updater、引导弹窗
- 测试：§3.3。

### M9 — Beta 与切换
- [ ] 发 `2.0.0-beta.N`（不标 Latest），全量回归清单（Gemini Flash 根据本计划和现有功能整理，Opus 审定）
- [ ] 修完回归问题后 Opus 最终验收
- [ ] 用户确认后：先发桥接版并标 Latest，再发 `2.0.0` 并标 Latest；删除 `electron/` 和 Electron 依赖
- 测试：回归清单三平台全过；体积报告附在 release 说明里。

---

## 6. 分工与审查

派活前在 osa-a1 跑 `agent-quota`，按余量调整。所有执行线程走 T3 `delegate_task`；Mac/Windows 上的 spike 和手动验收派到对应机器（`target.environmentId`）。

| 工作 | 作者 | 审查 | 理由 |
|---|---|---|---|
| M0 最小工程 + S2/S4 | Sol | Opus | 跨 Rust/前端，要求准确 |
| S1、S5、S7、S8 的 Mac 执行与记录 | Sol（mac 线程） | Opus 审结果 | 需要在 Mac 上边测边改 |
| S2、S5、S8 的 Windows 执行与记录 | Sol（pc 线程） | Opus | 同上 |
| S3 Linux 矩阵（wsl + VM） | Sol（wsl 线程） | Opus | 同上 |
| M1 骨架与 CI | Sol | Opus | 多文件、定接口 |
| M2 网络/cookie/登录 | Sol | Opus | 安全相关（cookie、白名单） |
| M3 媒体协议 | Sol | Opus | 核心难点 |
| M4 歌词窗口、thumbar（unsafe Win32） | Sol | Opus | 平台细节多 |
| M4 托盘、快捷键字符串转换 | Grok | Sol | 单文件、规则明确 |
| M5 解码、资源下载、worker 迁移 | Sol | Opus | 需要和 bench 基线对齐 |
| M5 `lyrics/`（netease、lrclib 照搬 TS 逻辑） | Grok | Sol | 一对一翻译，范围小 |
| M6 下载核心与合并 | Sol | Opus | 状态机复杂 |
| M6 本地音乐扫描、字体列表 | Grok | Sol | 单模块、输入输出明确 |
| `src/platform/` 各方法的薄封装（接口定好之后） | Grok | Sol | 机械改动 |
| M7 updater 与 release workflow | Sol | Opus | 发版风险 |
| M7 Linux/Win 打包配置 | Grok | Sol | 配置文件 |
| M8 `migrate.rs` | Sol | Opus | 解密与数据安全 |
| M8 桥接版 Electron 改动 | Sol | Opus | 只有一次机会 |
| 迁移说明、Linux 依赖说明、release notes、回归清单初稿 | Gemini Flash | Opus | 文本工作 |
| 最终验收（每个里程碑和 M9） | Opus | — | 规则要求 |

执行者只勾选自己负责的清单项，并在该项下追加简短结果说明，不改其他内容。

---

## 7. 待用户拍板

| # | 决定 | 选项 | 推荐默认值 | 理由 |
|---|---|---|---|---|
| D1 | Linux 打包格式 | A. 只发 deb/rpm；B. 只发 AppImage；C. deb/rpm + AppImage（AppImage 不受 10 MB 约束） | **C**（待用户拍板） | deb/rpm 满足 10 MB，AppImage 留给需要自动更新或非 deb/rpm 发行版的用户，代价只是 CI 多一个产物 |
| D2 | 视频下载时的音视频合并 | A. 按需下载 ffmpeg；B. Rust 重封装（`mp4` crate，stream copy）；C. 只用系统 ffmpeg | **B**（待用户拍板） | 只是把 fMP4 的音轨和视频轨拷进一个 MP4，不转码；约 +0.3 MB，免去 30–80 MB 的下载和每平台/架构的二进制维护。M6 不通过时退到 C |
| D3 | 桌面歌词锁定后的悬停 | A. Rust 每 100 ms 轮询光标位置，进入解锁按钮区域时临时取消穿透；B. 锁定后完全穿透，只能从托盘或快捷键解锁 | **A**（待用户拍板） | 保留现在的交互；轮询开销很小，且只在锁定且窗口可见时运行 |
| D4 | 按需资源托管 | A. 只用 GitHub Release；B. 只用国内镜像；C. 两者，按顺序回退 | **C**（待用户拍板） | 国内用户访问 GitHub 下载不稳定。sha256 写在随包的清单里，镜像不可信也不影响安全。国内镜像的具体选择（Gitee Release、对象存储等）需要用户提供账号 |
| D5 | Tauri 首版版本号与 bundle id | A. `2.0.0`，沿用 `com.biu.wood3n`；B. 继续 `-autolyrics.N` 后缀 | **A**（待用户拍板） | 框架换了，主版本号升级表达清楚；Tauri updater 只在 Tauri 版本之间比较，不受旧通道后缀限制。沿用 bundle id 时 Mac 新版直接覆盖旧 `.app` |
| D6 | Linux 代理策略（M0实测新增） | A. 只代理Rust流量，webview图片等直连并在设置明示；B. 验证独立context或loopback排除后再支持webview代理 | **A，待用户拍板**（M0先采用此退路） | WebKitGTK共享WebContext且ignore_hosts为空，webview代理会接管S2 loopback媒体，导致播放失败；图片探针成功不能算代理方案通过 |

D1：用户已接受推荐默认值（2026-11-02 23:54 SGT）。

D2：用户已接受推荐默认值（2026-11-02 23:54 SGT）。

D3：用户已接受推荐默认值（2026-11-02 23:54 SGT）。

D4：用户已接受推荐默认值（2026-11-02 23:54 SGT）。

D5：用户已接受推荐默认值（2026-11-02 23:54 SGT）。

## 风险

| 风险 | 影响 | 缓解 |
|---|---|---|
| WebKitGTK 各发行版版本差异 | Linux 上播放、透明窗口表现不一 | S3/S5 覆盖三个主流发行版；最低版本写进 deb/rpm 依赖 |
| B 站接口依赖浏览器行为（极验、风控） | 登录或部分接口失败 | S6 早测；风控相关请求头（`Origin`、UA、`buvid`）在 Rust 侧统一处理 |
| 卡拉 OK 分支在重写期间继续改 Electron 代码 | 移植遗漏 | M4 开始前对比 `fix/desktop-lyrics-sync` 合入 master 后的 `electron/desktop-lyrics.ts`，差异列入 M4 清单 |
| upstream（wood3n/biu）继续发 Electron 版 | 合并上游功能变难 | M1 起只把上游的前端改动挑选合入；主进程改动按需在 Rust 侧重做 |
| 签名私钥丢失 | 所有客户端无法自动更新 | 用户备份 `TAURI_SIGNING_PRIVATE_KEY`；丢失时只能手动重装一次 |
