# Tauri 版质量排查（2026-11）

owner = Claude（osa-a1）
状态：已批准（2026-11-10，用户授权「全权交由你负责，不需要问我」）
分支：`audit/quality-2026-11-10`（基于 `feat/tauri` `c6037b70` / `v2.0.8`）

## 背景
`v2.0.8` 修了播放时报 `The operation is not supported.`：部分 B 站 CDN 节点给 `.m4s` 返回
`Content-Type: application/octet-stream`，Tauri 回环代理原样转发，WKWebView 不嗅探内容、拒绝解码；
`v2.0.7` 起磁盘缓存把这个类型写进 `.json`，同一首歌之后每次都失败。

这类问题的共同来源：
1. Electron → Tauri：Chromium 换成 WKWebView / WebView2 / WebKitGTK，浏览器层原来容忍的行为（MIME 嗅探、HTTP 缓存、混合内容、直连 CDN）现在由我们自己的 Rust 代理承担。
2. 迁移方案 `tauri-lean.md` 把验收定为「用户装上用」，没有真实 WebView 的播放测试；CI 只跑 Linux 单测。
3. `2.0.0`–`2.0.7` 在 2026-11-05 一天内发布，多数修复由用户撞到后回补。

## 排查范围与分工

| 项 | 范围 | 执行 | 方式 |
|---|---|---|---|
| A | Rust 媒体代理与磁盘缓存：`src-tauri/src/media.rs`、`media_cache.rs`、`bili.rs`、`main.rs` 中代理启动部分 | GPT-6.1-Sol（pi，high） | 只读审计 + 可运行 `cargo test` |
| B | 前端播放状态机：`src/store/play-list.ts`、播放相关组件、`src/platform/index.ts` 的媒体 URL 与错误处理 | GPT-6.1-Sol（pi，high） | 只读审计 + 可运行 `vitest` |
| C | 兼容层与其余 Rust 模块：`src/platform/index.ts` 的未实现方法与调用方、`invoke` 命令名/参数与 Rust `#[tauri::command]` 对照、`migration.rs`、`updater.rs`、`store.rs`、`download/` | Gemini 4 Argon（pi，medium） | 只读审计（`file:line` 陈述清单模板） |

报告写到 `docs/audits/2026-11-10/<A|B|C>.md`。

## 后续步骤
- [x] 1. A/B/C 三份审计报告交回。
  A 6 条、B 8 条、C 15 条，见 `docs/audits/2026-11-10/{A,B,C}.md`。
- [ ] 2. Claude（osa-a1）逐条核对「指出问题」类条目，分为：确认（附复现或代码证据）/ 不成立 / 待实机验证。结果写入 `docs/audits/2026-11-10/triage.md`。
  已完成：A/B 全部确认（A01、A03 下调为 P2）；C 确认 13 条、待核 2 条（C3、C8，3c 中核实为成立），8 条进入修复，7 条 P3 暂不修。
- [x] 3. 确认的问题按严重度修复：每个修复带回归测试；Rust 侧与状态机修复由 Sol 实现，Opus 做代码逻辑审查；Opus 实现的小修复由 Sol 审查。
  - [x] 3a. 前端修复（GPT-6.1-Sol）：B01–B08，只改 `src/`、`tests/`。设计要点：
    - 共用一个播放代际：切歌、`ensureAudioSrcValid`、`refreshCurrentAudioSource`、启动恢复、重试都捕获 `playId` + 代际，每个 await 后核对；过期请求不得写 `audio.src`、列表条目、元数据或重试播放（B01）。
    - 给核心 `audio` 加 `error` 处理，与 `play()` 拒绝共用一套恢复：同一曲同一代际最多刷新链接一次并恢复位置；仍失败则 toast、`isPlaying=false`、冻结歌词锚点，不自动连跳（B02、B08）。
    - `isPlaying` 在 `play()` 失败时回滚；加载中再点暂停使在途播放意图失效；异常分类按 `DOMException.name`（B03）。
    - `next()` 校验 `nextId` 在列表中；`del`/`delPage`/`playList` 清理失效 `nextId`；持久化恢复校验 `playId`/`nextId`（B04）。
    - 播放位置存为 `{ playId, currentTime }`，保存 0；`clear` 清除；恢复时只接受匹配当前曲目、有限非负的值；旧的纯数字格式直接丢弃（B05）。
    - 「下一首」先移除目标，再在剩余列表找当前项，插到 +1（B06）。
    - 所有切歌分支都更新 Now Playing（B07）。
    - 每条至少一个回归测试，可参考 `/tmp/biu-audit-B/playback-audit.test.ts`。
    - 执行结果（2026-11-10，Sol）：已修复 B01–B08，修改 `src/store/play-list.ts`、`src/store/play-progress.ts`、`src/app.tsx`，新增 `tests/play-list-recovery.test.ts`（43 项，含跨代际、错误双入口去重、恢复上限、子窗口与强制静音保护）。修复前原复现用例为 `15 failed | 3 passed`；修复后 `npx vitest run` 为 `21 passed / 276 passed`（原 233 + 新增 43）。`npx tsc --noEmit -p tsconfig.json` 退出 2，共 118 条既有诊断，与使用 HEAD 原文件的基线逐字一致，改动文件无诊断；全仓库 ESLint 为既有 `19 errors, 6 warnings`，与 HEAD 基线数量一致，改动文件定向 ESLint 退出 0。`pnpm lint` 遇到 Node 引擎限制（要求 22.17.1，当前 24.22.2），已按验收命令回退 `npx eslint src tests`。未修改 `src-tauri/`，未提交或推送。真实 WKWebView 播放与异构代码逻辑审查留待步骤 4／5。日志：`/tmp/biu-B-regression-red.log`、`/tmp/biu-B-vitest-final.log`、`/tmp/biu-B-tsc.log`、`/tmp/biu-B-tsc-baseline.log`、`/tmp/biu-B-lint-final.log`。
  - [x] 3b. Rust 修复（GPT-6.1-Sol）：A01–A06，只改 `src-tauri/`。设计要点：
    - 前台与 fill 请求带 `Accept-Encoding: identity`；上游返回非 identity 的 `Content-Encoding` 时，前台原样带上该头转发，fill 不入缓存（A01）。
    - fill 只缓存 `audio/*`、`video/mp4` 或经 `normalize_media_content_type` 归一化后的媒体类型；`lookup` 遇到非媒体类型条目删除并按未命中处理（A02）。
    - 上游无 `Content-Length` 时下游用 chunked framing，上游出错不写结束块（A03）。
    - 重定向策略与 `is_media_url_allowed` 一致：静态后缀或已登记动态主机，保留跳数上限与 scheme 检查（A04）。
    - `.aac` → `audio/aac`（A05）。
    - 后台 fill 全局并发上限 2，超过时跳过本次 fill（A06）。
    - 每条至少一个单元测试，可参考 `/tmp/biu-audit-A/`。
    - 执行结果（2026-11-10）：修改 `media.rs`、`media_cache.rs`、`bili.rs`，新增 `media_tests.rs`；按设计完成 identity 请求／编码透传与拒绝缓存、媒体类型双端校验并删除旧非媒体条目、无长度响应 chunked framing、独立媒体 client 的逐跳动态白名单、AAC MIME 和全局共享缓存 fill 并发上限 2。新增 10 项测试；修复前 `cargo test --locked a0 -- --nocapture` 为 2 通过／8 失败，六类缺陷均有失败断言；修复后新增 10 项全部通过，完整 `cargo test --locked` 为 50 通过／0 失败，`cargo clippy --all-targets --locked -- -D warnings` 退出 0、无警告，`cargo fmt --check` 退出 0、无输出。实际日志在 `/tmp/biu-audit-A/3b-{red,green,cargo-test,clippy,fmt}.log`。本执行者未修改 `src/`、`tests/`，未提交；代码逻辑交叉审查及 mac WKWebView 实测留给步骤 4／5。
  - [x] 3c. 第二轮修复（GPT-6.1-Sol，3a/3b 完成后）：C1、C2、C3、C4、C5、C8、C9、C13，见 `docs/audits/2026-11-10/triage.md`。设计要点：
    - C1：启动时代理设置无效则记 warn 并按无代理启动，不 panic；`proxy_set` 对无效 host 返回错误。
    - C2：`applyMigratedLocalStorage` 单条 `setItem` 失败只记日志，仍调用 `migration_local_storage_done`。
    - C3：先读插件行为确认是否覆盖 `desktop_lyrics.rs` 的位置；确认冲突则用 `with_denylist` 排除 `desktop-lyrics`、`mini-player`。
    - C4：接入 `tauri-plugin-autostart`，`store_set` 写入 `autoStart` 变化时同步启用/禁用；补 capability。
    - C5：ffmpeg 缓存与用户设置路径绑定，设置变化时失效。
    - C8：已下载阶段再次请求下载时重发 `downloaded` 消息；`perform_check` 命中已下载时也带上版本与更新日志。
    - C9：`RunEvent::Exit` 中调用 `install_downloaded_update_on_exit`（函数已幂等）。
    - C13：只允许删除音频扩展名文件。
    - C2（Opus）：`applyMigratedLocalStorage` 单条写入失败只记日志；新增测试 `applyMigratedLocalStorage skips an entry that fails to write and still acks`。
    - 执行结果（2026-11-10，Sol，Rust 侧）：C1、C3、C4、C5、C8、C9、C13 已完成；C2 未触碰，3c 总勾选留规划者核对。修改 `src-tauri/{Cargo.toml,Cargo.lock,src/{main.rs,bili.rs,store.rs,download/mod.rs,updater.rs,local_music.rs}}`，保留 3b 既有改动。代理启动失败 warn 后无代理重试，并校验 host；ffmpeg 缓存绑定设置；更新完成消息可重放，已下载检查重发版本/日志与下载状态，退出安装消费数据一次；删除复用扫描音频扩展名。C3 冲突依据：`~/.cargo/registry/src/index.crates.io-1949cf8c6b5b557f/tauri-plugin-window-state-2.5.0/src/lib.rs:433-455` 在 ready 时恢复窗口、`:206-234` 恢复物理位置/尺寸、`:279-282` show/focus，与 `src-tauri/src/desktop_lyrics.rs:123-168,235-245` 自管逻辑坐标及初始隐藏冲突，已 denylist 两个悬浮窗口。C4 使用兼容 Tauri 2.12.5 的 autostart 2.7.0、macOS LaunchAgent；启动及 autoStart 变化时同步，失败只 warn；仅调用 Rust API，不增加前端 capability。新增 12 项测试，最终 `cargo test --locked`：62 passed / 0 failed；`cargo clippy --all-targets --locked -- -D warnings`：退出 0、无警告；`cargo fmt --check`：退出 0、无输出。C3 真实焦点/屏幕恢复、C4 登录后自启动需要 macOS 图形会话及登录周期，本机无该环境，未作系统集成通过声明；C4 单测覆盖启动同步、变更判定、失败不传播。日志：`/tmp/biu-3c-{cargo-test,clippy,fmt}.log`，失败复现：`/tmp/biu-3c-{red,updater-red,proxy-host-red}.log`。未修改前端、未提交或推送。
- [x] 4. 需要真实 WKWebView 的条目在 mac 上实测（播放、拖动、缓存命中、过期链接）。
  用新增的 `scripts/wkwebview-media-probe/` 在 macOS 26.7.2 实测 18 个响应形态，结果见 triage.md 末节与探针 README。未覆盖：整个应用的界面操作（切歌、拖动、缓存命中），mac 锁屏无法操作 GUI，留到第 7 步安装后实测。
- [x] 5. 代码逻辑审查：Sol / Opus 交叉审查全部修复（不自审）。
  Opus 审查 Sol 写的 3a、3b、3c：通过。Sol 审查 Opus 写的 C2 与探针：C2 无问题；探针 3 条（P2 用例未执行、P3 越界 Range 返回 206、P3 复用旧临时目录）已修复并在 mac 重跑 18 个用例。
- [x] 6. 验收：`npx vitest run`、`cargo test`、`cargo clippy --all-targets -- -D warnings` 全部通过。
  2026-11-10：vitest 277/277（原 233）；cargo test 62/62（原 40）；clippy 0 警告；fmt 无差异；改动文件 ESLint 仅有 HEAD 既有的 `tests/platform.test.ts:202`；knip 退出 0、条目与基线相同。
- [ ] 7. 合入 `feat/tauri`、发版、mac 安装：用户授权范围内执行，结果记录在本文件。
  2026-11-10：6 个提交已推送 `origin/audit/quality-2026-11-10`，`origin/feat/tauri` 快进到 `e6817d7e`；Tauri Build run `42798967070` 六个平台全部成功。mac 冒烟（CI arm64 包从临时目录启动，未替换 /Applications）：启动无 panic，恢复当前曲目时经代理 5 次缓存命中，`autoStart=false` 未生成 LaunchAgent，系统 quit 事件正常退出。未完成：界面内播放、拖动、切歌、缓存未命中首播（mac 锁屏，MediaRemote 命令不能送达）。`v2.0.9` 发版等这项实测后执行。
  - [ ] 7a. mac 界面实测（Gemini 4 Argon，mac 上 pi + lcu computer use；2026-11-10 用户提议）：用 CI 包 `e6817d7e`（`/tmp/biu-ci-e6817d7e.dmg`）从临时目录启动，系统静音；依次验证：续播当前曲目、进度前进、拖动进度、下一首/上一首、播放一首未缓存的歌、加载中点暂停、暂停后继续、Cmd+Q 退出。证据：每步截图路径与时间、`Biu.log` 新增行。结束后恢复静音前的音量状态、删除临时副本。视觉/行为证据由 Opus 核对后勾选。
    - 2026-11-10 第 1 次（Argon，mac）：20:00–20:29 CST 每分钟查一次，屏幕始终锁定，未执行；未改音量、未启动应用。dmg 保留在 mac 的 `/tmp/biu-ci-e6817d7e.dmg`。

## 不做
- 不改 UI 外观，不加新功能，不做大重构。
- 不动 Electron 遗留分支。
