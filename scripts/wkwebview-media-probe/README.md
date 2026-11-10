# WKWebView 媒体响应探针（仅 macOS）

用真实 WKWebView 播放回环代理可能发出的各种响应（MIME、长度、分块、截断、错误状态），每个用例输出一行 JSON：`playResult`、`errorCode`、事件时间线。

```bash
scripts/wkwebview-media-probe/run.sh
```

需要 Xcode 命令行工具（`swiftc`）、`ffmpeg`、`python3`。锁屏时也能跑：轮询由 Swift 原生定时器驱动，不依赖页面定时器。

可选：把一段真实 B 站 DASH 音频保存为 `scripts/wkwebview-media-probe/real.m4s`（不要提交），会追加 `r1_real_octet` / `r2_real_mp4` 两个用例。

## 2026-11-10 结果（macOS 26.7.2）

| 用例 | 结果 |
|---|---|
| fMP4，`audio/mp4`，有长度 | 正常播完 |
| ffmpeg fMP4，`application/octet-stream` | 正常播完（WebKit 能嗅探出格式） |
| 真实 B 站 DASH `.m4s`（`ftyp` 兼容品牌含 `avc1`、`dash`），`application/octet-stream` | `NotSupportedError`，code 4 |
| 同一真实文件，`audio/mp4` | 正常播放 |
| 无 `Content-Length`（c3 chunked、c12 关闭连接结束），即使内容完整 | `NotSupportedError`，code 4；对照 c13（有长度、同样忽略 Range 回 200）正常，失败原因是缺长度 |
| ADTS `.aac` 标 `audio/mp4` / `audio/aac` | 失败 / 正常 |
| 403、502、`200 text/html` | 全部 `NotSupportedError`，code 4 |
| gzip 压缩体 + `Content-Encoding: gzip` | 正常播完 |

结论：WKWebView 把所有加载失败都报成 `NotSupportedError: The operation is not supported.`；B 站 CDN 返回的 `octet-stream` 对真实 DASH 分片无法嗅探，必须由代理给出 `audio/mp4`。
