# biu 重写为 Tauri + Rust（精简版）

owner = Claude（osa-a1）

状态：已批准（2026-11-04）。取代 `tauri-rewrite.md`，旧方案和 M0 证据只作参考。

## 目标

用 Tauri + Rust 替换 Electron，保留现有 React 界面和全部功能，让安装包降到 10 MB 左右。

## 已知结论（不再测）

- 包体积：dmg 4.7 MB、NSIS 4.2 MB、deb 6.3 MB、AppImage 154 MB。
- 音频：三个平台都走 loopback HTTP 代理，不做 `biu-media://`。
- Mac 不用 `panic=abort`。Windows 建窗 command 用 `async`。
- WebView2 代理窗口要独立 `data_directory`。

## 规则

- 一条分支 `feat/tauri`，从 `feat/auto-lyrics-p1` 开始，只推到 `origin`（mianmian7/biu）。
- 每一步结束都有一个能装的包。验收是用户装上用，不再做 soak、内存规则和三机取证。
- CI 只打 mac arm64、win x64、linux x64 三个包。其他架构放到第 4 步。
- 每步由执行者之外的模型审一次，Opus 验收。
- 测试不外放声音，自动测试一律带静音。
- 复用 `spike/tauri-m0-ci` 的代码，按需搬，不整体合并。

## 步骤

### 第 1 步：能听歌

- [x] `src-tauri/` 工程，复用 spike 的 HTTP 代理、依赖和窗口配置。
  已建立 `src-tauri/`（`com.biu.wood3n`、无边框主窗、单实例、无 `panic=abort`、异步 `ensure_main_window`）并接入 `rsbuild` 前端构建。
- [x] `src/platform/` 实现 `window.electron`，未做的方法返回明确错误。
  已在 `src/platform/index.ts` 实现完整 `ElectronAPI` 类型兼容层，第 1 步方法走 Tauri `invoke`，其余 51 个第 2 步起方法返回明确「未实现」错误或警告且不触发白屏。
- [x] 主窗口、存储、cookie、`bili_request`、扫码登录、播放。
  已实现 `store_get/set/clear`、`bili_request`（6 个 axios 实例接入自定义 adapter，保留 WBI/极验拦截器与 Rust `cookies.json` 0600 持久化）、扫码登录与 loopback HTTP 音频代理（白名单校验 + `206 Partial Content` 流式 Range 转发 + `toMediaUrl()`）。
- [x] CI 出三个包。
  已新增 `.github/workflows/tauri-build.yml`，在 `feat/tauri` push 与 `workflow_dispatch` 时零 secrets 构建 macOS arm64 dmg（ad-hoc 签名，`CI=true`）、Windows x64 NSIS、Linux x64 deb 与含 `gst-libav` 的 AppImage，上传产物并打印字节数。
- 用户验收：装上，登录，搜歌，播放，拖进度。
  2026-11-05 用户在 Mac 上验收通过：扫码登录、播放正常。首次播放弹过一次 “The operation timed out”，之后正常；日志为空，来源未定位，第 2 步补超时和日志。

### 第 2 步：日常功能

- [ ] 歌词、罗马音、桌面歌词、迷你播放器。
- [ ] 托盘、全局快捷键、媒体键。
- 用户验收：桌面歌词透明、置顶、点击穿透，媒体键可用。

### 第 3 步：下载与本地

- [ ] 下载、本地音乐、字体、对话框。
- [ ] 自动歌词。
- 用户验收：下载一首，本地播放，自动歌词能出结果。

### 第 4 步：替换 Electron

- [ ] Tauri updater 和发布流程。
- [ ] 老用户设置和登录迁移。
- [ ] 补其余架构，删除 `electron/` 和 Electron 依赖。
- 用户验收：从 Electron 版升级，设置和登录都在。

## 第 2–4 步实施决定（owner 2026-11-05 补充）

用户 2026-11-05 授权：之后的测试由 owner 自行安排，不再询问用户，推进到全部完成。以下决定在原目标内细化，按此授权直接执行。

- **请求超时：** `bili_request` 和媒体代理加 `connect_timeout` 10 s、整体超时按 axios `timeout`（默认 15 s）；GET 遇连接或超时错误重试 1 次；失败写 `warn` 日志，只记 host 和 path，不记 query 和 cookie。播放错误也写日志。
- **ffmpeg：** 沿用 `electron/ffmpeg/` 里的精简二进制，作 Tauri sidecar（`externalBin`）打包，下载转封装和自动歌词解码都用它。dmg 仍须 ≤ 10 MB。
- **罗马音词典：** kuromoji 词典 18 MB，不打进安装包。首次需要罗马音时下载到应用数据目录（npmmirror 优先，jsdelivr 备用，校验 sha256），之后离线使用。
- **自动歌词对齐：** `shared/auto-lyrics/` 的 TS 代码放进 webview 的 Web Worker 运行；解码走 ffmpeg sidecar。
- **桌面歌词、迷你播放器：** 独立 Tauri 窗口；透明、置顶、点击穿透用 Tauri 窗口 API。
- **媒体键：** 三平台用 `souvlaki`（M0 已证明 Linux 的 `mediaSession` 不可用）。
- **网易云、lrclib：** Rust 命令转发，host 白名单。
- **测试：** 执行者在 osa-a1 的 xvfb 里自动测试，一律静音（`BIU_SILENT=1` 时 `audio.muted = true`），匿名账号。Mac 实机只在第 4 步结束时由 owner 做一次静音冒烟。不使用用户的登录 cookie。
- **CI：** Mac job 改用 `macos-15`（`macos-26` arm64 两次排队超时被取消）。

## 分工

| 步骤 | 执行 | 审查 |
|---|---|---|
| 第 1 步 | Gemini 4 Argon，osa-a1 | Opus |
| 第 2、3 步 | Gemini 4 Argon，两个线程可并行 | Opus |
| 第 4 步 | Gemini 4 Argon | Opus |

GPT-6.1-Sol 这周 7d 余量只有 7% 和 36%，暂不用。
