# Mac Tauri M0 验收记录

## 结论与 owner 决定

**Mac M0 尚未通过，不能据此批准 M1。**

2026-11-03 owner 决定：继续使用已有 `Biu M0.app`，不修复 DMG；S1 保持 `pending-owner (samples needed)`，DMG 体积和 S7 保持 **PARKED**。本轮已按此决定执行可用的运行时探针和 cargo bloat。`docs/plans/tauri-rewrite.md`、打包脚本和应用源码均未修改。

主要阻塞：自定义协议在成功完成初始播放/10 次 seek 后发生 non-unwinding panic，进程 SIGABRT；HTTP 退路在约 261 s 内未记录 `aac_playing` 或 `soak_start`，由执行者终止。因此没有完成 3600 s soak，没有 warmup 后 OLS 斜率。HTTP 这次退出为执行者 SIGTERM，不能描述为 HTTP 应用崩溃。

只读 `ioreg` 检查观察到 `CGSSessionScreenIsLocked=true`。没有尝试解锁、自动点击或绕过权限。锁屏与 HTTP 停滞的因果关系未确认；后续 controls 模式能在同一会话经本地 HTTP 播放 AAC，说明不能仅用锁屏解释 remote/expired 路径的停滞。S5 没有产生 `lyrics_frames`，不能给出动画 fps 通过结论。

## S1–S8 状态表

| 项目 | 状态 | 实测数字与限制 |
|---|---|---|
| S1 | pending-owner (samples needed) | 六个 fMP4 样本缺失，0/6 实测。没有改名伪造 fMP4。初始合成 AAC/原始 FLAC 不替代 S1。 |
| S2 | 未通过 / soak 阻塞 | Custom 初始 10 次 seek 为 218–228 ms，258 次 `range` 均 206，随后约 50.235 s 时 SIGABRT；最后观测 soak 时长 30.003 s。HTTP 261.028 s 内未进入 `aac_playing`/soak，主动终止。3600 s 未完成；两段 OLS 均 N/A。HTTP 新增原生 WebKit cohort + 主进程的观测 RSS 峰值 245.727232 MB，非完整 soak 总内存结论。 |
| S3 | not applicable | 不适用；Linux VM 矩阵待测。 |
| S4 | 部分完成；DMG PARKED | `.app` 文件逻辑总量 8,226,719 bytes / 8.226719 MB；交付 release binary 8,258,528 bytes / 8.258528 MB；app 内可执行文件 8,210,656 bytes / 8.210656 MB。cargo bloat 成功。DMG bytes/MB 与 ≤8/≤10 MB 判定 PARKED。 |
| S5 | API 探针完成；现场项待验证 | 80.936 s 探针记录 `lyrics_api` 和 60 s `lyrics_poll_alive`；没有 `lyrics_frames`，fps N/A。原生新增 XPC cohort + 主进程 ps CPU 快照均值 0.7375%、最大 4.6%，共 16 点；未证明动画运行，不能据此判 CPU<10% 通过。透明、点击穿透、悬停和全屏置顶待现场。 |
| S6 | 图片通过；webview 代理不通过；登录待验证 | 直接与 proxy 配置两轮 hdslb 图片均 `loaded=true`、180 px；两轮均 `controls_playing` 且 exit 0。代理探针 `proxy_image.loaded=false`，服务器无代理形式 `/art.svg` 请求。二维码/极验原型未实现，未使用真实凭据。 |
| S7 | PARKED | 按 owner 决定等 DMG 修复。未生成测试密钥，未构建 0.0.2，未安装或替换应用，未做更新。 |
| S8 | API 可用；系统集成待现场 / 待补 | `mediaSession=true`；custom 进入 seek 阶段，说明已越过固定标题/封面和四个 action handler 的注册。0 个实体媒体键动作，Now Playing 未验证。当前 handler 只记录 `media_action`，无显式播放/暂停/切歌操作；Mac souvlaki 接线未实现。 |

## 执行环境与传输

- 架构：`arm64`。
- macOS：`27.0.1`，build `26A434`。
- Xcode developer directory：`/Applications/Xcode.app/Contents/Developer`；Clang `21.0.0 (clang-2100.3.34.2)`。
- Homebrew `7.0.10`；Node `v26.10.0`；npm `11.19.1`；Python `3.14.6`。
- Rust `1.97.1`，Cargo `1.97.1`，`stable-aarch64-apple-darwin (default)`；Git `2.55.0`。
- WebKit framework：short version `22625`，build `22625.1.29.11.28`。运行时 UA 为 WebKit 兼容 UA，不能用其 Intel/10_15_7 字段替代真实 arm64/macOS 版本。
- 新 clone：`/Users/mianmian/Developer/biu-m0-transfer/biu-tauri-m0`，分支 `spike/tauri-m0`。现有 `/Users/mianmian/Developer/projects/biu` 未操作。

传输验证：

```text
biu-tauri-m0.bundle: OK
m0-inputs.tar.gz: OK
```

```text
9bdd4a9b47742372d69c18f0c576a6eb1368961794030c71b69422baa0874bca  biu-tauri-m0.bundle
a564da90964aff2ad2a6800d9aace2f150dfe1dfebb5d5b2d533615284f4968e  m0-inputs.tar.gz
```

原构建历史：Rust 编译成功，耗时 2m 48s；ad-hoc `.app` 生成成功；DMG 的 `bundle_dmg.sh` AppleScript 报 `Expected end of line but found class name. (-2741)`，build exit 1。`mac-build.log` 原样保留；本轮没有再次执行 DMG 构建。

## S2：自定义协议与 HTTP 退路

### Custom

实际命令由 `mac-run-s2.py` 调用，使用原 `.app`，环境为 `BIU_MODE=custom`、`BIU_SOAK_SECONDS=3600`、显式 fixture 开关。夹具服务器为独立 Python 进程，绑定 `127.0.0.1:18765`。原 `sample_memory.py --seconds 3600` 同时运行；应用提前退出时采样脚本按原行为结束。

- app PID：96190；exit -6（SIGABRT）；记录运行 50.234656 s。
- seed 2401；targets：`56, 26, 235, 162, 169, 255, 83, 100, 271, 154`。
- 对应 seek ms：`218, 225, 225, 223, 223, 226, 228, 223, 224, 225`，均 ≤1500 ms。
- AAC duration：299.975442 s；这是 WKWebView 对合成约 5 min 夹具的读数。
- waveform maximum：0.8001530766487122；AudioContext state `running`。
- 原始 FLAC duration 10.031020 s；时钟到 2.214487 s。没有把它当 fMP4 FLAC。
- 258 次 `range` 事件均为 206；119 次测试 `url_refresh`。日志是 custom `range`，没有套 HTTP-only checker。没有声称每次 seek 都一定发网请求；缓存和具体请求归属仍以原日志为准。
- 已记录 `soak_start`、1 个 `soak_reload`；最后 `soak` 时间为开始后 30.003 s。没有 `complete`。
- 原始终端 panic 摘要经 URL/UUID 脱敏后保存为 `mac-s2-custom-crash.txt`：`panic in a function that cannot unwind` / `thread caused non-unwinding panic. aborting.`。没有修改代码或试图修复。

### HTTP fallback

Custom 失败后改为 `BIU_MODE=full`，仍配置 3600 s，使用相同已有 `.app` 和独立 fixture server。

- app PID：97192；观测 261.027800 s。
- 2 次初始 HTTP Range，均 206：`bytes=0-1`（2 bytes）和 `bytes=0-4852593`（4,852,594 bytes）。这是初始装载；不把整段初始 Range 当成 seek 证据。
- 2 次测试 `url_refresh`；没有 `aac_playing`、seek、`soak_start` 或 `complete`。
- 曾用 `open -a` 激活已有测试 app，没有改变结果；未自动处理任何系统对话框。
- 在保存锁屏状态及未进入 soak 的事实后，执行者仅向该测试 app 发送 SIGTERM。exit -15 为主动停止，非崩溃。
- 原 `check_evidence.py playback` 实际退出 1：`AssertionError: no completion event`，见 `mac-check-playback.txt`。
- S5 的独立 full 模式 80.936 s 探针再次走到 remote/expired Range 下载，没有 `aac_playing`；controls 模式的本地 AAC 正常。这些现象保留给 owner 调查，没有换 URL 或改 harness 来绕开失败。

### 内存读取与斜率

**排除前 600 s 后的 OLS：N/A。最后 30 min 的 OLS：N/A。** 两轮都没有有效 3600 s soak，没有 warmup 后样本，不能对短序列计算一个数字冒充验收斜率。

已直接尝试并成功执行：`ps` RSS、`vmmap -summary <pid>`、`footprint -p <pid> -f bytes`，没有提权。GPU/Networking 的 lsof 可见测试 app 路径关联；WebContent 进程由 launchd 托管，直接路径关联不足，单独作为基线之后新增的 WebKit cohort 保存，未冒充全部确定归属的总内存。原有其他应用 WebKit 及 iOS Simulator WebKit 排除在 cohort 总和之外。

- Custom 原 parent-tree 样本 5 点，RSS peak 107,020,288 bytes，last 104,808,448 bytes。该树漏掉 launchd WebKit。
- HTTP 原 parent-tree 样本 26 点，RSS peak 105,906,176 bytes，last 59,523,072 bytes。
- Custom 主进程 + 已有路径关联 XPC 子集 peak 183,943,168 bytes；缺少两个 WebContent，非总内存。
- HTTP 主进程 + 已有路径关联 XPC 子集 peak 174,669,824 bytes；完整新增 native WebKit cohort 加主进程的观测 peak 245,727,232 bytes（245.727232 MB）。cohort 是观测集合，不保证所有进程均已完成确定归属。
- 每个被测进程的 `vmmap` 摘要、footprint 字节值、工具 exit 和 PID 信息在 `mac-memory-probes.json`；周期原始记录在 `mac-s2-*-memory.jsonl` 和 `mac-s2-*-xpc-memory.jsonl`。
- `launchctl procinfo` 报需 root，未尝试提权。只读 responsibility API 未返回可用 responsible PID，未据此推断归属。
- 未要求 owner 用 Activity Monitor 读 RSS，因为 ps/vmmap/footprint 已可读。有效 soak 和 WebContent 归属的后续证据仍需补齐。真实 CDN 续签始终未测试。

## S4：体积与 bloat

所有 MB 为 bytes / 1,000,000。体积在运行 bloat **之前**记录：

| 对象 | bytes | MB |
|---|---:|---:|
| `.app` 内 4 个普通文件逻辑总量 | 8,226,719 | 8.226719 |
| 原 release `target/release/biu-m0` | 8,258,528 | 8.258528 |
| `.app/Contents/MacOS/biu-m0` | 8,210,656 | 8.210656 |
| DMG | PARKED | PARKED |

`du -sk` 的 app 分配量为 8044 KiB；它与文件逻辑大小的口径不同。目录逻辑大小、裸 executable 和压缩下载包的门槛不同，不能用以上数字判 DMG ≤8 MB 或 ≤10 MB。

`cargo install cargo-bloat --locked` 成功，无 sudo，安装 `cargo-bloat 0.12.1` 到用户 home。`cargo bloat --release --crates -n 15` 成功，未调用 DMG 打包。前五名：tauri 904.8 KiB、std 754.6 KiB、lofty 426.1 KiB、tokio 375.4 KiB、rustfft 312.2 KiB。完整输出在 `mac-bloat.txt`；分析副本显示 file size 10.9 MiB，不能代替前述交付二进制大小。原 `.app` 未被 bloat 替换。

## S5、S6、S8 的无头边界

S5：使用 full 模式自动创建同一个 lyrics 原型入口，未自动点击 UI。API 返回透明/置顶/穿透设置和 100 ms 光标轮询；60 s 轮询心跳存在。80.936 s 观测中无 `lyrics_frames`。ps CPU 的 16 个点为启动及等待状态，不能用于宣布 60 fps / CPU<10% 通过。当前常显解锁按钮和完整状态机缺失的限制保持不变。

S6：无代理 controls 25.270 s、代理 controls 25.273 s 均 exit 0；两轮记录本地 HTTP AAC `controls_playing`、hdslb 图片 loaded true / 180 px。代理窗口记录 `proxy_image.loaded=false`。服务器只有 custom 阶段一条非 proxy 的 `/art.svg`（Referer 空），没有代理 `/art.svg`；没有把这条非 proxy 请求当作代理通过。webview 代理子项不通过；正式是否采用只代理 Rust 流量交 owner 按计划退路决定。二维码/极验原型未实现，未尝试外部浏览器登录代替。

S8：只确认 WebKit mediaSession API 存在，以及 custom 初始播放阶段已越过 metadata/action handler 注册。未验证 macOS UI 展示或物理媒体键。源码检查发现现有四个 handler 只 report `media_action`；不能声称它们已控制播放。Mac souvlaki 仍未接线。

## 单一 owner 待办清单

以下均为**待执行**，不计为本轮结果。S2、S8 的 remote 播放依赖 runtime blocker 处理或现场复核。不要修改 DMG 脚本。出现权限/许可/Gatekeeper/TCC 提示时执行者停止该步骤，由 owner 亲自检查决定，不使用 xattr 或安全配置绕过。

各 Terminal 先设置：

```bash
export PATH="/opt/homebrew/bin:$HOME/.cargo/bin:$PATH"
cd ~/Developer/biu-m0-transfer/biu-tauri-m0/spikes/m0
export BIU_SPIKE_FIXTURES=1 BIU_FIXTURES="$PWD/fixtures"
APP="$PWD/src-tauri/target/release/bundle/macos/Biu M0.app/Contents/MacOS/biu-m0"
```

1. **解锁屏幕并与 Claude（osa-a1）处理 S2 runtime blocker，然后补完整 soak。** Owner 亲自在 Mac 锁屏界面解锁。先查看 `mac-s2-custom-crash.txt`、`mac-s2-http.jsonl`、`mac-check-playback.txt`；不要把解锁视为已修复。打开第二个 Terminal 启动夹具服务器：

   ```bash
   cd ~/Developer/biu-m0-transfer/biu-tauri-m0/spikes
   python3 scripts/fixture_server.py --fixtures m0/fixtures --log evidence/mac-owner-server.jsonl
   ```

   Custom 已失败，现场可先复核 HTTP fallback。在设置好公共变量的 Terminal 执行：

   ```bash
   BIU_MODE=full BIU_SOAK_SECONDS=3600 BIU_LOG="$PWD/../evidence/mac-owner-s2-http.jsonl" "$APP" &
   APP_PID=$!
   python3 ../scripts/sample_memory.py --pid "$APP_PID" --seconds 3600 --out ../evidence/mac-owner-s2-http-memory.jsonl
   wait "$APP_PID"
   ```

   观察是否出现 `aac_playing`、10 次 seek、`soak_start` 和最终 `complete`。必须从 `soak_start` 实际满 3600 s；用无提权 ps/vmmap/footprint 补录 native WebKit XPC，不能只用 parent-tree RSS。再按 README 计算 warmup 600 s 后全段与最后 30 min 两个 OLS。若仍停在初始 Range 下载，保留现场结果交 Claude，不自行改协议、URL 或 harness。

2. **S1 提供样本并实际听音；同时补 S2 的扬声器确认。** 将六个有权使用、每首 >90 s 的 fMP4 放入 `fixtures/`，名称为 `flac-1.m4a`–`flac-3.m4a`、`dolby-1.m4a`–`dolby-3.m4a`。启动：

   ```bash
   BIU_MODE=interactive BIU_LOG="$PWD/../evidence/mac-owner-s1.jsonl" "$APP"
   ```

   在窗口选择一个样本 → 点击 `Custom protocol: 60 s + 5 seeks + waveform` → 等待 `sample_complete` → 听是否有声音/中断。逐首记录 codec、来源说明（不含签名 URL）、连续播放 ms、五次 seek ms、波形。S2 播放现场另记录实际音频输出。若出现 codec 失败，只采用 README 的 AAC/Dolby 退路。

3. **S5 透明、点击穿透、悬停、全屏置顶和动画。** 启动：

   ```bash
   BIU_MODE=interactive BIU_LOG="$PWD/../evidence/mac-owner-s5.jsonl" "$APP"
   ```

   点击 `Lyrics prototype` → 下方放普通窗口 → 检查透明边缘有无黑边 → 在锁定歌词上点击，看是否传给下方窗口 → 移入/移出左上 100×60 区域，记录可操作区域响应 ms → 用另一应用的绿色窗口按钮进入全屏，观察歌词是否仍置顶。保持实际可见动画 60 s，收集 `lyrics_frames`；用无提权 ps 采样相关进程 CPU。完整解锁状态机缺失必须继续标未完成，不用 API 成功代替交互通过。

4. **S8 Now Playing 与实体媒体键。** 在 S2 remote 播放可用后启动（夹具服务器保持运行）：

   ```bash
   BIU_MODE=full BIU_SOAK_SECONDS=3600 BIU_LOG="$PWD/../evidence/mac-owner-s8.jsonl" "$APP"
   ```

   出现 `aac_playing` 后打开 macOS Control Center → Now Playing，记录标题、艺术家、封面。依次按实体 Pause、Play、Previous、Next，逐个记录 `media_action`、前端时钟停止/继续和实际声音。当前 handler 只记录事件，若事件到了但播放状态未变也必须记失败。没有稳定播放时不判 Now Playing 通过。不要用 Mac 尚未接线的 souvlaki 推断退路通过。

S6 代理失败和二维码/极验缺页已交技术 owner；无需 owner 为本轮重复屏幕操作。S7 和 DMG 继续 PARKED，解除需另有 owner 决定。没有额外的 Activity Monitor 待办。

## 证据、安全与验证

保留首轮构建/环境/传输日志。本轮新增 `mac-s2-*` 原始事件、parent-tree/XPC RSS、运行记录、脱敏 crash、`mac-session-state.json`、`mac-memory-probes.json`、`mac-runtime-summary.json`、S5/S6 探针事件/CPU 记录、fixture server 事件、bloat 输出与安装日志；采集脚本仅放在 `evidence/`，未改变应用测试逻辑。

`mac-s2-custom-xpc-cohort.jsonl` 是 custom 退出后补采工具的一条 app 已退出记录，不用于 RSS 峰值、斜率或验收。有效 HTTP cohort 样本保存在原 `mac-s2-http-xpc-memory.jsonl` 的 `cohort_processes` / `cohort_rss_bytes`。

只暂存明确列出的 evidence 文件。原始 stdout、内存工具完整输出和服务器 PID 等留在忽略的 `m0/private/`；不提交音频、产物、钥匙、签名或 private。没有生成更新密钥或改动安装目录。没有 push、merge、tag、分支删除、发布或对外回传。没有提权、权限审批或安全绕过。未启动其他 agent，不宣称独立代码审查或 Opus 最终验收通过。

原构建日志两处 create-dmg 尾随空格和 bloat 构建日志的末尾空行均原样保留；whitespace 验证仅排除这两个原始工具日志。完成记录时已检查所有新 JSON/JSONL 可解析、十次 seek 和 Range/206 数量、S6 loaded true/false 与服务器证据、既有 `.app` 大小和源码/方案未变。功能 checker 的未通过结果保持原样，未伪装通过。
