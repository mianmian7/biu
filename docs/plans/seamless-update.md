# 三平台无感更新、发布窗口误报与 autolyrics 通道过渡

owner = Claude（osa-a1）

状态：待用户批准。批准前不派执行线程，不改代码。

## 决策

Mac 用**自签名代码签名证书**（免费，CI 持有私钥）给每个版本签名，继续用 electron-updater 自带的 Squirrel.Mac 安装更新。不写自定义替换器，不用 Homebrew tap。

| 方案 | 结论 | 原因 |
|---|---|---|
| 自签名证书 + 原生 Squirrel.Mac | **选用** | Squirrel.Mac 只要求新包满足运行中 app 的 designated requirement。ad-hoc 签名的 requirement 是 cdhash，每个版本都会变；自签名证书的 requirement 是 `identifier "com.biu.wood3n" and certificate leaf = H"…"`，跨版本不变。几乎不需要新代码。更新的真实性由证书私钥保证，私钥只放在 CI secret 里（效果等同 Tauri 的签名公钥）。TCC 授权也会随 requirement 稳定下来，不会每版重置。 |
| 自写替换器 + Ed25519 签名（仿 Tauri） | 备选（Plan B） | 能用，但要自己处理原子替换、回滚、Translocation、目录权限，大约 150 行脚本和代码。只有步骤 1 的 spike 失败时才启用，启用前须重新批准方案。 |
| Homebrew tap | 不选 | 用户仍要自己执行 `brew upgrade`，做不到无感；还要多维护一个 tap 仓库。 |
| 只提示 / 打开 release 页 | 不选 | 不解决问题。保留为各平台的手动降级路径（manual 模式）。 |

自签名证书方案的依据：
- `electron-updater` 6.6.2 的 `MacUpdater.js:207-250` 会把 zip 交给 Squirrel.Mac，由 Squirrel.Mac 校验签名。
- electron-builder 支持非 Apple 证书：`app-builder-lib/out/codeSign/macCodeSign.js:228-245` 在找 Developer ID 时会回退到 non-Apple certificate。前提是该证书在 `security find-identity -v -p codesigning` 中显示为 valid，也就是要先把它加入系统信任。
- 社区已有端到端验证（noxaur/finite PR #3）：两个用同一自签名证书签名的版本，退出时由 Squirrel 成功替换，并出现 `ShipIt: Installation completed successfully`。
- 这个方案仍需在本机 macOS 27.0.1 arm64 上实测，见步骤 1。

已知代价：
- 第一次装自签名版本时，Gatekeeper 仍要求在「隐私与安全性」里点「仍要打开」，和现在的 ad-hoc 包一样。之后的更新没有 quarantine，不会再弹窗。
- 现有 ad-hoc 安装（≤ 1.16.1-autolyrics.5）要**手动安装一次**桥接版本。
- 证书丢失后，Mac 客户端无法再自动更新，只能重新手动安装一次。证书 p12 必须备份。

## 目标

1. Win NSIS、Linux AppImage、Mac（自签名、位于可写目录、未 translocate）：后台自动下载；正常退出时安装，下次启动就是新版本；也提供「立即重启更新」按钮。
2. 不支持自动安装的场景（Win portable、deb/rpm、Mac ad-hoc / 未签名 / translocated / 目录不可写），统一进入 manual 模式：沿用现在的“下载后打开文件夹”，不自动下载，不调用 Squirrel。
3. 发布窗口期（tag 已推送、release 仍是 draft）不再出现 `Cannot find latest-mac.yml` 报错。
4. ≤ 1.16.1-autolyrics.5 的客户端能升级到桥接版本；从桥接版本开始，不再依赖 prerelease channel 匹配。

## 非目标

- 不买 Apple 证书，不做公证。
- Windows 不做代码签名，也不加额外的签名校验。
- 不改 deb/rpm 的安装方式。
- 不做灰度（`stagingPercentage`），不做远程开关服务。

## 通道过渡设计

现状（electron-updater 6.6.2 的 `GitHubProvider.js:51-80`）：`allowPrerelease=true`，且当前版本带 `autolyrics` 后缀，所以客户端从 `releases.atom` 里只挑同后缀的 tag。后果有三个：
- 不带后缀的正式版（比如 `v1.17.0`）永远不会提供给这些客户端；
- 只要 atom 里出现一个还没有 release 资产的 tag，就会 404；
- 每次检查还会先请求一次 `autolyrics-mac.yml`，404 后才回退到 `latest-mac.yml`。

新设计：`allowPrerelease = false`。
- 客户端改为请求 `https://github.com/<owner>/<repo>/releases/latest`，Accept 设为 JSON。只有已发布、且标记为 Latest 的 release 会被返回（已用 curl 实测，返回 `tag_name: v1.16.1-autolyrics.5`）。然后从这个 tag 下载 `latest*.yml`。
- 版本比较只用 `semver.gt`。`AppUpdater.js:315-339` 不过滤 prerelease，所以带后缀的版本号照样可用。
- draft 不会成为 Latest，发布窗口期因此天然不存在。
- 只有 tag、没有 release 的 upstream tag（比如以前的 `v1.16.3`）也不再影响客户端。
- `release.yml` 的 publish job 已经执行 `--prerelease=false --latest`，不需要改。

过渡规则：
1. 桥接版本**必须**叫 `1.16.1-autolyrics.6`（带 `autolyrics` 后缀）。旧客户端按旧逻辑只认这个后缀。
2. 桥接版本发布后，版本号只需要比当前 Latest 更大。建议继续用 `<upstream 基线>-autolyrics.N`：跟进 upstream 1.17.0 时就叫 `1.17.0-autolyrics.1`，这样不会和 upstream 的 tag 撞名。改用纯正式版号（比如 `1.17.0`）对新客户端同样可用，但必须在 `.6` 发布之后。
3. 不能发比当前 Latest 更小的版本并标记为 Latest。`allowDowngrade` 保持 `false`，客户端会忽略这种版本。
4. 以后要做测试通道时：发 `--prerelease` 且不标 Latest 的 release，新客户端看不到它。

## owner 前置步骤（只有 owner 做，执行者不碰 secret）

- [ ] O1. 在 mac 上生成自签名代码签名证书，有效期 20 年，CN 设为 `Biu Self-Signed Code Signing`。CN 不能以 `Developer ID` / `Apple` 开头。
  ```bash
  mkdir -m 700 ~/biu-codesign && cd ~/biu-codesign
  openssl req -x509 -newkey rsa:3072 -nodes -days 7300 -keyout key.pem -out cert.pem \
    -subj "/CN=Biu Self-Signed Code Signing" \
    -addext "basicConstraints=critical,CA:false" \
    -addext "keyUsage=critical,digitalSignature" \
    -addext "extendedKeyUsage=critical,codeSigning"
  openssl pkcs12 -export -legacy -inkey key.pem -in cert.pem -name "Biu Self-Signed Code Signing" -out biu-codesign.p12   # 交互输入导出密码
  openssl x509 -in cert.pem -outform DER -out biu-codesign.cer
  ```
- [ ] O2. 把 `biu-codesign.p12` 和导出密码存进密码管理器（证书唯一备份）。
- [ ] O3. 设置仓库 secret（交互输入，密钥不出现在命令参数里）：
  ```bash
  base64 -i biu-codesign.p12 | gh secret set BIU_MAC_CERT_P12 -R mianmian7/biu
  gh secret set BIU_MAC_CERT_PASSWORD -R mianmian7/biu   # 交互粘贴
  ```
- [ ] O4. 把**公钥证书** `biu-codesign.cer` 交给执行者，提交为 `plugins/mac/biu-codesign.cer`。公钥证书不是机密。`key.pem` 不交给执行者，O2 完成后删除。
- [ ] O5. 步骤 1 需要一张测试证书，执行者在 mac 上另外生成一张临时证书（CN 设为 `Biu Test Signing`），不要用 O1 的正式证书。

## 执行步骤

执行者只勾选自己负责的条目，并在条目下追加简短结果；不修改其他内容。

执行顺序：2 → 3 → 4 → 6.1 / 6.2 → 1（spike 闸门，在 mac 上）→ 5 → 6.3–6.6 → 7。
步骤 1 依赖 2–4 的代码，但它是决策闸门：1 不通过，就不做 5 及之后的步骤。

### 1. Spike：在 mac 本机验证自签名证书 + Squirrel 可行（决策闸门）

- [ ] 1.1 在 mac 上生成临时证书 `Biu Test Signing`（参照 O1 的命令），导入 login keychain，再用 `security add-trusted-cert -r trustRoot -p codeSign -k ~/Library/Keychains/login.keychain-db biu-test.cer` 设为代码签名信任（会弹系统授权框，由 mac 上的用户确认）。确认 `security find-identity -v -p codesigning` 中出现这张证书。
- [ ] 1.2 先完成步骤 2、3 的代码改动（可以在本地分支上做）。然后在本地工作区临时把 `package.json` 的 version 改为 `1.16.1-autolyrics.90`，执行 `BIU_UPDATE_FEED_URL=http://127.0.0.1:8765 CSC_NAME="Biu Test Signing" pnpm build`，得到版本 A；把 version 改为 `.91` 再构建，得到版本 B。不要提交 version 改动。
- [ ] 1.3 检查 A 的签名：`codesign -dv --verbose=2 dist/.../Biu.app` 显示 `Authority=Biu Test Signing`，`codesign -d -r- ...` 包含 `certificate leaf = H"`，`codesign --verify --deep --strict` 通过。
- [ ] 1.4 把 A 的 dmg 装到 `/Applications`（先用 `ditto` 备份现有 `Biu.app` 到 `~/biu-backup/`）。在 B 的 artifacts 目录里执行 `python3 -m http.server 8765 --bind 127.0.0.1`，然后启动 A。
- [ ] 1.5 预期结果：
  - A 自动下载 B，`main.log` 出现 `Proxy server for native Squirrel.Mac` 和 `nativeUpdater.update-downloaded`；
  - 用 Cmd+Q 退出后重新打开，`defaults read /Applications/Biu.app/Contents/Info CFBundleShortVersionString` 输出 `1.16.1-autolyrics.91`；
  - 全程没有 App Management / Gatekeeper / 钥匙串弹窗；
  - 再构建一个 `.92`，测试「立即重启更新」按钮，确认更新后自动重启。
- [ ] 1.6 验证 manual 降级：把 A 换成一个 ad-hoc 签名的构建（不带 `CSC_NAME`，并删掉临时证书的信任），确认它进入 manual 模式，日志里没有 Squirrel 报错。
- [ ] 1.7 如果签名阶段因为 timestamp 失败，在 `mac` 配置里加 `timestamp: "none"` 后重试。如果 hardened runtime 导致启动失败，改成 `hardenedRuntime: false` 后重试。两处改动都要记录在结果里。
- [ ] 1.8 清理：恢复 `/Applications/Biu.app` 备份，删除临时证书、信任设置和测试缓存（`~/Library/Caches/biu-updater`、`~/Library/Caches/com.biu.wood3n.ShipIt`）。
- **闸门**：1.5 或 1.6 不通过时，停止执行，并报告失败步骤、`main.log` 片段和 `codesign` 输出。不要自行改用 Plan B。

### 2. 主进程：更新模式、开关、错误分类

- [x] 2.1 新增 `electron/updater/update-mode.ts`，内容是纯函数，加一个很薄的 IO 封装：
  - `type UpdateMode = "auto" | "manual"`
  - `resolveUpdateMode({ platform, env, execPath, codeSignatureKind, bundleParentWritable })`：
    - win32：设置了 `PORTABLE_EXECUTABLE_DIR` 或 `PORTABLE_EXECUTABLE_FILE` 时返回 manual，否则返回 auto；
    - linux：设置了 `APPIMAGE` 时返回 auto，否则返回 manual；
    - darwin：`execPath` 包含 `/AppTranslocation/` 或以 `/Volumes/` 开头时返回 manual；`codeSignatureKind !== "certificate"` 时返回 manual；`!bundleParentWritable` 时返回 manual；其余返回 auto。
  - `parseCodesignOutput(stderr): "certificate" | "adhoc" | "unsigned"`：输出含 `Signature=adhoc` 时返回 adhoc；含 `code object is not signed` 时返回 unsigned；含 `Authority=` 时返回 certificate。
  - `detectUpdateMode()`：只在 darwin 上执行 `execFile("/usr/bin/codesign", ["-dv", "--verbose=2", bundlePath])`，`bundlePath` 是从 `app.getPath("exe")` 往上三级得到的 `.app`；再用 `fs.promises.access(dirname(bundlePath), W_OK)` 检查目录可写。检测结果缓存起来，并写一行 `log.info("[updater] mode=…")`。
- [x] 2.2 修改 `electron/updater/index.ts`：
  - `allowPrerelease` 从 `true` 改为 `false`；`allowDowngrade` 保持默认的 `false`；
  - `setupAutoUpdater` 改为 async：先 `await detectUpdateMode()`，再设置 `autoDownload = mode === "auto"`、`autoInstallOnAppQuit = mode === "auto"`；
  - manual 模式下，Mac 必须保持 `autoInstallOnAppQuit = false`，否则 Squirrel 会报签名错误；
  - 新增 `classifyUpdateError(err): "unpublished" | "network" | "other"`：`err.code` 为 `ERR_UPDATER_CHANNEL_FILE_NOT_FOUND`、`ERR_UPDATER_LATEST_VERSION_NOT_FOUND`、`ERR_UPDATER_NO_PUBLISHED_VERSIONS` 时归为 unpublished；`ENOTFOUND`、`ECONNRESET`、`ETIMEDOUT`、`net::ERR_*` 归为 network；
  - 启动检查和每小时检查都改成 `autoUpdater.checkForUpdates().catch(...)`，只写 `log.warn`，消除 `unhandledRejection`；
  - `error` 事件只在下载或安装阶段（用一个 `phase` 变量跟踪）才转发给渲染进程，检查阶段的错误只写日志；
  - 新增 `update-downloaded` 时把 `{ status: "downloaded", mode }` 发给渲染进程。
- [x] 2.3 修改 `electron/ipc/app.ts`：
  - `checkUpdate` 遇到 unpublished 错误时，返回 `{ isUpdateAvailable: false, notice: "新版本正在发布，请几分钟后再试" }`，不再返回 error；
  - 新增 `channel.app.getUpdateMode` handler，返回缓存的 mode；
  - `quitAndInstall` 在 mode 为 auto 时调用 `autoUpdater.quitAndInstall(true, true)`（Win 静默安装并重启；AppImage 重启；Mac 的 `MacUpdater.quitAndInstall` 忽略参数，并因 `autoRunAppAfterInstall=true` 自动重启），mode 为 manual 时直接返回 false。
- [x] 2.4 修改 `electron/ipc/channel.ts`：增加 `getUpdateMode: "app:get-update-mode"`。
- [x] 2.5 修改 `electron/main.ts`：调用处改为 `setupAutoUpdater(...)` 并 catch 错误后写日志。`will-quit` 中的 `autoUpdater.removeAllListeners()` 保留：BaseUpdater 的退出安装挂在 `app.once("quit")` 上，不受影响。

执行结果（2.1–2.5）：已实现模式检测与缓存、后台下载、错误分类、IPC 模式查询和安装门控；检查失败记录 warn。错误分类独立为纯函数模块，便于在 Linux 上测试。

### 3. 预加载与渲染进程

- [x] 3.1 `electron/preload.ts`：删除同步的 `isSupportAutoUpdate`，改为 `getUpdateMode: () => ipcRenderer.invoke(channel.app.getUpdateMode)`。同步修改 `shared/types/renderer.d.ts`；在 `shared/types/app-update.d.ts` 里给 `CheckAppUpdateResult` 加 `notice?: string`，给 `DownloadAppMessage` 加 `mode?`。
- [x] 3.2 `src/store/app-update.ts`：增加 `mode`、`downloadStatus`（`idle | downloading | downloaded | error`）和 `downloadProgress`。`src/app.tsx` 启动时读取 mode，并全局订阅 `onDownloadAppProgress`，这样弹窗关闭后状态也不会丢。auto 模式下载完成时弹一次 toast：「新版本已下载，退出时自动安装」，附带「立即重启」按钮。
- [x] 3.3 `src/components/release-note-modal/index.tsx`：从 store 读取状态。
  - auto 模式：不显示「下载更新」按钮；下载中显示进度；下载完成显示「立即重启更新」，同时提示「也可以稍后，退出时自动安装」。
  - manual 模式：保持现在的行为（「下载更新」和「打开安装包文件夹」）。
- [x] 3.4 `src/components/update-check-button/index.tsx`：结果带 `notice` 时，显示 `color: "warning"` 的 toast。

执行结果（3.1–3.4）：模式改为异步 IPC；App 全局订阅下载事件，store 保留弹窗关闭期间的状态；auto 下载完成提示与重启按钮、manual 文件夹入口及发布窗口 warning toast 已实现。

### 4. 构建配置

- [x] 4.1 `plugins/electron-build.ts`：
  - 设置了 `BIU_UPDATE_FEED_URL` 时，`publish` 改用 `{ provider: "generic", url }`，只用于本地 E2E；
  - 末尾的 `.catch(error => logger.error(error))` 改成写日志后重新抛出（或设置 `process.exitCode = 1`）。现在构建失败也会让 CI 显示成功；
  - 如果步骤 1.7 需要 `timestamp: "none"` 或 `hardenedRuntime: false`，在这里加上。
- [x] 4.2 `electron/updater/dev-app-update.yml` 不改。

执行结果（4.1–4.2）：generic feed 环境变量覆盖已实现，构建失败记录后重新抛出；dev feed、平台打包目标、签名配置与版本号均未改动。timestamp / hardenedRuntime 等待 Mac spike 结果。

### 5. CI（`.github/workflows/release.yml`，只改 build job）

- [ ] 5.1 在 `Install dependencies` 之后新增一步，条件 `if: runner.os == 'macOS'`：
  ```bash
  sudo security add-trusted-cert -d -r trustRoot -p codeSign \
    -k /Library/Keychains/System.keychain plugins/mac/biu-codesign.cer
  ```
- [ ] 5.2 `Build & Publish` 的 env 增加以下三项，只在 mac 上生效，避免 Windows 拿 `CSC_LINK` 去签 exe：
  ```yaml
  CSC_LINK: ${{ runner.os == 'macOS' && secrets.BIU_MAC_CERT_P12 || '' }}
  CSC_KEY_PASSWORD: ${{ runner.os == 'macOS' && secrets.BIU_MAC_CERT_PASSWORD || '' }}
  CSC_NAME: ${{ runner.os == 'macOS' && 'Biu Self-Signed Code Signing' || '' }}
  ```
- [ ] 5.3 在 `Build & Publish` 之后新增一步签名守卫，条件 `if: runner.os == 'macOS'`：对 `dist/artifacts/mac/Biu.app` 和 `dist/artifacts/mac-arm64/Biu.app`，`codesign -dv --verbose=2` 的输出必须含 `Authority=Biu Self-Signed Code Signing`，`codesign -d -r-` 的输出必须含 `certificate leaf = H"`，否则 `exit 1`。这样 publish job 不会把 ad-hoc 包发布成 Latest。
- [ ] 5.4 publish job 不改。

### 6. 测试

- [ ] 6.1 单元测试：新增 `tests/updater/update-mode.test.ts`、`tests/updater/classify-error.test.ts`，覆盖：
  - `resolveUpdateMode` 全矩阵：win 安装版 / portable、linux AppImage / deb、mac certificate / adhoc / unsigned / translocated / `/Volumes` / 目录不可写；
  - `parseCodesignOutput` 三种输出（用真实 `codesign` 输出片段作 fixture，可以取本机已安装的 ad-hoc Biu 的输出）；
  - `classifyUpdateError` 对各个 `ERR_UPDATER_*` code 和网络错误的分类。
  - 运行命令：`pnpm vitest run tests/updater`。
- [ ] 6.2 静态检查：`pnpm exec tsc --noEmit -p tsconfig.json`，以及 `pnpm exec eslint electron/updater electron/ipc src/components/release-note-modal src/components/update-check-button src/store/app-update.ts`。
- [ ] 6.3 Mac E2E：在 mac 上按步骤 1 的流程，用正式代码再跑一遍，包括「退出时安装」「立即重启更新」「manual 降级」三项。
- [ ] 6.4 Windows E2E：在 pc 上用同样的 generic feed 构建 A/B 安装版，验证：
  - 自动下载；
  - 正常退出后静默安装（无安装窗口），下次启动是 B；
  - 「立即重启更新」静默安装并重启；
  - `%APPDATA%\biu` 的数据还在（`uninstaller.nsh:209-213` 只在非 `isUpdated` 时删除 app data）；
  - portable 版进入 manual 模式。
- [ ] 6.5 Linux AppImage E2E（可选，在 wsl 上用 WSLg）：A 退出后 `$APPIMAGE` 文件被换成 B；deb 安装进入 manual 模式。
- [ ] 6.6 发布窗口回归：generic feed 目录里删掉 `latest-mac.yml` 后启动 A，确认后台检查不弹任何错误、日志只有 warn，手动点「检查更新」显示 warning toast 而不是「检查更新失败」。对 GitHub provider 的行为，在 `.6` 发布时观察即可（见 7.3）。

### 7. 桥接发布（owner 单独授权后才执行）

- [ ] 7.1 合并到发布分支，版本号设为 `1.16.1-autolyrics.6`，推送 tag。推送和打 tag 需要 owner 另行授权。
- [ ] 7.2 验收 CI：mac job 的签名守卫通过；release 资产和现在相同（4 个 `latest*.yml` 加 zip、dmg、blockmap）；publish job 把它标为 Latest。
- [ ] 7.3 在 CI 跑的过程中（draft 阶段），用 `.5` 客户端（旧逻辑）和本地 `.6` 构建分别观察日志：`.6` 应该没有 404。
- [ ] 7.4 Mac 上手动安装一次 `.6` 的 dmg；Windows 上 `.5` 客户端按现有按钮安装。之后 `.7` 起全部自动更新。

## 回滚

| 场景 | 操作 |
|---|---|
| 步骤 1 spike 失败 | 停止；回报 owner；需要的话写 Plan B 方案并重新审批。本地改动丢弃，没有任何远端影响。 |
| 合并前发现问题 | 丢弃分支。 |
| `.6` 的 CI 失败 | 签名守卫或构建失败时，publish job 不会运行，release 停在 draft；删除 draft 和 tag（需要 owner 授权），修好后重新发。 |
| 某个已发布版本有问题，但 app 还能启动 | 发一个版本号更大的修复版，客户端会自动更新过去。想立即停止扩散时，把上一个好版本重新标成 Latest：`gh release edit <good-tag> --latest`，或者把坏版本改回 draft。新客户端只看 `/releases/latest`，会停止拿到坏版本（需要 owner 授权）。 |
| 新版本启动即崩溃 | 已经更新的机器只能手动重装旧版本：Mac 装旧 dmg；Win 运行旧 setup.exe，app data 不会被删；Linux 换回旧 AppImage。同时按上一行停止扩散。 |
| 整体关闭无感更新 | revert 本方案的代码并发布新版本，客户端回到 manual 流程。Mac 上已暂存的更新可以删除 `~/Library/Caches/com.biu.wood3n.ShipIt` 和 `~/Library/Caches/biu-updater` 清掉。 |
| 证书泄露或丢失 | 按 O1 重新生成证书并替换 secret 和 `plugins/mac/biu-codesign.cer`；新证书签名的第一个版本，Mac 上要手动安装一次。 |

## 风险

- 自签名证书和 Squirrel 在 macOS 27 上的行为，目前只有社区实测，步骤 1 必须真机验证。
- 常驻托盘或从不退出的用户会一直停在旧版本，直到下一次退出。下载完成后的 toast 提供了「立即重启」按钮。
- Windows 如果当初装到了需要提权的目录，静默更新会弹 UAC。
- 首次安装仍要绕过 Gatekeeper，和现在一样。

## 执行者不要做的事

- 不碰 secret，不生成正式证书，不提交 `key.pem` 或 p12。
- 不推送、不打 tag、不发版、不编辑 release，除非 owner 对第 7 步单独授权。
- 不改 publish job，不改 Windows 和 Linux 的打包目标。
- spike 失败时不自行改用自写替换器。
- 不提交 E2E 用的临时 version 改动和测试证书。
