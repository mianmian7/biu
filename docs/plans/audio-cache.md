# 在线音频磁盘缓存

owner = Claude（osa-a1）
状态：已批准（2026-11-05，用户批准）
分支：`feat/tauri`

## 问题
用户反馈：同一首歌重播时，还要加载很久，像在重新下载。

## 诊断（已核对代码）
- 旧 Electron 版：`<audio>` 直接请求 B 站 CDN，拦截器补 Referer。Chromium 自带的 HTTP 磁盘缓存会缓存音频响应。播放列表保存着链接，链接过期（`deadline`）前重播同一首歌直接命中缓存。
- Tauri 版：所有在线音频都经过本机回环代理（`src-tauri/src/media.rs` 的 `http_media`）。代理给每个响应加了 `Cache-Control: no-store`，WebKit 不缓存，每次播放都从 CDN 重新下载。
- 结论：迁移到 Tauri 后丢失了音频缓存，需要在代理层补回。

## 设计
### 1. 缓存键
- B 站音频 URL 的路径对同一首歌、同一音质是固定的，例如 `/upgcxcode/xx/xx/<cid>/<cid>-1-30280.m4s`。
- 查询参数（`deadline`、`upsig` 等）每次获取都会变。CDN 主机名也会在多个镜像之间换。
- 缓存键 = `sha256(URL 路径)`，不含主机名和查询参数。
- 只缓存 `GET` 请求、已登记的媒体主机，并且路径以 `.m4s`、`.flac`、`.mp3`、`.m4a`、`.mp4` 结尾。

### 2. 存储
- 目录：`app_cache_dir()/audio-cache/`。
- 每首歌两个文件：`<key>.bin` 存完整内容；`<key>.json` 存 `content-type` 和总长度。
- 先写 `<key>.part`，下载完整、校验长度与 `Content-Length` 一致后，再原子 rename。下载不完整就删除，不提交。
- 容量上限默认 1 GB，常量 `AUDIO_CACHE_MAX_BYTES`。超过上限时按文件修改时间淘汰最旧的；命中时更新修改时间（LRU）。

### 3. 读写流程（`http_media`）
- **命中**：用缓存文件响应，支持 Range、HEAD，`Content-Type` 取自 `.json`。和 `serve_local_file` 共用 Range 处理代码：抽出一个按文件路径响应的内部函数。本地文件的白名单检查保留在 `serve_local_file` 里。
- **未命中**：照现在的流程转发。同时在后台启动一次完整下载写入缓存。
  - 同一个键同一时间只下载一次，用进行中集合去重。
  - 后台下载带 Referer 和 Origin，只限制空闲超时，沿用 `MEDIA_IDLE_TIMEOUT`。
- 代价：第一次播放的歌，流量约为两倍（播放流加后台下载）。音频文件一般 3–10 MB，无损 30 MB 以上。
- 日志：`[media_cache] hit/miss/stored/evicted`，带键的前 8 位和大小，不写完整 URL。

### 4. 不改的部分
- 前端、下载模块、本地文件播放都不改。
- 缓存只服务回环代理。

### 5. 暂不做（以后需要再加）
设置页里的缓存上限和「清除音频缓存」按钮。

## 清单
- [ ] T1 缓存模块（键、存储、`.part` 原子提交、LRU 淘汰、进行中去重），新建 `src-tauri/src/media_cache.rs`。
- [ ] T2 `http_media` 接入：命中走缓存文件响应（支持 Range 和 HEAD）；未命中照常转发，并在后台填充缓存。
- [ ] T3 Rust 测试，用模拟上游：
  - 第一次请求转发，并在后台写入缓存；
  - 第二次请求（不同主机、不同查询参数、同一路径）不访问上游，内容一致；
  - 命中缓存时 Range 和 HEAD 正确；
  - 上游中途断开时不提交缓存；
  - 超过上限时淘汰最旧的。
- [ ] T4 验收：
  - `cd src-tauri && cargo test`、`cargo clippy --all-targets` 通过；
  - `npx vitest run` 通过；
  - Linux 实机（Xvfb）：播放一首歌，切走再切回。第二次的 `[switch] ... load` 明显缩短，日志出现 `[media_cache] hit`。
- [ ] 代码逻辑审查（执行者为 GPT-6.1-Sol，审查改派 Opus 或 Gemini 4 Argon）
- [ ] 视觉/动态审查：不适用（无 UI 改动）
- [ ] 最终验收（Opus）
