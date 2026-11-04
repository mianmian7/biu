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

- [ ] `src-tauri/` 工程，复用 spike 的 HTTP 代理、依赖和窗口配置。
- [ ] `src/platform/` 实现 `window.electron`，未做的方法返回明确错误。
- [ ] 主窗口、存储、cookie、`bili_request`、扫码登录、播放。
- [ ] CI 出三个包。
- 用户验收：装上，登录，搜歌，播放，拖进度。

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

## 分工

| 步骤 | 执行 | 审查 |
|---|---|---|
| 第 1 步 | Gemini 4 Argon，osa-a1 | Opus |
| 第 2、3 步 | Gemini 4 Argon，两个线程可并行 | Opus |
| 第 4 步 | Gemini 4 Argon | Opus |

GPT-6.1-Sol 这周 7d 余量只有 7% 和 36%，暂不用。
