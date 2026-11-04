#!/usr/bin/env bash
# 步骤 1.4–1.6：装 A，起 B 的 generic feed，验证后台下载 → 退出时 Squirrel 安装 → 重启后为 B；再验证 ad-hoc 降级。
set -uo pipefail
A=${1:?A artifacts}; B=${2:?B artifacts}; EV=${3:?evidence dir}; A_VER=$4; B_VER=$5
mkdir -p "$EV"
LOG="$HOME/Library/Logs/Biu/main.log"
plist_ver() { /usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' /Applications/Biu.app/Contents/Info.plist; }
wait_log() { # pattern timeout
  for _ in $(seq 1 "$2"); do grep -qE "$1" "$LOG" 2>/dev/null && return 0; sleep 1; done; return 1; }
fail() { echo "SPIKE FAIL: $*" | tee -a "$EV/result.txt"; collect; exit 1; }
collect() {
  cp "$LOG" "$EV/main.log" 2>/dev/null
  cp -R "$HOME/Library/Caches/com.biu.wood3n.ShipIt" "$EV/ShipIt" 2>/dev/null
  ls -la "$HOME/Library/Caches/biu-updater" > "$EV/updater-cache.txt" 2>&1
  pgrep -fl Biu > "$EV/ps.txt" 2>&1
}

dmg=$(ls "$A"/Biu-*-mac-arm64.dmg | head -1)
mnt=$(hdiutil attach -nobrowse -readonly "$dmg" | awk -F'\t' '/\/Volumes\//{print $NF}')
ditto "$mnt/Biu.app" /Applications/Biu.app
hdiutil detach "$mnt" -quiet
echo "installed=$(plist_ver)" | tee "$EV/result.txt"
codesign -dv --verbose=2 /Applications/Biu.app > "$EV/A-codesign.txt" 2>&1
codesign -d -r- /Applications/Biu.app >> "$EV/A-codesign.txt" 2>&1

(cd "$B" && python3 -m http.server 8765 --bind 127.0.0.1 > "$EV/feed.log" 2>&1) &
sleep 2
curl -sf http://127.0.0.1:8765/latest-mac.yml > "$EV/latest-mac.yml" || fail "feed not serving latest-mac.yml"

rm -f "$LOG"
open /Applications/Biu.app
wait_log 'mode=(auto|manual)' 120 || fail "no mode log"
grep -q 'mode=auto' "$LOG" || fail "A not in auto mode"
wait_log 'has been downloaded' 600 || fail "update not downloaded"
sleep 30  # 等 Squirrel.Mac 从本地代理取完 zip
osascript -e 'tell application "Biu" to quit' || pkill -TERM -x Biu
for _ in $(seq 1 180); do [ "$(plist_ver)" = "$B_VER" ] && break; sleep 1; done
[ "$(plist_ver)" = "$B_VER" ] || fail "after quit version=$(plist_ver), expected $B_VER"
echo "quit-install: OK ($A_VER -> $(plist_ver))" | tee -a "$EV/result.txt"
sleep 15
pgrep -x Biu >/dev/null && echo "relaunch-after-install: running" | tee -a "$EV/result.txt" \
  || echo "relaunch-after-install: not running" | tee -a "$EV/result.txt"
codesign --verify --deep --strict /Applications/Biu.app && echo "B verify: OK" | tee -a "$EV/result.txt"
collect
cp "$LOG" "$EV/main-auto.log" 2>/dev/null
pkill -x Biu; sleep 3

# 1.6 manual 降级：A 重新做 ad-hoc 签名
rm -rf /Applications/Biu.app "$HOME/Library/Caches/biu-updater" "$HOME/Library/Caches/com.biu.wood3n.ShipIt"
mnt=$(hdiutil attach -nobrowse -readonly "$dmg" | awk -F'\t' '/\/Volumes\//{print $NF}')
ditto "$mnt/Biu.app" /Applications/Biu.app; hdiutil detach "$mnt" -quiet
codesign --force --deep --sign - /Applications/Biu.app
rm -f "$LOG"
open /Applications/Biu.app
wait_log 'mode=(auto|manual)' 120 || fail "adhoc: no mode log"
sleep 30
grep -q 'mode=manual' "$LOG" || fail "adhoc not manual"
grep -qiE 'squirrel|ShipIt|code signature' "$LOG" && fail "adhoc: squirrel activity in log"
[ "$(plist_ver)" = "$A_VER" ] || fail "adhoc: version changed"
echo "adhoc-manual: OK" | tee -a "$EV/result.txt"
cp "$LOG" "$EV/main-adhoc.log"
pkill -x Biu
echo "SPIKE PASS" | tee -a "$EV/result.txt"
