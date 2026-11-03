# Mac Tauri M0 acceptance — stopped at DMG packaging

## Decision and execution boundary

**M0 Mac acceptance is incomplete. This run provides no approval to start M1.**

Execution date: 2026-11-03 UTC. Executor followed the owner-approved `docs/plans/tauri-rewrite.md` and read `spikes/README.md` completely before installation/build. The plan and application sources were not modified.

All checkout operations used `/Users/mianmian/Developer/biu-m0-transfer/biu-tauri-m0`, branch `spike/tauri-m0`, cloned from the local bundle. `/Users/mianmian/Developer/projects/biu` was not inspected, switched, or modified. The transfer directory did not exist before this run. SSH used the existing `osa-a1-tailnet` alias with `BatchMode=yes` and `StrictHostKeyChecking=yes`; no credential or host-key interaction occurred.

README §1.1 `tauri build --ci --bundles dmg` exited **1**. Rust compilation succeeded in **2m 48s**, and the ad-hoc-signed `.app` was generated. DMG packaging failed in `bundle_dmg.sh` with AppleScript syntax error **-2741**, `Expected end of line but found class name.` The log also contains hdiutil deprecation warnings; their causal relationship to the AppleScript failure was not established. No completed DMG exists.

Per the explicit instruction to stop on an uncovered problem rather than improvise, further runtime tests and S7 preparation were stopped. No packaging workaround, source fix, security change, permission approval, or alternative build was attempted. The existing `.app` was not launched. A successful Rust compile does not establish runtime correctness. No sudo/password/license/Gatekeeper/TCC prompt was observed in tool output; this is not proof that future GUI launches need no permission.

## Transport verification

`shasum -a 256 -c transport-SHA256SUMS.txt`:

```text
biu-tauri-m0.bundle: OK
m0-inputs.tar.gz: OK
```

Full SHA256:

```text
9bdd4a9b47742372d69c18f0c576a6eb1368961794030c71b69422baa0874bca  biu-tauri-m0.bundle
a564da90964aff2ad2a6800d9aace2f150dfe1dfebb5d5b2d533615284f4968e  m0-inputs.tar.gz
```

The archive was checked for absolute paths, parent traversal, and links before extraction into `spikes/`. Its supplied audio consists only of `aac.m4a` and `tone.flac`; no file was renamed to impersonate fMP4 FLAC.

## System and tools

| Property | Actual value |
|---|---|
| `uname -m` | `arm64` |
| macOS | `27.0.1`, build `26A434` |
| Xcode developer directory | `/Applications/Xcode.app/Contents/Developer` |
| Clang | `21.0.0 (clang-2100.3.34.2)` |
| Homebrew | `7.0.10` |
| Node | `v26.10.0` |
| npm | `11.19.1` |
| Python | `3.14.6` |
| Rust | `rustc 1.97.1 (8bab26f4f 2026-07-14)` |
| Cargo | `1.97.1 (c980f4866 2026-06-30)` |
| Active toolchain | `stable-aarch64-apple-darwin (default)` |
| Git | `2.55.0` |
| System WebKit framework short version | `22625` |
| System WebKit framework build | `22625.1.29.11.28` |

WebKit values are read from the installed framework Info.plist; the WKWebView runtime was not launched. Existing prerequisites were reused. No Homebrew install or rustup bootstrap was needed. `rustup component add rustfmt` reported that rustfmt was already up to date. `npm ci --ignore-scripts --no-audit --no-fund` succeeded (`added 2 packages in 2s`). PATH included `/opt/homebrew/bin` and `$HOME/.cargo/bin`.

## S1–S8 status table

| Item | Status | Measurements / evidence / blockers |
|---|---|---|
| S1 | pending-owner (samples needed) | All six `flac-1..3.m4a`, `dolby-1..3.m4a` are missing. 0/6 owner samples tested; playback, seek ms and waveform: N/A. No substitute fMP4 was fabricated. |
| S2 | blocked; not run | Stopped after uncovered DMG build failure. Custom protocol not launched; HTTP fallback not attempted. Soak 0/3600 s; 0 seek measurements; RSS, post-600 s OLS and final-30-min OLS: N/A. Fixture server not started. WebKit RSS and real CDN renewal remain unverified. |
| S3 | not applicable | 不适用；Linux VM 矩阵待测。No Mac gst-libav test performed. |
| S4 | failed (DMG build); binary size recorded | No completed DMG; dmg bytes/MB: N/A, ≤8 MB target and ≤10 MB hard limit unassessed. Release binary: 8,258,528 bytes / 8.258528 MB. Bundled app executable: 8,210,656 bytes / 8.210656 MB. `cargo-bloat` install/analysis not run after stop. These executable sizes do not establish download-package compliance. |
| S5 | blocked; needs owner in person | Runtime not launched; frames/fps, CPU%, hover latency: N/A. Transparency, click-through, fullscreen always-on-top unverified. README identifies an incomplete unlock state machine. |
| S6 | blocked; not run | Image width/load and proxy `/art.svg`/empty Referer: N/A. QR login and Geetest are not implemented in this prototype and remain unverified. No browser login or real credentials used. |
| S7 | blocked; not run | No 0.0.1→0.0.2 update performed. `~/Applications/Biu M0.app` was absent at preflight and was not created. No local test key generated, 0.0.2 build, update server, install, signature verification or preservation test performed. `/Applications`, DMG and AppTranslocation cases also unverified. |
| S8 | blocked; needs owner in person | Runtime not launched. Now Playing title/art, physical pause/play/previous/next and `media_action` events: N/A. Mac souvlaki wiring is not implemented per README. |

No OLS calculation was possible because no memory series was collected. No zero slope or zero RSS is asserted. Once S2 can run, both slopes must use decimal MB/min, exclude the first 600 s for the full-segment slope, and separately use the final 30 min. Both slopes must be ≤0 for that run's memory condition; launchd-reparented WebKit RSS still needs Activity Monitor evidence.

## Existing artifact sizes

`mac-sizes.json` records the release executable and bundled executable, measured after the failed build and before any bloat rebuild. The failed build left no `*.dmg` file in `target/release/bundle/dmg/`. No build artifacts, fixture audio, private directory, signatures, or keys are included in the evidence commit.

## Owner actions in person / authorized follow-up

These are **pending instructions**, not executed results. Resolve the packaging blocker with plan owner Claude (osa-a1) before resuming the acceptance run. Do not apply an unapproved packaging workaround. The failed clone already exists; resume inside it rather than rerunning the new-directory transfer step.

### 1. Review and resolve the build blocker

In Terminal:

```bash
cd ~/Developer/biu-m0-transfer/biu-tauri-m0/spikes
less evidence/mac-build.log
```

Ask Claude (osa-a1) to determine the approved response to `bundle_dmg.sh` AppleScript error -2741. After an approved resolution, rerun README §1.1 in this clone. If a license, Finder automation permission, Gatekeeper, or other OS prompt appears, the owner must inspect that exact prompt and decide in person. This run did not establish which, if any, permission is required. Do not use xattr removal or security-setting changes to bypass it.

### 2. Set common launch commands after the blocker is resolved

In a Terminal window:

```bash
export PATH="/opt/homebrew/bin:$HOME/.cargo/bin:$PATH"
cd ~/Developer/biu-m0-transfer/biu-tauri-m0/spikes/m0
export BIU_SPIKE_FIXTURES=1 BIU_FIXTURES="$PWD/fixtures"
export BIU_SOAK_SECONDS=3600
APP="$PWD/src-tauri/target/release/bundle/macos/Biu M0.app/Contents/MacOS/biu-m0"
```

In a second Terminal, start and retain the fixture server for S2/S6:

```bash
cd ~/Developer/biu-m0-transfer/biu-tauri-m0/spikes
python3 scripts/fixture_server.py --fixtures m0/fixtures --log evidence/mac-server.jsonl
```

### 3. S1: provide samples and listen

Put six authorized fMP4 samples longer than 90 s into `spikes/m0/fixtures/` with the exact README names. In the common-launch Terminal:

```bash
BIU_MODE=interactive BIU_LOG="$PWD/../evidence/mac-s1.jsonl" "$APP"
```

In the app, select one sample → click `Custom protocol: 60 s + 5 seeks + waveform` → wait for `sample_complete`. Listen for actual sound and interruption. Repeat separately for all six. Record encoding/source description without signed URLs, `canPlayType`, playback ms, all five seek ms, and waveform maximum. Apply only the README codec fallbacks if needed.

### 4. S2: full soak and Activity Monitor

Run the exact custom-protocol command sequence in README §1.3, including the separate `sample_memory.py --seconds 3600` process and waiting for the app. Only if custom protocol fails, retain its logs and repeat with `BIU_MODE=full` and HTTP evidence names. Do not replace the 3600 s soak with a brief smoke test. Compute and record both required OLS slopes from actual samples.

Open Spotlight → type `Activity Monitor` → press Return → choose View → All Processes → open the Memory tab. Locate `biu-m0` and related WebKit WebContent/Networking/GPU services. Record PID association, timestamp, and memory readings throughout the soak, especially after 600 s and during the final 30 min. Relate these readings to the process-tree sampler; do not call main-process RSS total app memory. Confirm actual audible playback in person.

### 5. S5: lyrics visuals, mouse behavior, fps and CPU

Launch:

```bash
BIU_MODE=interactive BIU_LOG="$PWD/../evidence/mac-s5.jsonl" "$APP"
```

Click `Lyrics prototype` → place a normal window under the lyrics → inspect transparent edges → click through locked lyrics → move the pointer into and out of the top-left 100×60 area → time the actionable hover region. Put another app into fullscreen with its green window button and check whether lyrics stay above it. In Activity Monitor select CPU and record all related process CPU during 60 s animation; collect `lyrics_frames` for fps. Record incomplete unlock-state behavior as incomplete, even if window APIs return success.

### 6. S6: images and proxy probe

With the fixture server running, launch separately, closing the prior app between launches:

```bash
BIU_MODE=controls BIU_LOG="$PWD/../evidence/mac-s6.jsonl" "$APP"
BIU_MODE=controls BIU_WEBVIEW_PROXY=http://127.0.0.1:18765 BIU_LOG="$PWD/../evidence/mac-s6-proxy.jsonl" "$APP"
```

Inspect loaded image(s). Match proxy-server `/art.svg` evidence to an empty Referer. Record each probe separately. QR/Geetest acceptance requires an owner-approved reproduction page first; do not substitute an external browser login or click through authorization prompts on the owner's behalf.

### 7. S8: Now Playing and physical media keys

Launch with the fixture server available:

```bash
BIU_MODE=full BIU_LOG="$PWD/../evidence/mac-s8.jsonl" "$APP"
```

While audio is playing, open macOS Control Center → Now Playing. Check title and cover art. Press physical Pause, Play, Previous and Next keys individually. For each action record the frontend clock/state, audible response, and any `media_action` log. Do not infer native media-key support from API presence.

### 8. S7: local updater and owner-only location cases

After a successful 0.0.1 DMG build, follow README §1.5 exactly. Recheck that `~/Applications/Biu M0.app` is absent before copying anything. Generate a local-only test key with output redirected to private logs; never print or transfer keys. Preserve the 0.0.1 app, build/sign 0.0.2, and serve `private/http` on loopback port 18766. Launch the installed test app using:

```bash
export BIU_UPDATE_PRIVATE="$PWD/private" BIU_UPDATE_ALLOW_MAC_TEST_LOCATIONS=1
BIU_MODE=update BIU_LOG="$PWD/../evidence/mac-update-user-applications.jsonl" "$HOME/Applications/Biu M0.app/Contents/MacOS/biu-m0"
BIU_MODE=verify-update BIU_LOG="$PWD/../evidence/mac-update-user-applications.jsonl" "$HOME/Applications/Biu M0.app/Contents/MacOS/biu-m0"
```

Record startup 0.0.1 → verified/install 0.0.2 → restart 0.0.2 and both store-preserved flags. These verify fixture stores only. For `/Applications`, the owner must personally place the 0.0.1 **Biu M0.app**, then repeat the commands with that path; do not replace Biu.app or elevate privileges. In Finder, double-click the generated 0.0.1 DMG, inspect any Gatekeeper dialog personally, and launch the mounted test app with the same environment as README specifies. Record `update_manual` and the actual execution path; shell launch does not prove AppTranslocation. If blocked, retain the error and mark that case blocked.

## Evidence manifest and safety

Evidence added by this run:

- `mac-results.md` — this report, S1–S8 table and pending owner actions.
- `mac-environment.txt` — recorded system/tool/framework versions.
- `mac-transport-verification.txt` — checksum check and full digests.
- `mac-sizes.json` — executable sizes and failed-DMG flag.
- `mac-build.log` — full failed build log.
- `mac-npm-ci.log`, `mac-rustfmt-install.log` — prerequisite command output.

Evidence validation checked all seven explicit files, the eight status rows, executable sizes against disk, absent completed DMG/updater key/installed test app, and reran both transport checksum checks successfully. A limited text scan found no matching private-key, cookie-header or token-query material. `git diff --cached --check` reported two trailing spaces emitted by create-dmg in the raw `mac-build.log` (lines 648 and 656); that original log is preserved unchanged. Whitespace validation excludes only that raw build log.

Only these explicit evidence files are staged. `.log` files are force-added individually because spike logs are normally ignored. No push, merge, tag, branch deletion, publication, remote write, secret access, updater-key generation, or application installation was performed. The owner plan is unchanged. No independent code review or Opus final acceptance is claimed.
