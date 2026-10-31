> 历史记录：以下结果不代表当前提交。2026-10-30 的阈值、版本降级和评测有效性复核见 [最新复核](docs/auto-lyrics-review-2026-10-30.md)。

# Auto lyrics：识别错配修复（GIRLS / The Kid LAROI）

提交：`d095ae9`，分支 `feat/auto-lyrics-p1`，**未 push**。

## 改了什么

| 文件 | 改动 |
|---|---|
| `shared/auto-lyrics/identify.ts` | `strongMatch`：单个英文词的歌名（如 `GIRLS`）在 `clean(text)` 中命中后，如果后面紧跟空格和另一个拉丁词，就判为不匹配。这样 `Girls Band Cry` / `GIRLS BAND CRY` 不再认作 `GIRLS`。多词英文歌名和中日文逻辑不变；`clean()` 会先去掉 `MV`、`Official Video`、`现场` 等噪声，所以 `米津玄師 Lemon MV` 仍能命中。 |
| `shared/auto-lyrics/title-candidates.ts` | `buildTitleCandidates` 按空白切词时，连续的拉丁词合成一个候选（`Girls Band Cry`），不再拆成 `Girls` / `Band` / `Cry`。 |
| `tests/auto-lyrics/identify.test.ts` | 新增 2 个用例，用四个真实标题覆盖：`strongMatch("GIRLS"/"girls")===false`；带 LAROI《GIRLS》候选时 `identifyRules` 返回 `unidentified`；正例（`The Kid LAROI - GIRLS (Official Video)`、`【MV】GIRLS / The Kid LAROI`、`米津玄師 Lemon MV`）仍为 true；BV1Qh 的 queries 含 `空の箱`、`トゲナシトゲアリ`，四个标题的 `buildSearchQueries` + `identifyQueries` 都不含单独的 Girls/Band/Cry。 |

修复后各标题的 `buildSearchQueries`：

- BV19T421S7nv：`名もなき何もかも, 薄明の序奏, トゲナシトゲアリ, 皆无其名, ガールズバンドクライ`
- BV15t421K7Le：`2024年3月16日, 薄明の序奏, ガールズバンドクライ, トゲナシトゲアリ, 名もなき何もかも, …`
- BV1QhieYjEY2：`GBC, 无刺有刺, 咆哮の奏, 空の箱, Girls Band Cry, トゲナシトゲアリ`
- BV1dy75z8Enu：`爆ぜて咲く。, 互动拉满，还有最爱的桃仁饭看！, …`

已知取舍：歌名是单个英文词、标题里歌名后面紧跟艺人英文名（如 `GIRLS The Kid LAROI`）时会变成 unidentified。结果是不出歌词，不会错配。

## 测试

| 命令 | 结果 |
|---|---|
| 未改代码时跑 `npx vitest run tests/auto-lyrics/identify.test.ts` | 2 failed / 8 passed（新用例在旧代码上确实失败） |
| `npx vitest run tests/auto-lyrics/identify.test.ts` | 10 passed，exit 0 |
| `npx vitest run`（全量） | 11 files，71 tests passed，exit 0 |
| `npx eslint` / `npx prettier --check`（3 个改动文件） | 0 问题 |
| `npx tsc --noEmit -p .` | auto-lyrics 相关 0 错误；`src/layout/side/collection`、`src/pages/user-profile/favorites` 有既有错误，与本次改动无关 |

## 仍须在 Mac 重跑（真机网易云搜索 + Electron bench）

osa-a1 上网易云搜索有地区限制，B 站接口返回 412，所以真机行为还没验证。

| bvid | 修复前 | 期望 |
|---|---|---|
| BV15t421K7Le | 名もなき何もかも OK | 不变 |
| BV1QhieYjEY2 | unidentified | 空の箱 / トゲナシトゲアリ（不再用 Girls/Band 查询；能否命中取决于网易云对 `空の箱` 的结果） |
| BV19T421S7nv | 错配 GIRLS / The Kid LAROI | 名もなき何もかも，最差为 unidentified，不得出现 LAROI |
| BV1dy75z8Enu | 错配 girls / The Kid LAROI | 爆ぜて咲く，最差为 unidentified，不得出现 LAROI |

---

# 第二轮：BV19 / BV1dy 漏识修复（`660deb2`，2026-10-29，未 push）

| 原因 | 改法（`shared/auto-lyrics/identify.ts`） |
|---|---|
| BV19：トゲナシトゲアリ `2149020724` 通过 transName `皆无其名` 命中，另外两条通过 `名もなき何もかも` 命中；`chooseRule` 按匹配到的字符串去重，把同一首歌当成两首 | `RuleMatch.titles` 记录命中的全部歌名、alias 和 transName；只要候选之间共享任一名字，就算同一首歌（连通分量） |
| BV1dy：标题里没有艺人名；`爆ぜて咲く` 命中 トゲナシトゲアリ×3、闪耀圣爱音、晓月亨，被判为歧义 | 标题里没有艺人名时，如果某个艺人组在同一首歌的候选里占**严格多数**，就选这个组（confidence 0.65）；1:1 和平票仍判 unidentified |
| BV1dy 的查询带着 `。` 和后面的闲聊 | 查询生成时去掉 `。` 之后的内容（`dropChatter`），只影响查询，匹配仍用原标题 |

d095ae9 的 GIRLS 防护没有改动。新测试用 Mac 实网录下的网易云结果，LAROI《GIRLS》也放在候选里。

## 验证

| 项 | 结果 |
|---|---|
| 新用例在 d095ae9 代码上 | 3 failed / 10 passed |
| `npx vitest run tests/auto-lyrics/identify.test.ts` | 13 passed |
| `npx vitest run`（全量） | 12 files / 75 passed；不含他人留下的 `zz-dump.test.ts` 时 11 files / 74 passed |
| eslint / prettier（2 个改动文件） | 0 问题 |
| `tsc --noEmit` | 125 个错误，与基线相同，auto-lyrics 下 0 个 |
| Mac Electron bench（`/tmp/al-bench-660deb2`，真实 B 站和网易云） | 4/4 matched，见下表 |

| bvid | status | song.name | 艺人 / id | 对齐 conf | 行数 | 总耗时 |
|---|---|---|---|---|---|---|
| BV15t421K7Le | matched | 名もなき何もかも | トゲナシトゲアリ / 2149020724 | 0.72 | 42 | 3.7 s |
| BV1QhieYjEY2 | matched | 空の箱 (井芹仁菜、河原木桃香) | トゲナシトゲアリ / 2142858141 | 0.68 | 32 | 3.1 s |
| BV19T421S7nv | matched | 名もなき何もかも | トゲナシトゲアリ / 2149020724 | 0.73 | 42 | 3.7 s |
| BV1dy75z8Enu | matched | 爆ぜて咲く | トゲナシトゲアリ / 2149019978 | 0.87 | 64 | 3.8 s |

额外跑了 identifyOnly（`/tmp/al-idonly-660deb2`，8 条），没有错配：BV1StSoBPEwu p3 命をくれよ、p5 荊の薔薇、BV1DV81zKEYm サヨナラサヨナラサヨナラ 均为 matched；p1、p2、p4 和 BV19s6cBxEmc 为 unidentified；BV1gLHj6XEWY 为 not-music。

## 还剩

- BV19s6cBxEmc（【拉刺】爆ぜて咲く / 爆裂绽放）漏识：候选里有《爆ぜて咲》（Jav，213 s），这个名字是 `爆ぜて咲く` 的子串，被当成另一首歌。可以考虑“较长命中名覆盖它的子串名”。未做。
- BV1StSoBPEwu p1（命运之花【完整版】）、p2、p4 漏识：本轮没有查原因。
- 多数规则依赖网易云收录的条目数量。对于原唱只有 1 个条目、翻唱有多个条目的歌，可能选到翻唱组。翻唱的歌词与原唱相同，再加上原曲时长、人声起点和对齐置信度三道把关，风险有限，但不是零。

## db05966：较长命中名覆盖前缀歌名（2026-10-29）

- 改动：`shared/auto-lyrics/identify.ts` 的 `identifyRules` 在 `chooseRule` 前丢掉“所有命中名都是另一候选命中名的真子串”的候选；`tests/auto-lyrics/identify.test.ts` 新增 1 个用例（旧代码失败，新代码通过）。
- 验证：`npx vitest run` 11 个文件 75 个通过；eslint / prettier 无问题。
- Mac Electron bench（`/tmp/al-bench-db05966`，本机拷贝 `/tmp/al-probe/bench-db05966/`）：

| bvid | 模式 | status | song.name | conf |
|---|---|---|---|---|
| BV15t421K7Le | pipeline | matched | 名もなき何もかも | 0.72 |
| BV1QhieYjEY2 | pipeline | matched | 空の箱 | 0.68 |
| BV19T421S7nv | pipeline | matched | 名もなき何もかも | 0.73 |
| BV1dy75z8Enu | pipeline | matched | 爆ぜて咲く | 0.87 |
| BV19s6cBxEmc | pipeline | matched | 爆ぜて咲く | 0.91 |
| BV1StSoBPEwu p1 / p2 / p4 | identify | unidentified | — | 0 |
| BV1StSoBPEwu p3 / p5 | identify | matched | 命をくれよ / 荊の薔薇 | 0.8 |
| BV1gLHj6XEWY | identify | not-music | — | 1 |
| BV1DV81zKEYm | identify | matched | サヨナラサヨナラサヨナラ | 0.8 |

- BV1StSoBPEwu 仍漏识的原因（Mac 真实网易云搜索）：
  - p1「命运之花」：トゲナシトゲアリ《運命の華》3 条靠译名命中，另有 5 条同名中文歌（不同艺人），无多数。
  - p2「无需既定的未来」：搜索返回《もう何もいらない未来》，但标题里没有它的名字或别名，规则无证据。
  - p4「arrow」：niki《ARROW》2 条、トゲナシトゲアリ《arrow》1 条；按“最多票”放宽会错配到 niki，所以没放宽。
  - 三者都只有“GBC”系列名能指向 トゲナシトゲアリ，需要“系列名 → 艺人”提示才能安全识别。
- 内存（P1 流水线，Mac bench `memory` 字段）：首首冷启动主进程 + 对齐进程峰值增量约 345 MB，之后 207–279 MB；需求已拍板“峰值内存不硬限”。

## fdbd49b：对齐裁决同名歧义 + 非音乐分区书名号（2026-10-29）

- `identify` 新增 `ambiguous`（≤3 个 contender，同艺人发行条数多的在前）；`service.run` 对每个 contender 取词、找原曲、对齐，`verifyScore ≥ 0.72` 且领先 ≥ 0.05 才 matched。
- 非音乐分区但标题有《》「」时也搜索并走裁决。
- bench-runner 改用与 App 相同的 `pageParams`：单 P 视频的分 P 名（上传文件名）不再当 pageTitle。BV1gL 之前漏识就是 bench 传了 `TG-2026-10-04-172657959`。
- 交叉 bench（`scripts/auto-lyrics-bench/cross.bench-run.ts`）：粗对齐 conf 区分不了同歌/错歌；verifyScore 同歌 ≥ 0.787、错歌 ≤ 0.671。TV size 截短后分不开（GapDTW 不能跳过剪掉的段落）。
- Mac Electron bench（`/tmp/al-bench-verify3`，本机拷贝 `/tmp/al-probe/bench-verify3/`）：12 首完整流水线 11 首 matched、歌名全对、0 错配；p4（TV size arrow）unidentified。
- `npx vitest run` 81 通过；bench 5 项通过；tsc 125（基线）；eslint 无问题。
