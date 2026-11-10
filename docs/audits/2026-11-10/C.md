# 审计报告 C：兼容层与其余 Rust 模块（Electron → Tauri 2）

## 疑似问题汇总表

| 编号 | 文件:行号 | 触发条件 | 后果 | 严重度 |
|---|---|---|---|---|
| 1 | `src-tauri/src/main.rs:253-256`、`src-tauri/src/bili.rs:304-305`、`src/pages/settings/index.tsx:100-105` | 用户在代理设置中输入不合法的 `host`（如包含 `http://`、`:端口` 或空格）且设置了 `port`，`updateSettings` 将其写入 `app-settings.json` | 下次启动时 `BiliState::new(cookie_path, saved_proxy).expect("failed to initialize BiliState")` 因 `reqwest::Proxy::all` 返回 `Err` 直接 panic，应用无法启动且重启无法自愈 | P0 |
| 2 | `src/platform/index.ts:61-66`、`src/index.tsx:24-28` | `applyMigratedLocalStorage` 中 `storage.setItem(key, value)` 抛出异常（如 WebView `QuotaExceededError` 或 `SecurityError`） | 异常未捕获且位于 `invokeTauri("migration_local_storage_done")` 之前，`boot()` 在加载 `./app` 前中断，`migrated-local-storage.json` 未被清理，后续每次启动均白屏 | P1 |
| 3 | `src-tauri/src/main.rs:146-153`、`src-tauri/src/desktop_lyrics.rs:228-245` | `tauri_plugin_window_state` 未配置 `with_denylist` 排除 `"desktop-lyrics"` 与 `"mini-player"`，且保留了 `StateFlags::VISIBLE` | `"desktop-lyrics"` 窗口创建时触发 `window_state` 插件的 `restore_state`，覆盖 `desktop_lyrics.rs` 的逻辑像素位置/尺寸并在曾可见时强制调用 `show()` 与 `set_focus()` 抢占焦点 | P1 |
| 4 | `src-tauri/src/store.rs:29,171-202`、`src/pages/settings/system-settings.tsx:407` | 用户在常规设置中开启或关闭「开机自启动」（`autoStart`） | 前端仅将 `autoStart` 布尔值写入 `app-settings.json`，`src-tauri` 未集成 `tauri-plugin-autostart` 或任何系统登录项注册逻辑，开关完全无效 | P1 |
| 5 | `src-tauri/src/download/mod.rs:161-166`、`src-tauri/src/store.rs:195-200` | 应用运行期已解析并缓存过 `RESOLVED_FFMPEG`，之后用户在设置页修改 `ffmpegPath` | `store_set` 未调用 `clear_resolved_ffmpeg_path()`，且 `resolve_ffmpeg_path` 在检查 `user_setting` 前直接返回 `RESOLVED_FFMPEG` 缓存路径，新 `ffmpegPath` 在重启前不生效 | P1 |
| 6 | `src-tauri/src/store.rs:171-202`、`src/pages/settings/export-import.tsx:56-58` | 通过「导入配置」或 `useSettings.getState().reset()` 修改 `app-settings` 中的 `proxySettings`，或前端调用 `setStore` 删除顶层键 | `write_store` 只同步 `allowed_local_dirs` 而不调用 `bili.update_proxy`；导入配置后立即 `window.location.reload()` 既未 `await` 落盘也不会重新执行 Rust `setup`；且 `write_store` 未先 `store.clear()`，被删除的顶层键残留在磁盘 store 中 | P1 |
| 7 | `src-tauri/src/lyrics.rs:76-88,219-224,307-312`、`src-tauri/src/auto_lyrics.rs:51-58` | 用户配置了 HTTP/SOCKS 代理（`proxySettings`） | `lyrics_http_client`、kuromoji 词典下载客户端及 `AutoLyricsState.netease_client` 均独立构建 `reqwest::Client`，未读取或应用 `ProxySettings` | P2 |
| 8 | `src-tauri/src/updater.rs:403-411,579-582,612-633`、`src/components/release-note-modal/index.tsx:37-44`、`src/components/update-check-button/index.tsx:18-30` | (1) `manual` 模式下载完成后再次触发 `check_app_update` 并点击「下载更新」；或 (2) `auto` 模式下载完成后页面重载再点击「检查更新」 | (1) `manual` 模式下 `state.downloaded` 为 `None` 但 `state.phase` 为 `UpdaterPhase::Downloaded`，再次调用 `download_app_update` 时 `try_begin_download()` 返回 `false` 且不发 `"app:update-message"`，弹窗永久卡在 `downloading`；(2) `auto` 模式下 `perform_check` 命中 `downloaded_summary()` 提前返回而不发 `"app:update-available"`，弹窗显示「暂无更新日志」 | P2 |
| 9 | `src-tauri/src/main.rs:314-319`、`src-tauri/src/tray.rs:258` | `auto` 更新包已下载完成，用户通过系统级退出（如 macOS `Cmd+Q` / Dock 退出，而非托盘菜单 `quit_application`）退出应用 | `RunEvent::Exit` 分支仅调用 `dl_mgr.shutdown`，未调用 `updater::install_downloaded_update_on_exit(app)`，退出时不会安装已下载的更新 | P2 |
| 10 | `src-tauri/src/migration.rs:655-663,859-868` | `tauri-migration.json` 的 `cookies` 数组中同时包含合法 Cookie 与至少一条格式异常的 Cookie | 单条 Cookie 解析失败使 `cookies_failed > 0`，`"cookies"` 被加入 `pending` 而非 `done`；用户在 Tauri 中退出登录后，后续最多 `MAX_MIGRATION_ATTEMPTS`（5）次启动仍会重复导入旧 Electron 登录 Cookie | P2 |
| 11 | `src-tauri/src/bili.rs:23,49`、`src/pages/settings/proxy-settings.tsx:76-80` | 代理端口输入超过 `65535`（如 `70000`）或 `setCookie` 传入浮点型 `expirationDate` | Rust 侧 `ProxySettings.port: Option<u16>` 与 `CookieSetInput.expiration_date: Option<i64>` 反序列化失败；`proxySettings` 写入 `app-settings.json` 后会导致 `parse_proxy_settings_value` 整段返回 `None` | P2 |
| 12 | `src-tauri/src/store.rs:61-70`、`shared/settings/desktop-lyrics.ts:2-17` | 首次启动或 `store_clear("app-settings")` 读取 Rust 侧默认 `desktopLyricsStyle` | `default_app_settings_json()` 中的 `desktopLyricsStyle` 仍使用旧字段（`layout`、`playedColor`、`strokeColor`），与前端 `DesktopLyricsStyle`（`lineMode`、`colorPreset`、`effect` 等）不一致 | P3 |
| 13 | `src-tauri/src/local_music.rs:62,160-195` | 渲染层调用 `scan_local_music` 传入任意目录路径，随后调用 `delete_local_music_file` | `scan_local_music_dirs` 无条件将传入的 `dirs` 注册进 `BiliState.allowed_local_dirs`，且 `delete_local_music_file` 不校验音频扩展名即可删除白名单目录下的任意文件 | P2 |
| 14 | `src-tauri/capabilities/default.json:17-22`、`src-tauri/tauri.conf.json:12,27-29` | `csp: null` 且 `withGlobalTauri: true` 下向全部三个窗口授予 `store:default`、`dialog:default`、`updater:default`、`process:default` | 前端仅直接调用 `plugin:app\|version`、`plugin:opener\|open_url` 与 `plugin:window\|*`，其余插件均由 Rust 命令封装，多余的插件 IPC 权限扩大了渲染层攻击面 | P3 |
| 15 | `src/platform/index.ts:257,269-277,294-296`、`src-tauri/src/download/mod.rs:1323,2777` | `getAppVersion` 失败回退、`minimizeWindow` 等未捕获 Promise、下载列表非原子写盘、未登录下载音频默认 `vipStatus=2` | `getAppVersion` 异常时回退到旧 Electron 版本号 `"1.16.1-autolyrics.6"`；`media-downloads.json` 直接 `fs::write` 非原子覆盖；未登录下载 `sid` 音频时默认按 VIP 请求 `quality=3` | P3 |

---

## 1. `src/platform/index.ts` 未实现 / 空实现 / warn 方法与调用方排查

### 1.1 残留的「未实现」辅助函数与常量
`src/platform/index.ts` 顶部仍保留并导出了迁移早期的占位辅助函数与常量：
- `NOT_IMPLEMENTED_MESSAGE = "未实现"`（`src/platform/index.ts:4`）
- `notImplementedError(method: string): Error`（`src/platform/index.ts:6-8`）
- `rejectNotImplemented<T = never>(method: string): Promise<T>`（`src/platform/index.ts:10-12`）
- `warnNotImplemented(method: string): void`（`src/platform/index.ts:14-16`）
- `noopUnsubscribe(method: string): VoidFunction`（`src/platform/index.ts:18-21`）
- `UNIMPLEMENTED_ELECTRON_METHODS = [] as const`（`src/platform/index.ts:240`）

经 `grep` 检索 `src/` 全目录：
- `src/` 下没有任何代码调用 `notImplementedError`、`rejectNotImplemented`、`warnNotImplemented` 或 `noopUnsubscribe`。
- `UNIMPLEMENTED_ELECTRON_METHODS` 仅在 `tests/platform.test.ts:8,20-21` 中被断言为空数组 `[]`。

### 1.2 `createTauriElectronAPI()` 中的空实现 / 静默降级方法及 `src/` 调用方
虽然 `createTauriElectronAPI()`（`src/platform/index.ts:242-403`）已为 `ElectronAPI` 的全部方法提供了实现，但以下方法在特定构建或运行条件下为空操作（no-op）或静默吞掉错误：

1. **`toggleDevTools`**（`src/platform/index.ts:294-296` → `src-tauri/src/main.rs:112-122`）
   - 行为：前端调用 `void invokeTauri("devtools_toggle")`。在 Rust 端 `devtools_toggle` 中，`#[cfg(not(debug_assertions))]` 分支为 `let _ = app;` 空实现。
   - `src/` 调用方：`src/layout/navbar/dev/index.tsx:19`（`<IconButton onPress={window.electron.toggleDevTools}>`，仅在 `isDev` 为 `true` 时渲染）。
2. **`listenTauriEvent` 驱动的所有事件订阅方法**（`src/platform/index.ts:70-98`）
   - 行为：当 `window.__TAURI__?.event?.listen` 不存在时，不打印警告也不抛错，直接返回空清理函数 `() => {}`；当 `listen()` 返回的 Promise reject 时，第 90 行 `.catch(() => {})` 静默忽略。
   - 涉及方法及 `src/` 调用方：
     - `navigate`（`src/platform/index.ts:304`）：`src/app.tsx:40-41`
     - `onShortcutCommand`（`src/platform/index.ts:308`）：`src/app.tsx:69-70`
     - `onPlayerCommand`（`src/platform/index.ts:313-322`）：`src/app.tsx:51-52`
     - `onDesktopLyricsContent`（`src/platform/index.ts:328-343`）：`src/pages/desktop-lyrics/index.tsx:99`
     - `onDesktopLyricsToggleRequested`（`src/platform/index.ts:346`）：`src/common/utils/desktop-lyrics.ts:295`
     - `onDesktopLyricsLockToggleRequested`（`src/platform/index.ts:347`）：`src/common/utils/desktop-lyrics.ts:299`
     - `onDesktopLyricsCommand`（`src/platform/index.ts:351`）：`src/common/utils/desktop-lyrics.ts:302`
     - `syncMediaDownloadTaskList`（`src/platform/index.ts:379-380`）：`src/pages/download-list/index.tsx:46`
     - `onUpdateAvailable`（`src/platform/index.ts:393-394`）：`src/app.tsx:150`
     - `onDownloadAppProgress`（`src/platform/index.ts:397`）：`src/app.tsx:172`
3. **`getAppVersion`**（`src/platform/index.ts:257`）
   - 行为：`invokeTauri<string>("plugin:app|version").catch(() => "1.16.1-autolyrics.6")`，失败时硬编码回退到旧 Electron 版本号 `"1.16.1-autolyrics.6"`。
   - `src/` 调用方：`src/pages/settings/index.tsx:111`。
4. **未捕获 Promise 拒绝的窗口命令**（`src/platform/index.ts:269-277, 294-296`）
   - `minimizeWindow`（第 270 行）、`toggleMaximizeWindow`（第 273 行）、`closeWindow`（第 276 行）、`toggleDevTools`（第 295 行）均使用 `void invokeTauri(...)` 且未附带 `.catch(...)`。
   - `src/` 调用方：`src/components/window-action/index.tsx:31,35,39`、`src/layout/navbar/dev/index.tsx:19`。

---

## 2. 前端 `invoke` / `invokeTauri` 调用与 Rust `#[tauri::command]` 逐一对照

### 2.1 全量命令、参数键与 `generate_handler!` 注册对照表

| 前端调用位置 | 命令名 | 前端传入参数键 | Rust `#[tauri::command]` 位置与签名参数 | `main.rs` 注册行 | 对照结论 |
|---|---|---|---|---|---|
| `src/platform/index.ts:52` | `migration_local_storage` | 无 | `migration.rs:998` `() -> Result<Map<String, Value>, String>` | `186` | 匹配 |
| `src/platform/index.ts:66` | `migration_local_storage_done` | 无 | `migration.rs:1006` `() -> Result<(), String>` | `187` | 匹配 |
| `src/platform/index.ts:111` | `media_base` | 无 | `media.rs:17` `() -> String` | `194` | 匹配 |
| `src/platform/index.ts:246` | `store_get` | `{ name }` | `store.rs:160` `(name: String)` | `188` | 匹配 |
| `src/platform/index.ts:247` | `store_set` | `{ name, value }` | `store.rs:165` `(name: String, value: Value)` | `189` | 匹配 |
| `src/platform/index.ts:248` | `store_clear` | `{ name }` | `store.rs:233` `(name: String)` | `190` | 匹配 |
| `src/platform/index.ts:250` | `cookie_get` | `{ key }` | `bili.rs:662` `(key: String)` | `192` | 匹配 |
| `src/platform/index.ts:252` | `cookie_set` | `{ payload: { name, value, expirationDate } }` | `bili.rs:670` `(payload: CookieSetInput)` (`name`, `value`, `expirationDate: Option<i64>`) | `193` | **类型隐患**：`expirationDate` 为 `Option<i64>`，若传入浮点秒数会反序列化失败 |
| `src/platform/index.ts:254` | `proxy_set` | `{ proxySettings }` | `bili.rs:678` `(proxy_settings: ProxySettings)` (`type`, `host`, `port: Option<u16>`, `username`, `password`) | `194` | **类型隐患**：前端 `proxy-settings.tsx:79` 未限制 `port <= 65535`，超出 `u16` 时反序列化失败 |
| `src/platform/index.ts:257` | `plugin:app\|version` | 无 | Tauri 内置 `app::version` | 内置 | 匹配 |
| `src/platform/index.ts:265` | `plugin:opener\|open_url` | `{ url }` | `tauri-plugin-opener` `(url: String, with: Option<String>)` | 插件 | 匹配 |
| `src/platform/index.ts:270` | `plugin:window\|minimize` | `{ label: "main" }` | Tauri 内置 `window::minimize` `(label: Option<String>)` | 内置 | 匹配 |
| `src/platform/index.ts:273` | `plugin:window\|toggle_maximize` | `{ label: "main" }` | Tauri 内置 `window::toggle_maximize` `(label: Option<String>)` | 内置 | 匹配 |
| `src/platform/index.ts:276` | `window_close` | 无 | `main.rs:91` `()` | `196` | 匹配 |
| `src/platform/index.ts:278` | `plugin:window\|is_maximized` | `{ label: "main" }` | Tauri 内置 `window::is_maximized` `(label: Option<String>)` | 内置 | 匹配 |
| `src/platform/index.ts:286` | `plugin:window\|is_fullscreen` | `{ label: "main" }` | Tauri 内置 `window::is_fullscreen` `(label: Option<String>)` | 内置 | 匹配 |
| `src/platform/index.ts:295` | `devtools_toggle` | 无 | `main.rs:113` `()` | `199` | 匹配 |
| `src/platform/index.ts:299` | `lyrics_search_netease` | `{ params }` | `lyrics.rs:185` `(params: Value)` | `200` | 匹配 |
| `src/platform/index.ts:300` | `lyrics_get_netease` | `{ params }` | `lyrics.rs:193` `(params: Value)` | `201` | 匹配 |
| `src/platform/index.ts:301` | `lyrics_search_lrclib` | `{ params }` | `lyrics.rs:213` `(params: Value)` | `202` | 匹配 |
| `src/platform/index.ts:306` | `update_playback_state` | `{ isPlaying }` | `media_keys.rs:118` `(is_playing: bool)` | `217` | 匹配 |
| `src/platform/index.ts:309` | `shortcut_register` | `{ id, accelerator }` | `shortcut.rs:85` `(id: String, accelerator: String)` | `213` | 匹配 |
| `src/platform/index.ts:310` | `shortcut_unregister` | `{ id }` | `shortcut.rs:135` `(id: String)` | `214` | 匹配 |
| `src/platform/index.ts:311` | `shortcut_register_all` | 无 | `shortcut.rs:157` `()` | `215` | 匹配 |
| `src/platform/index.ts:312` | `shortcut_unregister_all` | 无 | `shortcut.rs:163` `()` | `216` | 匹配 |
| `src/platform/index.ts:324` | `toggle_mini_player` | 无 | `desktop_lyrics.rs:561` `()` | `212` | 匹配 |
| `src/platform/index.ts:326` | `desktop_lyrics_sync` | `{ payload }` | `desktop_lyrics.rs:355` `(payload: Value)` | `204` | 匹配 |
| `src/platform/index.ts:345` | `desktop_lyrics_get_content` | 无 | `desktop_lyrics.rs:417` `()` | `205` | 匹配 |
| `src/platform/index.ts:349` | `desktop_lyrics_command` | `{ command }` | `desktop_lyrics.rs:436` `(command: String)` | `206` | 匹配 |
| `src/platform/index.ts:353` | `desktop_lyrics_start_drag` | 无 | `desktop_lyrics.rs:459` `()` | `207` | 匹配 |
| `src/platform/index.ts:356` | `desktop_lyrics_end_drag` | 无 | `desktop_lyrics.rs:489` `()` | `208` | 匹配 |
| `src/platform/index.ts:359` | `desktop_lyrics_set_ignore_mouse` | `{ ignore }` | `desktop_lyrics.rs:509` `(ignore: bool)` | `209` | 匹配 |
| `src/platform/index.ts:362` | `desktop_lyrics_set_content_height` | `{ height }` | `desktop_lyrics.rs:521` `(height: f64)` | `210` | 匹配 |
| `src/platform/index.ts:365` | `desktop_lyrics_reset_position` | 无 | `desktop_lyrics.rs:536` `()` | `211` | 匹配 |
| `src/platform/index.ts:370` | `select_directory` | `{ title }` | `dialog.rs:10` `(title: Option<String>)` | `218` | 匹配 |
| `src/platform/index.ts:371` | `select_file` | 无 | `dialog.rs:41` `()` | `219` | 匹配 |
| `src/platform/index.ts:372` | `open_directory` | `{ path }` | `dialog.rs:58` `(path: Option<String>)` | `220` | 匹配 |
| `src/platform/index.ts:373` | `show_file_in_folder` | `{ filePath }` | `dialog.rs:94` `(file_path: String)` | `221` | 匹配 |
| `src/platform/index.ts:374` | `get_fonts` | 无 | `fonts.rs:60` `()` | `222` | 匹配 |
| `src/platform/index.ts:375` | `scan_local_music` | `{ dirs }` | `local_music.rs:161` `(dirs: Vec<String>)` | `223` | 匹配 |
| `src/platform/index.ts:376` | `delete_local_music_file` | `{ filePath }` | `local_music.rs:174` `(file_path: String)` | `224` | 匹配 |
| `src/platform/index.ts:378` | `download_get_list` | 无 | `download/mod.rs:3194` `()` | `225` | 匹配 |
| `src/platform/index.ts:381` | `download_add` | `{ task }` | `download/mod.rs:3201` `(task: MediaDownloadInfo)` | `226` | 匹配 |
| `src/platform/index.ts:382` | `download_add_list` | `{ tasks }` | `download/mod.rs:3212` `(tasks: Vec<MediaDownloadInfo>)` | `227` | 匹配 |
| `src/platform/index.ts:383` | `download_pause` | `{ id }` | `download/mod.rs:3225` `(id: String)` | `228` | 匹配 |
| `src/platform/index.ts:384` | `download_resume` | `{ id }` | `download/mod.rs:3235` `(id: String)` | `229` | 匹配 |
| `src/platform/index.ts:385` | `download_retry` | `{ id }` | `download/mod.rs:3246` `(id: String)` | `230` | 匹配 |
| `src/platform/index.ts:386` | `download_cancel` | `{ id }` | `download/mod.rs:3257` `(id: String)` | `231` | 匹配 |
| `src/platform/index.ts:387` | `download_clear` | 无 | `download/mod.rs:3267` `()` | `232` | 匹配 |
| `src/platform/index.ts:392` | `check_app_update` | 无 | `updater.rs:797` `()` | `242` | 匹配 |
| `src/platform/index.ts:395` | `get_update_mode` | 无 | `updater.rs:804` `()` | `243` | 匹配 |
| `src/platform/index.ts:396` | `download_app_update` | 无 | `updater.rs:809` `()` | `244` | 匹配 |
| `src/platform/index.ts:398` | `quit_and_install` | 无 | `updater.rs:814` `()` | `245` | 匹配 |
| `src/platform/auto-lyrics-worker.ts:315` | `auto_lyrics_state_snapshot` | 无 | `auto_lyrics.rs:796` `()` | `239` | 匹配 |
| `src/platform/auto-lyrics-worker.ts:341` | `auto_lyrics_view` | `{ bvid }` | `auto_lyrics.rs:326` `(bvid: String)` | `233` | 匹配 |
| `src/platform/auto-lyrics-worker.ts:361` | `auto_lyrics_search_netease` | `{ query }` | `auto_lyrics.rs:380` `(query: String)` | `234` | 匹配 |
| `src/platform/auto-lyrics-worker.ts:365` | `auto_lyrics_get_netease_lyric` | `{ songId }` | `auto_lyrics.rs:487` `(song_id: i64)` | `235` | 匹配 |
| `src/platform/auto-lyrics-worker.ts:373` | `auto_lyrics_search_bili_videos` | `{ keyword }` | `auto_lyrics.rs:557` `(keyword: String)` | `236` | 匹配 |
| `src/platform/auto-lyrics-worker.ts:385` | `auto_lyrics_fetch_audio` | `{ bvid, cid }` | `auto_lyrics.rs:676` `(bvid: String, cid: Option<i64>)` | `237` | 匹配 |
| `src/platform/auto-lyrics-worker.ts:396` | `auto_lyrics_decode_audio` | `{ token }` | `auto_lyrics.rs:775` `(token: String)` | `238` | 匹配 |
| `src/platform/auto-lyrics-worker.ts:418` | `auto_lyrics_cache_set` | `{ key, value }` | `auto_lyrics.rs:809` `(key: String, value: Value)` | `240` | 匹配 |
| `src/platform/auto-lyrics-worker.ts:425` | `auto_lyrics_ref_set` | `{ id, value }` | `auto_lyrics.rs:821` `(id: String, value: Value)` | `241` | 匹配 |
| `src/platform/romaji.ts:27` | `kuromoji_ensure_dict` | 无 | `lyrics.rs:424` `()` | `203` | 匹配 |
| `src/platform/romaji.ts:105`、`src/common/utils/logger.ts:23`、`src/store/play-list.ts:209,301,1124` | `frontend_log` | `{ level, message }` | `main.rs:97` `(level: String, message: String)` | `197` | 匹配 |
| `src/service/request/index.ts:104` | `bili_request` | `{ request }` | `bili.rs:654` `(request: BiliRequestInput)` (`timeoutMs` 通过 `alias = "timeoutMs"` 映射) | `191` | 匹配 |
| `src/store/play-list.ts:195` | `is_silent_mode` | 无 | `main.rs:106` `()` | `198` | 匹配 |

### 2.2 死命令（已注册但前端无调用）
- `ensure_main_window`（`src-tauri/src/main.rs:58-88`，注册于第 195 行）：`src/` 下无任何调用方。

---

## 3. `src-tauri/src/migration.rs` 旧数据迁移失败处理审计

### 3.1 迁移一半失败与重试机制
- **Store 级与项级粒度重试**：`migrate_from_candidates_with`（`src-tauri/src/migration.rs:735-945`）通过 `MigrationMarker` 的 `done`、`pending`、`complete` 和 `attempts` 字段跟踪进度。已完成项记录在 `done` 中，下次启动重试时通过 `is_done(&done, item)`（第 773 行）跳过；失败项记录在 `pending` 中，最多重试 `MAX_MIGRATION_ATTEMPTS = 5` 次（第 19、747 行）。
- **`tauri-migration.json` 暂时不可读时的回退保护**：若 `tauri-migration.json` 存在但读取或解析失败（第 799-806 行），`"export"` 被加入 `pending`，先回退读取原始 store 文件，且对缺失的原始 store 文件不标记为 `done`（第 887-888 行），留待下次成功读取 `tauri-migration.json` 时补齐。
- **缺陷 1（Cookie 部分失败导致退出登录后被重新导入）**：在 `import_cookies_into_bili_state`（第 631-675 行）中，单条 Cookie 格式错误（`cookie_set_string` 或 `store.parse` 失败，第 655-663 行）与 `bili_state.save_cookies()` 落盘失败（第 668-672 行）同样累加 `failed`。当 `export.cookies` 含有 1 条永久格式异常的 Cookie 和若干合法登录 Cookie 时，合法 Cookie 已在首次启动写入 `cookies.json`，但因 `cookies_failed > 0`，`"cookies"` 被推入 `pending` 而非 `done`（第 863-866 行）。若用户随后在 Tauri 中退出登录（清空 Cookie），后续最多 4 次启动仍会再次执行 `import_cookies_into_bili_state`，将旧 Electron Cookie 重新写回。
- **缺陷 2（前端 `applyMigratedLocalStorage` 未捕获 `storage.setItem` 异常）**：Rust 端在写入 `migrated-local-storage.json` 后即把 `"localStorage"` 标为 `done`（第 869-879 行）。前端 `applyMigratedLocalStorage`（`src/platform/index.ts:49-68`）在第 62 行执行 `storage.setItem(key, value)` 时未包裹 `try/catch`，一旦抛出 `QuotaExceededError` 或 `SecurityError`，会在调用第 66 行 `invokeTauri("migration_local_storage_done")` 前直接 reject，导致 `src/index.tsx:26` 的 `boot()` 终止且 `migrated-local-storage.json` 永久残留。

### 3.2 重复迁移与覆盖保护
- 当 `migration.json` 中 `complete == true` 或 `attempts >= 5` 时直接跳过（第 747-754 行）。
- `merge_store_without_overwriting`（第 416-556 行）对已存在的 Tauri 数据采取不覆盖策略：`app-settings` 仅补齐缺失键（第 433-444 行）；`user-login-info` 在已登录（`isLogin == true` 或存在 `mid`）时不覆盖（第 469-476 行）；`media-downloads` 与其余 store 仅插入不存在的键（第 515-549 行）；Cookie 仅在 `!store.contains(&domain, path, &c.name)` 时导入（第 652-654 行）。

### 3.3 旧文件缺失或格式异常时的行为
- **无 Electron 目录**：`locate_electron_user_data_dir` 返回 `None` 时直接跳过且不写 `migration.json`（第 780-783 行）。
- **`migration.json` 损坏**：若 `tauri_data_dir.join("migration.json")` 存在但 JSON 解析失败（第 760-763 行），打日志后永久跳过迁移，不会删除或重写损坏的 marker。
- **`tauri-migration.json` 版本号不等于 1**：当 `payload.version != 1` 时（第 792-798 行），不将 `"export"` 加入 `pending`，直接回退到读取原始 store 文件，此时 `cookies` 与 `localStorage` 不会被迁移且 `marker.complete` 仍会被置为 `true`。

---

## 4. `updater.rs`、`store.rs`、`download/` 及入口中的 `unwrap()` / `expect()` / `panic!` 审计

在 `src-tauri/src/` 全目录中未检索到 `panic!`、`unreachable!`、`todo!` 或 `unimplemented!` 宏调用。各目标模块的非测试代码 `unwrap()` / `expect()` 分布如下：

### 4.1 `src-tauri/src/store.rs`
- **无任何 `.unwrap()` 或 `.expect()` 调用**（全部使用 `?`、`unwrap_or`、`unwrap_or_default`、`unwrap_or_else`）。

### 4.2 `src-tauri/src/updater.rs`
- **非测试代码中共有 28 处 `.unwrap()`，无 `.expect()`**：
  - 全部 28 处均为 `UpdaterState` 内部 `std::sync::Mutex::lock().unwrap()`：
    - `self.phase.lock().unwrap()` / `state.phase.lock().unwrap()`：第 `268, 442, 594, 600, 614, 636, 718, 742, 766, 772, 822, 830, 861` 行（共 13 处）。
    - `self.downloaded.lock().unwrap()` / `state.downloaded.lock().unwrap()`：第 `281, 716, 770, 828, 842, 860` 行（共 6 处）。
    - `s.mode.lock().unwrap()` / `state.mode.lock().unwrap()`：第 `374, 380, 763, 818` 行（共 4 处）。
    - `state.pending_update.lock().unwrap()`：第 `427, 584, 589` 行（共 3 处）。
    - `state.downloaded_file.lock().unwrap()`：第 `615, 717` 行（共 2 处）。
  - **外部输入可达性**：各临界区内仅做内存读写与枚举赋值，无可能 panic 的操作，外部网络/磁盘输入不会使上述 `Mutex` 中毒（poison）。

### 4.3 `src-tauri/src/download/mod.rs`
- **非测试代码中共有 66 处 `.unwrap()`，无 `.expect()`**：
  - `resolved_ffmpeg_cache().read()/write().unwrap()`：第 `45, 162, 247` 行（共 3 处）。
  - `self.chunk_idle_timeout.read()/write().unwrap()`：第 `1180, 2124` 行（共 2 处）。
  - `self.generations.lock().unwrap()`：第 `1184, 1193, 1201, 1247` 行（共 4 处）。
  - `self.workers.lock().unwrap()` / `self.mgr.workers.lock().unwrap()`：第 `1212, 1230, 1246, 1540, 1572, 3141, 3168` 行（共 7 处）。
  - `self.tasks.read()/write().unwrap()`：第 `1305, 1312, 1329, 1336, 1515, 3065, 3093, 3138, 3157, 3168` 行（共 10 处）。
  - `self.pending_updates.lock().unwrap()`：第 `1378, 1408, 3063, 3091, 3164, 3172` 行（共 6 处）。
  - `self.last_flush.lock().unwrap()` / `mgr.last_flush.lock().unwrap()`：第 `1383, 1400` 行（共 2 处）。
  - `task_arc.lock().unwrap()` / `t.lock().unwrap()`：第 `1315, 1331, 1337, 1530, 1616, 1637, 1679, 1721, 1734, 1793, 1825, 1858, 1914, 1945, 1992, 2009, 2139, 2151, 2236, 2255, 2344, 2403, 2422, 2450, 2499, 2592, 2636, 2734, 2753, 2818, 2851, 2878, 2942, 2971, 3032, 3067, 3071, 3095, 3100, 3140, 3160, 3175` 行（共 42 处）。
  - **外部输入可达性**：所有 66 处均为 `Mutex`/`RwLock` 锁获取，内部无越界切片或算术溢出（`chunk.end - chunk.start + 1` 由 `split_chunks` 第 1039-1054 行保证 `start <= end`，`saturating_sub` 用于差值扣减），网络/磁盘/用户输入不会直接触发这些 `.unwrap()` panic。

### 4.4 关联模块（`main.rs`、`migration.rs`、`lyrics.rs`）中可受外部/磁盘输入影响的 `expect()` / `unwrap()`
- **`src-tauri/src/main.rs:256`（高危，受磁盘/用户输入触发）**：
  `BiliState::new(cookie_path, saved_proxy).expect("failed to initialize BiliState")`
  `saved_proxy` 来自磁盘 `app-settings.json`（由用户在设置页输入）。当 `proxy_type` 为 `"http"` / `"socks4"` / `"socks5"` 且 `host` 含非法 URL 字符（如 `"http://127.0.0.1"` 或 `"127.0.0.1:7890"`）且 `port > 0` 时，`build_reqwest_client`（`src-tauri/src/bili.rs:305`）中的 `reqwest::Proxy::all(&proxy_url)` 返回 `Err`，直接触发 `.expect(...)` panic。
- **`src-tauri/src/main.rs:282`**：`AutoLyricsState::new(...).expect("failed to initialize AutoLyricsState")`（仅在 `reqwest::Client::builder().build()` 失败时触发）。
- **`src-tauri/src/main.rs:294`**：`tokio::net::TcpListener::from_std(listener).unwrap()`（第 286-287 行已执行 `set_nonblocking(true)?`）。
- **`src-tauri/src/migration.rs:338, 344`**：`HmacSha512::new_from_slice(password).expect("HMAC accepts any key size")`（HMAC 支持任意长度密钥，不会失败）。
- **`src-tauri/src/migration.rs:372`**：`let iv: [u8; 16] = raw[..16].try_into().unwrap();`（受第 369 行 `if raw.len() < 17` 保护，不会失败）。

---

## 5. `main.rs`、`tauri.conf.json`、`capabilities/default.json` 权限与配置审计

### 5.1 前端实际使用的插件 API 覆盖情况
- `src-tauri/tauri.conf.json:12` 配置了 `"withGlobalTauri": true`，向渲染层注入 `window.__TAURI__.core.invoke` 与 `window.__TAURI__.event.listen`。
- `src-tauri/capabilities/default.json:3` 配置了 `"windows": ["main", "desktop-lyrics", "mini-player"]`，覆盖了应用全部三个窗口标签。
- 前端直接调用的 `plugin:*` 命令均已在 `src-tauri/capabilities/default.json` 中授权：
  - `plugin:app|version`（`src/platform/index.ts:257`）← `"core:default"`（第 5 行）
  - `plugin:opener|open_url`（`src/platform/index.ts:265`）← `"opener:default"`（第 17 行，允许 `http://*`、`https://*`、`mailto:*`、`tel:*`）
  - `plugin:window|minimize`（`src/platform/index.ts:270`）← `"core:window:allow-minimize"`（第 6 行）
  - `plugin:window|toggle_maximize`（`src/platform/index.ts:273`）← `"core:window:allow-toggle-maximize"`（第 9 行）
  - `plugin:window|is_maximized`（`src/platform/index.ts:278`）← `"core:window:allow-is-maximized"`（第 15 行）
  - `plugin:window|is_fullscreen`（`src/platform/index.ts:286`）← `"core:window:allow-is-fullscreen"`（第 16 行）
  - `data-tauri-drag-region` 触发的 `plugin:window|start_dragging` 与 `plugin:window|internal_toggle_maximize` ← `"core:window:allow-start-dragging"`（第 14 行）与 `"core:default"`（第 5 行）

### 5.2 权限过宽与插件配置问题
1. **多余的插件 IPC 权限（`src-tauri/capabilities/default.json:18-22`）**：
   前端从未直接调用 `plugin:dialog|*`、`plugin:store|*`、`plugin:window-state|*`、`plugin:updater|*`、`plugin:process|*`（相关功能全部通过自定义 Rust 命令 `select_directory`、`store_get`/`store_set`、`check_app_update` 等在 Rust 端调用插件 Rust API 完成）。在 `src-tauri/tauri.conf.json:27-29` 设置 `"csp": null` 且开启 `"withGlobalTauri": true` 的情况下，保留 `"dialog:default"`、`"store:default"`、`"updater:default"`、`"process:default"` 使渲染层可直接调用 `plugin:store|*` 读写任意存储文件或调用 `plugin:process|exit`。
2. **`tauri-plugin-window-state` 缺少窗口排除列表（`src-tauri/src/main.rs:146-153`）**：
   `tauri_plugin_window_state::Builder::default()` 未调用 `.with_denylist(&["desktop-lyrics", "mini-player"])`，导致由 `desktop_lyrics.rs` 自行管理位置和显隐的 `"desktop-lyrics"` 悬浮窗及固定尺寸的 `"mini-player"` 窗口被纳入 `.window-state.json` 持久化与恢复。

---

## Claims

C1: `src/platform/index.ts` sets `UNIMPLEMENTED_ELECTRON_METHODS = [] as const`. — src/platform/index.ts:240
C2: `src/platform/index.ts` defines `notImplementedError`, `rejectNotImplemented`, `warnNotImplemented`, and `noopUnsubscribe` with `NOT_IMPLEMENTED_MESSAGE = "未实现"`. — src/platform/index.ts:4-21
C3: `devtools_toggle` in `src-tauri/src/main.rs` has body `let _ = app;` under `#[cfg(not(debug_assertions))]`. — src-tauri/src/main.rs:112-122
C4: `ensure_main_window` is registered in `tauri::generate_handler!` in `src-tauri/src/main.rs`. — src-tauri/src/main.rs:58-88,195
C5: `getAppVersion` in `src/platform/index.ts` catches rejections from `invokeTauri<string>("plugin:app|version")` and returns `"1.16.1-autolyrics.6"`. — src/platform/index.ts:257
C6: `applyMigratedLocalStorage` calls `storage.setItem(key, value)` without a `try/catch` block before calling `invokeTauri("migration_local_storage_done")`. — src/platform/index.ts:59-66
C7: `boot` in `src/index.tsx` awaits `applyMigratedLocalStorage()` before importing `./app`. — src/index.tsx:24-28
C8: `src-tauri/src/main.rs` calls `BiliState::new(cookie_path, saved_proxy).expect("failed to initialize BiliState")` inside `.setup()`. — src-tauri/src/main.rs:253-256
C9: `build_reqwest_client` in `src-tauri/src/bili.rs` formats `proxy_url` as `"{scheme}://{host}:{port}"` and propagates `reqwest::Proxy::all(&proxy_url)` errors via `?`. — src-tauri/src/bili.rs:299-306
C10: `useSystemSettingsForm` in `src/pages/settings/index.tsx` calls `updateSettings(patch)` and `window.electron.setProxySettings(values.proxySettings as ProxySettings)` inside `watch`. — src/pages/settings/index.tsx:97-107
C11: `ProxySettings` in `src-tauri/src/bili.rs` defines `port` as `Option<u16>`. — src-tauri/src/bili.rs:19-27
C12: `ProxySettings` in `src/pages/settings/proxy-settings.tsx` sets `port` to `num` whenever `Number.isFinite(num) && num > 0`. — src/pages/settings/proxy-settings.tsx:76-81
C13: `CookieSetInput` in `src-tauri/src/bili.rs` defines `expiration_date` (`#[serde(rename = "expirationDate")]`) as `Option<i64>`. — src-tauri/src/bili.rs:45-50
C14: `default_app_settings_json` in `src-tauri/src/store.rs` defines `desktopLyricsStyle` with keys `layout`, `align`, `showTranslation`, `showRomaji`, `karaoke`, `playedColor`, `unplayedColor`, and `strokeColor`. — src-tauri/src/store.rs:61-70
C15: `defaultDesktopLyricsStyle` in `shared/settings/desktop-lyrics.ts` defines `DesktopLyricsStyle` with `lineMode`, `subScale`, `fontFamily`, `bold`, `colorPreset`, `customPlayedColor`, `customUnplayedColor`, `effect`, `karaokeSubLines`, and `opacity`. — shared/settings/desktop-lyrics.ts:2-17
C16: `SystemSettingsTab` in `src/pages/settings/system-settings.tsx` binds a `Switch` component to the `autoStart` field of `AppSettings`. — src/pages/settings/system-settings.tsx:404-417
C17: `write_store` in `src-tauri/src/store.rs` inserts each `(k, v)` from `obj` via `store.set(k.clone(), v.clone())` without calling `store.clear()`. — src-tauri/src/store.rs:190-194
C18: When `name == "app-settings"`, `write_store` in `src-tauri/src/store.rs` only invokes `sync_allowed_local_dirs_from_value(&bili, &final_val)`. — src-tauri/src/store.rs:195-200
C19: `resolve_ffmpeg_path` in `src-tauri/src/download/mod.rs` returns the path cached in `RESOLVED_FFMPEG` if `cached.exists()` is true before inspecting `user_setting`. — src-tauri/src/download/mod.rs:161-169
C20: `lyrics_http_client` in `src-tauri/src/lyrics.rs` builds a static `reqwest::Client` without proxy settings. — src-tauri/src/lyrics.rs:76-88
C21: `AutoLyricsState::new` in `src-tauri/src/auto_lyrics.rs` builds `netease_client` without proxy settings. — src-tauri/src/auto_lyrics.rs:51-58
C22: `tauri_plugin_window_state::Builder::default()` in `src-tauri/src/main.rs` is configured with `StateFlags::all() - StateFlags::DECORATIONS` and no `with_denylist` call. — src-tauri/src/main.rs:146-153
C23: `import_cookies_into_bili_state` in `src-tauri/src/migration.rs` increments `failed` when `cookie_set_string(c, now_secs).map(|(raw, url)| store.parse(&raw, &url))` does not return `Some(Ok(_))`. — src-tauri/src/migration.rs:655-663
C24: `migrate_from_candidates_with` in `src-tauri/src/migration.rs` pushes `"cookies"` to `pending` instead of `done` when `cookies_failed > 0`. — src-tauri/src/migration.rs:859-868
C25: `migrate_from_candidates_with` in `src-tauri/src/migration.rs` returns `None` when `read_marker` returns `None` and `tauri_data_dir.join(MIGRATION_MARKER_FILE).exists()` is true. — src-tauri/src/migration.rs:745-763
C26: `src-tauri/src/store.rs` contains zero `.unwrap()`, `.expect()`, or `panic!` calls. — src-tauri/src/store.rs:1-314
C27: All 28 non-test `.unwrap()` calls in `src-tauri/src/updater.rs` are `Mutex::lock().unwrap()` calls on `UpdaterState` fields. — src-tauri/src/updater.rs:268-861
C28: All 66 non-test `.unwrap()` calls in `src-tauri/src/download/mod.rs` are `Mutex::lock().unwrap()` or `RwLock::read()/write().unwrap()` calls. — src-tauri/src/download/mod.rs:45-3175
C29: In `"manual"` update mode, `download_update_internal` in `src-tauri/src/updater.rs` sets `*state.phase.lock().unwrap() = UpdaterPhase::Downloaded` without setting `state.downloaded`. — src-tauri/src/updater.rs:612-633
C30: When `state.try_begin_download()` returns `false`, `download_update_internal` in `src-tauri/src/updater.rs` returns `Ok(())` without emitting `"app:update-message"`. — src-tauri/src/updater.rs:579-582
C31: When `state.downloaded_summary()` returns `Some((version, notes))`, `perform_check` in `src-tauri/src/updater.rs` returns `CheckAppUpdateResult` without emitting `"app:update-available"`. — src-tauri/src/updater.rs:403-411
C32: The `tauri::RunEvent::Exit` branch in `src-tauri/src/main.rs` calls `dl_mgr.shutdown` and does not call `updater::install_downloaded_update_on_exit`. — src-tauri/src/main.rs:314-319
C33: `scan_local_music_dirs` in `src-tauri/src/local_music.rs` calls `state.register_local_dir(dir_path)` for each directory in `dirs`. — src-tauri/src/local_music.rs:58-63
C34: `src-tauri/capabilities/default.json` includes `"dialog:default"`, `"store:default"`, `"window-state:default"`, `"updater:default"`, and `"process:default"` in `permissions`. — src-tauri/capabilities/default.json:4-23
