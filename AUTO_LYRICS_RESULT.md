# 自动歌词 v2 — P1 交付结果（2026-10-29）

分支 `feat/auto-lyrics-p1`，本地提交 3 个（trailer `Executor: claude`），未 push：

| 提交 | 内容 |
|---|---|
| `901dc3f` | P1 主进程流水线：识别 → 并行取（网易云歌词、B 站原曲、视频音频）→ ffmpeg 解码 → utilityProcess 粗对齐（先发事件）→ 逐行；IPC `autoLyrics:*`；`--auto-lyrics-bench=<config>` 真机评测入口 |
| `b53b550` | 借用 #263 的标题候选函数（`shared/auto-lyrics/title-candidates.ts`，注明来源，只抄纯函数，不抄打分）；`parseLrc` 去掉“作词/作曲/编曲”等署名行；新增端到端 bench `pipeline.bench-run.ts` |
| `2e62b11` | Lyrics 组件最小接入：缓存未命中时自动触发，先显示粗对齐再换逐行；B 站字幕降为兜底；在搜索框手动选的歌词不会被晚到的自动结果覆盖；IPC 只需 `bvid+cid`，其余参数由主进程从 view 接口补齐 |

## 还能砍掉的重复活（对照 #263）

1. 标题清洗不再自己扩规则：#263 的 `buildTitleCandidates` 已经直接拿来生成补充查询（例如《雨爱》这种 2 字书名号），接受与否仍由 `identify.ts` 判定。
2. #263 的打分函数（`scoreNeteaseSong`/`isAcceptableMatch`）不抄：我们的规则会拒绝有歧义的结果，而且还有原曲时长和对齐置信度两道把关，比它严。
3. LRCLIB 兜底不做：没有原曲参考就不出词（拍板），LRCLIB 对 Live 没有帮助；#204 已经有手动搜索。
4. 桌面浮窗和顶部歌词栏不做：#249/#263/#264 三家都在做，等上游合入后自然会有展示面。
5. P2 里的设置开关、longtask 监测、“用这首歌对齐”按钮后移；现在只接 Lyrics 组件。

## 怎么验的（osa-a1，Node 22.17.1，`source /tmp/auto-lyrics-build-env.sh`）

| 命令 | 退出码 | 结果 |
|---|---|---|
| `pnpm vitest run` | 0 | 11 个文件，69 个用例通过 |
| `pnpm exec tsc --noEmit -p tsconfig.json` | 非 0 | 125 个错误，和上游 master 基线一样；新增和改动的文件没有新错误 |
| `pnpm exec eslint <改动文件>` | 0 | 无报错 |
| `pnpm build` | 0 | 6 个 Linux 包都打成功；日志有 `align worker (cjs) files: align-worker.cjs` |
| `AUTO_LYRICS_BENCH_DATA=~/gbc-auto-work/lyrics-proto pnpm bench:auto-lyrics` | 0 | 4 个文件全过（parity、synth、silver、pipeline），共 267 s |

端到端 bench（`scripts/auto-lyrics-bench/pipeline.bench-run.ts`）跑的是真实的 `AutoLyricsService`：识别规则、网易云 **lyric 接口实时请求**、原曲时长闸门、人声起点抽查、粗对齐事件和逐行 GapDTW 都是真的。替换掉的部分：B 站下载和 ffmpeg 换成本地 wav；网易云**搜索**换成 Mac 录下的元数据目录回放（原因见下）。

| Live | 状态 | 识别结果 | 粗对齐 offset | 置信度 | 行数 | 粗对齐事件 | 完成 |
|---|---|---|---|---|---|---|---|
| BV19T421S7nv | matched | 名もなき何もかも / トゲナシトゲアリ | 64.90 s | 0.73 | 42 | 2.98 s | 3.91 s |
| BV15t421K7Le | matched | 名もなき何もかも / トゲナシトゲアリ | -0.21 s | 0.72 | 42 | 2.09 s | 2.75 s |
| BV1QhieYjEY2 | matched | 空の箱 / トゲナシトゲアリ | 52.55 s | 0.93 | 32 | 2.15 s | 2.88 s |
| BV1dy75z8Enu | matched | 爆ぜて咲く / トゲナシトゲアリ | 5.27 s | 0.87 | 64 | 2.33 s | 3.17 s |

银标粗对齐（silver bench，和 P0 相同）：5 首里 4 首误差 ≤ 1 s（0.35 / 0.64 / 0.39 / 0.11 s），BV1rdp4efEEV 是 1.89 s。

端到端跑出来一个产品问题，已修：网易云 LRC 的署名行（作词、作曲、编曲）被当成歌词参与对齐，时间落在 58 s 这类位置。另外两处是 fixture 的问题，不涉及产品代码：搜索回放最初按子串包含匹配，3 首被判成 unidentified（推测是全标题查询命中了短歌名，造成歧义），改成精确匹配后恢复；原曲映射表没认出带后缀的曲名“空の箱 (井芹仁菜、河原木桃香)”。

## 还剩啥

- **Mac 真机端到端还没跑**（真实 B 站下载、ffmpeg、utilityProcess、Electron 内计时 A1/A2/A3）。osa-a1 跑不了，原因有两个：B 站接口返回 412；网易云 `api/search/get` 有地区限制，海外 IP 返回 “93”“29” 这类无关结果，lyric 接口正常。Mac 上还没有 biu 仓库，代码也没 push，所以要么授权 push 到 fork 的 `feat/auto-lyrics-p1`，要么把 git bundle 拷到 Mac，二选一。拿到代码后用 Electron 跑：`--auto-lyrics-bench=cases.json`，内容是 `{"outDir":"out","cases":[{"bvid":"BV15t421K7Le"},{"bvid":"BV1StSoBPEwu","page":2}],"identifyOnly":[]}`；pipeline bench 也可以加 `AUTO_LYRICS_NETEASE=live` 换成真实搜索。
- 端到端这 4 首没有银标，只能确认有输出、置信度在 0.72–0.93；准确度目前只有 silver bench 的 5 首证据。
- 识别 C1/C2/C4（104 条）还没算：评测 json 里没有视频标题，要在 Mac 上用 bench 入口的 `identifyOnly` 补拉 view 数据。
- `--auto-lyrics-bench` 入口没有限定只在开发构建里可用（要显式传参数才会触发），P2 收尾时决定要不要加开关。
- 合集和串烧（P4）、翻译和罗马音的验收（P3）、设置开关、E2 卡顿监测都没动。
