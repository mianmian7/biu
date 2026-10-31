# 自动歌词 v2 实施计划

状态：草案（2026-10-28），等群里审阅；未提交。依据 `docs/requirements-auto-lyrics.md`（含 §2.1 拍板）和 `docs/research-auto-lyrics.md`（含 §5 V1–V3 结果）。

## 0. 前提

| 项 | 第一版的做法 |
|---|---|
| 内容 | 单曲（MV、原曲搬运、Live）+ 分 P 合集；单 P 串烧放在 P4 |
| Live | 以粗对齐为准，逐行尽力（不卡 B1'） |
| 共享缓存 | 不做，只用本地缓存 |
| 没有原曲 / 识别失败 | 不自动出词，提示用户走现有的手动搜索 |
| 内存（E1） | 只记录，不设门槛 |
| 播放不卡（E2） | 必须达到：0 次卡顿，渲染进程没有超过 200 ms 的长任务 |
| 发布 | 只发 fork；上游代码要能继续合并进来，所以新代码尽量放在新文件里，改动现有文件只做接线 |

## 1. 架构

```
渲染进程                         主进程                                       utilityProcess
Lyrics 组件 ──IPC──▶ AutoLyricsService（队列、取消、预取、缓存）
  ▲  进度事件                    ├─ identify：规则识别（AI 可选，P5）
  │                              ├─ 歌词：网易云 search + lyric（复用 api/netease-lyric.ts）
  │                              ├─ 参考：B 站 wbi 搜原曲 + 打分 + 版本校验
  │                              ├─ 音频：view → playurl（复用 api/dash-url.ts）→ 下载
  │                              ├─ 解码：内置 ffmpeg 子进程 → 22050 Hz 单声道 PCM
  └──────────── 结果 ────────────┴─ 对齐 ─────────────────────────────▶ features + 粗对齐 + 逐行（纯 TS）
```

关键取舍：

1. **解码用 Biu 内置的 ffmpeg**（`electron/ffmpeg/*`，四个平台已经随安装包分发，路径解析见 `electron/utils.ts`），在主进程里起子进程，输出 `-f s16le -ac 1 -ar 22050` 的 PCM。这样解码不在渲染进程里，不碰 WebAudio 的原生内存，也不需要 WebCodecs。Mac 上用 ffmpeg 命令行解一首约 0.25 s（实测）。在 Electron 里的耗时 P1 要实测；如果明显变慢，退回已经实测过的 `decodeAudioData` 方案（V3）。
2. **计算放在 utilityProcess**，不在渲染进程，避免影响播放和 UI（E2）。PCM 用 `MessagePort` 传过去。
3. **算法代码放在 `shared/auto-lyrics/`，用纯 TS、零依赖**。主进程、utilityProcess 和 vitest 都能直接引用，评测脚本也用同一份代码。
4. **识别规则移植自旧版**（`biu-archive` 分支 `main` 的 `tools/gbc-sync/gbc_sync/auto.py`：`clean`、`title_variants`、`identify_rules`、`choose_rule`、`LIVE`/`COLLECTION`/`NON_SONG` 等正则），只移植规则，不带 Python 架构。

## 2. 新增与改动的文件

新增：

| 路径 | 内容 |
|---|---|
| `shared/auto-lyrics/fft.ts`、`features.ts` | STFT、CENS、onset（移植自原型 `align/src.v1/features.js`） |
| `shared/auto-lyrics/align.ts` | `estimateGlobalOffset`、`alignLinesGapDTW`、`findMedleySongs`（移植自 `src.v1/dtw.js`；`alignLinesWithRepeats` 在真实 Live 上没有收益，暂不移植） |
| `shared/auto-lyrics/lrc.ts` | LRC 解析与生成；翻译、罗马音按行对齐 |
| `shared/auto-lyrics/identify.ts` | 标题清洗、候选打分、Live/合集/非歌曲判断（纯函数） |
| `shared/auto-lyrics/reference.ts` | 原曲候选打分（时长差、关键词、版本校验） |
| `shared/auto-lyrics/types.ts` | 任务参数、结果、缓存条目（zod schema） |
| `electron/auto-lyrics/service.ts` | 任务队列：切歌时取消，预取下一首，写缓存 |
| `electron/auto-lyrics/sources.ts` | 网易云、B 站搜索、playurl、下载（复用 `electron/ipc/api/*`） |
| `electron/auto-lyrics/decode.ts` | ffmpeg 子进程解码成 PCM |
| `electron/auto-lyrics/align-worker.ts` | utilityProcess 入口 |
| `electron/ipc/auto-lyrics.ts` | IPC handler |
| `src/store/auto-lyrics.ts` | 渲染进程状态（每个 bvid-cid 的阶段、结果、错误） |
| `src/components/lyrics/use-auto-lyrics.ts` | Lyrics 组件用的 hook |
| `scripts/auto-lyrics-bench/` | 评测脚本（不打包）：合成金标、银标、识别评测、计时 |
| `tests/auto-lyrics/*.test.ts` | 单元测试 |

改动（只做接线）：`electron/ipc/channel.ts`、`electron/ipc/index.ts`、`electron/preload.ts`、`shared/types/lyrics.d.ts`（缓存字段）、`src/components/lyrics/index.tsx`（接入 hook 和状态显示）、`src/components/full-screen-player/settings-panel.tsx` 与设置 store（开关）、`plugins/electron-build.ts`（只有 utilityProcess 入口需要单独打包时才改）。

缓存：沿用 `StoreNameMap.LyricsCache`，键仍是 `bvid-cid`。`MusicLyrics` 增加可选字段 `source: "auto" | "manual"`、`algoVersion`、`songId`、`refBvid`、`confidence`。用户手动修改过的条目（`source: "manual"` 或者调过 offset）不会被自动结果覆盖（优先级：用户修正 > 自动 > B 站字幕）。

## 3. 阶段

每个阶段单独开一个分支提交，合到 `feat/auto-lyrics-v2`。每个阶段结束都要跑 `pnpm vitest run`、`pnpm exec eslint <改动的文件>`、`pnpm exec tsc --noEmit -p tsconfig.json` 和 `pnpm build`，并贴出实际输出（package.json 里没有 lint 脚本，eslint 平时由 lint-staged 调用）。上游 master（`7d49920`）本身就有 125 个 tsc 错误，所以 tsc 的门禁是“新增文件 0 个错误，总数不增加”。

### P0 算法移植和评测脚本（不接 UI）

- 把原型 `src.v1` 移植成 `shared/auto-lyrics/` 下的 TS，行为保持不变。
- `scripts/auto-lyrics-bench/`：合成金标、银标（含 §5.2 的评测口径修正）、4 个 bench Live；数据目录通过环境变量 `AUTO_LYRICS_BENCH_DATA` 指定，数据不进仓库。
- 单元测试：用几秒的合成正弦或和弦信号，测粗对齐偏移、逐行单调性、LRC 解析。

验收：
- TS 版和原型在合成金标、5 个银标上的输出逐行差 ≤ 10 ms（同样的输入）。
- 合成金标：中位数 ≤ 300 ms，P90 ≤ 700 ms，覆盖率 ≥ 90%，MC 误显示 ≤ 1 行。
- 银标粗对齐：至少 4/5 首 ≤ 1 s。

### P1 主进程流水线（无 UI，用开发入口触发）

- `identify.ts` 移植规则；`reference.ts` 加版本校验：原曲时长要和网易云时长对上；用 LRC 首句时间附近的能量起伏抽查，排除 DECORATOR 那种街机版或剪辑版。
- `sources.ts`：B 站搜索复用 `encodeParamsWbi`，遇到 `v_voucher` 风控时退避重试；playurl 选低码率音频（V3 用约 53 kbps，下载约 1.1 MB）。
- `decode.ts`：ffmpeg 解码，输出 PCM 流；同时记录耗时和峰值内存。
- `service.ts`：一首歌的流程是识别 → 并行取（歌词、原曲、视频音频）→ 解码 → 粗对齐（先发事件）→ 逐行（再发事件）。切歌就取消任务；当前歌完成后再预取下一首。
- 开发入口：只在开发构建里暴露 `autoLyrics:debugRun`，让评测脚本能在真实 Electron 里跑 12 个案例和 5 个银标。

验收（Mac 真机）：
- A1：粗对齐 P90 ≤ 3 s；A2：单曲逐行 P90 ≤ 15 s；A3：Live 逐行 P90 ≤ 30 s。冷启动单独报告。
- E1：只记录峰值内存，报告里写明解码方式（ffmpeg 或 decodeAudioData）。
- 识别（只用规则）：在 `identify_eval_results-1.16.2.json` 的 104 条上算出 C1（命中率 ≥ 80%）、C2（认错率 ≤ 8%）、C4（误触发率 ≤ 5%）。达不到的话，先报告差距，再决定是调规则还是把 AI 提前（P5）。
- 原曲：12 个案例的原曲选择全部人工核对过（版本对、时长对）；没有原曲时返回“无参考”，不出歌词。

### P2 UI 接入

- `use-auto-lyrics.ts` 接到 `Lyrics`：没有缓存时自动触发；先显示粗对齐，再无缝换成逐行；置信度低的行弱化显示。
- 状态：识别中、对齐中、失败原因（未识别、无原曲、非音乐）。失败时显示“手动搜索”入口，打开现有的 `LyricsSearchModal`。
- 设置：增加“自动歌词”开关，默认开。非音乐视频（按分区和标题规则判断）静默跳过。
- 手动修正：在搜索框里手动选了歌，就用这首歌重新对齐（新增“用这首歌自动对齐”按钮）；offset 沿用现有逻辑，用户改过就标记为 manual。

验收：
- 12 个案例在 Mac 上从点播放到看到歌词，录屏或日志里的时间戳满足 A1、A2。
- E2：对齐期间 0 次播放卡顿，渲染进程没有超过 200 ms 的长任务（用 `PerformanceObserver` 记录 longtask，写进日志）。
- 手动搜索、手动选歌、调 offset 之后重启，结果仍然保留，不会被自动结果覆盖。
- 新增的组件和 hook 有 vitest 测试：缓存命中、失败时回退到手动、用户修正优先。

### P3 翻译和罗马音

- 翻译：用网易云 `tlyric`，按时间映射到对齐后的行；没有就不显示（AI 补翻译放在 P5）。
- 罗马音：先用网易云 `romalrc`。缺的时候本地生成：在 kuroshiro + kuromoji、其他 JS 方案之间实测选型，要求日语歌的显示率 D2 = 100%、打包体积增量（E3）≤ 25 MB；词典可以首次使用时再下载。
- 界面：翻译和罗马音各自有开关，沿用现有的翻译显示位置。

验收：bench 里所有日语歌 D2 = 100%；有 `tlyric` 的歌 D1 = 100%；记录安装包体积变化。

### P4 合集和串烧

- 分 P 合集：每个分 P 单独识别（分 P 标题优先），P1 已经覆盖，这里只验收。
- 单 P 串烧：从简介和置顶评论里解析时间轴歌单；解析不出来就让用户编辑歌单（增加、删除、排序）；然后用 `findMedleySongs` 定位每首，再逐首对齐。
- 测试样本：`BV1rL4y1H7Ab`（A-SOUL，16 首，置顶评论里有时间轴）、`BV1Eh411Z7kp`（周深，简介里有歌单），再补 1 个日语串烧。

验收：B5，每首起止误差 ≤ 3 s，判对率 ≥ 90%（以评论时间轴作为参考）；漏检的歌要计入。

### P5 AI（可选）

- 设置里可以填一个 OpenAI 兼容的 base URL 和 key，不填就完全不用 AI。
- 用途：规则识别失败时做识别（C3 ≥ 90%），以及补缺的翻译。
- 验收：C3；不配 key 时行为和 P1–P4 完全一样。

### P6 跨平台和发布

- 真机：Apple 芯片 Mac（mac）、Windows（pc）、Linux（wsl）上都跑 P1 的计时和 P2 的 E2 检查。
- Intel Mac：用 GitHub Actions 的 Intel macOS runner（具体 runner 名称以当时文档为准）打包，并跑无头对齐冒烟测试（`scripts/auto-lyrics-bench` 的合成样本子集）。
- 体积：记录安装包体积变化（E3）。
- 发布：在 fork 上发预发布版本，由用户确认后再转正式版。提交、推送、发布各自都需要授权。

## 4. 需求指标落在哪个阶段

| 指标 | 阶段 | 第一版是否作为门禁 |
|---|---|---|
| A1、A2、A3 | P1、P2 | 是 |
| A4、F3（共享缓存） | — | 否，第一版不做共享缓存 |
| B1、B3、B4（金标） | P0 | 是 |
| B1'（银标逐行） | P0 起持续记录 | 否，Live 以粗对齐为准 |
| B2（粗对齐） | P0、P1 | 是 |
| B5（串烧） | P4 | 是（P4 内） |
| C1、C2、C4 | P1 | 是 |
| C3 | P5 | 是（P5 内） |
| D1、D2 | P3 | 是 |
| E1 | P1 起持续记录 | 否，只观测 |
| E2 | P2 | 是 |
| E3 | P3、P6 | 是 |
| E4 | P1 | 是（首次使用只下载音频和歌词） |
| F1 | P6 | 是 |
| F2 | — | 不适用（没有服务端） |

## 5. 分工建议

| 阶段 | 建议执行者 | 原因 |
|---|---|---|
| P0 | Pi（Gemini/Grok）或 Codex | 机械移植，有现成的对照数据 |
| P1 | Codex 主写；Mac 上的实测派给 Mac 上的 T3 线程或 subagent | 跨文件、要接 Electron |
| P2 | Codex 或 Sonnet subagent | UI 接线，加测试 |
| P3、P4、P5 | 可以并行 | 彼此独立 |
| 验收 | 协调者（Claude） | 跑验收命令，核对数字 |

## 6. 风险

| 风险 | 应对 |
|---|---|
| B 站搜索风控（`v_voucher`）或偶尔变慢（V3 里出现过 3.9 s、7.1 s） | 退避重试；同一首歌的原曲选择缓存在本地（按网易云 songId）；搜索超时就先只显示“对齐中” |
| 选错原曲版本（DECORATOR） | `reference.ts` 做版本校验；粗对齐置信度太低时不出词 |
| ffmpeg 解码在某个平台上失败 | 回退到 `decodeAudioData`（已经实测可用） |
| 规则识别率不够 | P1 先量化差距；必要时把 P5 的 AI 识别提前 |
| Live 里额外多唱的段对不上 | 第一版接受；置信度低的行弱化显示 |
| 上游改动和接线冲突 | 新代码放新文件，接线点尽量少 |
