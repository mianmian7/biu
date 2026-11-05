# 桌面歌词卡拉OK：换句与进度用同一时钟

owner = Claude（osa-a1）
状态：已批准（2026-11-05，用户指定由 Claude 编写）
分支：`feat/tauri`

## 问题
用户反馈：进入下一句歌词时，高亮经常不是从行首开始，而是已经走了一段，有时直接从中间开始。

## 诊断（已核对代码）
1. 换句和进度使用两个不同的时钟。
   - 进度：桌面歌词窗口每帧用锚点外推（`getDesktopLyricsLineProgress`，`src/common/utils/playback-anchor.ts`）。
   - 换句：主窗口在 `computeDesktopLyricsPayload`（`src/common/utils/desktop-lyrics.ts`）里用 `usePlayProgress.currentTime` 计算 `index`。这个值只在 `audio.ontimeupdate` 时更新，WebKit 大约 250ms 一次，之后还要经过一次跨窗口传递。
   - 结果：新的一句总是晚 0–250ms 以上才显示，显示时进度已经按准确时钟走了一段。句子越短越明显。
2. 锚点只在 play / pause / seek / waiting 等事件时记录，`ontimeupdate` 不校准。外推时钟和音频时钟之间的偏差会一直累积，直到下一个事件；偏差越大，新一句开始时的位置越靠中间。

## 设计
### 1. 锚点漂移校准（主窗口）
`src/store/play-list.ts` 的 `audio.ontimeupdate`：当 `!audio.paused` 且锚点 `running` 时，计算 `drift = getAnchorPositionMs(anchor, 0, anchorNow()) - audio.currentTime * 1000`。`|drift| > 120` 时调用 `markPlaybackAnchor(audio, true)`。阈值以内不动，避免每次 `timeupdate` 都重发 payload。每次校准写一条 `frontend_log`（info，带 drift 值），同一首歌最多记 20 条，用于确认真实漂移幅度。

### 2. 桌面歌词窗口自己换句
- `DesktopLyricsPayload` 新增 `upcoming?: { index, text, translation?, romaji?, nextText?, lineStartMs, lineEndMs }`，内容是下一句（当前句为 `index + 1`；前奏状态 `index = -1` 时为第一句）。由 `computeDesktopLyricsPayload` 生成，复用 `getLineHighlightEnd` 和 `getCompanionByTime`。
- `src/common/utils/playback-anchor.ts` 新增纯函数 `resolveDisplayedLine(payload, now)`：用 `getAnchorPositionMs` 计算当前位置；当位置 `>= upcoming.lineStartMs` 时，返回由 `upcoming` 组成的当前行（index、文本、起止时间），否则返回 payload 自身的当前行。
- `src/pages/desktop-lyrics/index.tsx`：渲染和 `getProgress` 都改为基于 `resolveDisplayedLine` 的结果。用 rAF 检测显示的 index 是否变化，变化时触发重渲染，不额外加定时器。主窗口之后发来的 payload 与本地结果一致，不会闪回。
- 跳转（seek）后锚点 `seq` 变化，主窗口会发新的 payload，以新 payload 为准。

### 修订 1（2026-11-05，回应 Sol 代码审查的两项 P2）
- `upcoming` 改为数组，带当前句之后最多 8 句（`DESKTOP_LYRICS_UPCOMING_LINES`）。`resolveDisplayedLine` 取其中最后一个已开始的句子（同一时间的多句取最后一句，与 `getActiveLyricIndex` 一致），并把其后的句子留在返回值里，rAF 继续换句，直到没有后续句。
- `seek()` 先设置 `audio.currentTime` 并记录暂停锚点，再更新 `usePlayProgress`，桌面歌词不会收到“新行号 + 旧锚点”的内容。

### 3. 不改的部分
`getLineHighlightEnd` 的行时长估算、双行交替布局（index 奇偶）、前奏显示、翻译和罗马音的取法都不变。主进程（Rust）只透传 payload，不需要改。

## 清单
- [x] T1 锚点漂移校准（设计 §1）。
  - 结果：`correctAnchorDrift` 在 `ontimeupdate` 里校准，`onloadstart` 重置锚点和日志计数。Linux 实测日志出现 `[anchor] re-anchored after 1698ms drift at 52ms`。
- [x] T2 payload 增加 `upcoming`，更新 `shared/types/desktop-lyrics.d.ts`（设计 §2）。
  - 结果：`buildUpcomingLine` 生成 `upcoming`，前奏状态给第一句。
- [x] T3 `resolveDisplayedLine`，以及桌面歌词窗口改用它（设计 §2）。
  - 结果：桌面歌词窗口用 rAF 等到 `upcoming.lineStartMs` 后自行换句，进度按显示行计算。
- [x] T4 测试：`tests/playback-anchor.test.ts` 覆盖 `resolveDisplayedLine` 的换句边界（换句前 1ms、换句当刻、跳过 upcoming 后的 seek 场景）；`tests/play-list.test.ts` 覆盖漂移超过 / 未超过阈值两种情况；`tests/desktop-lyrics.test.ts` 覆盖 `upcoming` 的生成（含前奏状态）。
  - 结果：新增 5 个 `resolveDisplayedLine` 用例、1 个 `upcoming` 用例、4 个漂移阈值用例。
- [x] T5 验收：`npx vitest run` 全部通过；`npx tsc --noEmit -p .` 的错误数不比改动前多（改动前为 118）；`cd src-tauri && cargo test` 通过。
  - 结果：vitest 20 文件 226 用例通过；tsc 错误 118（与改动前相同）；cargo test 30 通过。Linux 录屏（Xvfb，2.5s 一句的测试歌词，15 句）：每句走满所需时间修复前 0.67s，修复后 2.40–2.43s。
- [ ] 代码逻辑审查（GPT-6.1-Sol）
  - 第 1 轮（2cb0f82c）：2 项 P2（向后 seek 闪错行；一次同步间隔内跨多句）。已按修订 1 修复，待第 2 轮复审。
- [ ] 视觉/动态审查（Gemini 4 Argon）：在 Mac 上录一首有逐行歌词的歌，至少 1 分钟，用连续帧检查每次换句时高亮是否从行首开始。
- [ ] 最终验收（Opus）
