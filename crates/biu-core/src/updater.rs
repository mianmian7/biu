//! Seamless self-updater backed by GitHub Releases (`latest.json` + SHA-256 verification).
//!
//! Supports two channels:
//! - `stable`: `https://github.com/mianmian7/biu/releases/latest/download/latest.json`
//! - `nightly`: `https://github.com/mianmian7/biu/releases/download/nightly/latest.json`

use crate::bili::{USER_AGENT, allowlist_redirect_policy};
use anyhow::{Context, Result, ensure};
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use tokio::io::AsyncWriteExt;
use url::Url;

pub const GITHUB_REPO: &str = "mianmian7/biu";
pub const STABLE_MANIFEST_URL: &str =
    "https://github.com/mianmian7/biu/releases/latest/download/latest.json";
pub const NIGHTLY_MANIFEST_URL: &str =
    "https://github.com/mianmian7/biu/releases/download/nightly/latest.json";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Nightly,
}

impl UpdateChannel {
    pub fn from_str_lossy(value: &str) -> Self {
        if value.eq_ignore_ascii_case("nightly") {
            Self::Nightly
        } else {
            Self::Stable
        }
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Stable => "stable",
            Self::Nightly => "nightly",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Stable => "正式版",
            Self::Nightly => "抢鲜版",
        }
    }

    pub fn manifest_url(self) -> &'static str {
        match self {
            Self::Stable => STABLE_MANIFEST_URL,
            Self::Nightly => NIGHTLY_MANIFEST_URL,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlatformAsset {
    pub url: String,
    pub sha256: String,
    #[serde(default)]
    pub size: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateManifest {
    pub version: String,
    #[serde(default)]
    pub tag: String,
    #[serde(default)]
    pub channel: String,
    #[serde(default)]
    pub commit: String,
    #[serde(default)]
    pub pub_date: String,
    #[serde(default)]
    pub notes: String,
    #[serde(default)]
    pub release_url: String,
    #[serde(default)]
    pub platforms: HashMap<String, PlatformAsset>,
}

#[derive(Debug, Clone)]
pub struct UpdateInfo {
    pub version: String,
    pub tag: String,
    pub channel: UpdateChannel,
    pub commit: String,
    pub pub_date: String,
    pub notes: String,
    pub release_url: String,
    pub asset: PlatformAsset,
}

#[derive(Debug, Clone)]
pub enum UpdateCheckResult {
    UpToDate,
    Available(Box<UpdateInfo>),
}

pub fn current_platform_key() -> &'static str {
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    {
        "macos-aarch64"
    }
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    {
        "macos-x86_64"
    }
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    {
        "windows-x86_64"
    }
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    {
        "windows-aarch64"
    }
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    {
        "linux-x86_64"
    }
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    {
        "linux-aarch64"
    }
    #[cfg(not(any(
        all(target_os = "macos", target_arch = "aarch64"),
        all(target_os = "macos", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "x86_64"),
        all(target_os = "windows", target_arch = "aarch64"),
        all(target_os = "linux", target_arch = "x86_64"),
        all(target_os = "linux", target_arch = "aarch64"),
    )))]
    {
        "unsupported"
    }
}

pub fn parse_semver(raw: &str) -> Option<(u64, u64, u64)> {
    let trimmed = raw.trim().trim_start_matches(['v', 'V']);
    let core = trimmed.split(['-', '+']).next()?;
    let mut parts = core.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().unwrap_or("0").parse().ok()?;
    let patch = parts.next().unwrap_or("0").parse().ok()?;
    Some((major, minor, patch))
}

pub fn is_newer_release(
    current_version: &str,
    current_commit: &str,
    channel: UpdateChannel,
    manifest: &UpdateManifest,
) -> bool {
    let Some(local_ver) = parse_semver(current_version) else {
        return false;
    };
    let Some(remote_ver) = parse_semver(&manifest.version) else {
        return false;
    };
    if remote_ver > local_ver {
        return true;
    }
    if remote_ver < local_ver {
        return false;
    }
    if channel == UpdateChannel::Nightly {
        let local = current_commit.trim().to_ascii_lowercase();
        let remote = manifest.commit.trim().to_ascii_lowercase();
        if local.len() >= 7 && remote.len() >= 7 && local != "unknown" {
            return !local.starts_with(&remote) && !remote.starts_with(&local);
        }
    }
    false
}

pub fn is_allowed_update_url(url: &Url) -> bool {
    if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
        return false;
    }
    matches!(
        url.host_str(),
        Some(
            "github.com"
                | "objects.githubusercontent.com"
                | "release-assets.githubusercontent.com"
                | "github-releases.githubusercontent.com"
                | "raw.githubusercontent.com"
        )
    )
}

#[derive(Clone)]
pub struct UpdaterClient {
    client: reqwest::Client,
}

impl UpdaterClient {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(15))
            .timeout(Duration::from_secs(10 * 60))
            .redirect(allowlist_redirect_policy(is_allowed_update_url))
            .build()?;
        Ok(Self { client })
    }

    pub async fn check(
        &self,
        current_version: &str,
        current_commit: &str,
        channel: UpdateChannel,
    ) -> Result<UpdateCheckResult> {
        let ts = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);
        let url = format!("{}?t={ts}", channel.manifest_url());
        let resp = self
            .client
            .get(&url)
            .header("Cache-Control", "no-cache")
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("无法连接 GitHub Releases 检查更新")?;

        if resp.status() == reqwest::StatusCode::NOT_FOUND {
            return Ok(UpdateCheckResult::UpToDate);
        }

        let manifest: UpdateManifest = resp
            .error_for_status()
            .map_err(reqwest::Error::without_url)?
            .json()
            .await
            .map_err(reqwest::Error::without_url)
            .context("解析更新清单 latest.json 失败")?;

        if !is_newer_release(current_version, current_commit, channel, &manifest) {
            return Ok(UpdateCheckResult::UpToDate);
        }

        let platform = current_platform_key();
        let Some(asset) = manifest.platforms.get(platform).cloned() else {
            return Ok(UpdateCheckResult::UpToDate);
        };
        let parsed = Url::parse(&asset.url).context("更新包地址无效")?;
        ensure!(
            is_allowed_update_url(&parsed),
            "更新包下载地址不在允许的 GitHub 域名白名单内"
        );
        ensure!(
            asset.sha256.trim().len() == 64
                && asset.sha256.trim().bytes().all(|b| b.is_ascii_hexdigit()),
            "更新清单缺少合法的 SHA-256 校验值"
        );

        let release_url = if manifest.release_url.is_empty() {
            format!(
                "https://github.com/{GITHUB_REPO}/releases/tag/{}",
                if manifest.tag.is_empty() {
                    format!("v{}", manifest.version)
                } else {
                    manifest.tag.clone()
                }
            )
        } else {
            manifest.release_url
        };

        Ok(UpdateCheckResult::Available(Box::new(UpdateInfo {
            version: manifest.version,
            tag: manifest.tag,
            channel,
            commit: manifest.commit,
            pub_date: manifest.pub_date,
            notes: manifest.notes,
            release_url,
            asset,
        })))
    }

    pub async fn download_update<F>(
        &self,
        info: &UpdateInfo,
        staging_dir: &Path,
        mut on_progress: F,
    ) -> Result<PathBuf>
    where
        F: FnMut(u64, Option<u64>) + Send,
    {
        let parsed = Url::parse(&info.asset.url).context("更新包地址无效")?;
        ensure!(
            is_allowed_update_url(&parsed),
            "更新包下载地址不受信任"
        );

        tokio::fs::create_dir_all(staging_dir).await?;
        let filename = parsed
            .path_segments()
            .and_then(|mut segs| segs.next_back())
            .filter(|name| !name.is_empty())
            .unwrap_or("biu-update.archive");
        let dest = staging_dir.join(format!("download-{filename}"));
        let part = staging_dir.join(format!("download-{filename}.part"));

        let resp = self
            .client
            .get(parsed)
            .send()
            .await
            .map_err(reqwest::Error::without_url)
            .context("下载更新包失败")?
            .error_for_status()
            .map_err(reqwest::Error::without_url)?;

        let total = resp.content_length().or(info.asset.size);
        let mut file = tokio::fs::File::create(&part).await?;
        let mut hasher = Sha256::new();
        let mut downloaded = 0u64;
        let mut last_reported = 0u64;
        let mut stream = resp.bytes_stream();

        on_progress(0, total);
        while let Some(chunk) = stream.next().await {
            let bytes = chunk.map_err(reqwest::Error::without_url)?;
            file.write_all(&bytes).await?;
            hasher.update(&bytes);
            downloaded += bytes.len() as u64;
            if downloaded - last_reported >= 128 * 1024 {
                last_reported = downloaded;
                on_progress(downloaded, total);
            }
        }
        file.flush().await?;
        drop(file);
        on_progress(downloaded, total);

        let actual_hex = hex_lower(&hasher.finalize());
        if let Err(err) = verify_sha256_hex(&actual_hex, &info.asset.sha256) {
            let _ = tokio::fs::remove_file(&part).await;
            return Err(err);
        }

        tokio::fs::rename(&part, &dest).await?;
        Ok(dest)
    }
}

pub fn hex_lower(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut out = String::with_capacity(bytes.len() * 2);
    for &b in bytes {
        out.push(HEX[(b >> 4) as usize] as char);
        out.push(HEX[(b & 0x0f) as usize] as char);
    }
    out
}

pub fn verify_sha256_hex(actual: &str, expected: &str) -> Result<()> {
    let expected = expected.trim().to_ascii_lowercase();
    let actual = actual.trim().to_ascii_lowercase();
    ensure!(
        expected.len() == 64 && actual == expected,
        "更新包 SHA-256 校验失败（文件可能损坏）"
    );
    Ok(())
}

/// Resolve the enclosing `.app` bundle directory from a macOS executable path
/// (`.../Biu.app/Contents/MacOS/biu` -> `.../Biu.app`).
pub fn macos_app_bundle_from_exe(exe: &Path) -> Option<PathBuf> {
    let macos_dir = exe.parent()?;
    if macos_dir.file_name()?.to_str()? != "MacOS" {
        return None;
    }
    let contents_dir = macos_dir.parent()?;
    if contents_dir.file_name()?.to_str()? != "Contents" {
        return None;
    }
    let app_dir = contents_dir.parent()?;
    if app_dir.extension()?.to_str()? != "app" {
        return None;
    }
    Some(app_dir.to_path_buf())
}

/// Extract a verified update archive and spawn a detached helper that atomically
/// replaces the running app once this process exits, then relaunches it.
///
/// The caller should quit immediately after this returns `Ok(())`.
pub fn install_and_relaunch(archive_path: &Path) -> Result<()> {
    ensure!(archive_path.exists(), "更新包文件不存在");
    let exe = std::env::current_exe().context("无法获取当前可执行文件路径")?;

    #[cfg(target_os = "macos")]
    {
        install_macos(archive_path, &exe)
    }
    #[cfg(target_os = "windows")]
    {
        install_windows(archive_path, &exe)
    }
    #[cfg(target_os = "linux")]
    {
        install_linux(archive_path, &exe)
    }
    #[cfg(not(any(target_os = "macos", target_os = "windows", target_os = "linux")))]
    {
        let _ = exe;
        anyhow::bail!("当前平台暂不支持自动替换更新")
    }
}

#[cfg(target_os = "macos")]
fn install_macos(archive_path: &Path, exe: &Path) -> Result<()> {
    use anyhow::{anyhow, bail};
    use std::process::{Command, Stdio};

    let target_app = macos_app_bundle_from_exe(exe).ok_or_else(|| {
        anyhow!("当前处于命令行开发运行模式，请在安装后的 Biu.app 中执行自动更新")
    })?;
    let parent_dir = target_app
        .parent()
        .ok_or_else(|| anyhow!("无法定位应用安装目录"))?;

    // Check write permission before detaching so mounted read-only DMGs fail cleanly.
    let probe = parent_dir.join(format!(".biu-update-probe-{}", std::process::id()));
    match std::fs::File::create(&probe) {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
        }
        Err(_) => {
            bail!(
                "目录 {} 无写入权限，请先将 Biu.app 移至「应用程序」文件夹或有写权限的目录后再自动更新",
                parent_dir.display()
            );
        }
    }

    let stage_root = std::env::temp_dir().join(format!("biu-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage_root);
    std::fs::create_dir_all(&stage_root)?;

    let status = Command::new("/usr/bin/tar")
        .arg("-xzf")
        .arg(archive_path)
        .arg("-C")
        .arg(&stage_root)
        .status()
        .context("调用 tar 解压更新包失败")?;
    ensure!(status.success(), "解压 macOS 更新包失败");

    let staged_app = find_first_app_bundle(&stage_root)
        .ok_or_else(|| anyhow!("更新包内未找到 Biu.app"))?;
    let staged_macos_dir = staged_app.join("Contents").join("MacOS");
    ensure!(staged_macos_dir.is_dir(), "更新包中的 App 结构不完整");

    let _ = Command::new("/usr/bin/xattr")
        .args(["-dr", "com.apple.quarantine"])
        .arg(&staged_app)
        .status();

    let script_path = stage_root.join("apply-update.sh");
    let script = r#"#!/bin/sh
set -eu
PID="$1"
STAGED_APP="$2"
TARGET_APP="$3"
ARCHIVE="$4"
STAGE_ROOT="$5"

i=0
while kill -0 "$PID" 2>/dev/null; do
  i=$((i + 1))
  if [ "$i" -ge 150 ]; then
    break
  fi
  sleep 0.1
done

BACKUP_APP="${TARGET_APP}.biu-backup"
rm -rf "$BACKUP_APP"
mv "$TARGET_APP" "$BACKUP_APP"
if mv "$STAGED_APP" "$TARGET_APP"; then
  rm -rf "$BACKUP_APP"
else
  mv "$BACKUP_APP" "$TARGET_APP" || true
  exit 1
fi

rm -f "$ARCHIVE" || true
open -n "$TARGET_APP" >/dev/null 2>&1 || open "$TARGET_APP" >/dev/null 2>&1 || true
rm -rf "$STAGE_ROOT" || true
"#;
    std::fs::write(&script_path, script)?;

    Command::new("/bin/sh")
        .arg(&script_path)
        .arg(std::process::id().to_string())
        .arg(&staged_app)
        .arg(&target_app)
        .arg(archive_path)
        .arg(&stage_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("启动无缝替换脚本失败")?;

    Ok(())
}

#[cfg(target_os = "macos")]
fn find_first_app_bundle(dir: &Path) -> Option<PathBuf> {
    let entries = std::fs::read_dir(dir).ok()?;
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() && path.extension().and_then(|e| e.to_str()) == Some("app") {
            return Some(path);
        }
    }
    None
}

#[cfg(target_os = "windows")]
fn install_windows(archive_path: &Path, exe: &Path) -> Result<()> {
    use std::process::{Command, Stdio};

    let stage_root = std::env::temp_dir().join(format!("biu-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage_root);
    std::fs::create_dir_all(&stage_root)?;

    let ps_script = stage_root.join("apply-update.ps1");
    let script = r#"param(
  [int]$AppPid,
  [string]$ArchivePath,
  [string]$TargetExe,
  [string]$StageRoot
)
$ErrorActionPreference = 'Stop'
for ($i = 0; $i -lt 150; $i++) {
  if (-not (Get-Process -Id $AppPid -ErrorAction SilentlyContinue)) { break }
  Start-Sleep -Milliseconds 100
}
$extractDir = Join-Path $StageRoot 'extracted'
Expand-Archive -Path $ArchivePath -DestinationPath $extractDir -Force
$newExe = Get-ChildItem -Path $extractDir -Filter '*.exe' -Recurse | Select-Object -First 1
if (-not $newExe) { throw 'No executable found in update archive' }
$backup = "$TargetExe.bak"
if (Test-Path $backup) { Remove-Item -Force $backup }
Move-Item -Path $TargetExe -Destination $backup -Force
try {
  Move-Item -Path $newExe.FullName -Destination $TargetExe -Force
  Remove-Item -Force $backup -ErrorAction SilentlyContinue
} catch {
  Move-Item -Path $backup -Destination $TargetExe -Force -ErrorAction SilentlyContinue
  throw
}
Remove-Item -Force $ArchivePath -ErrorAction SilentlyContinue
Start-Process -FilePath $TargetExe
Remove-Item -Recurse -Force $StageRoot -ErrorAction SilentlyContinue
"#;
    std::fs::write(&ps_script, script)?;

    Command::new("powershell")
        .args([
            "-NoProfile",
            "-WindowStyle",
            "Hidden",
            "-ExecutionPolicy",
            "Bypass",
            "-File",
        ])
        .arg(&ps_script)
        .arg("-AppPid")
        .arg(std::process::id().to_string())
        .arg("-ArchivePath")
        .arg(archive_path)
        .arg("-TargetExe")
        .arg(exe)
        .arg("-StageRoot")
        .arg(&stage_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("启动 Windows 自动更新脚本失败")?;

    Ok(())
}

#[cfg(target_os = "linux")]
fn install_linux(archive_path: &Path, exe: &Path) -> Result<()> {
    use std::process::{Command, Stdio};

    let stage_root = std::env::temp_dir().join(format!("biu-update-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&stage_root);
    std::fs::create_dir_all(&stage_root)?;

    let status = Command::new("tar")
        .arg("-xzf")
        .arg(archive_path)
        .arg("-C")
        .arg(&stage_root)
        .status()
        .context("调用 tar 解压更新包失败")?;
    ensure!(status.success(), "解压 Linux 更新包失败");

    let new_bin = stage_root.join("biu").join("biu");
    let new_bin = if new_bin.is_file() {
        new_bin
    } else {
        stage_root.join("biu-gpui").join("biu-gpui")
    };
    ensure!(new_bin.is_file(), "更新包内未找到可执行文件");

    let script_path = stage_root.join("apply-update.sh");
    let script = r#"#!/bin/sh
set -eu
PID="$1"
NEW_BIN="$2"
TARGET_EXE="$3"
ARCHIVE="$4"
STAGE_ROOT="$5"

i=0
while kill -0 "$PID" 2>/dev/null; do
  i=$((i + 1))
  if [ "$i" -ge 150 ]; then
    break
  fi
  sleep 0.1
done

chmod +x "$NEW_BIN"
cp -f "$NEW_BIN" "${TARGET_EXE}.new"
chmod +x "${TARGET_EXE}.new"
mv -f "${TARGET_EXE}.new" "$TARGET_EXE"
rm -f "$ARCHIVE" || true
nohup "$TARGET_EXE" >/dev/null 2>&1 &
rm -rf "$STAGE_ROOT" || true
"#;
    std::fs::write(&script_path, script)?;

    Command::new("/bin/sh")
        .arg(&script_path)
        .arg(std::process::id().to_string())
        .arg(&new_bin)
        .arg(exe)
        .arg(archive_path)
        .arg(&stage_root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .context("启动 Linux 无缝替换脚本失败")?;

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest(version: &str, commit: &str) -> UpdateManifest {
        UpdateManifest {
            version: version.into(),
            tag: format!("v{version}"),
            channel: "stable".into(),
            commit: commit.into(),
            pub_date: String::new(),
            notes: String::new(),
            release_url: String::new(),
            platforms: HashMap::new(),
        }
    }

    #[test]
    fn semver_and_nightly_commit_comparison() {
        assert_eq!(parse_semver("v2.1.0-gpui"), Some((2, 1, 0)));
        assert!(is_newer_release(
            "2.1.0",
            "abc1234",
            UpdateChannel::Stable,
            &manifest("2.1.1", "def5678")
        ));
        assert!(!is_newer_release(
            "2.1.0",
            "abc1234",
            UpdateChannel::Stable,
            &manifest("2.1.0", "def5678")
        ));
        // Nightly triggers update when version matches but commit changed.
        assert!(is_newer_release(
            "2.1.0",
            "abc1234",
            UpdateChannel::Nightly,
            &manifest("2.1.0", "def567890123")
        ));
        // Same commit does not re-trigger nightly update.
        assert!(!is_newer_release(
            "2.1.0",
            "abc1234",
            UpdateChannel::Nightly,
            &manifest("2.1.0", "abc123499999")
        ));
    }

    #[test]
    fn url_allowlist_and_sha256_verification() {
        assert!(is_allowed_update_url(
            &Url::parse("https://github.com/mianmian7/biu/releases/latest/download/latest.json")
                .unwrap()
        ));
        assert!(is_allowed_update_url(
            &Url::parse("https://objects.githubusercontent.com/github-production-release-asset/foo")
                .unwrap()
        ));
        assert!(!is_allowed_update_url(
            &Url::parse("http://github.com/mianmian7/biu/releases/latest/download/latest.json")
                .unwrap()
        ));
        assert!(!is_allowed_update_url(
            &Url::parse("https://evil.example.com/latest.json").unwrap()
        ));

        let digest = hex_lower(&Sha256::digest(b"biu-release-bytes"));
        assert!(verify_sha256_hex(&digest, &digest.to_uppercase()).is_ok());
        assert!(verify_sha256_hex(&digest, &"0".repeat(64)).is_err());
    }

    #[test]
    fn resolves_macos_app_bundle_only_for_packaged_apps() {
        assert_eq!(
            macos_app_bundle_from_exe(Path::new("/Applications/Biu.app/Contents/MacOS/biu")),
            Some(PathBuf::from("/Applications/Biu.app"))
        );
        assert_eq!(
            macos_app_bundle_from_exe(Path::new("/Users/me/biu/target/release/biu")),
            None
        );
    }
}
