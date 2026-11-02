# Tauri v2 M0：最小复现与跨平台交接

状态：osa-a1（Ubuntu 24.04 / aarch64）执行已记录在 `../docs/plans/tauri-rewrite.md` 的 M0 项下。Mac 和 Windows **尚未执行**。本目录不进入正式产品。M0 尚未完成三平台验收，不能据此进入 M1。

审查修复后的Linux复测：`cargo test --release` 为 **6 passed / 0 failed**。实际soak **3600.008 s / 120轮重新加载**，全部轮次核对代理和上游Range；排除600 s warmup后OLS为 **−0.254942 MB/min**，最后30 min为 **−0.335103 MB/min**，仅本轮满足预设条件，不宣称长期无泄漏。359点原始RSS、逐轮流量和判定已提交到下列evidence文件。

## 边界与证据

- 只在 `spike/tauri-m0` 本地提交。没有 push、tag、release、远端资源或 secrets 变更。
- `m0/src-tauri/` 是一个主窗口、自定义协议、歌词原型、官方插件和计划依赖；`m0/web/` 是独立测试页。`web/assets/` 内嵌了既有 `dist/web`，用于 S4 **体积**测试；现有产品页还没有 ElectronAPI→Tauri 适配，不能把本工程当成可登录的完整产品。
- Linux WebKitGTK 2.52.6 拒绝媒体自定义协议（`Requested protocol: biu-media (allowed: no)`）。Linux 测试采用 S2 退路 A：Rust 绑定 `127.0.0.1:0`，每次启动生成一个随机 token，本次进程的请求共用，退出即失效；不是每请求一次性 token。返回200初始流、206 Range流。Host严格校验为实际`127.0.0.1:<port>`，ACAO仅为app源（Linux/macOS为`tauri://localhost`，Windows为`http://tauri.localhost`），显式外部Origin拒绝；原生媒体后端缺省Origin时仍须正确Host及token。token不写日志。
- 夹具及写死403刷新必须显式设置`BIU_SPIKE_FIXTURES=1`才启用（Linux runner自动设置）；默认关闭。HTTP退路仅允许`http://127.0.0.1:18765`的固定夹具路径。已移除`*.akamaized.net`等后缀放行；这些规则不得带入M2，M2只能放行playurl返回的具体host。**真实B站CDN、真实URL续签尚未验证**。所有手动测试终端都须设置测试开关，不能把该开关当成正式配置。
- 缓存命中的 seek 可以没有网络请求；实际发生的 Range 请求必须返回 206，不能重新请求整文件。首个无 Range 请求返回 200 符合 HTTP 语义。一般原始日志忽略提交；60 min实测RSS序列特例提交为`evidence/soak-60min-rss.jsonl`，逐轮代理/上游Range证明为`soak-60min-rounds.json`，斜率/判定为`soak-60min-summary.json`。历史30 min缓存文件循环不能证明代理内存通过，内存格先记“待定”。
- `bundleMediaFramework: true` 的 AppImage 已通过 AAC / FLAC / 10 次 seek 冒烟。初始未打媒体插件的 AppImage 未通过解码检测，不能交付。AppImage 不受 D1 的 10 MB 限制，优先交付 deb/rpm。
- 无头测试验证解码、时钟、波形、API 与 D-Bus。它不验证实际扬声器、窗口透明视觉效果、真实合成器、Wayland、实体媒体键或全屏 app 上方置顶。
- 当前 S7 用虚拟登录夹具验证 store 保留，不代表 B 站登录迁移已通过。更新命令模拟“退出并安装”：下载、验签、安装，然后退出；正式退出生命周期尚未接入。
- 任何 GUI 授权、UAC、Gatekeeper、安装许可、扫码确认或验证码交互：**执行代理停下并报告，由用户亲自处理，不自动同意**。不要用 `xattr -dr`、系统安全配置修改或权限提升绕过。
- 私钥只在本机 `m0/private/`，不传给另一台机器。另一台机器生成自己的测试钥匙。不要回传 private、cookie、二维码、验证码响应或真实音频。

## 0. 代码如何从 osa-a1 取得（分支没有 push）

本次收尾在 osa-a1 的 spike worktree 中创建两个本地文件，均不入 Git：

- `spikes/m0/private/biu-tauri-m0.bundle`：完整本地分支，可在新目录 clone，无需 GitHub。
- `spikes/m0/private/m0-inputs.tar.gz`：**仅**既有 `dist/web` 快照、原创 440 Hz 正弦波 AAC（300 s）和 FLAC（10 s）。不含私钥或用户音乐。
- `spikes/m0/private/transport-SHA256SUMS.txt`：传输校验。

下面命令假定用户已配置 `osa-a1-tailnet` SSH 别名。若 SSH 要密码、主机指纹确认、连接失败或别名不存在，停下请用户处理，不猜测 IP，不修改网络。全程不 push。

### Mac（在新的工作目录中）

```bash
mkdir -p ~/Developer/biu-m0-transfer
cd ~/Developer/biu-m0-transfer
scp 'osa-a1-tailnet:~/.t3/worktrees/biu/spike-tauri-m0/spikes/m0/private/biu-tauri-m0.bundle' .
scp 'osa-a1-tailnet:~/.t3/worktrees/biu/spike-tauri-m0/spikes/m0/private/m0-inputs.tar.gz' .
scp 'osa-a1-tailnet:~/.t3/worktrees/biu/spike-tauri-m0/spikes/m0/private/transport-SHA256SUMS.txt' .
shasum -a 256 -c transport-SHA256SUMS.txt
git clone --branch spike/tauri-m0 biu-tauri-m0.bundle biu-tauri-m0
cd biu-tauri-m0
git status -sb
tar -xzf ../m0-inputs.tar.gz -C spikes
```

### Windows（PowerShell，在新的工作目录中）

```powershell
New-Item -ItemType Directory -Force "$HOME\Developer\biu-m0-transfer" | Out-Null
Set-Location "$HOME\Developer\biu-m0-transfer"
scp 'osa-a1-tailnet:~/.t3/worktrees/biu/spike-tauri-m0/spikes/m0/private/biu-tauri-m0.bundle' .
scp 'osa-a1-tailnet:~/.t3/worktrees/biu/spike-tauri-m0/spikes/m0/private/m0-inputs.tar.gz' .
scp 'osa-a1-tailnet:~/.t3/worktrees/biu/spike-tauri-m0/spikes/m0/private/transport-SHA256SUMS.txt' .
Get-FileHash .\biu-tauri-m0.bundle, .\m0-inputs.tar.gz -Algorithm SHA256
Get-Content .\transport-SHA256SUMS.txt # 比较上面的摘要，不能忽略不一致
# 以下目录必须尚不存在。
git clone --branch spike/tauri-m0 .\biu-tauri-m0.bundle biu-tauri-m0
Set-Location .\biu-tauri-m0
git status -sb
tar -xzf ..\m0-inputs.tar.gz -C spikes
```

也可在已含基线 `ff1e6a6` 的独立 clone 中从 osa-a1 本地仓库 fetch。不要在用户正在使用的主 checkout 切分支：

```bash
git fetch 'osa-a1-tailnet:~/Developer/projects/biu' spike/tauri-m0:refs/remotes/osa-a1/spike-tauri-m0
# 只在新建的独立工作目录中检出；不要 merge 或 push。
git switch --create spike/tauri-m0 --no-track refs/remotes/osa-a1/spike-tauri-m0
```

## 1. Mac：MateBook，Apple Silicon arm64

### 1.1 安装与构建

本轮Mac测试要求 **macOS 14+**（tauri已启用`macos-proxy`，WKWebView代理API要求macOS 14+）。先由用户准备Xcode Command Line Tools、Homebrew、Node.js（22或更新LTS）、Python 3、Rust stable。以下安装命令出现 GUI/许可确认时暂停，由用户处理；执行者不替用户确认。

```bash
uname -m                         # 必须记录 arm64
sw_vers
xcode-select -p                  # 若缺少：用户运行 xcode-select --install 并完成安装
# Homebrew 已安装后：
brew install node python
curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs -o /tmp/biu-m0-rustup.sh
sh /tmp/biu-m0-rustup.sh -y --profile minimal
source "$HOME/.cargo/env"
rustup component add rustfmt
node --version; npm --version; python3 --version; rustc --version
cd ~/Developer/biu-m0-transfer/biu-tauri-m0/spikes
npm ci --ignore-scripts --no-audit --no-fund
mkdir -p evidence m0/private
cd m0
../node_modules/.bin/tauri build --ci --bundles dmg > ../evidence/mac-build.log 2>&1
# macOS 覆盖配置自动选择 dmg / ad-hoc 签名 / icns。
export BIU_SPIKE_FIXTURES=1 BIU_FIXTURES="$PWD/fixtures"
export BIU_LOG="$PWD/../evidence/mac.jsonl"
export BIU_SOAK_SECONDS=3600
```

开第二个终端，提供本地上游（保持运行，测试结束 Ctrl-C）：

```bash
cd ~/Developer/biu-m0-transfer/biu-tauri-m0/spikes
python3 scripts/fixture_server.py --fixtures m0/fixtures --log evidence/mac-server.jsonl
```

### 1.2 S1：fMP4 FLAC / Dolby，各三首

1. 用户自行提供有权使用且每首长于 90 s 的 B 站 fMP4 文件，放在 `spikes/m0/fixtures/`，命名 `flac-1.m4a`–`flac-3.m4a`、`dolby-1.m4a`–`dolby-3.m4a`。**把原始 FLAC 改名为 m4a 不算 fMP4 FLAC**。不把音频加入 Git 或回传包。
2. 启动交互测试页：

```bash
cd ~/Developer/biu-m0-transfer/biu-tauri-m0/spikes/m0
export BIU_SPIKE_FIXTURES=1 BIU_FIXTURES="$PWD/fixtures" BIU_LOG="$PWD/../evidence/mac-s1.jsonl" BIU_MODE=interactive
./src-tauri/target/release/bundle/macos/'Biu M0.app'/Contents/MacOS/biu-m0
```

3. 在测试页选择一个样本，点击 `Custom protocol: 60 s + 5 seeks + waveform`，等到 `sample_complete`。逐首执行；不要同时点击两次。
4. 每首记录：文件编码/来源说明（不含签名 URL）、`canPlayType`（`flac`、`fLaC`、`ec-3`）、连续播放毫秒、5 次 seek 的恢复毫秒、非零波形最大值、是否有实际声音/中断。日志包含 `sample_*` 和 `waveform`。第一次需确认音频输出，用户亲自操作。
5. FLAC 不通过：复测合成 AAC，并记录“Mac lossless 降级最高码率 AAC，音质设置需标注”。ec-3 不通过：按 `canPlayType` 跳过 Dolby。两个退路是计划允许的，不能伪造原格式通过。

### 1.3 S2：自定义协议优先，然后 HTTP 退路

```bash
cd ~/Developer/biu-m0-transfer/biu-tauri-m0/spikes/m0
export BIU_SPIKE_FIXTURES=1 BIU_FIXTURES="$PWD/fixtures" BIU_LOG="$PWD/../evidence/mac-s2-custom.jsonl"
export BIU_MODE=custom BIU_SOAK_SECONDS=3600
./src-tauri/target/release/bundle/macos/'Biu M0.app'/Contents/MacOS/biu-m0 &
APP_PID=$!
python3 ../scripts/sample_memory.py --pid "$APP_PID" --seconds 3600 --out ../evidence/mac-s2-custom-memory.jsonl
wait "$APP_PID"
```

若自定义协议失败，保留失败日志，改为 `BIU_MODE=full`、日志名 `mac-s2-http.jsonl` 重跑同一命令。HTTP 退路测试可执行 `python3 ../scripts/check_evidence.py playback ../evidence/mac-s2-http.jsonl`。自定义协议日志是 `range` 事件，不要套用仅检查 HTTP 退路的断言。

记录：10 个 seed=2401 的随机目标、每次 ≤1500 ms、实际网络请求的 Range/206/字节数、403→新地址继续播放、≥60 min内存曲线。soak每30 s重新加载带round的新URL并强制Range seek，不再`audio.loop`缓存文件。排除600 s warmup后计算全段和最后30 min OLS斜率（MB/min）；两段均≤0才可记本轮内存通过，正斜率仍待定，不用GC回落代替斜率。缓存命中单独注明。macOS 的 WebKit 服务可能被 launchd 接管，脚本只统计可识别的子进程；须用 Activity Monitor 补记录相关 WebKit 服务 RSS，不能把主进程数值当总内存。合成上游不等同于真实 CDN 续签，真实 CDN 单列待测。

### 1.4 S3–S6、S8

| 项目 | 直接操作 | 必须记录 / 通过条件 |
|---|---|---|
| S3 | 不在 Mac 测 Linux gst-libav | 填“不适用；Linux VM 矩阵待测” |
| S4 | `stat -f '%z %N' src-tauri/target/release/biu-m0 src-tauri/target/release/bundle/dmg/*.dmg`；在 `src-tauri/` 执行 `cargo install cargo-bloat --locked`、`cargo bloat --release --crates -n 15 > ../../evidence/mac-bloat.txt` | dmg 字节及 MB=字节/1000000，arm64 目标 ≤8 MB，硬上限 10 MB；bloat 会重建未剥离副本，其文件大小不等于交付二进制 |
| S5 | 用 `BIU_MODE=interactive` 启动，点击 `Lyrics prototype`；将光标移入窗口左上 100×60 解锁区域，移出；在下方放普通窗口；再把另一 app 全屏 | 透明无黑边、全屏 app 上方置顶、锁定点击穿透、悬停 ≤1000 ms 出现可操作区域。当前按钮只是原型，不含完整解锁状态机；缺失行为填不通过/待补，不视作完成。记录 60 s 的 `lyrics_frames`、Activity Monitor 歌词相关进程 CPU，目标60 fps、CPU<10% |
| S6 | `BIU_MODE=controls` 启动可测 no-referrer 下 hdslb 图片；`BIU_WEBVIEW_PROXY=http://127.0.0.1:18765` 可打开独立 proxy 测试窗口 | 图片成功、代理服务器记录 `/art.svg` 且 Referer为空。该探针在Mac上要求macOS 14+。Linux实测代理作用于共享context，与S2 loopback冲突，**Linux webview代理不通过，采用只代理Rust流量**；正式策略列为计划§7 D6待用户拍板。**二维码/极验页面尚未接入这个播放器原型，不能在此工程完成“完整登录”验收**；需补登录复现页，保持待验证，不用外部浏览器登录代替 |
| S8 | `BIU_MODE=full` 播放时打开 macOS 正在播放，查看标题/封面，用实体媒体键暂停/播放/上一首/下一首 | 记录每个动作、前端时间是否停止/继续及 `media_action`。若原生 mediaSession 不行，计划退路是 souvlaki；当前 souvlaki 启动接线只实现 Linux，Mac 需另补，不能声称已验证 |

### 1.5 S7：仅本地 HTTP 的 0.0.1 → 0.0.2

下面命令只生成本机测试钥匙，不打印。确保 `m0/private/updater.key` 尚不存在，否则复用，不覆盖。先保存 0.0.1 `.app`，再构建 0.0.2。

```bash
cd ~/Developer/biu-m0-transfer/biu-tauri-m0/spikes/m0
mkdir -p private/http
chmod 700 private
../node_modules/.bin/tauri signer generate --ci -p '' -w private/updater.key > private/signer-generate.log 2>&1
chmod 600 private/updater.key private/updater.key.pub
ditto 'src-tauri/target/release/bundle/macos/Biu M0.app' 'private/Biu M0 0.0.1.app'
../node_modules/.bin/tauri build --ci --bundles dmg --config '{"version":"0.0.2"}' > ../evidence/mac-build-002.log 2>&1
tar -czf private/http/Biu-M0-0.0.2.app.tar.gz -C src-tauri/target/release/bundle/macos 'Biu M0.app'
../node_modules/.bin/tauri signer sign -f private/updater.key -p '' --app-version 0.0.2 private/http/Biu-M0-0.0.2.app.tar.gz > private/signer-sign.log 2>&1
python3 ../scripts/make_update_manifest.py --artifact private/http/Biu-M0-0.0.2.app.tar.gz --target darwin-aarch64 --out private/http
# 单独终端运行（结束后 Ctrl-C）：
# python3 -m http.server 18766 --bind 127.0.0.1 --directory private/http
```

用户确认测试目录没有其他应用后，先测 `~/Applications`。不替换正式 `Biu.app`：

```bash
mkdir -p "$HOME/Applications"
test ! -e "$HOME/Applications/Biu M0.app" || { echo 'Existing M0 test app: stop and ask user'; exit 1; }
ditto 'private/Biu M0 0.0.1.app' "$HOME/Applications/Biu M0.app"
export BIU_UPDATE_PRIVATE="$PWD/private" BIU_UPDATE_ALLOW_MAC_TEST_LOCATIONS=1
export BIU_LOG="$PWD/../evidence/mac-update-user-applications.jsonl" BIU_MODE=update
"$HOME/Applications/Biu M0.app/Contents/MacOS/biu-m0"
export BIU_MODE=verify-update
"$HOME/Applications/Biu M0.app/Contents/MacOS/biu-m0"
```

分别检查：`startup 0.0.1`、`update_verified 0.0.2`、安装成功、重启 `0.0.2`、`settings_preserved=true`、`login_fixture_preserved=true`。这些只验证虚拟 store 保留。下载失败/权限弹窗保留原始错误，Mac 退路为 manual 替换。

`/Applications/Biu M0.app` 需要用户亲自放入测试 0.0.1，再把上述路径改为 `/Applications/Biu M0.app` 重跑。如果不能写，不提升权限，记 manual。用户再挂载本机生成的 0.0.1 dmg 并从 dmg 启动（用户自己处理 Gatekeeper），设置同样环境运行 `.app/Contents/MacOS/biu-m0`；应记录 `update_manual`（dmg / AppTranslocation）。**若因 Gatekeeper 无法启动，则此格为阻塞，不能推断 manual 检测通过。** 从 shell 启动未必会 translocate，必须记录实际路径是否含 `AppTranslocation`，缺少真实 translocation 的测试仍待完成。

### 1.6 Mac 要回传什么

在 `spikes/evidence/mac-results.md` 按 S1–S8 写“通过/不通过/待验证”、系统版本、arm64、WKWebView 版本、真实数字及阻塞。回传 `mac*.jsonl`（先去 cookie/URL/token）、`mac-bloat.txt`、`mac-results.md` 和必要的无敏感信息截图。不要回传音频、构建产物、private 或签名文件。

```bash
cd ~/Developer/biu-m0-transfer/biu-tauri-m0/spikes
# 确认以下文件名不含 private/音频；不要打包整个 evidence 或工程。
python3 - <<'PY'
from pathlib import Path
import tarfile
with tarfile.open('m0/private/mac-results.tar.gz','w:gz') as t:
    for p in Path('evidence').glob('mac*'):
        if p.suffix in {'.jsonl','.txt','.md','.png'}: t.add(p,arcname=p.as_posix())
PY
# 用户检查脱敏后再通过 T3 附件或自己的 scp 回传，不自动对外发送。
```

## 2. Windows：PC，x64 / WebView2

### 2.1 安装与构建

用户准备 Git for Windows、Node.js LTS、Python 3、Rust stable MSVC、Visual Studio 2022 Build Tools 的“Desktop development with C++”（MSVC v143 + Windows SDK）和 WebView2 Evergreen Runtime。Windows 11 通常已有 WebView2，但仍记录实际版本。安装 GUI / UAC 必须由用户确认，执行代理不代点。

可在普通 PowerShell 检查或启动交互安装（已安装的跳过；不要加自动接受协议的参数）：

```powershell
git --version; node --version; python --version; rustc --version
winget install --exact --id OpenJS.NodeJS.LTS --interactive
winget install --exact --id Python.Python.3.12 --interactive
winget install --exact --id Rustlang.Rustup --interactive
winget install --exact --id Microsoft.VisualStudio.2022.BuildTools --interactive
# 用户在 VS Installer 选择 C++ 桌面开发和 Windows SDK；完成后新开终端。
winget install --exact --id Microsoft.EdgeWebView2Runtime --interactive
```

若系统禁用 winget，用户从官方安装器安装；不要修改执行策略/系统安全配置绕过。完成 §0 的 bundle 导入后：

```powershell
Set-Location "$HOME\Developer\biu-m0-transfer\biu-tauri-m0\spikes"
npm ci --ignore-scripts --no-audit --no-fund
New-Item -ItemType Directory -Force evidence,m0\private | Out-Null
Set-Location m0
& ..\node_modules\.bin\tauri.cmd build --ci --bundles nsis *> ..\evidence\pc-build.log
if ($LASTEXITCODE -ne 0) { throw 'Build failed; inspect pc-build.log' }
$env:BIU_SPIKE_FIXTURES='1'; $env:BIU_FIXTURES="$PWD\fixtures"
$env:BIU_SOAK_SECONDS='3600'
```

另一个 PowerShell 终端提供上游：

```powershell
Set-Location "$HOME\Developer\biu-m0-transfer\biu-tauri-m0\spikes"
python scripts\fixture_server.py --fixtures m0\fixtures --log evidence\pc-server.jsonl
```

### 2.2 S1–S6、S8

S1 只要求 Mac，PC 可附测 `canPlayType`，不能把 Windows 结果填入 Mac 栏。S3 是 Linux 矩阵，不适用 PC。

S2 先测 WebView2 自定义协议，失败再切 HTTP：

```powershell
Set-Location "$HOME\Developer\biu-m0-transfer\biu-tauri-m0\spikes\m0"
$env:BIU_SPIKE_FIXTURES='1'; $env:BIU_FIXTURES="$PWD\fixtures"; $env:BIU_LOG="$PWD\..\evidence\pc-s2-custom.jsonl"
$env:BIU_MODE='custom'; $env:BIU_SOAK_SECONDS='3600'
$app=Start-Process -FilePath "$PWD\src-tauri\target\release\biu-m0.exe" -PassThru
python ..\scripts\sample_memory.py --pid $app.Id --seconds 3600 --out ..\evidence\pc-s2-custom-memory.jsonl
Wait-Process -Id $app.Id -ErrorAction SilentlyContinue
# 自定义协议失败才重跑：
# $env:BIU_MODE='full'; $env:BIU_LOG="$PWD\..\evidence\pc-s2-http.jsonl"
# 同上 Start-Process / sample_memory / Wait-Process。
# HTTP 退路完成后：python ..\scripts\check_evidence.py playback ..\evidence\pc-s2-http.jsonl
```

| 项目 | 操作 | 数字与判定 |
|---|---|---|
| S2 | 对照 S2 JSONL，检查 Range 头、206、10 个随机 seek，测试403刷新，每30 s重新加载并强制Range，跑满≥60 min | 每次恢复≤1500 ms；记录整个进程树RSS，含msedgewebview2.exe，按Mac节相同的600 s warmup后OLS斜率规则判定，不以末次回落记通过。部分缓存命中注明。真实 CDN 过期续签仍另测 |
| S4 | `Get-Item .\src-tauri\target\release\biu-m0.exe, .\src-tauri\target\release\bundle\nsis\*.exe | Select-Object Name,Length,@{n='MB';e={$_.Length/1000000}}`；`Push-Location src-tauri; cargo install cargo-bloat --locked; cargo bloat --release --crates -n 15 > ..\..\evidence\pc-bloat.txt; Pop-Location` | x64 NSIS≤7 MB，硬上限10 MB，裸 exe 单独记录。WebView2 bootstrapper 是在线安装，不能把系统 runtime 计入小包冒充自包含 |
| S5 | `BIU_MODE=interactive` 启动并点击歌词原型；下面放一个记事本窗口，测试透过歌词点击、移到左上解锁区域、普通窗口上置顶、透明边缘 | 悬停≤1000 ms、扫光60 s 的 fps、任务管理器歌词相关进程 CPU<10%。当前解锁按钮是原型、非完整状态机，不能标产品交互已通过 |
| S6 | `BIU_MODE=controls` 可检查 hdslb 图片；设置 `$env:BIU_WEBVIEW_PROXY='http://127.0.0.1:18765'` 启动可检查 proxy 窗口，然后 `Remove-Item Env:BIU_WEBVIEW_PROXY` | 记录图片加载、代理日志 Referer 为空。二维码和极验最小页面尚未接入，保持待验证；必须由用户进行交互授权，不可自动扫码或同意 |
| S8 | 播放时打开 Windows 媒体浮层/SMTC，观察标题/封面；实体媒体键播放/暂停/上下首 | 记录前端是否停止/继续和 `media_action`。如 mediaSession 不行，计划退路 souvlaki 的 Windows 接线尚未实现，记录待补；不能用 Linux MPRIS 通过替代 |

### 2.3 S7：当前用户 NSIS 的本地更新

只安装测试产品 **Biu M0**（identifier `com.biu.spike.m0`），不能覆盖正式 Biu。私钥只在 PC 本地 `spikes/m0/private/` 生成，不拷贝 osa-a1 的钥匙。

```powershell
Set-Location "$HOME\Developer\biu-m0-transfer\biu-tauri-m0\spikes\m0"
New-Item -ItemType Directory -Force private\http | Out-Null
if (-not (Test-Path private\updater.key)) {
    & ..\node_modules\.bin\tauri.cmd signer generate --ci --password= -w private\updater.key *> private\signer-generate.log
}
Copy-Item .\src-tauri\target\release\bundle\nsis\*.exe .\private\Biu-M0-0.0.1-setup.exe
# 用临时配置文件避免 PowerShell 吞掉 JSON 引号。
'{"version":"0.0.2"}' | Set-Content -Encoding ascii .\private\v002.json
& ..\node_modules\.bin\tauri.cmd build --ci --bundles nsis --config .\private\v002.json *> ..\evidence\pc-build-002.log
$artifact=(Get-ChildItem .\src-tauri\target\release\bundle\nsis\*0.0.2*.exe | Select-Object -First 1).FullName
& ..\node_modules\.bin\tauri.cmd signer sign -f .\private\updater.key --password= --app-version 0.0.2 $artifact *> private\signer-sign.log
python ..\scripts\make_update_manifest.py --artifact $artifact --target windows-x86_64 --out private\http
# 单独终端：python -m http.server 18766 --bind 127.0.0.1 --directory private\http
```

用户亲自运行 `private\Biu-M0-0.0.1-setup.exe`，选择当前用户安装，**把安装目录设为本 worktree 的 `spikes\m0\private\install`**。出现 UAC/安装许可时执行代理停下，由用户决定，不静默确认。安装后找到该目录内的实际 exe（通常 `biu-m0.exe`）：

```powershell
$env:BIU_UPDATE_PRIVATE="$PWD\private"
$env:BIU_LOG="$PWD\..\evidence\pc-update.jsonl"; $env:BIU_MODE='update'
$appExe=(Get-ChildItem .\private\install -Recurse -Filter biu-m0.exe | Select-Object -First 1).FullName
if (-not $appExe) { throw 'Confirm actual test install location before continuing' }
& $appExe
# 安装器/权限窗口由用户处理；保存完成结果后再重启：
$env:BIU_MODE='verify-update'
& $appExe
```

检查0.0.1→0.0.2、下载进度、签名验证、退出安装、下一次启动版本、虚拟设置/登录夹具保留。失败保留错误，不改 registry/secrets，不升级为管理员安装。NSIS 可能重启应用；记录真实启动次数，不机械套用 Linux 两次 startup 的断言。

### 2.4 PC 要回传什么

写 `evidence/pc-results.md`，逐项 S1–S8 状态，Windows build、CPU/架构、WebView2 版本、每次seek ms、≥60 min RSS及warmup后斜率、NSIS/exe 字节与 MB、歌词 fps/CPU、更新结果/权限阻塞。回传 `pc*.jsonl`、`pc-bloat.txt`、`pc-results.md` 和无敏感信息截图；先人工脱敏。不要回传 private、音频、登录信息或安装器。

```powershell
Set-Location "$HOME\Developer\biu-m0-transfer\biu-tauri-m0\spikes"
python -c "from pathlib import Path; import tarfile; t=tarfile.open('m0/private/pc-results.tar.gz','w:gz'); [t.add(p,arcname=p.as_posix()) for p in Path('evidence').glob('pc*') if p.suffix in {'.jsonl','.txt','.md','.png'}]; t.close()"
# 用户审查脱敏后自行附到 T3 / scp；不要自动发送。
```

## 3. osa-a1 Linux 复现与验收命令

依赖来自 Tauri v2 官方 prerequisites：`libwebkit2gtk-4.1-dev build-essential curl wget file libxdo-dev libssl-dev libayatana-appindicator3-dev librsvg2-dev pkg-config`；测试用 `xvfb gstreamer1.0-tools gstreamer1.0-libav gstreamer1.0-plugins-good`，本机既有 `openbox`、`dbus-run-session`。本次没有改系统配置、卸载包或重启机器。`patchelf` 由 `apt-get download` + `dpkg-deb -x` 解包到 `m0/runtime/patchelf`，没有 sudo 安装它。

```bash
# 在 spike worktree 根目录，输入目录为既有 dist/web 快照；不改源目录。
bash spikes/scripts/prepare_linux.sh /path/to/existing/dist/web
cd spikes
npm ci --ignore-scripts --no-audit --no-fund
source "$HOME/.cargo/env"
cd m0/src-tauri
BIU_SPIKE_FIXTURES=1 BIU_FIXTURES="$PWD/../fixtures" BIU_LOG="$PWD/../../evidence/unit.jsonl" cargo test --release
cd ..
# patchelf 缺少时，仅解包到本 worktree：
mkdir -p runtime/patchelf
(cd runtime/patchelf && apt-get download patchelf && dpkg-deb -x ./*.deb .)
PATH="$PWD/runtime/patchelf/usr/bin:$PATH" APPIMAGE_EXTRACT_AND_RUN=1 ../node_modules/.bin/tauri build --ci --bundles deb,rpm,appimage
cd ../..
dbus-run-session -- xvfb-run -a python3 spikes/scripts/run_linux.py --root spikes --seconds 3600 --name linux-repro
python3 spikes/scripts/check_evidence.py playback spikes/evidence/linux-repro.jsonl
python3 spikes/scripts/analyze_soak.py --events spikes/evidence/linux-repro.jsonl --memory spikes/evidence/linux-repro-memory.jsonl --server spikes/evidence/linux-repro-server.jsonl --out spikes/evidence
# 隔离时同时设置 SYSTEM_PATH 和独立 REGISTRY；只设置 PLUGIN_PATH 不会禁用系统插件。
GST_PLUGIN_SYSTEM_PATH_1_0="$PWD/spikes/m0/runtime/gst-no-libav" GST_PLUGIN_PATH_1_0='' GST_REGISTRY="$PWD/spikes/m0/runtime/isolated-registry.bin" dbus-run-session -- xvfb-run -a python3 spikes/scripts/run_linux.py --root spikes --mode missing --seconds 3 --name missing-repro
python3 spikes/scripts/check_evidence.py missing spikes/evidence/missing-repro.jsonl
BIU_NATIVE_MEDIA=1 dbus-run-session -- xvfb-run -a python3 spikes/scripts/run_linux.py --root spikes --mode controls --seconds 20 --name controls-repro
python3 spikes/scripts/check_evidence.py controls spikes/evidence/controls-repro.jsonl
```

S7 Linux 重跑（复用本地测试 key，已运行时先重新把 **0.0.1** AppImage 复制为 installed）：

```bash
cd spikes/m0
cp 'src-tauri/target/release/bundle/appimage/Biu M0_0.0.1_aarch64.AppImage' private/installed.AppImage
PATH="$PWD/runtime/patchelf/usr/bin:$PATH" APPIMAGE_EXTRACT_AND_RUN=1 ../node_modules/.bin/tauri build --ci --bundles appimage --config '{"version":"0.0.2"}'
../node_modules/.bin/tauri signer sign -f private/updater.key -p '' --app-version 0.0.2 'src-tauri/target/release/bundle/appimage/Biu M0_0.0.2_aarch64.AppImage' > private/sign.log 2>&1
python3 ../scripts/make_update_manifest.py --artifact 'src-tauri/target/release/bundle/appimage/Biu M0_0.0.2_aarch64.AppImage' --target linux-aarch64 --out private/http
cd ../..
dbus-run-session -- xvfb-run -a python3 spikes/scripts/update_linux.py --root spikes
python3 spikes/scripts/check_evidence.py update spikes/evidence/update.jsonl
```

首次运行需先按 Mac 节的 `tauri signer generate` 命令在 Linux 本地生成 key。`update_linux.py` 只替换 private 下 AppImage，并把 XDG store/cache 放在 private；不运行 apt/rpm 安装器，不弹系统授权框。

`cargo bloat --release --crates -n 15` 在 `spikes/m0/src-tauri` 执行，结果写 `../../evidence/cargo-bloat.txt`。它会重建未 strip 的分析副本，测交付体积请使用 `evidence/sizes.json` 中已打包产物和哈希，或重新 release build 后再测；不能用 bloat 的 ELF 大小代替发布二进制。

## 4. 仍待完成的矩阵

- Mac S1 全部、S2/S5/S6/S7/S8；Windows S2/S5/S6/S7/S8。mac/Windows 下载包体积待对应机器或 CI。
- **wsl 的 WebKitGTK（S2必测）**，以及独立Linux x64实机/VM；Ubuntu 24.04 / Fedora 41+ / Arch干净VM、Fedora libav 依赖源是否可解析（可能需要 RPM Fusion）、Wayland GNOME/KDE、真实 X11 合成器、物理媒体键、真实 CDN URL 续签。
- S5 完整解锁 UI/锁定状态机、S6 QR/极验最小页面、Mac/Windows souvlaki 接线尚未实现。上述项目不能用当前播放器冒烟代替。
- 本轮按用户采纳的Claude审查意见逐条修复；未启动新的审查，不宣称最终验收通过。歌词悬停轮询现持续到窗口Destroyed，不再65 s截止；S5仍为“未验证（仅API调用无报错）”。
