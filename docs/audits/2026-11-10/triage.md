# 审计核对（Claude（osa-a1），2026-11-10）

核对方式：逐条读对应源码行；A/B 两份报告都附带 `/tmp/biu-audit-A`、`/tmp/biu-audit-B` 下的临时复现测试。

## 前端（B）

| ID | 结论 | 定级 | 核对依据 |
|---|---|---|---|
| B01 | 确认 | P1 | `ensureAudioSrcValid`、`refreshCurrentAudioSource` 在 await 后写当前 `playId` 的条目，无代际检查 |
| B02 | 确认 | P1 | `src/` 内没有核心 `audio` 的 `onerror` / `error` 监听 |
| B03 | 确认 | P1 | `togglePlay` 先置 `isPlaying = true`；`playAudioSafely` 失败分支不回滚；`NotAllowed` 按 message 判断 |
| B04 | 确认 | P1 | `next()` 直接消费 `nextId`；`del`、`playList` 不清理 |
| B05 | 确认 | P1 | `play-progress.ts` 只存 truthy 值、无曲目 ID |
| B06 | 确认 | P1 | `play-list.ts:853` 插入位置用旧 `currentItemIndex`，目标在当前项之后时插到前面 |
| B07 | 确认 | P2 | 有效 URL / 本地分支 return 前不调 `updateMediaSession` |
| B08 | 确认 | P1 | `refreshCurrentAudioSource` 换 `src` 不保存/恢复位置 |

## Rust（A）

| ID | 结论 | 定级 | 核对依据 |
|---|---|---|---|
| A01 | 确认（低概率） | P2（审计定 P0） | reqwest 未开解压 feature，也不发 `Accept-Encoding`；服务器通常不会压缩。按加固处理 |
| A02 | 确认 | P1 | fill 只校验 200 与长度，`text/html` 会入缓存；与 2.0.8 修复的问题同类 |
| A03 | 确认 | P2 | 断流后正常 `shutdown`；B 站 CDN 通常带 `Content-Length` |
| A04 | 确认 | P1 | `is_bili_redirect_allowed` 只查静态后缀，`is_media_url_allowed` 还查动态集合 |
| A05 | 确认 | P2 | `local_audio_mime` 把 `aac` 映射为 `audio/mp4` |
| A06 | 确认 | P2 | `in_progress` 只按键去重，没有并发上限 |

## 兼容层（C，Argon 报告，Claude 抽查）

| ID | 结论 | 定级 | 核对依据 / 处理 |
|---|---|---|---|
| C1 | 确认 | P1（审计定 P0） | 设置页每次输入变化都落盘 `proxySettings`；`build_client` 中 `reqwest::Proxy::all` 失败即 `Err`，`main.rs` 启动时 `BiliState::new(...).expect(...)` 直接 panic，之后每次启动都崩。→ 3c 修 |
| C2 | 确认（低概率） | P3 | `applyMigratedLocalStorage` 中 `setItem` 抛错会让 `boot()` 中断。→ 3c 修（try/catch） |
| C3 | 待核实 | P2 | `window_state` 插件未排除 `desktop-lyrics`/`mini-player`，可能与 `desktop_lyrics.rs` 自存的位置冲突。→ 3c 先实测再决定 |
| C4 | 确认 | P2 | 设置页「开机自启动」只写 `autoStart`，Rust 侧无任何登录项逻辑，开关无效。→ 3c 接入 `tauri-plugin-autostart` |
| C5 | 确认 | P3 | `resolve_ffmpeg_path` 先返回缓存，改 `ffmpegPath` 后重启前不生效。→ 3c 修 |
| C6 | 确认（部分） | P3 | 导入配置不即时应用代理。暂不修 |
| C7 | 确认 | P3 | 歌词/自动歌词 client 不走用户代理。暂不修 |
| C8 | 待核实 | P2 | manual 模式已下载后再点下载，`try_begin_download` 返回 false 且不发消息，弹窗可能卡在 downloading。→ 3c 核实后修 |
| C9 | 确认 | P2 | 只有托盘「退出」会调用 `install_downloaded_update_on_exit`；Cmd+Q 等走 `RunEvent::Exit` 不安装。→ 3c 修 |
| C10 | 确认 | P3 | 迁移 cookie 部分失败会重复导入。暂不修 |
| C11 | 部分 | P3 | 端口超 65535 时代理设置被整体忽略，不崩溃。暂不修 |
| C12 | 确认 | P3 | Rust 默认 `desktopLyricsStyle` 字段过时。暂不修 |
| C13 | 确认 | P3 | `delete_local_music_file` 不校验音频扩展名。→ 3c 加扩展名校验 |
| C14 | 确认 | P3 | 插件权限偏宽。暂不修 |
| C15 | 确认 | P3 | 杂项（版本回退值、下载列表非原子写）。暂不修 |

## mac 实机（WKWebView，macOS 26.7.2，Claude 执行）

工具：`scripts/wkwebview-media-probe/run.sh`（本次新增）。页面在 `localhost`、媒体在 `127.0.0.1`，跨源，与 `tauri://localhost` → 回环代理一致。

| 用例 | 结果 | 影响的条目 |
|---|---|---|
| 真实 B 站 DASH `.m4s`（`upos-sz-mirrorcoso1`，CDN 实际返回 `application/octet-stream`），原样 `octet-stream` | `NotSupportedError`，code 4 | 2.0.8 根因成立 |
| 同一文件 `audio/mp4` | 正常播放（203.69 s） | 2.0.8 修复有效 |
| ffmpeg 生成的 fMP4，`octet-stream` | 正常 | 合成文件测不出 2.0.8 的问题 |
| 无 `Content-Length`（chunked 或关闭连接结束，即使完整） | code 4 | A03：改为 chunked 不比原来差；WKWebView 需要长度，属于平台限制 |
| ADTS 标 `audio/mp4` / `audio/aac` | 失败 / 正常 | A05：旧版本地 `.aac` 在 mac 上不能播，修复有效 |
| 403 / 502 / `200 text/html` | 全部 code 4 + `NotSupportedError` | B02/B08：WebKit 把所有加载失败报成同一个错误；前端按 code 2/3/4 与 `NotSupportedError` 统一走「刷新一次」是对的 |
| gzip 体 + `Content-Encoding: gzip` | 正常 | A01：前台原样转发编码头可行 |
