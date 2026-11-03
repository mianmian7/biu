# M0 / S4：Mac dmg 打包失败（AppleScript -2741）修复方案

owner = Claude（osa-a1）

状态：待用户批准（2026-11-03 起草）。上级方案：`docs/plans/tauri-rewrite.md` §2 S4、§5 M0。

## 现象

MateBook（macOS 27.0.1 / 26A434，arm64；Node 26.10，Rust 1.97.1；`@tauri-apps/cli` 2.12.4，`tauri` 2.12.5，内置 create-dmg 1.2.1）上，`tauri build --ci --bundles dmg` 生成 `Biu M0.app` 成功，`bundle_dmg.sh` 在 Finder 美化步骤失败：

```
createdmg.tmp.…:379:385: script error: Expected end of line but found class name. (-2741)
Failed running AppleScript
```

`bundle/macos/` 留下 `rw.93765.Biu M0_0.0.1_aarch64.dmg`。hdiutil 的 deprecated 警告只是警告，与失败无关。

## 根因（2026-11-03 在 Mac 上只读诊断，已复现）

1. 失败在 AppleScript **编译**阶段，未进入运行阶段。把 `share/create-dmg/support/template.applescript` 代入数值后在 `/tmp` 用 `osacompile` 编译，第 15 行 `tell container window` 报同一错误；379:385 正是 `window` 这个词。
2. 同一模板只把 `tell application "Finder"` 换成 `tell application id "com.apple.finder"` 或 `tell application "/System/Library/CoreServices/Finder.app"`，编译通过。按名称引用 Finder 时，`disk "x"`、`folder "x"`、`container window` 都无法解析；Music、Safari、System Events 按名称引用正常。所以按名称解析 `Finder` 时没有加载到 Finder 的脚本字典。
3. LaunchServices 中登记了**两个**名为 `Finder` 的 app：系统的 `/System/Library/CoreServices/Finder.app`（`com.apple.finder`，有 `Finder.sdef`），以及用户 Library 下一个来源不明、ad-hoc 签名、无脚本字典的后台 app。后者也叫 `Finder`，开机登录后一直在运行。`tell application "Finder"` 解析到了后者，因此编译时用的是空字典。详情已单独报告给 owner；该 app 的安全处理不属于本仓库范围。
4. 上游：没有对应的 macOS 27 / Tauri / create-dmg 缺陷，也不需要升级版本。同类 -2741 的已知原因就是“应用名解析到错误的 app，字典没加载”（例如 itermocil #43）。tauri-bundler 在环境变量 `CI=true`（且未设置 `TAURI_BUNDLER_DMG_IGNORE_CI=true`）时会给 `bundle_dmg.sh` 加 `--skip-jenkins`，跳过整段 Finder AppleScript。

## 方案比较

| 选项 | 是否解决 | dmg 体积 | S7 updater | 未来 CI | 维护 |
|---|---|---|---|---|---|
| A. owner 清理那个冒名 app 后原样重跑 | 是（仅这台 Mac） | 不变 | 无影响 | 无关 | 无改动；但构建仍依赖 Finder 自动化与 TCC |
| **B. 构建时设 `CI=true`（Tauri 自动加 `--skip-jenkins`）** | **是，且不再依赖 Finder/TCC** | 少一个 `.DS_Store`/窗口布局，体积变化可忽略；仍是 create-dmg 默认 UDZO 压缩 | 无影响（S7 用 `bundle/macos` 下的 `.app` 打 `.app.tar.gz`，不用 dmg） | 与 GitHub Actions 一致（runner 自带 `CI=true`，CI 产物本来就不跑 AppleScript） | 零代码，只改 README 两条构建命令 |
| C. `--bundles app` + 自写 `hdiutil create -format UDZO` 脚本 | 是 | 与 B 相当 | 无影响 | 需自己维护脚本、Applications 链接、命名 | 多一个脚本，重复 Tauri 已有功能 |
| D. 升级 tauri CLI/bundler | 否（根因在本机环境） | — | — | — | — |
| E. fork/修改 create-dmg 模板改用 bundle id | 是 | 不变 | 无影响 | 每次生成 `bundle_dmg.sh` 都会被覆盖，需要 fork bundler | 不值得 |

**选 B**，A 由 owner 另行处理（不阻塞 B，但见“待 owner 决定”）。理由：B 不改代码，绕开 Finder 自动化和 TCC 弹窗（README 已把这类弹窗列为需 owner 当面处理的阻塞项），与将来 CI 的 dmg 产物完全一致，不影响 S4 体积口径和 S7 更新链。dmg 失去自定义窗口布局，M0 不需要它；正式发版的 dmg 外观到 M7 再定。C 只作为 B 失败时的退路。

## 步骤与改动文件

清单（执行者完成一项勾一项，在该项下追加一句结果）：

- [ ] 1. 改 `spikes/README.md`（唯一改动文件）：§1.1 的 `tauri build --ci --bundles dmg` 与 §1.5 的 0.0.2 构建命令前都加 `CI=true `；在 §1.1 该命令下加一行注释：`# CI=true 让 Tauri 传 --skip-jenkins，跳过 Finder AppleScript（与 GitHub Actions 一致；dmg 无自定义窗口布局）。`。§1.1 的日志改写到 `../evidence/mac-build-dmgfix.log`，保留原失败日志 `mac-build.log` 不覆盖。不改 `tauri.conf.json`、`tauri.macos.conf.json`、脚本或 Rust 代码。
  - 执行：Claude（osa-a1）自己做（两行文本，派出去不划算），一个本地 commit，不 push。
- [ ] 2. Mac 验证（下节命令），并按 README §1.4 S4、§1.5 S7 重跑，回填 `tauri-rewrite.md` S4/S7 项下结果。
  - 执行：Sol（mac 线程，`delegate_task` + mac `environmentId`），与 §6 “S1、S5、S7、S8 的 Mac 执行与记录”同一角色；必须等 soak 线程 mcp:2d74e759 结束后开始，不与其抢 CPU、不动其进程。顺带审查第 1 步的 README 改动（作者 Claude，审查者 Sol）。
  - 最终验收：Opus（osa-a1）核对回传的实际输出。
- 退路：第 2 步构建在 `hdiutil` 阶段仍失败时，停下回报完整日志，由 Claude 另写选项 C 的方案并重新取得批准；执行者不自行写打包脚本。

## Mac 验证命令（第 2 步）

在 Mac 的 `~/Developer/biu-m0-transfer/biu-tauri-m0/spikes/m0` 执行，先按 README §0 的方式（osa-a1 分支不 push，用 bundle 传输）取得含第 1 步的提交；不 merge、不 push。

```bash
cd ~/Developer/biu-m0-transfer/biu-tauri-m0/spikes/m0
# 0. 清理上次失败残留（只删本工程的中间镜像），确认没有挂着的旧卷
ls /Volumes | grep -i 'biu' && echo "有旧卷，先 hdiutil detach 再继续"
rm -f src-tauri/target/release/bundle/macos/rw.*.dmg

# 1. 构建
CI=true ../node_modules/.bin/tauri build --ci --bundles dmg > ../evidence/mac-build-dmgfix.log 2>&1; echo "exit=$?"   # 必须 exit=0
grep -n 'skip running AppleScript' ../evidence/mac-build-dmgfix.log                                                # 必须命中
ls src-tauri/target/release/bundle/macos/rw.*.dmg 2>/dev/null && echo "残留 rw 镜像=失败"

# 2. 镜像校验与内容
D='src-tauri/target/release/bundle/dmg/Biu M0_0.0.1_aarch64.dmg'
hdiutil verify "$D"                                       # 必须 "checksum ... is VALID"
hdiutil imageinfo "$D" | grep -E '^Format( Description)?:' # 期望 UDZO
M=$(mktemp -d /tmp/biu-dmg.XXXXXX)
hdiutil attach -nobrowse -readonly -mountpoint "$M" "$D"
ls -la "$M"                                               # 必须有 "Biu M0.app" 和指向 /Applications 的 Applications 链接
test -d "$M/Biu M0.app" && test -L "$M/Applications" && echo contents-ok
codesign --verify --deep --strict --verbose=2 "$M/Biu M0.app"   # ad-hoc 签名应校验通过
hdiutil detach "$M"

# 3. 体积（S4）
stat -f '%z %N' src-tauri/target/release/biu-m0 "$D"     # dmg 字节/1,000,000：目标 ≤ 8 MB，硬上限 10 MB
```

然后：

- README §1.4 S4 全行（含 `cargo bloat`，输出到 `../../evidence/mac-bloat.txt`）。
- README §1.5 S7 全流程（其中 0.0.2 构建已带 `CI=true`）。S7 的 updater 产物是 `bundle/macos` 下打包的 `.app.tar.gz`，与 dmg 无关；dmg 只用于 “从 dmg 直接运行（translocated）→ manual 模式” 那一格，挂载后需 owner 在 Finder 里双击操作，Gatekeeper 弹窗由 owner 当面处理。
- 回传：`mac-build-dmgfix.log`、上面每条命令的实际输出、dmg 字节数、S4/S7 结果；不回传 dmg、私钥或 private 目录。

## 待 owner 决定

1. **安全（优先）**：Mac 上那个冒名 `Finder` 的后台 app 来源不明，需要 owner 先确认它是否是自己装的软件。如果不能确认，建议先按安全事件处理，再在这台 Mac 上继续生成 S7 测试钥匙或做登录类测试（S6）。本方案不包含任何清理动作，执行者也不得删除或停止它。
2. 是否接受 M0 及以后 CI 的 dmg 不带自定义 Finder 窗口布局（选项 B 的唯一代价）。正式版 dmg 外观可在 M7 再定。
