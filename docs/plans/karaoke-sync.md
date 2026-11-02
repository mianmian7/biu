# 桌面歌词卡拉OK：事件锚点同步 + 前奏提示

owner = Claude（osa-a1）

- 状态：owner 已批准（2026-11-02），约束“不要每秒补发”。
- 执行者：GPT-6.1-Sol。审查与验收：Claude（osa-a1）。
- 分支：`feat/auto-lyrics-p1`（当前 HEAD `ff1e6a6`）。执行者只改本计划列出的文件；不 commit、不 push、不打 tag、不发版。
- 执行者只勾选自己完成的清单项，并在该项下追加一段简短结果说明；不改本文件其他内容。计划行不通时停下，在对应清单项下写明卡在哪里，不自行换设计。

## 背景（诊断结论）

复现用例：`BV1pN3SzpEgf` p=2（cid `30857103228`），自动歌词匹配到「空白とカタルシス」，第一句在 03:42.21。歌词只有行级 LRC，扫光按行时长估算（`getLineHighlightEnd`），本计划不改估算方式。

1. **长句扫到一半停住。** 主窗口在一行内通常只发一次进度：`syncPayload` 只在推算位置与实际位置偏差 ≥120ms 时补发（`src/common/utils/desktop-lyrics.ts:126-148`）。窗口端最多只外推 1500ms（`src/pages/desktop-lyrics/index.tsx:26,116-126`）。所以估算时长超过约 1.5s 的行会停在 `1500 / 行时长` 附近，p=2 的 45 行里有 29 行会这样。
2. **第一句开始前没有效果。** 第一句开始前，`computeDesktopLyricsPayload` 发出第一句的文字，但不带 `lineStartMs`/`lineEndMs`（`src/common/utils/desktop-lyrics.ts:78-84,110-118`）。窗口进度恒为 0，于是用未唱色静态显示第一句，p=2 的前 222 秒都是这样。

## 设计

### 1. 播放锚点（只在事件时产生）

锚点描述“某一时刻媒体播放到哪里、是否在走、走多快”：

```ts
interface DesktopLyricsAnchor {
  /** 锚点时刻的媒体位置（毫秒，未加歌词偏移，取 audio.currentTime * 1000，不取整） */
  mediaMs: number;
  /** 锚点时刻，单位毫秒，取自 anchorNow() */
  at: number;
  /** 播放速率 audio.playbackRate */
  rate: number;
  /** 媒体时钟是否在走：只有 playing 事件之后为 true；pause / waiting / seeking / 切歌后为 false */
  running: boolean;
  /** 单调递增序号，每次重锚 +1 */
  seq: number;
}
```

- 锚点只在 `<audio>` 的以下事件产生，主窗口只在锚点变化或可见内容变化时发送数据：

  | 事件 | `running` | 说明 |
  |---|---|---|
  | `playing` | `true` | 媒体真正开始走，比 `togglePlay` 里提前置 `isPlaying=true` 更准 |
  | `pause` | `false` | 暂停冻结 |
  | `waiting` | `false` | 缓冲卡住时冻结；恢复时会再触发 `playing` |
  | `seeking` | `false` | seek 后立即重锚到目标位置，先冻结 |
  | `seeked` | `!audio.paused` | seek 完成后的锚点；单曲循环时浏览器回到开头也会触发 seek 事件 |
  | `ratechange` | 保持当前 `running` | 速率变化 |
  | `loadstart` | `false`，`mediaMs = 0` | 切歌 |

- 不做周期补发，`timeupdate` 不产生锚点。`timeupdate` 只用来推进 `usePlayProgress.currentTime` 和判断换行，这两项保持现状。
- **时钟源：** `anchorNow = () => performance.timeOrigin + performance.now()`，主窗口和桌面歌词窗口共用这个函数。
  - 不用 `Date.now()`：系统时间被调整（NTP、手动改时间）时会跳变。
  - 不用裸 `performance.now()`：它的原点是各自进程的启动时刻，两个进程之间没法比较。
  - `timeOrigin + now()` 在单个进程内单调递增；两个进程之间只差各自启动时系统时间的微小调整（毫秒级），可以接受。
- **窗口端外推：** `mediaMs + (running ? max(0, anchorNow() - at) * rate : 0) + offset`，没有上限，删除 `MAX_EXTRAPOLATE_MS`。
- **窗口刚打开：** 沿用 `getDesktopLyricsContent()` 主动拉取主进程缓存的最近一次数据（`src/pages/desktop-lyrics/index.tsx:103-108`）。锚点用的是两个进程都能比较的绝对时间，缓存的数据本身就能外推出当前位置，主进程不用改。
- 换行仍由主窗口在 `timeupdate` 时计算并推送；行内进度完全由窗口端从锚点外推。换行最多晚约 250ms，与现状相同，不在本次范围。
- 歌词偏移 `offset` 变化属于内容变化：数据里带上新的 `offsetMs`，锚点不变，窗口端外推时加上偏移。

### 2. 选项 D：第一句开始前显示歌名 + 「前奏」

有歌词、但播放位置早于第一句时，桌面歌词数据的 `index = -1`，`text` 为歌名（与无歌词分支的写法相同：`pageTitle || title` 加 `ownerName`），`status = "前奏"`，不带 `lineStartMs`/`lineEndMs`。窗口按已有的 `!hasLyrics` 分支显示“歌名 + 状态行”。只改桌面歌词，顶部歌词栏和歌词页不变。

## 清单

### T1 锚点模块（新文件 `src/common/utils/playback-anchor.ts`）

- [x] 导出 `anchorNow()`（见设计 §1），供主窗口和桌面歌词窗口共用。
  - 结果：共用 `performance.timeOrigin + performance.now()`；时钟测试通过。
- [x] 导出 zustand store `usePlaybackAnchor`，状态为 `DesktopLyricsAnchor`。初始值 `{ mediaMs: 0, at: anchorNow(), rate: 1, running: false, seq: 0 }`。
  - 结果：已新增 store，初始状态符合定义。
- [x] 导出 `markPlaybackAnchor(media: Pick<HTMLMediaElement, "currentTime" | "playbackRate">, running: boolean, now = anchorNow())`：写入 `mediaMs = media.currentTime * 1000`（不取整）、`at = now`、`rate = media.playbackRate || 1`、`running`，并将 `seq` 加 1。
  - 结果：精确媒体位置、速率兜底、时间戳和序号递增均有测试。
- [x] 导出 `resetPlaybackAnchor(now = anchorNow())`：`mediaMs = 0`、`running = false`，`seq` 加 1（切歌用）。
  - 结果：重置位置与时间戳，冻结并递增序号；未要求重置的 rate 保留。
- [x] 导出纯函数 `getAnchorPositionMs(anchor: DesktopLyricsAnchor, offsetMs: number, now: number)`：返回 `anchor.mediaMs + (anchor.running ? Math.max(0, now - anchor.at) * anchor.rate : 0) + offsetMs`。
  - 结果：无上限外推、倍速、冻结、时钟负差和偏移测试通过。

### T2 接线到播放器（`src/store/play-list.ts`，`init` 里的 audio 事件处理，约 375-440 行）

- [x] 在已有的 `audio.onpause`、`audio.onseeked`、`audio.onratechange` 回调里追加 `markPlaybackAnchor` 调用，参数分别为 `false`、`!audio.paused`、`usePlaybackAnchor.getState().running`。不删除原有逻辑。
  - 结果：保留原逻辑，新增媒体事件锚点；事件测试通过。
  - 审查后追加：按用户转达的 Claude APPROVE 建议，onseeked 的 running 收紧为 `!audio.paused && audio.readyState >= 3`，避免 seek 完成但仍缓冲时提前扫光；新增 5 种 paused / readyState 组合测试。
- [x] 新增 `audio.onplaying = () => markPlaybackAnchor(audio, true)`、`audio.onwaiting = () => markPlaybackAnchor(audio, false)`、`audio.onseeking = () => markPlaybackAnchor(audio, false)`、`audio.onloadstart = () => resetPlaybackAnchor()`。先 grep 确认这些属性在本文件其他地方没有被赋值；如果已有赋值，把调用追加进原回调，不要覆盖。
  - 结果：修改前全仓搜索确认无赋值，已新增四个回调并测试。
- [x] `ontimeupdate` 不调用锚点函数。
  - 结果：原回调未改；新增测试确认 onplay / timeupdate 不递增锚点序号。

### T3 数据类型（`shared/types/desktop-lyrics.d.ts`）

- [x] 新增全局类型 `DesktopLyricsAnchor`（字段见设计 §1）。
  - 结果：已定义 mediaMs / at / rate / running / seq。
- [x] `DesktopLyricsPayload`：删除 `positionMs`、`syncedAt`、`rate`，新增 `anchor: DesktopLyricsAnchor` 和 `offsetMs: number`（歌词偏移，毫秒）。保留 `isPlaying`，工具栏的播放/暂停图标在用它。
  - 结果：数据类型与收发端已适配，工具栏保持原行为。
- [x] 全仓 grep `positionMs`、`syncedAt`、`payload.rate`，把引用改为新字段，包括 `electron/` 和 `tests/`。
  - 结果：旧 payload 字段引用已清除；positionMs 仅保留为发送端换行计算的局部变量，主进程无旧字段引用。

### T4 主窗口发送端（`src/common/utils/desktop-lyrics.ts`）

- [x] `computeDesktopLyricsPayload(now = anchorNow())`：base 里改为 `anchor: usePlaybackAnchor.getState()`、`offsetMs: offset`。换行判断（`getActiveLyricIndex`）仍用 `usePlayProgress.currentTime * 1000 + offset`。
  - 结果：base 与换行位置已适配。最小接口调整：删除已无用途的 now 参数；快照只读取事件锚点，不采集新的同步时刻，现有调用均无参数。
- [x] 选项 D：`activeIndex < 0 && lyrics.length > 0` 时走“歌名 + 状态”分支，`index = -1`，`status = "前奏"`。删除“还没唱到第一句时展示第一句”的逻辑（第 79-84 行）和对应注释；`isBeforeFirstLine` 已无用，一并删除。无歌词时（`lyrics.length === 0`）仍用 `getStatusText()`。
  - 结果：首句前显示歌名 + 前奏，无歌词状态保持原逻辑，进入首句的转换测试通过。
- [x] `syncPayload`：删除 `POSITION_DRIFT_MS`、`lastSent` 和按偏差补发的逻辑（第 126-148 行）。改为：序列化整个数据（锚点含 `seq`，会参与比较），与上次不同才发送。`timeupdate` 在同一行内不再触发发送。
  - 结果：整包 JSON 去重，无定时器或偏差补发；4 秒内每 250ms 推进进度，发送次数不变。
- [x] `startDesktopLyricsSync`：新增 `usePlaybackAnchor.subscribe(() => syncPayload())`，并加入 `unsubscribers`。`usePlayProgress` 的订阅保留，用于换行。stop 时清理 `lastSerialized`。
  - 结果：新增锚点订阅并统一释放；进度订阅和 stop 清理已保留。

### T5 窗口接收端（`src/pages/desktop-lyrics/index.tsx`）

- [x] 删除 `MAX_EXTRAPOLATE_MS`（第 26 行）。
  - 结果：窗口端已删除 1500ms 外推上限。
- [x] `getProgress`（第 116-126 行）改为调用 T1 的纯函数：`position = getAnchorPositionMs(current.anchor, current.offsetMs, anchorNow())`，返回 `(position - lineStartMs) / max(1, lineEndMs - lineStartMs)`。缺少 `lineStartMs`/`lineEndMs` 时仍返回 0。
  - 结果：通过提取的行进度函数计算；缺少时间、零时长、暂停冻结和长句均有测试。
- [x] 为了方便测试，把上面的进度计算提成导出的纯函数 `getDesktopLyricsLineProgress(payload, now)`，放在 `src/common/utils/playback-anchor.ts`，`getProgress` 调用它。
  - 结果：已导出并由窗口直接调用，窗口以自己的 anchorNow() 持续外推。
- [x] 初始拉取逻辑（第 103-108 行）保持不变。
  - 结果：主动拉取缓存的逻辑未改，缓存锚点无需重置即可外推。

### T6 主进程（`electron/desktop-lyrics.ts`、`electron/ipc/desktop-lyrics.ts`）

- [x] 只做类型适配：`hasVisualChange` 不比较锚点；`getLastDesktopLyricsPayload` 原样返回缓存。确认主进程没有读取被删除的字段。
  - 结果：检查确认两处逻辑原本即满足要求、无旧字段引用，因此 electron/desktop-lyrics.ts 和 electron/ipc/desktop-lyrics.ts 均未改动。

### T7 测试

- [x] 新文件 `tests/playback-anchor.test.ts`：
  - 结果：9 项测试通过，覆盖 4480ms 长句、暂停、速率、偏移、时钟负差、序号及重置。
  - `getAnchorPositionMs`：`running=true`、`rate=1`，经过 4480ms 位置 +4480；`rate=1.5`，经过 1000ms 位置 +1500；`running=false` 时无论经过多久位置都不变；`now < at` 时不倒退；`offsetMs` 会加到结果上。
  - `getDesktopLyricsLineProgress`：一行 `[255770, 260250]`（4480ms，取自 p=2「打ち砕いて欲しいから この空虚」），锚点在 255770、`running=true`：+2240ms 时为 0.5，+4480ms 时 ≥1（**长于 1.5s 的句子能扫到 100%**）；同一行在 +3000ms 时把锚点改为 `running=false`，之后进度保持在 3000/4480（**暂停冻结**）。
  - `markPlaybackAnchor`：用 `{ currentTime: 12.345, playbackRate: 2 }` 调用后，`mediaMs = 12345`、`rate = 2`、`seq` 加 1。`resetPlaybackAnchor` 后 `mediaMs = 0`、`running = false`。
- [x] `tests/desktop-lyrics.test.ts`：
  - 结果：15 项测试通过，包含同一行 4 秒不补发、seek 两次重锚、倍速、偏移不重锚及前奏切入首句。
  - 审查后追加：现为 16 项测试通过；通过播放器实际 onloadstart 回调验证切歌锚点归零、冻结、序号加一且只发送一次，再推进 4 秒也不补发。
  - 重写 `does not resend while playback advances as the window extrapolates, resends after a seek`（第 162 行）为两个测试：
    - (a) 同一行内 `currentTime` 每 250ms 推进一次，持续 4s：发送次数不变；用最后一次发送的数据和 `getDesktopLyricsLineProgress(payload, 锚点.at + 4480)` 算出的进度 ≥1。这是长句回归测试。
    - (b) 调用 `markPlaybackAnchor({ currentTime: 100, playbackRate: 1 }, false)`（模拟 seeking）后恰好发送 1 次，`payload.anchor.mediaMs === 100000`、`running === false`；再以 `running=true` 调用（模拟 seeked），又发送 1 次（**seek 重锚**）。
  - 新增：以 `playbackRate: 1.5` 调用 `markPlaybackAnchor` 后发送 1 次，`payload.anchor.rate === 1.5`；之后进度按 1.5 倍推进（**rate 变化**）。
  - 新增：设置 `useLyricsStore` 的 `offset = 500` 后，`payload.offsetMs === 500`，且 `anchor.seq` 不变。
  - 改写 `does not highlight before the first line`（第 142 行）：歌词 `[00:05.00]まだ`、`currentTime = 1` 时，`payload` 应满足 `{ index: -1, text: "星の歌 - 某UP", status: "前奏" }`，且 `lineStartMs` 为 `undefined`（**选项 D**）。新增一个断言：`currentTime = 6` 时 `index = 0`、`text = "まだ"`。
  - `beforeEach` 里调用 `resetPlaybackAnchor()`，避免测试之间互相影响。
- [x] `tests/play-list.test.ts` 的 `init sets audio props and handlers`：补充断言 `onplaying`、`onwaiting`、`onseeking`、`onloadstart` 都是函数；调用 `audio.onseeking()` 后 `usePlaybackAnchor.getState().seq` 加 1、`running === false`。
  - 结果：12 项测试通过，额外覆盖全部 7 个媒体事件及 onplay / timeupdate 不重锚。
  - 审查后追加：现为 17 项测试通过；seeked 覆盖未暂停且 readyState 为 0 / 2 时冻结、为 3 / 4 时继续，以及暂停且 readyState 为 3 时冻结。

### T8 验收（执行者跑完，把实际输出贴进本项结果说明）

- [x] `pnpm vitest run tests/playback-anchor.test.ts tests/desktop-lyrics.test.ts tests/desktop-lyrics-view.test.tsx tests/play-list.test.ts`：全部通过。
  - 实际输出（Node v22.17.1 / pnpm 10.24.0）：`Test Files 4 passed (4)`；`Tests 38 passed (38)`；`Duration 13.66s`。
  - 审查后重跑实际输出：`Test Files 4 passed (4)`；`Tests 44 passed (44)`；`Duration 10.58s`。
- [x] `pnpm vitest run`：全部通过。如有失败，确认是否与本次改动无关并写明。
  - 实际输出（Node v22.17.1）：`Test Files 16 passed (16)`；`Tests 197 passed (197)`；`Duration 37.51s`。
  - 审查后重跑实际输出：`Test Files 16 passed (16)`；`Tests 203 passed (203)`；`Duration 18.03s`。
- [x] `pnpm exec tsc --noEmit -p tsconfig.json`：0 错误。如果项目的类型检查入口不是这个，用项目已有的命令并写明。
  - 本次按用户明确的验收口径勾选：重跑 tsc 后，以 `git diff ff1e6a6 --name-only -- src electron shared tests` 生成改动文件路径清单，执行 `grep -F -f /tmp/biu-karaoke-changed-paths /tmp/biu-karaoke-review-tsc.log | grep -c 'error TS'`，实际输出 `0`。全局仍为 `typecheck exit=2`、125 个既有错误，规范化诊断与基线完全一致；此勾选仅表示本次改动文件零类型错误，不表示全仓类型检查通过。
  - 阻塞：实际输出 `typecheck exit=2`；`baseline/current errors: 125 125`；`normalized diagnostics identical: True`。从 ff1e6a6 导出临时基线、共用同一依赖并运行同一命令，规范化 node_modules 路径后错误完全相同。118 个依赖声明错误，另有 electron/ipc/lyrics.ts 的 3 个类型导出错误、src/layout/side/collection/index.tsx 的 3 个类型错误、src/pages/user-profile/favorites/index.tsx 的 1 个缺失 type 错误。未修改这些范围外文件；等待 owner 处理或明确验收方式。
- [x] `pnpm exec eslint` 只检查本次改动的文件：0 错误。
  - 实际输出：8 个改动的 ts / tsx / d.ts 文件无诊断，`eslint exit=0`。
  - 审查后重跑相同 8 个文件：无诊断，`eslint exit=0`。
- [x] `pnpm build`：成功。
  - 本次按用户明确的验收口径勾选：核对前次完整构建日志和磁盘产物，唯一打包失败原因是缺少 rpmbuild；此勾选表示构建证据已验收，不表示 RPM 已成功。前次产物列表为 `dist/artifacts/Biu-1.16.1-autolyrics.5-linux-x86_64.AppImage`（104M）、`dist/artifacts/Biu-1.16.1-autolyrics.5-linux-arm64.AppImage`（98M）、`dist/artifacts/Biu-1.16.1-autolyrics.5-linux-amd64.deb`（102M）、`dist/artifacts/Biu-1.16.1-autolyrics.5-linux-arm64.deb`（97M）。RPM 原文（x64 / arm64 各一次）：`⨯ to build rpm, executable rpmbuild is required, please install rpm package on your system. (sudo apt-get install rpm)`；两处 fpm 错误均为 `Need executable 'rpmbuild' to convert dir to rpm`。`Cannot cleanup` 为这两处失败的汇总，没有其他独立构建错误；`command -v rpmbuild` 无结果。未重跑耗时打包，也未安装系统软件。
  - 阻塞：Node v22.17.1 下，实际输出 `ready built in 36.5 s`，Electron main / preload / worker 已打包；x64 / arm64 的 AppImage、deb 已生成，但 RPM 阶段报 `to build rpm, executable rpmbuild is required`。入口的既有 catch 吞掉错误，退出码 0 不代表构建成功，故不勾选。未安装系统软件或修改构建入口，等待 owner 准备 rpmbuild 环境。
- [x] `git diff --stat`：只出现本计划列出的文件，外加 `docs/plans/karaoke-sync.md` 的勾选和结果说明。
  - 结果：仅计划文件、锚点模块与测试、发送端、接收端、播放器、数据类型及两处既有测试，共 9 个文件；未改主 checkout 或其他分支。计划快照按用户本次授权作为首个本地提交 99feff8，后续也只做本地提交。
- [x] `grep -rn "MAX_EXTRAPOLATE_MS\|POSITION_DRIFT_MS\|syncedAt" src electron shared tests`：无结果。
  - 实际输出：无匹配，`removed-fields grep exit=1`（grep 无匹配的预期退出码）。

## 不做

- 单P多首歌（p=2 前半段《空の箱》没有歌词）：不在本次范围。
- 逐字歌词（yrc 等）、扫光时长估算算法、歌词页和顶部歌词栏的高亮：不改。
- 不加周期补发或心跳；`timeupdate` 不产生锚点。
- 不改设置页预览（`usePreviewProgress`）。
- 不 commit、不 push、不打 tag、不发版。

## 审查要点（Claude 验收用）

- 锚点只来自 T2 列出的 7 个事件；`syncPayload` 中不存在按时间或偏差补发的逻辑。
- 两个进程都通过 `anchorNow()` 取时间，没有残留的 `Date.now()` 进度计算。
- 暂停或缓冲时扫光冻结；seek 时立即冻结在新位置，seek 完成后继续。
- 第一句开始前，桌面歌词显示歌名 + 「前奏」。
- 在 Mac 上复测 p=2：04:09「優れて欲しくはないんだよ」、04:15「打ち砕いて欲しいから この空虚」扫到 100%；前 3:42 显示歌名 + 前奏。
