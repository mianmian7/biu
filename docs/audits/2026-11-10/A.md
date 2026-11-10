# A — Rust 媒体代理与磁盘缓存只读审计

审计日期：2026-11-10。审计基线：`c6037b70db3f213a64e9a8f93c07e2e5592b7946`，分支 `audit/quality-2026-11-10`，版本 2.0.8。

范围：`src-tauri/src/media.rs`、`media_cache.rs`、`bili.rs`；`main.rs` 的缓存初始化、代理启动；必要时核对 `src/platform/index.ts` 的媒体 URL 契约及本地 AAC 的扫描入口。已读 `docs/plans/quality-audit-2026-11.md`、`audio-cache.md`、`tauri-lean.md`。

**结论：复现 6 项实现问题。A05 的 MIME 错配已复现，其 WKWebView 播放后果标为待实机验证。未改源码、未改方案、未提交。** 以下 P0 表示满足所述触发条件的资源无法播放，不表示所有歌曲当前都受影响。模拟上游证明实现缺陷，不代表已经观察到线上 CDN 正在返回这些响应。

## 问题表（按严重度排序）

| 编号 | 严重度 | 问题与实际后果 | 关键位置 | 证据状态 |
|---|---|---|---|---|
| A01 | P0 播放不可用 | 未解压的 gzip 音频丢失 `Content-Encoding`；冷流及缓存命中均把压缩字节当作裸 MP4 发送，且错误缓存不会自行重填 | `src-tauri/src/media.rs:599-614`；`src-tauri/src/media_cache.rs:171-205` | 真实 fMP4 夹具 + 模拟 HTTP 已复现；未测真实 WebView |
| A02 | P1 功能错误 | `200 text/html` 临时错误页被当完整音频缓存；上游恢复后仍只读错误页 | `src-tauri/src/media_cache.rs:165-181`、`:90-112` | 模拟上游恢复后仍命中 HTML，新增上游请求为 0 |
| A03 | P1 功能错误 | chunked 上游不完整时，代理删除分块 framing 并正常关闭，向客户端报告成功的 EOF | `src-tauri/src/media.rs:594-608`、`:625-660` | 同一坏响应直连报 body 错误，经代理 `bytes()` 成功 |
| A04 | P1 功能错误 | 动态已登记 CDN 被初始 URL 白名单接受，但跳转到动态已登记域名仍被另一套规则拒绝，回 502 | `src-tauri/src/bili.rs:337-359`；对照 `:172-186` | 两端均已登记，模拟跳转结果 502，目标收到 0 次请求 |
| A05 | P1 功能错误 | 原始 ADTS `.aac` 被声明为 MP4 容器；本地扫描接受的格式与代理 MIME 不匹配 | `src-tauri/src/media.rs:203`；调用 `:272-281` | ADTS 实文件经过本地路由收到 `audio/mp4`；**播放后果待 mac 实机验证** |
| A06 | P2 体验或隐患 | 不同键的后台 fill 无并发/总字节预算，`.part` 不计入淘汰；快速切歌可突破 1 GB 上限并继续占带宽 | `src-tauri/src/media_cache.rs:115-136`、`:171-196`、`:225-236` | 缩小容量测试：预算 16 B，3 个活跃 fill 已占 36 B 临时文件 |

## 复现方法与证据边界

所有临时文件在 `/tmp/biu-audit-A/`。`prepare.py` 从仓库读取 `media.rs`、`media_cache.rs`、`bili.rs`，生成临时编译副本：

- 删除 Tauri IPC command 包装函数；这些函数不参与代理数据路径。
- `tauri::async_runtime::spawn` 替换为 `tokio::spawn`。
- `tauri::http` 替换为同一个版本的 `http` crate。
- Kuromoji 文件读取以返回 404 的 stub 代替，所有本次测试均不走该路由。
- 网络转发、MIME、白名单/重定向、缓存下载/提交/查找/淘汰逻辑不变；依赖直接使用本仓库 `target/debug/deps` 的编译产物，按 fingerprint 选择与项目一致的依赖。
- 临时 `cache_tests.rs` 在同模块内调用已有私有 `with_limit`，以 16 B 容量验证预算；没有改变生产容量常量或下载逻辑。

`harness.rs` 的新增测试**断言当前错误行为以证明其存在**，测试通过不表示缺陷已修复。共运行新增 7 项（6 个问题 + 1 个非 2xx 对照）和现有缓存 9 项，16/16 通过。另运行原仓库完整 `cargo test --locked`，40/40 通过。

## A01 — 压缩 representation 丢失编码，缓存永久保存压缩字节

**严重度：P0。**

**位置：** `src-tauri/src/media.rs:513-525`、`:599-614`、`:625-635`；`src-tauri/src/media_cache.rs:151-160`、`:171-205`；元数据定义 `:14-18`；`src-tauri/Cargo.toml:24` 的 reqwest features 及 `bili.rs:355-360` 的 client builder。

**触发条件：** 已允许的 `.m4s` 返回：

```http
HTTP/1.1 200 OK
Content-Type: audio/mp4
Content-Encoding: gzip
Content-Length: <压缩后的长度>

<gzip(有效的 AAC/fMP4 音频)>
```

当前 reqwest 0.12.28 未启用 `gzip` / `brotli` / `deflate` / `zstd` 解码 feature。前台与 fill 也没有请求 `Accept-Encoding: identity`。在缺少 `Accept-Encoding` 请求头时，上游使用内容编码不等于已经违反内容协商协议。

**实际行为：** `bytes_stream()` 得到压缩 representation；代理只保留长度、Range 相关头和 MIME，不保留 `Content-Encoding`。客户端收到 `Content-Type: audio/mp4`，但首字节是 gzip 的 `1f 8b`。fill 以压缩长度校验并成功提交，元数据没有 encoding 字段。后续缓存命中继续发送压缩文件，Range 也作用于压缩字节，无法还原成可寻址的裸音频。这是正常 HTTP 成功响应下的数据语义破坏，不是 `Content-Length` 自动解压后失配。

**复现输出：**

```text
A01 cold status=200 OK content-type=audio/mp4 content-encoding=None
A01 cold body magic=[1f, 8b] (gzip bytes, not decoded audio)
A01 cache body=gzip content-encoding=absent upstream_delta=0
```

输入夹具由 ffmpeg 生成有效 AAC/fMP4，然后 gzip；已通过 ffprobe 确认原夹具格式。未在 WKWebView 中加载本次夹具，不声称已观察到具体 `MediaError.code`。

**建议修法：** 前台和 fill 统一请求 identity representation，并检测意外的非 identity 编码。最小安全行为是拒绝未支持的编码，避免以错误音频写入缓存。若需要支持压缩，必须真正解码并同步处理长度、Range 与缓存 representation；只给冷流补一个 `Content-Encoding` 头不能修复缓存。为现有可能污染的缓存增加版本失效策略，避免旧条目继续命中。

**mac 验证：** 用同一有效 `.m4s` 的 identity/gzip 两种模拟上游响应，静音加载各自的 loopback URL；记录 `loadedmetadata`、`canplay`、`error`、`audio.error?.code`、`play()` 拒绝值。再让 fill 完成、停止上游，重复加载和拖动进度，确认缓存命中也通过。测试需使用已登记的模拟媒体主机，不能绕过白名单。

## A02 — `200` HTML 错误页会长期毒化音频缓存

**严重度：P1。**

**位置：** `src-tauri/src/media_cache.rs:165-181`、`:198-226`、`:90-112`；`src-tauri/src/media.rs:219-225`、`:493-504`。

**触发条件：** 已登记媒体主机的一次完整 GET 对 `.m4s` 返回 `200 OK`、正确的 `Content-Length`、`Content-Type: text/html` 和临时挑战/错误页。随后同一路径的上游恢复为有效音频。

**实际行为：** fill 仅检查 200、没有 `Content-Range`、长度相符；`text/html` 原样进入 `.json`。lookup 只检查文件大小和 header 字符合法，不检查明显错误的媒体类型。命中不发起上游请求，也没有错误条目的 TTL/失效入口。于是短暂 CDN 异常变成长期失败，每次重播都返回原 HTML；刷新签名和更换镜像不改变路径键。命中会更新 LRU 时间，反复重播还会延长错误条目的存留。

这里不把普通 403/404 错误页误报为缓存污染：非 200 的错误响应确实会被拒绝缓存。本项专指 HTTP 成功包装的非媒体内容。

**复现输出：**

```text
A02 after recovery status=200 OK content-type=text/html
A02 cache body=<html>temporary challenge</html> upstream_delta=0
```

测试在缓存提交后才切换上游到 `200 audio/mp4`；第二次请求没有到达健康上游。

**建议修法：** 写入与读取两端拒绝明确非媒体的类型（例如 HTML/JSON 错误页），允许现有合法媒体类型及经验证可归一化的通用类型；对预期音频格式增加轻量容器/签名验证，至少拒绝空文件与明显错误页。读取旧缓存时发现此类内容应删除/失效并回源；若增加播放失败后失效入口，应避免因本机不支持某 codec 而无休止重下合法文件。不要把 `text/html` 简单改名为 `audio/mp4`。

**mac 验证：** 先由模拟上游返回上述 HTML，待缓存提交后切为真实 fMP4；分别重播、重新获取 playurl、切换镜像。记录缓存命中与上游请求计数。修复后应能恢复到真实音频，不能继续命中 HTML。

## A03 — chunked 断流被重新包装为成功的 EOF

**严重度：P1。**

**位置：** `src-tauri/src/media.rs:594-608`、`:625-660`。

**触发条件：** 上游使用 `Transfer-Encoding: chunked`、无 `Content-Length`，在发送若干完整 chunk 后断开，尚未发送结束的零长度 chunk；或此类流在响应头后触发代理空闲超时。

最小响应：

```text
HTTP/1.1 200 OK\r\n
Content-Type: audio/mp4\r\n
Transfer-Encoding: chunked\r\n
Connection: close\r\n\r\n
4\r\nAUDI\r\n
<断开；缺少 0\r\n\r\n>
```

**实际行为：** reqwest 正确识别不完整上游并产出 body error。代理已经发送 200，删除了 `Transfer-Encoding` 且未提供新的分块 framing；此时下游靠连接关闭界定 body。错误分支只打印日志后 `break`，最后正常 `shutdown()` 并返回 `Ok(())`。下游因此把已收到的前缀视为完整成功的 HTTP body。完整 MP4 可能变成解码失败；渐进式可解码前缀也可能只播放部分内容。具体 WebView 是提前结束、卡住还是解码错误需要实机确认；HTTP 层丢失传输失败已被证明。

**对照输出：**

```text
A03 direct bytes() => Err (incomplete chunked body)
A03 proxy bytes() => Ok("AUDI"); HTTP 200, no Content-Length, no Transfer-Encoding
```

有可信 `Content-Length` 的前台断流会保留长度，下游可据此发现不完整；本项不泛化到所有响应。无长度的 fill 会拒绝提交，故本项主要影响前台流，不是另一个缓存半文件问题。

**建议修法：** 对无已知长度的下游使用正确的 HTTP/1.1 chunked framing，仅在上游真实成功 EOF 时写结束 chunk；上游失败/超时则不写结束 chunk，使下游同样感知不完整。响应头发出后不能再重发 502，也不能只改函数为 `Err` 却仍留下正常 close-delimited EOF。

**mac 验证：** 用有效音频的前半段构造缺结束 chunk 的上游，与完整 chunked 响应对照，记录媒体网络事件与错误。验收首先断言下游 HTTP 客户端读 body 失败，再检查 WKWebView 没把它当正常播完；不预先指定浏览器一定返回哪个错误码。

## A04 — 动态媒体白名单没有覆盖重定向校验

**严重度：P1。**

**位置：** `src-tauri/src/bili.rs:165-186`、`:268-272`、`:323-359`；`src-tauri/src/media.rs:455-475`、`:571-588`。

**触发条件：** playurl 返回的媒体主机已通过 `extract_and_register_playurl_hosts` 登记；该主机返回 302 到另一个也已登记、但不属于静态 B 站后缀的主机。即使只是同一动态主机的路径规范化跳转，也会遇到同样的判断差异。

**实际行为：** 初始请求经 `state.is_media_url_allowed()` 接受动态集合。共享 client 的 redirect policy 使用 `is_bili_redirect_allowed()`，只查询静态域名规则，不读取动态集合。结果产生非 connect/timeout 的 reqwest redirect error，前台直接回 502，后台 fill 同样失败。

**复现：** 用 `127.0.0.1` 和 `localhost` 模拟两台动态 CDN，二者都调用 `register_media_host`。初始请求连通，跳转目标的服务器正常，结果：

```text
A04 registered initial=127.0.0.1 registered destination=localhost; proxy=502 destination_requests=0
```

没有获取真实 B 站 playurl/观测生产 CDN 的跳转，因此**实际 CDN 是否正在触发此条件待验证**；动态登记与 redirect 的契约冲突已由生产 client builder 复现。

**建议修法：** 媒体前台和 fill 对每一跳使用一致的 URL 安全判定，包含 http(s)、无 userinfo、静态 CDN 或已登记主机，同时保留跳数上限。可以使用媒体专用 client/policy 或显式处理跳转。不要简单关闭 redirect 白名单，也不要把仅服务 API 的全局权限无条件扩大。

**mac 验证：** 在应用测试实例登记模拟主机，分别测试“动态主机到动态主机”“动态主机同域不同路径”“跳到未登记且不在静态白名单的主机”。前两者应播放；第三者仍须拒绝。随后检查真实 playurl 是否包含非静态主机及其重定向链。

## A05 — ADTS AAC 被标为 MP4，播放后果待实机验证

**严重度：P1。证据状态：MIME 错配已确认；严格 WebView 拒播待 mac 实机验证。**

**位置：** `src-tauri/src/media.rs:203`，本地调用 `:272-281`；扫描契约 `src-tauri/src/local_music.rs:13`、`:75-78`、`:99-115`；通用 MIME 修正路径 `src-tauri/src/media.rs:227`。

**触发条件：** 用户扫描并播放正常的原始 AAC/ADTS 文件 `track.aac`。本地音乐扫描显式接受 `.aac`，文件不是 ISO BMFF/MP4 容器。

**实际行为：** `local_audio_mime()` 将 `.aac` 与 `.m4a`、`.m4s`、`.mp4` 合并返回 `audio/mp4`；本地路由始终使用该结果。远程 `.aac` 上游缺 MIME/为 octet-stream 时也会走同一错误推断，不过 `.aac` 不属于磁盘缓存扩展名。本项已证实 Content-Type 与文件容器不符，不能把“这类文件已被扫描出来”当作“可被 WKWebView 播放”。

**真实夹具：** ffmpeg 生成 0.5 秒静音 ADTS AAC；ffprobe 输出：

```text
codec_name=aac
format_name=aac
```

复制到已登记本地目录，再经真实 `serve_http` / `serve_local_file` 路由请求：

```text
A05 local track.aac (ffprobe=aac/ADTS) -> content-type=audio/mp4, body magic=[ff, f1]; needs audio/aac
```

**建议修法：** 将原始 ADTS `.aac` 映射为 `audio/aac`；MP4 容器仍使用 `audio/mp4`。若要支持后缀与容器不符的文件，应依据实际格式识别，不能把所有 AAC 编码统一当作 MP4 容器。补充真实 ADTS AAC 与 AAC-in-MP4 两种夹具的响应测试。

**mac 验证：** 静音加载同一 ADTS 文件的两个测试响应，字节完全相同，只分别声明 `audio/mp4` 和 `audio/aac`。记录 `canPlayType()`、`loadedmetadata`、`canplay`、`audio.error`、`play()` 结果与拖动进度表现；同时加载 AAC-in-MP4 作为正常对照。若 WKWebView 容忍了 `audio/mp4`，则下调该平台播放影响，保留 MIME 错配修复；本审计未声称已观测到 mac 拒播。

## A06 — 后台 fill 的临时文件与并发没有共享预算

**严重度：P2。**

**位置：** `src-tauri/src/media.rs:506-508`；`src-tauri/src/media_cache.rs:115-136`、`:171-196`、`:225-236`。

**触发条件：** 多首不同路径未命中缓存，在下载尚未结束前快速切歌；每个上游都持续提供数据，单个文件小于等于 `max_bytes`，但总量超过预算。

**实际行为：** `in_progress` 仅去重同一个键，不限制不同键数量。fill 不持有供前台取消的句柄，切歌关闭前台 socket 不会停止之前的完整下载。`expected_len > max_bytes` 只限制单个文件；淘汰要到完整下载/提交后才运行，而且扫描只统计 `.bin`。多个 `.part` 可同时越过 1 GB，已完成 `.bin` 还可占另外一份预算。对于慢速但一直有数据的上游，30 秒 idle timeout 不会限制这些下载的总存活时长。

这与方案明确接受的“首播两份流量”分开：本项指出跨歌曲的累计并发和临时磁盘占用无上界；不把每次正常首播的双份流量另报为 bug，也不把能够最终完成的任务称为永久泄漏。

**缩小预算复现：** 使用原有 `with_limit` 设 16 B，三个不同键各返回 `Content-Length: 16`，分别先发送 12 B 后暂停，均未提交：

```text
A06 max_bytes=16 active_unique_fills=3 .part_total_bytes=36 (> budget); all fills still running
```

测试释放上游后全部任务正常结束，未留下测试任务。生产 1 GB 容量下结构相同；本审计没有实际写入数 GB 或进行长期压测。

**建议修法：** 给后台 fill 设独立的有限并发，并在接受任务时按预期大小预留共享字节预算，将 `.part` 与已提交缓存一起纳入容量控制。根据缓存策略取消或延后已过时的预取；可为后台完整下载设合理总时长/低速限制，保持前台按播放速度长时间读取的现有语义。

**mac 验证：** 测试上游限速，静音连续切换多首不同歌曲；观察后台上游连接数、切歌后传输量与 `audio-cache` 总占用（含 `.part`）。应在大量请求下仍保持明确的并发和磁盘上限；不要只检查 `.bin` 总和。

## 已核对而未报为缺陷的部分

- `media.rs:592-614` 保留上游非 2xx 状态；模拟 403、404、416、500 均原样返回且未入缓存。WebView 如何将非 2xx 映射为具体 `MediaError` 属于待实测行为，未据此捏造代理篡改状态的问题。
- `media_cache.rs:165-199` 拒绝非 200、带 `Content-Range`、无可验证长度、长度超限、流错误和长度不符。现有“断流不提交、重试可成功”的测试通过。
- `.part`/`.json.part`、JSON 提交标记和同键去重存在；原有并发测试通过。启动清理遗留 `.part` 的测试通过。
- lookup 在 storage lock 内打开文件后交给播放，现有 Linux 测试证明淘汰不破坏已打开读者；Windows 文件删除/替换仍需平台测试，未从 Linux 结果声称 Windows 已通过。
- 缓存命中时常规单区间、suffix range、越界 416、HEAD 均有原有测试。本审计没有把无实际浏览器输入证据的多区间 Range 限制扩成播放阻断。
- `allowed_media_hosts` 比通用 CDN 白名单更窄地控制缓存资格，是 `audio-cache.md` 明确要求的设计。未将这个差异本身误报为绕过或漏缓存；A04 是重定向集合不一致。
- 路径键忽略签名与镜像、保留质量编码，是设计明确选择。未找到 B 站真实 URL 的同路径不同资源证据，故未报告假定的碰撞。
- 本地文件使用 canonicalize 后按目录边界检查，现有路径白名单/Range 测试通过。未证明远程输入能绕过该检查。
- `main.rs:286-295` 只监听 IPv4 loopback，随机 UUID token 进入路径；请求检查 Host、token 和给定 Origin。未确认 token/CORS 绕过。
- 请求头读取限制 16 KiB、单次读取超时 5 秒；解析器对方法/HTTP 版本、重复 Host/Origin 和非法 Request URI 返回错误。未发现由普通非法 Range 或半包造成的 panic。单次读取超时不等于请求头总时限，且没有连接并发上限；本次未把需本机主动滥用的资源耗尽单独列为播放问题。
- 前台只在尚未发出下游响应头时重试连接/超时错误一次；没有在流已写出后重复响应头的路径。下游写入不设普通空闲上限以允许播放器背压，未把正常长播放误报成泄漏。

## 执行过的命令与关键输出

所有 shell 命令均先进入 `<repo>`。下列为复现所需命令及主要结果；只读定位还使用了 `git status --short`、`git rev-parse HEAD`、源码带行号输出。

### 依赖 feature 核验

```bash
cd <repo>
cargo tree --manifest-path src-tauri/Cargo.toml -e features -i reqwest@0.12.28 \
  > /tmp/biu-audit-A/reqwest-features.txt
```

实际图中 0.12.28 features 包括 `cookies`、`json`、`rustls-tls`、`socks`、`stream`；没有四种内容解压 feature。第一次不带版本的 `-i reqwest` 退出 101，提示同时存在 `reqwest@0.12.28` / `reqwest@0.13.5`；已改用 0.12.28 核对本模块使用的 client。

### 生成并识别真实音频夹具

```bash
cd <repo>
ffmpeg -hide_banner -loglevel error -f lavfi -i 'anullsrc=r=44100:cl=mono' \
  -t 0.5 -c:a aac -f adts /tmp/biu-audit-A/adts.aac
ffmpeg -hide_banner -loglevel error -f lavfi -i 'anullsrc=r=44100:cl=mono' \
  -t 0.5 -c:a aac -movflags frag_keyframe+empty_moov -f mp4 /tmp/biu-audit-A/valid.m4s
ffprobe -v error -show_entries format=format_name -show_entries stream=codec_name \
  -of default=noprint_wrappers=1 /tmp/biu-audit-A/valid.m4s
ffprobe -v error -show_entries format=format_name -show_entries stream=codec_name \
  -of default=noprint_wrappers=1 /tmp/biu-audit-A/adts.aac
file --mime-type /tmp/biu-audit-A/adts.aac
```

```text
codec_name=aac
format_name=mov,mp4,m4a,3gp,3g2,mj2
codec_name=aac
format_name=aac
/tmp/biu-audit-A/adts.aac: audio/x-hx-aac-adts
```

### 临时测试

```bash
cd <repo>
python3 /tmp/biu-audit-A/prepare.py
/tmp/biu-audit-A/harness --nocapture --test-threads=1
```

```text
Compiling unchanged media/cache/client logic (Tauri glue removed only)
CONTROL upstream 403/404/416/500 preserved, error responses not cached
test result: ok. 16 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.51s
```

各问题的实际观测值见前面 A01—A06。完整输出：`/tmp/biu-audit-A/results.log`。临时编译最初因多个已编译依赖的 rlib 不唯一退出；随后按应用 fingerprint 选取同版本/同 feature 依赖，成功编译运行，没有为解决编译而改任何生产逻辑。

### 原仓库完整测试

```bash
cd <repo>
cargo test --manifest-path src-tauri/Cargo.toml --locked
```

```text
Compiling biu v2.0.8 (<repo>/src-tauri)
Finished `test` profile [unoptimized + debuginfo] target(s) in 19.30s
running 40 tests
...
test result: ok. 40 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 18.63s
```

完整输出：`/tmp/biu-audit-A/cargo-test.log`。

## 未覆盖与交回事项

本次未操作 mac / Windows / Linux WebView 实机，未使用用户登录 cookie，未向真实 CDN 发请求；未跑 clippy、前端测试或做修复。以上模拟复现不能代替原生 WebView 的 codec 支持、媒体事件和错误码验证。

仓库内本任务仅新增本报告。开始审计时已有未跟踪 `docs/plans/quality-audit-2026-11.md`，保持不动。`cargo test` 正常写入项目已有的忽略构建目录，源码无差异。无 git commit。
