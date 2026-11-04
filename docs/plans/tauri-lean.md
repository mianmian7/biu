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

- [x] 歌词、罗马音、桌面歌词、迷你播放器。
  已实现 `src-tauri/src/lyrics.rs`（网易云与 LRCLIB host 白名单转发、Kuromoji 词典按需从 npmmirror/jsdelivr 下载至应用数据目录并做 12 文件 SHA-256 校验，webview 经 `DecompressionStream` 解压分词生成罗马音，安装包零词典体积）与 `src-tauri/src/desktop_lyrics.rs`（异步创建透明置顶无边框 `desktop-lyrics` / `mini-player` 窗口，`set_ignore_cursor_events` 点击穿透与解锁悬停恢复、光标拖动与 `desktop-lyrics.json` 位置持久化），并补齐 `bili_request` 与媒体代理 `connect_timeout` 10s、axios `timeoutMs`、GET 1 次重试及脱敏 `warn` 日志。
- [x] 托盘、全局快捷键、媒体键。
  已实现 `src-tauri/src/tray.rs`（托盘菜单、状态同步、左键唤起主窗、关闭主窗按设置隐藏到托盘）、`src-tauri/src/shortcut.rs`（`tauri-plugin-global-shortcut` 异步注册/注销与冲突检测）与 `src-tauri/src/media_keys.rs`（`souvlaki` 三平台媒体键与系统 Now Playing 状态同步）。
- 用户验收：桌面歌词透明、置顶、点击穿透，媒体键可用。

### 第 3 步：下载与本地

- [x] 下载、本地音乐、字体、对话框。
  已在 `src-tauri/` 实现 ffmpeg sidecar（`externalBin`）与媒体下载队列（暂停/继续/重试/取消/清空、流式转封装为 m4a/mp3、任务与设置持久化）、`lofty` 本地音乐扫描与删除、`fontdb` 系统字体枚举、`tauri-plugin-dialog`/`opener` 对话框，以及基于白名单目录的 loopback 本地音频流式代理（`206 Partial Content`）。
- [x] 自动歌词。
  已将 `shared/auto-lyrics/` 对齐管线接入 Webview Web Worker（`AUTO_LYRICS_ALGO_VERSION = "p1-gap-dtw-6"`），在 Rust 实现 B 站音频拉取、ffmpeg sidecar `s16le` 22050Hz 单声道 PCM 解码、网易云与 B 站参考音源检索（host 白名单）及应用数据目录缓存持久化。
- 用户验收：下载一首，本地播放，自动歌词能出结果。

### 第 4 步：替换 Electron

- [x] Tauri updater 和发布流程。
  `tauri-plugin-updater` 接入，`release.yml` 改为 Tauri 六目标构建，产出 `latest.json` 和 electron-updater 兼容的 `latest*.yml`；osa-a1 上 AppImage 自更新实测通过（检查、下载、验签、替换），自动重启未观察到。Mac 签名与 Win NSIS 钩子待 CI 和实机验证。
- [x] 老用户设置和登录迁移。
  已在 `feat/tauri-bridge`（`1.16.1-autolyrics.7`）实现启动完成与更新安装前原子导出全部 `StoreNameMap` store（含解密后的 `user-login-info`）及 `*.bilibili.com` cookie 到 `tauri-migration.json`（权限 `0600`）。已在 `feat/tauri-s4b` 新增 `src-tauri/src/migration.rs`，首次启动且无 `migration.json` 时从 Electron `userData` 导入 `tauri-migration.json`（或回退读取明文 store JSON 并按 `conf` 的 PBKDF2-HMAC-SHA512 + AES-256-CBC 解密 `user-login-info.json`），跳过过期 cookie、不覆盖已有 Tauri 数据与 cookie，且不删改 Electron 原文件。
- [x] 补其余架构，删除 `electron/` 和 Electron 依赖。
  CI 六目标（mac arm64/x64 在 Intel runner 交叉编译）；`electron/` 与 Electron 依赖已删，ffmpeg 移到 `src-tauri/ffmpeg/`，linux arm64 回退系统 ffmpeg。
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

## 第 4 步设计（owner 2026-11-05 补充）

现状：Electron 版 `.6` 的 Mac 包用自签名证书「Biu Self-Signed Code Signing」签名，electron-updater（`allowPrerelease=false`）读 GitHub latest release 的 `latest*.yml`，Mac 走 Squirrel.Mac，只校验新包是否满足 designated requirement（`identifier "com.biu.wood3n" and certificate leaf = H"…"`）。Tauri 版的 identifier 也是 `com.biu.wood3n`。

- **版本：** 桥接版 `1.16.1-autolyrics.7`（Electron），Tauri 首发 `2.0.0`。
- **桥接版（4b）：** 分支 `feat/tauri-bridge`，从 `feat/auto-lyrics-p1` 开。启动时和安装更新前，把全部 store（`user-login-info` 解密后）和 `*.bilibili.com` cookie 写到 Electron userData 下的 `tauri-migration.json`（0600，`version: 1`）。其余行为不变。
- **Tauri 导入（4b）：** 首次启动、且应用数据里没有迁移标记时，读 Electron userData（mac `~/Library/Application Support/Biu`，win `%APPDATA%\Biu`，linux `~/.config/Biu`）里的 `tauri-migration.json`，导入 store 和 cookie。没有导出文件时，直接读明文 store JSON，并解密 `user-login-info`（electron-store aes-256-cbc），cookie 跳过、需重新登录。导入后写标记，不删除 Electron 数据。
- **发布（4a）：** `release.yml` 改为 Tauri 构建，由 `v*` tag 触发。产物：
  - 各平台安装包。
  - Tauri updater 的 `latest.json`，供 Tauri 版之间更新。
  - electron-updater 兼容文件，让 Electron 版直接升级到 Tauri 版：`latest-mac.yml` + `Biu-<ver>-mac-<arch>.zip`（Tauri `.app`，用同一自签名证书签名），`latest.yml` + NSIS 安装包，`latest-linux*.yml` + AppImage。
- **Windows 过渡（4a）：** NSIS 安装钩子发现 Electron 版（`%LOCALAPPDATA%\Programs\biu`）时，静默卸载旧程序（保留用户数据），装完启动新版本。
- **架构（4a）：** 补 mac x64、win arm64、linux arm64；`electron/ffmpeg` 移到 `src-tauri/ffmpeg/`；删除 `electron/` 和 Electron 依赖。
- **签名密钥：** Tauri updater 私钥在 osa-a1 的 `~/.config/biu/tauri-updater.key`（0600），已存为仓库 secret `TAURI_SIGNING_PRIVATE_KEY`（密码为空）。私钥丢失后 Tauri 版无法自动更新。
- **发布权限：** 执行者不推 tag、不发 release。owner 测完后发布 `.7`，再发 `2.0.0`。

## 第 4 步审查修复（owner 2026-11-05 补充）

Sol 审查 `8eaf2533` 和桥接版 `1a835bb6`，结论“暂不建议发布”。按用户 2026-11-05 的授权，以下修复直接执行。

**升级链设计（取代上节“Electron 版直接升级到 Tauri 版”）：** electron-updater 6.6.2 的 `GitHubProvider.getLatestVersion` 返回 `{ tag, ...yml }`，yml 里的 `tag` 字段决定从哪个 release 下载安装包。

- Tauri release 里的 `latest*.yml` 复制自 `.7` release，并加一行 `tag: v1.16.1-autolyrics.7`。`.6` 及更早的 Electron 版读到它，升级到桥接版。
- Tauri release 另带 `tauri*.yml`（`tauri.yml` win x64、`tauri-arm64.yml` win arm64、`tauri-mac.yml`、`tauri-linux.yml`、`tauri-linux-arm64.yml`），指向 Tauri 安装包。
- 桥接版设 `autoUpdater.channel = "tauri"`（win arm64 设 `"tauri-arm64"`，用 `app.runningUnderARM64Translation` 识别），随后把 `allowDowngrade` 和 `allowPrerelease` 设回 `false`。频道文件不存在时按“没有更新”处理，不弹错误。
- `.7` release 照常设为 latest（`.7` 读不到 `tauri*.yml` 时在检查阶段只记日志，不弹错误）。随后发 `2.0.0`，设为 latest。

**清单：**

- [x] 版本改为 `2.0.0`；`release.yml` 构建前校验 tag、`package.json`、`tauri.conf.json`、`Cargo.toml` 一致。（owner）
  `be8671ce`。`prepare-release` 先校验四处版本，生成 feed 后再查 `tauri-mac.yml`、`latest.json` 的版本和 `latest-mac.yml` 的 `tag`。
- [x] 升级链：feed 脚本按上面的设计生成文件，并用 electron-updater 6.6.2 源码测试 `.6 → .7` 与 `.7 → 2.0.0` 的选版和下载 URL。（owner）
  `be8671ce`。`tests/updater/upgrade-chain.test.ts` 用真实 `GitHubProvider`：`.6` 在四个平台都选 `.7` 并从 `v1.16.1-autolyrics.7` 下载；`.7`（频道 `tauri` / `tauri-arm64`）选 `2.0.0`，Windows 两种架构各自拿到对应 exe。
- [x] updater：去掉 `pending_update` 的双重加锁；下载好的字节和对应 `Update` 绑定，下载后冻结，重新检查不覆盖。（owner）
  `be8671ce`。`downloaded: Mutex<Option<(Update, Vec<u8>)>>`；`try_begin_download` 让下载互斥；安装失败清空冻结状态。
- [x] 迁移：每项单独记录完成状态，失败项下次启动重试；cookie 按原 domain、path、secure、httpOnly 导入；歌词缓存保留来源字段；桥接版另导出 renderer `localStorage`（含 `refresh_token`、播放列表），Tauri 版首次启动在 store 初始化前写回缺失的键。（owner）
  `3b2fc916`。标记带 `complete`、`pending`、`attempts`，未完成最多重试 5 次；`src/index.tsx` 先调 `applyMigratedLocalStorage` 再动态 import `./app`。
- [x] 桥接版：退出和自动安装前串行完成一次导出。（owner）
  `feat/tauri-bridge` `b8b5469a`。导出串行执行；首次退出最多等 5 s 导出完成再退出；更新安装开始后不再拦截退出；localStorage 每 10 分钟刷新一次缓存。
- [ ] 下载：分块续传每次重试按文件实际长度重算 Range，校验 `206` 和 `Content-Range`；内部失败与取消分开；暂停、恢复、取消等待旧 worker 退出，加 generation 校验和读取空闲超时；ffmpeg 并行读 stdout/stderr，加超时和 kill；同名并发下载用任务独有的临时文件，提交时不覆盖已有文件；文件名按字节截断并处理 Windows 保留名。（Gemini 4 Argon）
- [x] 重定向：`bili_request`、歌词和自动歌词的 HTTP 客户端每一跳都检查 host 白名单。（owner）
  `3b2fc916`。`allowlist_redirect_policy`；kuromoji 下载有 sha256 校验，不限制。
- [ ] 代码逻辑审查（GPT-6.1-Sol）。
- [ ] Mac 签名链：用 `workflow_dispatch` 跑 `release.yml`（只构建不发布），在 Mac 上对正式签名的 zip 执行 `codesign --verify -R`，条件取 `.6` 的 designated requirement。（owner）
- [ ] Mac 实机静音冒烟：装 `2.0.0`，静音启动，设置和登录都在。（owner）
  2026-11-05 核对：用户 Mac 的 `/Applications/Biu.app` 已是第 1 步的 Tauri 测试包（7.9 MB、ad-hoc、版本号 `.6`、无 updater），Electron `.6` 不在了，这台 Mac 上测不了 `.6 → .7` 一跳。用户需手动装 `2.0.0` dmg；Tauri 数据目录 `com.biu.wood3n` 里的扫码登录保留，Electron 目录 `Biu` 的 store 按原样合并。

## 分工

| 步骤 | 执行 | 审查 |
|---|---|---|
| 第 1 步 | Gemini 4 Argon，osa-a1 | Opus |
| 第 2、3 步 | Gemini 4 Argon，两个线程可并行 | Opus |
| 第 4 步 | Gemini 4 Argon | Opus |

GPT-6.1-Sol 这周 7d 余量只有 7% 和 36%，暂不用。
