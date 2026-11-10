# B：前端播放状态机审计

审计日期：2026-11-10。基线：`audit/quality-2026-11-10` / `c6037b70`（v2.0.8，包含 `5a09d5cc`）。

只读检查源码；本次仅新增本报告。临时测试、配置和日志位于 `/tmp/biu-audit-B/`，没有向仓库添加测试或修改方案。

## 问题表（按严重度排序）

| ID | 严重度 | 问题 | 主要位置 | 证据状态 |
|---|---|---|---|---|
| B01 | P1 | 续播/错误刷新不检查请求归属，旧曲响应覆盖新曲音源及持久化条目 | `src/store/play-list.ts:349-430,1067-1108` | 两条异步时序测试复现 |
| B02 | P1 | 没有媒体 `error` 事件处理；开始播放后的网络/解码失败没有恢复或终止状态 | `src/store/play-list.ts:444-552,256-282` | 事件处理缺口和状态测试确认；真实媒体故障时序待 mac 实机验证 |
| B03 | P1 | 播放意图与实际状态混用：拒播后仍显示播放，加载期间点暂停也不能取消 | `src/store/play-list.ts:618-638,256-282,207-218` | 三种 DOMException + 双击时序测试复现 |
| B04 | P1 | 删除“下一首”或替换播放列表后保留旧 `nextId`，随后切入不存在的条目 | `src/store/play-list.ts:726-755,964-1020,1052-1061` | 删除、整表替换两条测试复现 |
| B05 | P1 | 播放位置不绑定曲目且不保存零，重启恢复旧位置到当前曲目 | `src/store/play-progress.ts:12-25`；`src/store/play-list.ts:561-564,1022-1040`；`src/app.tsx:201-208` | 持久化恢复测试复现 |
| B06 | P1 | 已有曲目设为“下一首”时插到当前曲目前面，顺序播放会回到刚播过的当前曲 | `src/store/play-list.ts:847-856` | 队列重排测试复现 |
| B08 | P1 | `NotSupportedError` 刷新音源时丢失当前播放/seek 位置，恢复后从头播放 | `src/store/play-list.ts:267-272,1075-1104` | 源码 + 媒体 src 重置模型确认；真实 WKWebView 待实机验证 |
| B07 | P2 | 有效 URL/本地音频切歌分支提前返回，系统 Now Playing 元数据停留在上一首 | `src/store/play-list.ts:1179-1188` | MediaSession 状态测试复现 |

没有发现能证明“所有曲目在所有会话均无法播放”的 P0。以上 P1 均有具体的错误功能或局部播放中断场景。

## 验证方法与结果

临时测试导入真实的 `usePlayList`、`usePlayProgress`、`isUrlValid`、媒体 URL 包装和播放锚点逻辑；只模拟网络数据、日志/上报和 toast。音频对象沿用仓库 `tests/setup.ts` 的 `FakeAudio`。没有调用真实 B 站 API，也没有真实媒体解码或 WKWebView 实测。

测试断言描述应有行为。失败表示复现了缺陷，不能读成“修复验收失败”。B08 额外模拟浏览器切换 `src` 时重置 `currentTime` 的副作用；仓库原始 `FakeAudio` 不模拟这个行为。B02 模拟播放已开始后的 `error` 事件入口，不声称已在真机重现每一种错误码的浏览器事件顺序。

所有 shell 命令均从以下目录执行：

```bash
cd <repo>
```

现有测试：

```bash
npx vitest run tests/play-list.test.ts tests/playback-anchor.test.ts
```

实际输出（退出码 0）：

```text
✓ tests/playback-anchor.test.ts (16 tests)
✓ tests/play-list.test.ts (24 tests)
Test Files  2 passed (2)
     Tests  40 passed (40)
```

审计复现测试（全部位于 `/tmp`，不需要复制文件到仓库）：

```bash
FORCE_COLOR=0 NO_COLOR=1 npx vitest run \
  --config /tmp/biu-audit-B/vitest.config.mts playback-audit.test.ts
```

最终实际输出（退出码 1）：

```text
❯ playback-audit.test.ts (18 tests | 15 failed)
Test Files  1 failed (1)
     Tests  15 failed | 3 passed (18)
  Duration  1.75s
```

同一组 18 项测试在前两轮也得到 `15 failed | 3 passed`。最终一轮将 B02 的锚点初始化改为调用真实 `audio.onplaying`，结果不变。

三条通过的对照测试分别证明：普通切歌订阅的旧请求会被丢弃；上次启动的代理 URL 会重新包装且能读取内层 deadline；迷你窗口续播不会调用 `audio.play()`。因此 B01 指向的是现有保护未覆盖的另外两条路径。

证据文件：

- `/tmp/biu-audit-B/playback-audit.test.ts`
- `/tmp/biu-audit-B/vitest.config.mts`
- `/tmp/biu-audit-B/vitest-baseline.log`
- `/tmp/biu-audit-B/vitest-red-1.log`
- `/tmp/biu-audit-B/vitest-red-2.log`
- `/tmp/biu-audit-B/vitest-red-final.log`

首次运行时临时配置使用了不可解析的绝对 `vitest/config` 包目录导入，出现启动错误。随后仅修正 `/tmp` 配置为包导入并连接已有 `node_modules`，以上均为实际执行测试后的结果。

## B01 — 续播与错误刷新会把旧曲音源写进新曲

**严重度：P1。**

**代码证据：**

- `src/store/play-list.ts:349-430`：`ensureAudioSrcValid()` 先捕获当前条目，然后等待 `toMediaUrl` / `getDashUrl` / `getAudioUrl`，返回后直接写 `audio.src`。在线刷新写列表时用的是返回时的 `state.playId`，没有核对最初条目的 ID。
- `src/store/play-list.ts:1067-1108`：`refreshCurrentAudioSource()` 同样捕获旧条目、等待请求，然后写入当前音频对象和返回时的当前列表项。
- `src/store/play-list.ts:1152-1154` 的 `playSwitchId` 只保护切歌订阅。前述两个函数和 `playAudioSafely()` 的重试不共享这项保护。
- `src/store/play-list.ts:1058` 将被写坏的 `list` 持久化。

**触发步骤：**

1. A 暂停后链接过期，点击播放，使 `ensureAudioSrcValid()` 请求 A 的新链接；让该请求延迟返回。
2. 在 A 的请求返回前切到 B，等待 B 开始播放。
3. A 的旧请求返回。
4. 另一条入口：A 的 `play()` 抛出 `NotSupportedError`，刷新请求尚未完成时切到 B，再让 A 的刷新返回。

**实际后果：**界面和 `playId` 仍为 B，实际音源和 B 条目里保存的 URL 都变成 A。后续重新播放 B 仍会使用 A 的有效链接，错误能跨重启保留。启动 `init()` 的链接刷新与用户切歌重叠时也存在相同缺口。

测试实际输出：

```text
B01 {
  playId: 'b',
  src: 'https://cdn.test/a-refreshed.m4s?deadline=4102444800',
  currentItemUrl: 'https://cdn.test/a-refreshed.m4s?deadline=4102444800'
}
B01 refresh {
  playId: 'b',
  src: 'https://cdn.test/a-retry.m4s?deadline=4102444800',
  currentItemUrl: 'https://cdn.test/a-retry.m4s?deadline=4102444800'
}
```

**建议修法：**让切歌、续播、启动恢复和错误重试共享播放请求代际；捕获 `playId` 和代际，每个 `await` 后核对。写列表时按捕获的 ID 查找，并再次确认它仍为当前项。过期请求不得写音源、重试播放或更新元数据。

## B02 — 播放开始后的媒体错误没有处理入口

**严重度：P1。媒体故障的真实事件顺序待实机验证。**

**代码证据：**

- `src/store/play-list.ts:444-552` 注册了 duration、timeupdate、playing、waiting、seek、play、pause、ended 等处理器，没有注册 `audio.onerror`。
- 全范围调用检查未找到另一个给核心 `audio` 注册 `error` 监听的组件。`src/components/audio-waveform/index.tsx:129-130` 只监听 play/pause。
- `src/store/play-list.ts:256-282` 的恢复仅存在于 `audio.play()` Promise 的 catch；该 Promise 一旦兑现，后来的媒体错误不会再次拒绝它。
- `next()` 只从 `ended` 等显式操作进入；媒体失败不等同于正常 `ended`。

**触发步骤：**

1. 播放未完整缓冲的长曲目，等待 `play()` 成功。
2. 拖到未缓冲位置，令后续 Range 请求断流或返回不可解码的媒体内容，触发 `MEDIA_ERR_NETWORK`（2）或 `MEDIA_ERR_DECODE`（3）。播放期间签名过期导致后续请求被拒也属于需要覆盖的路径。
3. 不主动手动切歌。

**实际后果：**前端没有刷新/重试、失败提示或终止状态处理，也不会因这个错误自动进入下一首。若浏览器没有另发 pause/waiting 事件，`isPlaying` 和歌词运行锚点还会保留播放状态；即使浏览器另发这些事件，恢复入口仍然缺失。

模拟事件输出（code 2、3 均相同）：

```text
B02 {
  code: 2,
  errorHandler: 'undefined',
  playId: 'a',
  isPlaying: true,
  anchorRunning: true,
  refreshCalls: 0,
  toasts: 0
}
```

code 4 的事件入口测试也显示没有处理器。但首次 `play()` 因不支持资源而拒绝时，现有 `NotSupportedError` catch 会执行一次刷新；不能将这条已存在的入口表述为完全没有恢复。

**建议修法：**增加按当前播放代际归属的 `error` 事件入口，与 Promise 拒绝共用一套有上限的恢复策略，并避免同一失败被两条入口重复恢复。失败时冻结锚点、校正播放状态；网络错误可刷新 URL，解码/格式错误可降级或换源；达到上限后给出明确停止/跳过策略，避免全列表循环重试。

**mac 验证步骤：**

1. 用 macOS Tauri 开发构建，打开 Web Inspector，记录核心音频的 error/waiting/pause/ended 事件及 `audio.error.code`。
2. 使用未缓存完的长曲目；播放成功后拖到缓冲区外，临时断网至媒体报错，再恢复网络。测试前确认实际读的是未缓存资源。
3. 当实际得到 code 2/3 时，检查是否发出刷新请求、是否切歌、是否有错误提示，以及主窗口/迷你播放器/桌面歌词是否仍显示播放。
4. 记录 macOS/WebKit 版本、错误码、事件顺序和缓冲范围。若只有 waiting/stalled，尚未构成本条 error 场景的实机复现。

## B03 — 拒播状态不回滚，加载中的暂停意图丢失

**严重度：P1。**

**代码证据：**

- `src/store/play-list.ts:627-632` 在获取链接和 `play()` 成功之前直接置 `isPlaying = true`。
- `src/store/play-list.ts:256-282` 捕获失败后只处理日志/toast，没有返回成功标记或纠正 `isPlaying`。
- `src/store/play-list.ts:627,633-637` 按 `audio.paused` 决定播放/暂停；请求链接时原生音频还在 paused，第二次点击会再次走“播放”。
- `src/store/play-list.ts:215-216` 用 `error.name` 识别 AbortError，却用 message 包含 `NotAllowed` 判断权限失败。WKWebView 的 message 不保证包含异常名称。

**触发步骤 A：**暂停后点击播放，让浏览器抛出 `NotAllowedError`；或让 `NotSupportedError` 刷新后的第二次播放仍失败；或让当前请求发生非切歌引起的 `AbortError`。

**实际后果 A：**原生音频仍 paused，界面却显示正在播放；状态会同步给迷你播放器和桌面歌词。测试分别得到：

```text
B03 { name: 'NotAllowedError', isPlaying: true, paused: true, toasts: 1 }
B03 { name: 'NotSupportedError', isPlaying: true, paused: true, toasts: 1 }
B03 { name: 'AbortError', isPlaying: true, paused: true, toasts: 0 }
```

**触发步骤 B：**暂停的 A 链接过期；点击播放，等待新链接期间点击已经显示出来的暂停按钮；然后让链接请求返回。

**实际后果 B：**第二次点击再次请求播放链接；两个请求完成后照常播放，用户的暂停意图没有生效。输出：

```text
B03 pending pause { isPlaying: true, paused: false, refreshCalls: 2 }
```

**建议修法：**分开存储播放意图、加载状态和媒体实际状态。暂停操作必须使在途播放意图失效；成功播放事件再确认 `isPlaying`，失败时按当前代际回滚，不能把旧请求的 AbortError 用来暂停新曲。异常分类统一检查 `DOMException.name`，权限失败应提供需要用户交互的明确状态。

## B04 — 失效 nextId 使播放进入不存在的条目

**严重度：P1。**

**代码证据：**

- `src/store/play-list.ts:749-755` 只判断 `nextId` 非空，未验证它存在于当前列表。
- `src/store/play-list.ts:964-1020` 的 `delPage` / `del` 删除列表项，没有清理指向该项的 `nextId`。
- `src/store/play-list.ts:726-736` 整表替换只更新 `list` / `playId`，没有清空旧 `nextId`。
- `src/store/play-list.ts:1060` 还会持久化这个悬空引用。

**触发步骤 A：**队列 A/B/C，播放 A；把 B 设为下一首；删除 B；点击下一首或等待 A 播放完。

**触发步骤 B：**把 B 设为下一首；通过“播放全部”换成 X/Y；随后切下一首。

**实际后果：**`playId` 被设为已不在列表中的 B，订阅会暂停当前音频却找不到新条目，播放停止。UI 的当前曲为空。随机模式再次 next 还可能在 `src/store/play-list.ts:786-787` 读取不存在的 `currentPlayItem.pageIndex`。

```text
B04 delete { playId: 'b', ids: [ 'a', 'c' ], currentItem: undefined, paused: true }
B04 replace { playId: 'b', titles: [ 'x', 'y' ], currentItem: undefined }
```

**建议修法：**删项时同步清除失效 `nextId`，整表替换时重置它；`next()` 消费前再次验证。持久化恢复也应校验 `playId` / `nextId` 属于列表。随机模式对失效当前索引做防御。

## B05 — 零位置没有持久化，旧位置被用于新曲

**严重度：P1。**

**代码证据：**

- `src/store/play-progress.ts:22-25` 只保存 truthy 的位置，不保存零。
- `src/app.tsx:201-205` 退出回调也先判断 truthy，零位置甚至不会调用保存。
- `src/store/play-list.ts:1022-1040` 的 clear 只把内存进度置零，没有清除旧 `play-current-time`。
- `src/store/play-progress.ts:12-18` 保存的是单个数值，没有曲目 ID；`src/store/play-list.ts:561-564` 恢复时无条件用于当时的当前曲。

**触发步骤：**

1. A 播到 120 秒并正常退出一次，磁盘留下 `play-current-time=120`。
2. 再打开后把 A 拖回 0 秒并保持暂停，或清空 A 后换一首新曲并在 0 秒暂停。
3. 正常退出，再启动。

**实际后果：**0 没有覆盖旧值；同曲会跳回旧位置，换过曲目则可能把 A 的位置用到新曲。临时测试执行了保存旧位置、clear、换曲、init 恢复全过程：

```text
B05 { saved: '120', restored: 120, currentPlayId: 'new' }
```

**建议修法：**用 `{ playId, currentTime }` 一起持久化，并保存合法的零值；clear 清除位置，恢复只接受匹配当前条目、有限且非负的位置。最终在 metadata 可用时按 duration 裁剪。仅在 `saveCurrentTime()` 改掉 truthy 判断不够，退出回调也需要修改。

## B06 — “下一首”重排把后面的曲目移到了前面

**严重度：P1。**

**代码证据：**`src/store/play-list.ts:847-856` 删除已有条目后，使用旧的 `currentItemIndex` 作为插入位置，没有 `+1`，也没有按删除后的列表重算当前项位置。

**触发步骤：**队列 `[A,B,C,D]`，当前播放 B；将已经存在且在 B 后面的 D 设为“下一首”。

**实际后果：**队列变为 `[A,D,B,C]`。`nextId` 会让下一次跳到 D，但顺序播放 D 结束后回到刚播过的 B，和列表“插到当前曲后面”的语义不符。

```text
B06 { ids: [ 'a', 'd', 'b', 'c' ], nextId: 'd' }
Expected: [ 'a', 'b', 'd', 'c' ]
```

**建议修法：**先移除待移动项，再在剩余列表中查找当前项，将目标插到当前项索引 +1。分别覆盖目标原来在当前项前面、后面和已经紧邻下一位的情况。

## B08 — 错误刷新音源后从零重新开始

**严重度：P1。待 mac 实机验证浏览器表现。**

**代码证据：**`src/store/play-list.ts:1079,1096` 刷新时直接给 `audio.src` 赋值；`src/store/play-list.ts:271` 随即重试播放，整个函数没有保存或恢复位置。与之相比，`ensureAudioSrcValid()` 的普通过期链接刷新在 `:380-383,411-414` 明确恢复了进度。

**触发步骤：**曲目暂停在 90 秒或拖动到 90 秒后续播；第一次 `play()` 因资源不支持而拒绝；刷新成功且第二次 `play()` 成功。

**实际后果：**切换 src 的媒体加载算法会重置媒体时间线；代码没有恢复 90 秒，因此重试从头开始。测试只为 `src` setter 补上时间归零副作用，调用真实刷新与续播代码，得到：

```text
B08 { currentTime: 0, storedTime: 0 }
Expected currentTime: 90
```

本条不声称普通的 deadline 过期续播一定从头开始；普通过期路径已经有恢复位置代码。当前问题限定于 `NotSupportedError` 的刷新分支。

**建议修法：**同一曲目、同一播放代际的恢复操作应捕获当前位置；换源后在能安全 seek 的阶段恢复并按时长裁剪，然后再完成播放重试。不能在刷新完成时取已经被媒体重置的 `currentTime`，也不能让旧曲恢复位置写到新曲。

**mac 验证步骤：**

1. 在 Tauri 开发构建的 Web Inspector 中，通过 `await import('/src/store/play-list.ts')` 取得实际共享的 `audio` / `usePlayList`。
2. 播放一首超过 90 秒的曲目，暂停并 seek 到 90 秒。
3. 仅在调试器内将该 audio 实例的 `play()` 包装为首次返回 `Promise.reject(new DOMException('audit', 'NotSupportedError'))`，后续调用原方法。不要修改源码或用假的 src setter。
4. 调用真实 `togglePlay()`，等待真实刷新及 `playing` 事件，记录 `audio.currentTime`。应恢复约 90 秒；若从零开始则完成浏览器实证。
5. 还原包装方法。这个探针控制异常入口，验证的是 WKWebView 的真实 src 重置与恢复行为；真实 CDN/编码故障仍需另行采样。

## B07 — 有效链接/本地曲目跳过 Now Playing 更新

**严重度：P2。**

**代码证据：**`src/store/play-list.ts:1179-1188` 的本地和有效 URL 两个分支，在 `resetAudioAndPlay()` 后直接 return。`updateMediaSession()` 只在后面的重新取链接/补全分 P 路径执行（`:1197,1232,1289`）。

**触发步骤：**先播放 A，再播放一次 B 让列表拥有 B 的有效 URL；切回 A，再在 URL 未过期时切 B。本地文件首次切歌也进入提前返回分支。

**实际后果：**应用内播放 B，系统 Now Playing 仍保留 A 的标题/封面，或本地播放一直没有更新系统曲目信息。

```text
B07 {
  playId: 'b',
  metadata: FakeMediaMetadata { data: { title: 'previous track' } }
}
```

**建议修法：**在所有已解析条目的切歌分支共用元数据更新；标题优先 `pageTitle || title`，封面用 `pageCover || cover`，并保证旧异步请求不能覆盖新曲元数据。

## 已检查而未另报为缺陷的项目

- **普通快速切歌：**订阅路径 `src/store/play-list.ts:1152-1154` 的代际检查在对照测试中能丢弃旧响应。不能将 B01 扩大成“所有切歌请求都没有保护”。
- **分 P 替换的重复播放：**`src/store/play-list.ts:1228-1264` 先替换条目，再由新 playId 的订阅启动播放；该分支没有在替换前额外调用 play。没有证据证明已修复的双重播放仍从这里发生。
- **跨启动代理 URL：**`src/platform/index.ts:131-170` 先 unwrap 再按当前 base 包装。在线有效链接的旧端口/旧令牌测试通过。`src/common/utils/audio.ts:158-160` 也从内层 CDN URL 判断 deadline。
- **重试上限：**单次 `playAudioSafely()` 最多执行首次播放及一次刷新后的重试；没有发现该函数自身递归造成无限重试。问题在 B02 的事件缺口、B03 的状态/意图和 B08 的位置丢失。
- **多窗口音频所有权：**`src/store/play-list.ts:258-260` 在子窗口拒绝启动播放；`src/app.tsx:48-68,100-102` 对播放器和快捷键事件有主窗口限制。对照测试未出现第二份播放。`src/common/utils/mini-player.ts:76-104` 将迷你播放器操作交回主窗口；当前证据不足以声称仍存在双音频实例同时播放。
- **错误状态跨窗口传播：**B03 的错误 `isPlaying` 会被 `src/common/utils/mini-player.ts:139-153` 和桌面歌词快照同步；这是主窗口状态错误的传播，不另列成重复问题。

## 完整性检查

现有测试与审计测试均未修改 tracked 文件；`git diff --exit-code` 退出码为 0。

审计开始时已有未跟踪文件 `docs/plans/quality-audit-2026-11.md`，本次保留且未修改。结束时还出现并行任务的 `A.md`，本次未修改。

最终执行 `git diff --exit-code` 及 `git status --short --untracked-files=all` 的实际输出：

```text
git diff exit=0
?? docs/audits/2026-11-10/A.md
?? docs/audits/2026-11-10/B.md
?? docs/plans/quality-audit-2026-11.md
```

本任务只写入 `B.md`；状态中没有源码或仓库内临时测试变更。
