use crate::bili::{extract_and_register_playurl_hosts, BiliState, BILI_ORIGIN, USER_AGENT};
use crate::store::read_store_object;
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Emitter, Runtime};
use tokio::io::AsyncWriteExt;
use tokio::sync::Semaphore;
use url::Url;

const CHUNK_SIZE: u64 = 10 * 1024 * 1024; // 10MB
const TASK_CONCURRENCY: usize = 3;
const CHUNK_CONCURRENCY: usize = 5;

const MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

const AUDIO_QUALITY_SORT: [i64; 8] = [30257, 30216, 30259, 30260, 30232, 30280, 30250, 30251];

pub const SYSTEM_FFMPEG_PATHS: &[&str] = &[
    "/opt/homebrew/bin/ffmpeg",
    "/usr/local/bin/ffmpeg",
    "/usr/bin/ffmpeg",
    "/snap/bin/ffmpeg",
];

static RESOLVED_FFMPEG: OnceLock<RwLock<Option<PathBuf>>> = OnceLock::new();

fn resolved_ffmpeg_cache() -> &'static RwLock<Option<PathBuf>> {
    RESOLVED_FFMPEG.get_or_init(|| RwLock::new(None))
}

pub fn clear_resolved_ffmpeg_path() {
    *resolved_ffmpeg_cache().write().unwrap() = None;
}

fn current_arch_label() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "x64"
    } else if cfg!(target_arch = "aarch64") {
        "arm64"
    } else if cfg!(target_arch = "x86") {
        "ia32"
    } else if cfg!(target_arch = "arm") {
        "arm"
    } else {
        "unknown"
    }
}

pub fn executable_arch(header: &[u8]) -> Option<&'static str> {
    // ELF
    if header.len() >= 20 && &header[0..4] == b"\x7fELF" {
        let little = header[5] == 1;
        if !little && header[5] != 2 {
            return None;
        }
        let machine = if little {
            u16::from_le_bytes([header[18], header[19]])
        } else {
            u16::from_be_bytes([header[18], header[19]])
        };
        return match machine {
            3 => Some("ia32"),
            40 => Some("arm"),
            62 => Some("x64"),
            183 => Some("arm64"),
            _ => None,
        };
    }
    // Thin Mach-O
    if header.len() >= 8 {
        let sig = u32::from_le_bytes([header[0], header[1], header[2], header[3]]);
        let little = sig == 0xfeed_face || sig == 0xfeed_facf;
        let big = sig == 0xcefa_edfe || sig == 0xcffa_edfe;
        if little || big {
            let cpu = if little {
                u32::from_le_bytes([header[4], header[5], header[6], header[7]])
            } else {
                u32::from_be_bytes([header[4], header[5], header[6], header[7]])
            };
            return match cpu {
                7 => Some("ia32"),
                12 => Some("arm"),
                0x0100_0007 => Some("x64"),
                0x0100_000c => Some("arm64"),
                _ => None,
            };
        }
    }
    // PE
    if header.len() >= 0x40 && &header[0..2] == b"MZ" {
        let lfanew = i32::from_le_bytes([header[0x3c], header[0x3d], header[0x3e], header[0x3f]]);
        if lfanew < 0 || (lfanew as usize) + 6 > header.len() {
            return None;
        }
        let off = lfanew as usize;
        if &header[off..off + 4] != b"PE\0\0" {
            return None;
        }
        let machine = u16::from_le_bytes([header[off + 4], header[off + 5]]);
        return match machine {
            0x14c => Some("ia32"),
            0x1c0 => Some("arm"),
            0x8664 => Some("x64"),
            0xaa64 => Some("arm64"),
            _ => None,
        };
    }
    None
}

fn read_header(path: &Path) -> Option<Vec<u8>> {
    let mut f = fs::File::open(path).ok()?;
    let mut buf = vec![0u8; 4096];
    let n = f.read(&mut buf).ok()?;
    buf.truncate(n);
    Some(buf)
}

fn probe_ffmpeg(candidate: &Path) -> Result<(), String> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(meta) = fs::metadata(candidate) {
            let mut perms = meta.permissions();
            if perms.mode() & 0o111 == 0 {
                perms.set_mode(0o755);
                let _ = fs::set_permissions(candidate, perms);
            }
        }
    }
    let mut cmd = Command::new(candidate);
    cmd.args(["-hide_banner", "-version"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000); // CREATE_NO_WINDOW
    }
    match cmd.status() {
        Ok(status) if status.success() => Ok(()),
        Ok(status) => Err(format!("exited with {status}")),
        Err(e) => Err(e.to_string()),
    }
}

pub fn resolve_ffmpeg_path(user_setting: Option<&str>) -> Result<PathBuf, String> {
    if let Some(cached) = resolved_ffmpeg_cache().read().unwrap().clone() {
        if cached.exists() {
            return Ok(cached);
        }
    }

    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Some(s) = user_setting.map(str::trim).filter(|s| !s.is_empty()) {
        candidates.push(PathBuf::from(s));
    }
    if let Ok(env_path) = std::env::var("BIU_FFMPEG_PATH") {
        let trimmed = env_path.trim();
        if !trimmed.is_empty() {
            candidates.push(PathBuf::from(trimmed));
        }
    }

    // Bundled sidecar next to current executable (Tauri externalBin location)
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            let sidecar_name = if cfg!(windows) {
                "ffmpeg.exe"
            } else {
                "ffmpeg"
            };
            candidates.push(dir.join(sidecar_name));
        }
    }

    // System paths on Unix
    if cfg!(unix) {
        for p in SYSTEM_FFMPEG_PATHS {
            candidates.push(PathBuf::from(p));
        }
    }

    // Dev workspace fallbacks (src-tauri/binaries or src-tauri/ffmpeg)
    let manifest_dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if let Ok(entries) = fs::read_dir(manifest_dir.join("binaries")) {
        for entry in entries.flatten() {
            let p = entry.path();
            if p.file_name()
                .and_then(|n| n.to_str())
                .is_some_and(|n| n.starts_with("ffmpeg"))
            {
                candidates.push(p);
            }
        }
    }
    let repo_ffmpeg_dir = manifest_dir.join("ffmpeg");
    let bundled_name = if cfg!(windows) {
        "ffmpeg.exe"
    } else if cfg!(target_os = "macos") {
        if cfg!(target_arch = "aarch64") {
            "ffmpeg-mac-arm64"
        } else {
            "ffmpeg-mac-x64"
        }
    } else {
        "ffmpeg-linux"
    };
    candidates.push(repo_ffmpeg_dir.join(bundled_name));

    let host_arch = current_arch_label();
    let mut seen = std::collections::HashSet::new();
    let mut rejected = Vec::new();

    for candidate in candidates {
        if !seen.insert(candidate.clone()) {
            continue;
        }
        if !candidate.exists() {
            rejected.push(format!("{}: missing", candidate.display()));
            continue;
        }
        if let Some(header) = read_header(&candidate) {
            if let Some(arch) = executable_arch(&header) {
                if arch != host_arch {
                    rejected.push(format!("{}: arch {arch}", candidate.display()));
                    continue;
                }
            }
        }
        match probe_ffmpeg(&candidate) {
            Ok(()) => {
                *resolved_ffmpeg_cache().write().unwrap() = Some(candidate.clone());
                return Ok(candidate);
            }
            Err(err) => {
                rejected.push(format!("{}: {err}", candidate.display()));
            }
        }
    }

    Err(format!(
        "decode-unavailable: {}",
        if rejected.is_empty() {
            "no ffmpeg candidate".to_string()
        } else {
            rejected.join("; ")
        }
    ))
}

pub fn user_ffmpeg_setting<R: Runtime>(app: &AppHandle<R>) -> Option<String> {
    read_store_object(app, "app-settings")
        .ok()
        .and_then(|v| {
            v.get("appSettings")?
                .get("ffmpegPath")?
                .as_str()
                .map(|s| s.to_string())
        })
        .filter(|s| !s.trim().is_empty())
}

pub fn encode_uri_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn sign_wbi_params_with_keys(
    params: &[(String, String)],
    img_key: &str,
    sub_key: &str,
    wts: u64,
) -> Vec<(String, String)> {
    if img_key.is_empty() || sub_key.is_empty() {
        return params.to_vec();
    }
    let orig = format!("{img_key}{sub_key}");
    let orig_bytes = orig.as_bytes();
    let mut mixin_key = String::with_capacity(32);
    for &idx in MIXIN_KEY_ENC_TAB.iter().take(32) {
        if let Some(&b) = orig_bytes.get(idx) {
            mixin_key.push(b as char);
        }
    }

    let mut all_params: Vec<(String, String)> = params.to_vec();
    all_params.push(("wts".to_string(), wts.to_string()));
    all_params.sort_by(|a, b| a.0.cmp(&b.0));

    let query = all_params
        .iter()
        .map(|(k, v)| {
            let filtered: String = v
                .chars()
                .filter(|c| !matches!(c, '!' | '\'' | '(' | ')' | '*'))
                .collect();
            format!(
                "{}={}",
                encode_uri_component(k),
                encode_uri_component(&filtered)
            )
        })
        .collect::<Vec<_>>()
        .join("&");

    let mut hasher = Md5::new();
    hasher.update(format!("{query}{mixin_key}").as_bytes());
    let w_rid: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let mut out = params.to_vec();
    out.push(("wts".to_string(), wts.to_string()));
    out.push(("w_rid".to_string(), w_rid));
    out
}

pub async fn get_wbi_keys(state: &BiliState) -> (String, String) {
    let Ok(resp) = state
        .client()
        .get("https://api.bilibili.com/x/web-interface/nav")
        .timeout(Duration::from_secs(10))
        .send()
        .await
    else {
        return (String::new(), String::new());
    };
    let Ok(body) = resp.json::<Value>().await else {
        return (String::new(), String::new());
    };
    let wbi_img = body.get("data").and_then(|d| d.get("wbi_img"));
    let extract_stem = |url_str: Option<&str>| -> String {
        let Some(s) = url_str else {
            return String::new();
        };
        let after_slash = s.rsplit('/').next().unwrap_or("");
        after_slash.split('.').next().unwrap_or("").to_string()
    };
    let img_key = extract_stem(
        wbi_img
            .and_then(|w| w.get("img_url"))
            .and_then(|v| v.as_str()),
    );
    let sub_key = extract_stem(
        wbi_img
            .and_then(|w| w.get("sub_url"))
            .and_then(|v| v.as_str()),
    );
    (img_key, sub_key)
}

pub async fn encode_params_wbi(
    state: &BiliState,
    params: &[(String, String)],
) -> Vec<(String, String)> {
    let (img_key, sub_key) = get_wbi_keys(state).await;
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    sign_wbi_params_with_keys(params, &img_key, &sub_key, now)
}

pub async fn get_web_interface_view(state: &BiliState, bvid: &str) -> Result<Value, String> {
    let mut url =
        Url::parse("https://api.bilibili.com/x/web-interface/view").map_err(|e| e.to_string())?;
    url.query_pairs_mut().append_pair("bvid", bvid);
    let resp = state
        .client()
        .get(url)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    resp.json::<Value>().await.map_err(|e| e.to_string())
}

pub async fn get_dash_url(
    state: &BiliState,
    bvid: &str,
    cid: &str,
    fnval: u32,
    qn: Option<u32>,
) -> Result<Value, String> {
    let mut base_params = vec![
        ("bvid".to_string(), bvid.to_string()),
        ("cid".to_string(), cid.to_string()),
        ("fnval".to_string(), fnval.to_string()),
    ];
    if let Some(q) = qn {
        base_params.push(("fnver".to_string(), "0".to_string()));
        base_params.push(("qn".to_string(), q.to_string()));
    }
    let signed = encode_params_wbi(state, &base_params).await;
    let mut url =
        Url::parse("https://api.bilibili.com/x/player/wbi/playurl").map_err(|e| e.to_string())?;
    {
        let mut pairs = url.query_pairs_mut();
        for (k, v) in &signed {
            pairs.append_pair(k, v);
        }
    }
    let resp = state
        .client()
        .get(url)
        .timeout(Duration::from_secs(15))
        .send()
        .await
        .map_err(|e| e.to_string())?;
    let body = resp.json::<Value>().await.map_err(|e| e.to_string())?;
    extract_and_register_playurl_hosts(state, &body);
    Ok(body)
}

pub fn sanitize_filename(input: &str) -> String {
    if input.is_empty() {
        return String::new();
    }
    let decoded = input
        .replace("&amp;", "&")
        .replace("&apos;", "'")
        .replace("&gt;", ">")
        .replace("&lt;", "<")
        .replace("&quot;", "\"")
        .replace("&nbsp;", " ");

    // Strip HTML tags (<...>)
    let mut no_html = String::with_capacity(decoded.len());
    let mut in_tag = false;
    for ch in decoded.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' && in_tag {
            in_tag = false;
        } else if !in_tag {
            no_html.push(ch);
        }
    }

    let replaced: String = no_html
        .chars()
        .map(|ch| {
            let code = ch as u32;
            if matches!(ch, '\\' | '/' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
                || code <= 0x1f
                || (0x80..=0x9f).contains(&code)
            {
                '_'
            } else {
                ch
            }
        })
        .collect();

    let collapsed = replaced.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed.trim().trim_end_matches('.');
    trimmed.chars().take(255).collect()
}

pub fn get_available_output_path(output_path: &Path) -> Result<PathBuf, String> {
    if !output_path.exists() {
        return Ok(output_path.to_path_buf());
    }
    let parent = output_path.parent().unwrap_or_else(|| Path::new("."));
    let stem = output_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    let ext = output_path
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();

    for i in 1..10000 {
        let candidate = parent.join(format!("{stem} ({i}){ext}"));
        if !candidate.exists() {
            return Ok(candidate);
        }
    }
    Err("Cannot find available output path".to_string())
}

pub fn convert_with_ffmpeg(
    ffmpeg_path: &Path,
    output_file_type: &str,
    audio_temp_path: Option<&Path>,
    video_temp_path: Option<&Path>,
    output_path: &Path,
    cancelled: &AtomicBool,
) -> Result<PathBuf, String> {
    if cancelled.load(Ordering::SeqCst) {
        return Err("Aborted".to_string());
    }
    let final_output = get_available_output_path(output_path)?;
    let mut args: Vec<String> = vec![
        "-y".to_string(),
        "-hide_banner".to_string(),
        "-loglevel".to_string(),
        "error".to_string(),
    ];

    if output_file_type == "video" {
        let v_path = video_temp_path
            .ok_or_else(|| "Video conversion requires video temp path".to_string())?;
        args.push("-i".to_string());
        args.push(v_path.to_string_lossy().into_owned());
        if let Some(a_path) = audio_temp_path {
            args.push("-i".to_string());
            args.push(a_path.to_string_lossy().into_owned());
            args.extend([
                "-c:v".to_string(),
                "copy".to_string(),
                "-c:a".to_string(),
                "copy".to_string(),
                "-shortest".to_string(),
            ]);
        } else {
            args.extend(["-c:v".to_string(), "copy".to_string()]);
        }
    } else {
        let a_path = audio_temp_path
            .ok_or_else(|| "Audio conversion requires audio temp path".to_string())?;
        args.push("-i".to_string());
        args.push(a_path.to_string_lossy().into_owned());
        args.extend(["-c:a".to_string(), "copy".to_string()]);
    }
    args.push(final_output.to_string_lossy().into_owned());

    let mut cmd = Command::new(ffmpeg_path);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn ffmpeg: {e}"))?;
    loop {
        if cancelled.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = fs::remove_file(&final_output);
            return Err("Aborted".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                if status.success() {
                    return Ok(final_output);
                }
                let mut stderr_str = String::new();
                if let Some(mut err_pipe) = child.stderr.take() {
                    let _ = err_pipe.read_to_string(&mut stderr_str);
                }
                let _ = fs::remove_file(&final_output);
                return Err(format!("ffmpeg exited {status}: {stderr_str}"));
            }
            Ok(None) => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                let _ = child.kill();
                return Err(e.to_string());
            }
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MediaDownloadChunk {
    #[serde(rename = "type")]
    pub chunk_type: String,
    pub name: String,
    pub start: u64,
    pub end: u64,
    pub done: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaDownloadInfo {
    pub output_file_type: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bvid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sid: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct MediaDownloadTask {
    pub id: String,
    pub output_file_type: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bvid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sid: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_codecs: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_bandwidth: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_resolution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_frame_rate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_path: Option<String>,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub total_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub downloaded_bytes: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_progress: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub merge_progress: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub convert_progress: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FullMediaDownloadTask {
    pub id: String,
    pub output_file_type: String,
    pub title: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bvid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sid: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_codecs: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_bandwidth: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_url: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_resolution: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_frame_rate: Option<String>,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub audio_total_bytes: u64,
    #[serde(default)]
    pub video_total_bytes: u64,
    #[serde(default)]
    pub downloaded_bytes: u64,
    #[serde(default)]
    pub download_progress: u32,
    #[serde(default)]
    pub merge_progress: u32,
    #[serde(default)]
    pub convert_progress: u32,
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_name: Option<String>,
    #[serde(default)]
    pub temp_dir: String,
    #[serde(default)]
    pub save_dir: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub audio_temp_path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub video_temp_path: Option<String>,
    #[serde(default)]
    pub chunks: Vec<MediaDownloadChunk>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl FullMediaDownloadTask {
    pub fn to_public_task(&self) -> MediaDownloadTask {
        MediaDownloadTask {
            id: self.id.clone(),
            output_file_type: self.output_file_type.clone(),
            title: self.title.clone(),
            bvid: self.bvid.clone(),
            cid: self.cid.clone(),
            sid: self.sid.clone(),
            cover: self.cover.clone(),
            audio_codecs: self.audio_codecs.clone(),
            audio_bandwidth: self.audio_bandwidth,
            video_resolution: self.video_resolution.clone(),
            video_frame_rate: self.video_frame_rate.clone(),
            save_path: self.save_path.clone(),
            status: self.status.clone(),
            total_bytes: Some(self.total_bytes),
            downloaded_bytes: Some(self.downloaded_bytes),
            download_progress: Some(self.download_progress),
            merge_progress: Some(self.merge_progress),
            convert_progress: Some(self.convert_progress),
            created_time: self.created_time,
            error: self.error.clone(),
        }
    }

    pub fn to_persisted_task(&self) -> FullMediaDownloadTask {
        let mut copy = self.clone();
        copy.status = match copy.status.as_str() {
            "downloading" | "waiting" => "downloadPaused".to_string(),
            "merging" => "mergePaused".to_string(),
            "converting" => "convertPaused".to_string(),
            other => other.to_string(),
        };
        copy
    }
}

fn value_id_to_string(val: &Option<Value>) -> Option<String> {
    match val {
        Some(Value::String(s)) if !s.trim().is_empty() => Some(s.trim().to_string()),
        Some(Value::Number(n)) => Some(n.to_string()),
        _ => None,
    }
}

pub fn is_url_valid(url_str: Option<&str>) -> bool {
    let Some(s) = url_str.filter(|s| !s.is_empty()) else {
        return false;
    };
    let Ok(parsed) = Url::parse(s) else {
        return false;
    };
    let Some(deadline_str) = parsed
        .query_pairs()
        .find(|(k, _)| k == "deadline")
        .map(|(_, v)| v.into_owned())
    else {
        return false;
    };
    let Ok(deadline) = deadline_str.parse::<u64>() else {
        return false;
    };
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    deadline > now
}

pub fn split_chunks(chunk_type: &str, total_size: u64, chunk_size: u64) -> Vec<MediaDownloadChunk> {
    if total_size == 0 || chunk_size == 0 {
        return Vec::new();
    }
    let count = total_size.div_ceil(chunk_size);
    let mut out = Vec::with_capacity(count as usize);
    for i in 0..count {
        let start = i * chunk_size;
        let end = ((i + 1) * chunk_size - 1).min(total_size - 1);
        out.push(MediaDownloadChunk {
            chunk_type: chunk_type.to_string(),
            name: format!("{chunk_type}.part{}", i + 1),
            start,
            end,
            done: false,
        });
    }
    out
}

pub fn sort_dash_audio(audio_list: &[Value]) -> Vec<Value> {
    let mut vec = audio_list.to_vec();
    vec.sort_by(|a, b| {
        let bw_a = a.get("bandwidth").and_then(|v| v.as_u64()).unwrap_or(0);
        let bw_b = b.get("bandwidth").and_then(|v| v.as_u64()).unwrap_or(0);
        if bw_a != bw_b {
            return bw_b.cmp(&bw_a);
        }
        let id_a = a.get("id").and_then(|v| v.as_i64()).unwrap_or(-1);
        let id_b = b.get("id").and_then(|v| v.as_i64()).unwrap_or(-1);
        let pos_a = AUDIO_QUALITY_SORT.iter().position(|&x| x == id_a);
        let pos_b = AUDIO_QUALITY_SORT.iter().position(|&x| x == id_b);
        match (pos_a, pos_b) {
            (None, _) => std::cmp::Ordering::Greater,
            (_, None) => std::cmp::Ordering::Less,
            (Some(ia), Some(ib)) => ib.cmp(&ia),
        }
    });
    vec
}

fn dash_item_url(item: &Value) -> Option<String> {
    for k in ["baseUrl", "base_url"] {
        if let Some(s) = item
            .get(k)
            .and_then(|v| v.as_str())
            .filter(|s| !s.is_empty())
        {
            return Some(s.to_string());
        }
    }
    for k in ["backupUrl", "backup_url"] {
        if let Some(arr) = item.get(k).and_then(|v| v.as_array()) {
            if let Some(s) = arr
                .first()
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            {
                return Some(s.to_string());
            }
        }
    }
    None
}

pub struct DownloadManager {
    pub tasks: RwLock<Vec<Arc<Mutex<FullMediaDownloadTask>>>>,
    pub cancel_flags: Mutex<HashMap<String, Arc<AtomicBool>>>,
    pub semaphore: Arc<Semaphore>,
    pub pending_updates: Mutex<HashMap<String, MediaDownloadTask>>,
    pub last_flush: Mutex<Instant>,
    pub flush_scheduled: AtomicBool,
    pub store_path: PathBuf,
}

impl DownloadManager {
    pub fn new(store_path: PathBuf) -> Arc<Self> {
        let mgr = Arc::new(Self {
            tasks: RwLock::new(Vec::new()),
            cancel_flags: Mutex::new(HashMap::new()),
            semaphore: Arc::new(Semaphore::new(TASK_CONCURRENCY)),
            pending_updates: Mutex::new(HashMap::new()),
            last_flush: Mutex::new(
                Instant::now()
                    .checked_sub(Duration::from_secs(10))
                    .unwrap_or_else(Instant::now),
            ),
            flush_scheduled: AtomicBool::new(false),
            store_path,
        });
        mgr.restore_from_disk();
        mgr
    }

    fn restore_from_disk(&self) {
        if !self.store_path.exists() {
            return;
        }
        let Ok(bytes) = fs::read(&self.store_path) else {
            return;
        };
        let Ok(json_val) = serde_json::from_slice::<Value>(&bytes) else {
            return;
        };
        let Some(obj) = json_val.as_object() else {
            return;
        };
        let mut restored: Vec<FullMediaDownloadTask> = Vec::new();
        for (k, v) in obj {
            if k == "downloads" {
                if let Some(nested) = v.as_object() {
                    for (_, task_val) in nested {
                        if let Ok(t) =
                            serde_json::from_value::<FullMediaDownloadTask>(task_val.clone())
                        {
                            restored.push(t.to_persisted_task());
                        }
                    }
                }
            } else if let Ok(t) = serde_json::from_value::<FullMediaDownloadTask>(v.clone()) {
                restored.push(t.to_persisted_task());
            }
        }
        restored.sort_by_key(|t| t.created_time.unwrap_or(0));
        let mut lock = self.tasks.write().unwrap();
        for t in restored {
            lock.push(Arc::new(Mutex::new(t)));
        }
    }

    pub fn save_to_disk(&self) {
        let lock = self.tasks.read().unwrap();
        let mut map = serde_json::Map::new();
        for task_arc in lock.iter() {
            let t = task_arc.lock().unwrap().to_persisted_task();
            if let Ok(val) = serde_json::to_value(&t) {
                map.insert(format!("downloads.{}", t.id), val);
            }
        }
        if let Some(parent) = self.store_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        if let Ok(bytes) = serde_json::to_vec_pretty(&Value::Object(map)) {
            let _ = fs::write(&self.store_path, bytes);
        }
    }

    pub fn get_task_list(&self) -> Vec<MediaDownloadTask> {
        let lock = self.tasks.read().unwrap();
        lock.iter()
            .map(|t| t.lock().unwrap().to_public_task())
            .collect()
    }

    fn find_task(&self, id: &str) -> Option<Arc<Mutex<FullMediaDownloadTask>>> {
        let lock = self.tasks.read().unwrap();
        lock.iter().find(|t| t.lock().unwrap().id == id).cloned()
    }

    pub fn broadcast_full<R: Runtime>(&self, app: &AppHandle<R>) {
        let data = self.get_task_list();
        let _ = app.emit(
            "download:sync",
            json!({
                "type": "full",
                "data": data,
            }),
        );
    }

    pub fn emit_task_update<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        task: MediaDownloadTask,
        force_now: bool,
    ) {
        self.pending_updates
            .lock()
            .unwrap()
            .insert(task.id.clone(), task);

        let now = Instant::now();
        let should_flush_immediately = {
            let mut last = self.last_flush.lock().unwrap();
            if force_now || now.duration_since(*last) >= Duration::from_millis(500) {
                *last = now;
                true
            } else {
                false
            }
        };

        if should_flush_immediately {
            self.flush_pending_updates(app);
        } else if !self.flush_scheduled.swap(true, Ordering::SeqCst) {
            let mgr = self.clone();
            let app_clone = app.clone();
            tauri::async_runtime::spawn(async move {
                tokio::time::sleep(Duration::from_millis(500)).await;
                mgr.flush_scheduled.store(false, Ordering::SeqCst);
                *mgr.last_flush.lock().unwrap() = Instant::now();
                mgr.flush_pending_updates(&app_clone);
            });
        }
    }

    fn flush_pending_updates<R: Runtime>(&self, app: &AppHandle<R>) {
        let updates: Vec<MediaDownloadTask> = {
            let mut pending = self.pending_updates.lock().unwrap();
            if pending.is_empty() {
                return;
            }
            pending.drain().map(|(_, v)| v).collect()
        };
        let _ = app.emit(
            "download:sync",
            json!({
                "type": "update",
                "data": updates,
            }),
        );
    }

    fn resolve_save_dir<R: Runtime>(&self, app: &AppHandle<R>, bili: &BiliState) -> String {
        let configured = read_store_object(app, "app-settings")
            .ok()
            .and_then(|v| {
                v.get("appSettings")?
                    .get("downloadPath")?
                    .as_str()
                    .map(|s| s.trim().to_string())
            })
            .filter(|s| !s.is_empty());
        let dir = configured.unwrap_or_else(|| {
            dirs::download_dir()
                .unwrap_or_else(std::env::temp_dir)
                .to_string_lossy()
                .into_owned()
        });
        bili.register_local_dir(Path::new(&dir));
        dir
    }

    pub fn add_task<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        bili: Arc<BiliState>,
        info: MediaDownloadInfo,
    ) {
        let id = uuid::Uuid::new_v4().to_string();
        let title = if info.title.trim().is_empty() {
            format!("biu下载-{id}")
        } else {
            info.title.clone()
        };
        let now_ms = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_millis() as u64;
        let save_dir = self.resolve_save_dir(app, &bili);
        let temp_dir = std::env::temp_dir()
            .join("biu-temp-downloader")
            .join(&id)
            .to_string_lossy()
            .into_owned();

        let full = FullMediaDownloadTask {
            id: id.clone(),
            output_file_type: if info.output_file_type.is_empty() {
                "audio".to_string()
            } else {
                info.output_file_type
            },
            title,
            cover: info.cover,
            created_time: Some(now_ms),
            bvid: info.bvid,
            cid: info.cid,
            sid: info.sid,
            audio_url: None,
            audio_codecs: None,
            audio_bandwidth: None,
            video_url: None,
            video_resolution: None,
            video_frame_rate: None,
            total_bytes: 0,
            audio_total_bytes: 0,
            video_total_bytes: 0,
            downloaded_bytes: 0,
            download_progress: 0,
            merge_progress: 0,
            convert_progress: 0,
            status: "waiting".to_string(),
            file_name: None,
            temp_dir,
            save_dir,
            save_path: None,
            audio_temp_path: None,
            video_temp_path: None,
            chunks: Vec::new(),
            error: None,
        };

        let task_arc = Arc::new(Mutex::new(full));
        self.tasks.write().unwrap().push(task_arc.clone());
        self.save_to_disk();
        self.broadcast_full(app);
        self.spawn_worker(app.clone(), bili, task_arc, false);
    }

    fn spawn_worker<R: Runtime>(
        self: &Arc<Self>,
        app: AppHandle<R>,
        bili: Arc<BiliState>,
        task_arc: Arc<Mutex<FullMediaDownloadTask>>,
        is_resume: bool,
    ) {
        let id = task_arc.lock().unwrap().id.clone();
        let cancel_flag = Arc::new(AtomicBool::new(false));
        if let Some(prev) = self
            .cancel_flags
            .lock()
            .unwrap()
            .insert(id.clone(), cancel_flag.clone())
        {
            prev.store(true, Ordering::SeqCst);
        }

        let mgr = self.clone();
        tauri::async_runtime::spawn(async move {
            let permit = mgr.semaphore.clone().acquire_owned().await;
            let Ok(_permit) = permit else {
                return;
            };
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
            mgr.run_task_pipeline(&app, &bili, &task_arc, &cancel_flag, is_resume)
                .await;
            mgr.save_to_disk();
        });
    }

    async fn run_task_pipeline<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        bili: &Arc<BiliState>,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
        cancel_flag: &Arc<AtomicBool>,
        is_resume: bool,
    ) {
        // Resolve video pages if bvid is set without cid
        let (bvid_opt, has_cid, output_file_type, task_id) = {
            let t = task_arc.lock().unwrap();
            (
                t.bvid.clone(),
                value_id_to_string(&t.cid).is_some(),
                t.output_file_type.clone(),
                t.id.clone(),
            )
        };

        if let Some(bvid) = bvid_opt.filter(|b| !b.is_empty() && !has_cid) {
            match get_web_interface_view(bili, &bvid).await {
                Ok(view_res) => {
                    let pages = view_res
                        .get("data")
                        .and_then(|d| d.get("pages"))
                        .and_then(|p| p.as_array())
                        .cloned()
                        .unwrap_or_default();
                    if pages.is_empty() {
                        self.fail_task(app, task_arc, "无法获取视频分集信息".to_string());
                        return;
                    } else if pages.len() == 1 {
                        if let Some(cid_val) = pages[0].get("cid").cloned() {
                            task_arc.lock().unwrap().cid = Some(cid_val);
                        }
                    } else {
                        for page in pages {
                            let cid = page.get("cid").cloned();
                            let title = page
                                .get("part")
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            let cover = page
                                .get("first_frame")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string());
                            self.add_task(
                                app,
                                bili.clone(),
                                MediaDownloadInfo {
                                    output_file_type: output_file_type.clone(),
                                    bvid: Some(bvid.clone()),
                                    cid,
                                    sid: None,
                                    title,
                                    cover,
                                },
                            );
                        }
                        self.cancel_task(app, &task_id).await;
                        return;
                    }
                }
                Err(e) => {
                    self.fail_task(app, task_arc, e);
                    return;
                }
            }
        }

        let current_status = task_arc.lock().unwrap().status.clone();
        let start_from_stage = if is_resume {
            match current_status.as_str() {
                "mergePaused" => "merge",
                "convertPaused" => "convert",
                _ => "download",
            }
        } else {
            "download"
        };

        if start_from_stage == "download" {
            {
                let mut t = task_arc.lock().unwrap();
                t.status = "downloading".to_string();
                let pub_t = t.to_public_task();
                drop(t);
                self.emit_task_update(app, pub_t, true);
            }

            if let Err(err) = self.stage_download(app, bili, task_arc, cancel_flag).await {
                if !cancel_flag.load(Ordering::SeqCst) {
                    self.fail_task(app, task_arc, err);
                }
                return;
            }
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
        }

        if start_from_stage == "download" || start_from_stage == "merge" {
            if let Err(err) = self.stage_merge(app, task_arc, cancel_flag).await {
                if !cancel_flag.load(Ordering::SeqCst) {
                    self.fail_task(app, task_arc, err);
                }
                return;
            }
            if cancel_flag.load(Ordering::SeqCst) {
                return;
            }
        }

        if let Err(err) = self.stage_convert(app, bili, task_arc, cancel_flag).await {
            if !cancel_flag.load(Ordering::SeqCst) {
                self.fail_task(app, task_arc, err);
            }
            return;
        }
        if cancel_flag.load(Ordering::SeqCst) {
            return;
        }

        let pub_t = {
            let mut t = task_arc.lock().unwrap();
            t.status = "completed".to_string();
            t.convert_progress = 100;
            t.to_public_task()
        };
        self.emit_task_update(app, pub_t, true);
    }

    async fn stage_download<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        bili: &Arc<BiliState>,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
        cancel_flag: &Arc<AtomicBool>,
    ) -> Result<(), String> {
        // 1. Check if URLs need refreshing
        let need_urls = {
            let t = task_arc.lock().unwrap();
            if t.output_file_type == "audio" {
                !is_url_valid(t.audio_url.as_deref())
            } else {
                !is_url_valid(t.video_url.as_deref())
                    || (t.audio_codecs.is_some() && !is_url_valid(t.audio_url.as_deref()))
            }
        };

        if need_urls {
            self.fetch_download_urls(app, bili, task_arc).await?;
        }
        if cancel_flag.load(Ordering::SeqCst) {
            return Ok(());
        }

        // 2. Fetch Content-Length for audio and/or video
        let (output_file_type, audio_url, video_url, id) = {
            let t = task_arc.lock().unwrap();
            (
                t.output_file_type.clone(),
                t.audio_url.clone(),
                t.video_url.clone(),
                t.id.clone(),
            )
        };

        let mut total_bytes = 0u64;
        let mut audio_total_bytes = 0u64;
        let mut video_total_bytes = 0u64;

        if let Some(a_url) = audio_url.as_deref().filter(|s| !s.is_empty()) {
            let len = self.get_content_length(bili, a_url).await;
            if len == 0 {
                return Err("can't get audio file size".to_string());
            }
            audio_total_bytes = len;
            total_bytes += len;
        }
        if output_file_type == "video" {
            let v_url = video_url
                .as_deref()
                .filter(|s| !s.is_empty())
                .ok_or_else(|| "can't get video url".to_string())?;
            let len = self.get_content_length(bili, v_url).await;
            if len == 0 {
                return Err("can't get video file size".to_string());
            }
            video_total_bytes = len;
            total_bytes += len;
        }

        let temp_dir_path = std::env::temp_dir().join("biu-temp-downloader").join(&id);
        fs::create_dir_all(&temp_dir_path).map_err(|e| e.to_string())?;

        let mut chunks = Vec::new();
        if audio_total_bytes > 0 {
            chunks.extend(split_chunks("audio", audio_total_bytes, CHUNK_SIZE));
        }
        if output_file_type == "video" && video_total_bytes > 0 {
            chunks.extend(split_chunks("video", video_total_bytes, CHUNK_SIZE));
        }

        {
            let mut t = task_arc.lock().unwrap();
            t.total_bytes = total_bytes;
            t.audio_total_bytes = audio_total_bytes;
            t.video_total_bytes = video_total_bytes;
            t.temp_dir = temp_dir_path.to_string_lossy().into_owned();
            if audio_total_bytes > 0 {
                t.audio_temp_path = Some(
                    temp_dir_path
                        .join("audio.m4s")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            if output_file_type == "video" {
                t.video_temp_path = Some(
                    temp_dir_path
                        .join("video.m4s")
                        .to_string_lossy()
                        .into_owned(),
                );
            }
            t.chunks = chunks.clone();
            t.downloaded_bytes = 0;
        }

        // 3. Check existing chunk files & queue pending chunks (concurrency = 5)
        let chunk_sem = Arc::new(Semaphore::new(CHUNK_CONCURRENCY));
        let mut join_handles = Vec::new();

        for (idx, chunk) in chunks.iter().enumerate() {
            let chunk_path = temp_dir_path.join(&chunk.name);
            let expected_size = chunk.end - chunk.start + 1;
            let mut current_size = fs::metadata(&chunk_path).map(|m| m.len()).unwrap_or(0);

            if current_size > expected_size {
                let _ = fs::remove_file(&chunk_path);
                current_size = 0;
            } else if current_size == expected_size {
                let pub_t = {
                    let mut t = task_arc.lock().unwrap();
                    t.downloaded_bytes += current_size;
                    t.download_progress = if t.total_bytes > 0 {
                        ((t.downloaded_bytes as f64 / t.total_bytes as f64) * 100.0).round() as u32
                    } else {
                        0
                    };
                    if let Some(c) = t.chunks.get_mut(idx) {
                        c.done = true;
                    }
                    t.to_public_task()
                };
                self.emit_task_update(app, pub_t, false);
                continue;
            }

            if current_size > 0 {
                let mut t = task_arc.lock().unwrap();
                t.downloaded_bytes += current_size;
            }

            let url = if chunk.chunk_type == "audio" {
                audio_url
                    .clone()
                    .ok_or_else(|| "audio url is missing".to_string())?
            } else {
                video_url
                    .clone()
                    .ok_or_else(|| "video url is missing".to_string())?
            };

            let sem = chunk_sem.clone();
            let bili_clone = bili.clone();
            let app_clone = app.clone();
            let mgr_clone = self.clone();
            let task_clone = task_arc.clone();
            let cancel_clone = cancel_flag.clone();
            let chunk_clone = chunk.clone();

            let handle = tauri::async_runtime::spawn(async move {
                let Ok(_permit) = sem.acquire_owned().await else {
                    return Err("Semaphore closed".to_string());
                };
                if cancel_clone.load(Ordering::SeqCst) {
                    return Ok(());
                }
                mgr_clone
                    .download_single_chunk(
                        &app_clone,
                        &bili_clone,
                        &task_clone,
                        &cancel_clone,
                        idx,
                        &chunk_clone,
                        &url,
                        &chunk_path,
                        current_size,
                    )
                    .await
            });
            join_handles.push(handle);
        }

        for h in join_handles {
            match h.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    cancel_flag.store(true, Ordering::SeqCst);
                    return Err(e);
                }
                Err(e) => {
                    cancel_flag.store(true, Ordering::SeqCst);
                    return Err(e.to_string());
                }
            }
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_single_chunk<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        bili: &BiliState,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
        cancel_flag: &Arc<AtomicBool>,
        chunk_idx: usize,
        chunk: &MediaDownloadChunk,
        url: &str,
        dest_path: &Path,
        offset: u64,
    ) -> Result<(), String> {
        let range_header = format!("bytes={}-{}", chunk.start + offset, chunk.end);
        let mut last_err = String::new();

        for attempt in 0..3 {
            if cancel_flag.load(Ordering::SeqCst) {
                return Ok(());
            }
            let resp_res = bili
                .client()
                .get(url)
                .header(reqwest::header::ORIGIN, BILI_ORIGIN)
                .header(reqwest::header::REFERER, BILI_ORIGIN)
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .header(reqwest::header::RANGE, &range_header)
                .send()
                .await;

            let mut resp = match resp_res {
                Ok(r) if r.status().is_success() || r.status().as_u16() == 206 => r,
                Ok(r) => {
                    last_err = format!("HTTP {}", r.status());
                    if attempt < 2 {
                        tokio::time::sleep(Duration::from_millis(300 * (attempt + 1))).await;
                    }
                    continue;
                }
                Err(e) => {
                    last_err = e.to_string();
                    if attempt < 2 {
                        tokio::time::sleep(Duration::from_millis(300 * (attempt + 1))).await;
                    }
                    continue;
                }
            };

            let mut file_opts = tokio::fs::OpenOptions::new();
            file_opts.create(true).write(true);
            if offset > 0 {
                file_opts.append(true);
            } else {
                file_opts.truncate(true);
            }
            let mut file = file_opts.open(dest_path).await.map_err(|e| e.to_string())?;

            let mut bytes_in_attempt = 0u64;
            let mut stream_err = None;
            loop {
                if cancel_flag.load(Ordering::SeqCst) {
                    let _ = file.flush().await;
                    return Ok(());
                }
                match resp.chunk().await {
                    Ok(Some(bytes)) => {
                        if let Err(e) = file.write_all(&bytes).await {
                            stream_err = Some(e.to_string());
                            break;
                        }
                        let len = bytes.len() as u64;
                        bytes_in_attempt += len;
                        let pub_t = {
                            let mut t = task_arc.lock().unwrap();
                            t.downloaded_bytes += len;
                            t.download_progress = if t.total_bytes > 0 {
                                ((t.downloaded_bytes as f64 / t.total_bytes as f64) * 100.0)
                                    .round()
                                    .min(100.0) as u32
                            } else {
                                0
                            };
                            t.to_public_task()
                        };
                        self.emit_task_update(app, pub_t, false);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        stream_err = Some(e.to_string());
                        break;
                    }
                }
            }
            let _ = file.flush().await;

            if let Some(e) = stream_err {
                // Roll back bytes counted in this failed attempt before retry
                {
                    let mut t = task_arc.lock().unwrap();
                    t.downloaded_bytes = t.downloaded_bytes.saturating_sub(bytes_in_attempt);
                }
                last_err = e;
                continue;
            }

            {
                let mut t = task_arc.lock().unwrap();
                if let Some(c) = t.chunks.get_mut(chunk_idx) {
                    c.done = true;
                }
            }
            return Ok(());
        }

        Err(format!("chunk {} failed: {last_err}", chunk.name))
    }

    async fn stage_merge<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
        cancel_flag: &Arc<AtomicBool>,
    ) -> Result<(), String> {
        let (temp_dir, audio_temp, video_temp, chunks, total_bytes) = {
            let mut t = task_arc.lock().unwrap();
            t.status = "merging".to_string();
            (
                PathBuf::from(&t.temp_dir),
                t.audio_temp_path.clone().map(PathBuf::from),
                t.video_temp_path.clone().map(PathBuf::from),
                t.chunks.clone(),
                t.total_bytes,
            )
        };

        let mut merged_bytes = 0u64;
        if let Some(ref ap) = audio_temp {
            if let Ok(m) = fs::metadata(ap) {
                merged_bytes += m.len();
            }
        }
        if let Some(ref vp) = video_temp {
            if let Ok(m) = fs::metadata(vp) {
                merged_bytes += m.len();
            }
        }

        {
            let mut t = task_arc.lock().unwrap();
            t.merge_progress = if total_bytes > 0 {
                ((merged_bytes as f64 / total_bytes as f64) * 100.0).round() as u32
            } else {
                0
            };
            let pub_t = t.to_public_task();
            drop(t);
            self.emit_task_update(app, pub_t, true);
        }

        if let Some(ref ap) = audio_temp {
            self.merge_chunks_by_type(
                app,
                task_arc,
                cancel_flag,
                "audio",
                &temp_dir,
                ap,
                &chunks,
                &mut merged_bytes,
                total_bytes,
            )?;
        }
        if cancel_flag.load(Ordering::SeqCst) {
            return Ok(());
        }
        if let Some(ref vp) = video_temp {
            self.merge_chunks_by_type(
                app,
                task_arc,
                cancel_flag,
                "video",
                &temp_dir,
                vp,
                &chunks,
                &mut merged_bytes,
                total_bytes,
            )?;
        }
        if cancel_flag.load(Ordering::SeqCst) {
            return Ok(());
        }

        for chunk in &chunks {
            let _ = fs::remove_file(temp_dir.join(&chunk.name));
        }
        task_arc.lock().unwrap().chunks.clear();
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn merge_chunks_by_type<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
        cancel_flag: &Arc<AtomicBool>,
        chunk_type: &str,
        temp_dir: &Path,
        dest_path: &Path,
        all_chunks: &[MediaDownloadChunk],
        merged_bytes: &mut u64,
        total_bytes: u64,
    ) -> Result<(), String> {
        let chunks: Vec<&MediaDownloadChunk> = all_chunks
            .iter()
            .filter(|c| c.chunk_type == chunk_type)
            .collect();
        if chunks.is_empty() {
            return Ok(());
        }

        let mut start_index = 0usize;
        if dest_path.exists() {
            let merged_size = fs::metadata(dest_path).map(|m| m.len()).unwrap_or(0);
            let mut accumulated = 0u64;
            for (i, c) in chunks.iter().enumerate() {
                let c_size = c.end - c.start + 1;
                if merged_size >= accumulated + c_size {
                    accumulated += c_size;
                    start_index = i + 1;
                } else {
                    if let Ok(f) = OpenOptions::new().write(true).open(dest_path) {
                        if f.set_len(accumulated).is_err() {
                            let _ = fs::remove_file(dest_path);
                            start_index = 0;
                        }
                    } else {
                        let _ = fs::remove_file(dest_path);
                        start_index = 0;
                    }
                    break;
                }
            }
        }

        if start_index >= chunks.len() || cancel_flag.load(Ordering::SeqCst) {
            return Ok(());
        }

        let mut out_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dest_path)
            .map_err(|e| e.to_string())?;

        let mut buf = vec![0u8; 64 * 1024];
        for c in chunks.iter().skip(start_index) {
            if cancel_flag.load(Ordering::SeqCst) {
                return Ok(());
            }
            let chunk_path = temp_dir.join(&c.name);
            let mut in_file = fs::File::open(&chunk_path).map_err(|e| e.to_string())?;
            loop {
                if cancel_flag.load(Ordering::SeqCst) {
                    return Ok(());
                }
                let n = in_file.read(&mut buf).map_err(|e| e.to_string())?;
                if n == 0 {
                    break;
                }
                out_file.write_all(&buf[..n]).map_err(|e| e.to_string())?;
                *merged_bytes += n as u64;
                let pub_t = {
                    let mut t = task_arc.lock().unwrap();
                    t.merge_progress = if total_bytes > 0 {
                        ((*merged_bytes as f64 / total_bytes as f64) * 100.0)
                            .round()
                            .min(100.0) as u32
                    } else {
                        0
                    };
                    t.to_public_task()
                };
                self.emit_task_update(app, pub_t, false);
            }
        }
        out_file.flush().map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn stage_convert<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        bili: &BiliState,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
        cancel_flag: &Arc<AtomicBool>,
    ) -> Result<(), String> {
        let (output_file_type, title, audio_codecs, mut save_dir, temp_dir, audio_temp, video_temp) = {
            let mut t = task_arc.lock().unwrap();
            t.status = "converting".to_string();
            let pub_t = t.to_public_task();
            let res = (
                t.output_file_type.clone(),
                t.title.clone(),
                t.audio_codecs.clone().unwrap_or_default(),
                t.save_dir.clone(),
                PathBuf::from(&t.temp_dir),
                t.audio_temp_path.clone().map(PathBuf::from),
                t.video_temp_path.clone().map(PathBuf::from),
            );
            drop(t);
            self.emit_task_update(app, pub_t, true);
            res
        };

        if save_dir.trim().is_empty() {
            save_dir = self.resolve_save_dir(app, bili);
        } else {
            bili.register_local_dir(Path::new(&save_dir));
        }
        fs::create_dir_all(&save_dir).map_err(|e| e.to_string())?;

        if output_file_type == "video" {
            let vp = video_temp
                .as_ref()
                .ok_or_else(|| "can't get video temp path".to_string())?;
            if !vp.exists() {
                return Err("can't get video temp path".to_string());
            }
        }

        let ext = if output_file_type == "video" {
            if audio_codecs.to_ascii_lowercase().contains("flac") {
                ".mkv"
            } else {
                ".mp4"
            }
        } else if audio_codecs.to_ascii_lowercase().contains("flac") {
            ".flac"
        } else if audio_codecs.to_ascii_lowercase().contains("mp3") {
            ".mp3"
        } else {
            ".m4a"
        };

        let sanitized = sanitize_filename(&title);
        let base_name = if sanitized.is_empty() {
            "download".to_string()
        } else {
            sanitized
        };
        let file_name = format!("{base_name}{ext}");
        let desired_save_path = Path::new(&save_dir).join(&file_name);

        let user_ffmpeg = user_ffmpeg_setting(app);
        let ffmpeg_bin = resolve_ffmpeg_path(user_ffmpeg.as_deref())?;
        let cancel_clone = cancel_flag.clone();
        let out_type_clone = output_file_type.clone();
        let a_temp_clone = audio_temp.clone();
        let v_temp_clone = video_temp.clone();
        let desired_clone = desired_save_path.clone();

        let final_save_path = tokio::task::spawn_blocking(move || {
            convert_with_ffmpeg(
                &ffmpeg_bin,
                &out_type_clone,
                a_temp_clone.as_deref(),
                v_temp_clone.as_deref(),
                &desired_clone,
                &cancel_clone,
            )
        })
        .await
        .map_err(|e| e.to_string())??;

        let final_name = final_save_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or(&file_name)
            .to_string();
        let final_str = final_save_path.to_string_lossy().into_owned();

        let pub_t = {
            let mut t = task_arc.lock().unwrap();
            t.save_dir = save_dir;
            t.save_path = Some(final_str);
            t.file_name = Some(final_name);
            t.to_public_task()
        };
        self.emit_task_update(app, pub_t, true);

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    async fn fetch_download_urls<R: Runtime>(
        &self,
        app: &AppHandle<R>,
        bili: &BiliState,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
    ) -> Result<(), String> {
        let (sid_opt, bvid_opt, cid_opt, output_file_type) = {
            let t = task_arc.lock().unwrap();
            (
                value_id_to_string(&t.sid),
                t.bvid.clone(),
                value_id_to_string(&t.cid),
                t.output_file_type.clone(),
            )
        };

        if let Some(sid) = sid_opt {
            let user_store =
                read_store_object(app, "user-login-info").unwrap_or_else(|_| json!({}));
            let mid = user_store
                .get("mid")
                .and_then(|v| {
                    v.as_str()
                        .map(|s| s.to_string())
                        .or_else(|| v.as_i64().map(|n| n.to_string()))
                })
                .unwrap_or_default();
            let vip_status = user_store
                .get("vipStatus")
                .and_then(|v| v.as_i64())
                .unwrap_or(2);
            let quality = if vip_status != 0 { "3" } else { "2" };

            let mut url = Url::parse("https://api.bilibili.com/audio/music-service-c/url")
                .map_err(|e| e.to_string())?;
            url.query_pairs_mut()
                .append_pair("mid", &mid)
                .append_pair("songid", &sid)
                .append_pair("quality", quality)
                .append_pair("privilege", "2")
                .append_pair("platform", "web");

            let resp = bili
                .client()
                .get(url)
                .timeout(Duration::from_secs(15))
                .send()
                .await
                .map_err(|e| e.to_string())?;
            let body = resp.json::<Value>().await.map_err(|e| e.to_string())?;
            extract_and_register_playurl_hosts(bili, &body);

            if let Some(cdn) = body
                .get("data")
                .and_then(|d| d.get("cdns"))
                .and_then(|c| c.as_array())
                .and_then(|a| a.first())
                .and_then(|v| v.as_str())
            {
                let stream_type = body
                    .get("data")
                    .and_then(|d| d.get("type"))
                    .and_then(|v| v.as_i64())
                    .unwrap_or(-1);
                let is_flac = stream_type == 3;
                let bw = match stream_type {
                    0 => 128_000,
                    1 => 192_000,
                    2 => 320_000,
                    _ => 0,
                };
                let mut t = task_arc.lock().unwrap();
                t.audio_url = Some(cdn.to_string());
                t.audio_codecs = Some(if is_flac { "flac" } else { "aac" }.to_string());
                t.audio_bandwidth = Some(bw);
            }
        } else if let (Some(bvid), Some(cid)) = (bvid_opt, cid_opt) {
            let dash_res = get_dash_url(bili, &bvid, &cid, 4048, None).await?;
            let dash = dash_res.get("data").and_then(|d| d.get("dash"));

            let audio_arr = dash
                .and_then(|d| d.get("audio"))
                .and_then(|a| a.as_array())
                .cloned()
                .unwrap_or_default();
            let sorted_audio = sort_dash_audio(&audio_arr);
            let best_video = dash
                .and_then(|d| d.get("video"))
                .and_then(|v| v.as_array())
                .and_then(|a| a.first());
            let flac_audio = dash
                .and_then(|d| d.get("flac"))
                .and_then(|f| f.get("audio"));
            let dolby_audio = dash
                .and_then(|d| d.get("dolby"))
                .and_then(|d| d.get("audio"))
                .and_then(|a| a.as_array())
                .and_then(|a| a.first());

            let chosen_audio = flac_audio
                .filter(|item| dash_item_url(item).is_some())
                .or_else(|| dolby_audio.filter(|item| dash_item_url(item).is_some()))
                .or_else(|| sorted_audio.first());

            let mut t = task_arc.lock().unwrap();
            if let Some(a_item) = chosen_audio {
                t.audio_url = dash_item_url(a_item);
                t.audio_codecs = a_item
                    .get("codecs")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                t.audio_bandwidth = a_item.get("bandwidth").and_then(|v| v.as_u64());
            }
            if output_file_type == "video" {
                if let Some(v_item) = best_video {
                    t.video_url = dash_item_url(v_item);
                    let w = v_item.get("width").and_then(|v| v.as_u64()).unwrap_or(0);
                    let h = v_item.get("height").and_then(|v| v.as_u64()).unwrap_or(0);
                    t.video_resolution = Some(format!("{w}*{h}"));
                    let fr_str = v_item
                        .get("frameRate")
                        .or_else(|| v_item.get("frame_rate"))
                        .and_then(|v| v.as_str())
                        .and_then(|s| s.parse::<f64>().ok())
                        .map(|f| (f.floor() as i64).to_string())
                        .unwrap_or_default();
                    t.video_frame_rate = Some(fr_str);
                }
            }
        }

        let t = task_arc.lock().unwrap();
        if t.output_file_type == "audio" && t.audio_url.as_deref().unwrap_or("").is_empty() {
            return Err("can't get audio url".to_string());
        }
        if t.output_file_type == "video" && t.video_url.as_deref().unwrap_or("").is_empty() {
            return Err("can't get video url".to_string());
        }
        Ok(())
    }

    async fn get_content_length(&self, bili: &BiliState, url: &str) -> u64 {
        for _ in 0..3 {
            if let Ok(resp) = bili
                .client()
                .head(url)
                .header(reqwest::header::ORIGIN, BILI_ORIGIN)
                .header(reqwest::header::REFERER, BILI_ORIGIN)
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .timeout(Duration::from_secs(10))
                .send()
                .await
            {
                if let Some(len) = resp
                    .headers()
                    .get(reqwest::header::CONTENT_LENGTH)
                    .and_then(|v| v.to_str().ok())
                    .and_then(|s| s.parse::<u64>().ok())
                    .filter(|&n| n > 0)
                {
                    return len;
                }
            }
        }

        if let Ok(resp) = bili
            .client()
            .get(url)
            .header(reqwest::header::ORIGIN, BILI_ORIGIN)
            .header(reqwest::header::REFERER, BILI_ORIGIN)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .timeout(Duration::from_secs(10))
            .send()
            .await
        {
            if let Some(len) = resp
                .headers()
                .get(reqwest::header::CONTENT_LENGTH)
                .and_then(|v| v.to_str().ok())
                .and_then(|s| s.parse::<u64>().ok())
            {
                return len;
            }
        }
        0
    }

    fn fail_task<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
        error: String,
    ) {
        let pub_t = {
            let mut t = task_arc.lock().unwrap();
            if matches!(
                t.status.as_str(),
                "downloadPaused" | "mergePaused" | "convertPaused"
            ) {
                return;
            }
            t.status = "failed".to_string();
            t.error = Some(error);
            t.to_public_task()
        };
        self.emit_task_update(app, pub_t, true);
    }

    pub fn pause_task<R: Runtime>(self: &Arc<Self>, app: &AppHandle<R>, id: &str) {
        if let Some(flag) = self.cancel_flags.lock().unwrap().remove(id) {
            flag.store(true, Ordering::SeqCst);
        }
        if let Some(task_arc) = self.find_task(id) {
            let pub_t = {
                let mut t = task_arc.lock().unwrap();
                t.status = match t.status.as_str() {
                    "downloading" | "waiting" => "downloadPaused".to_string(),
                    "merging" => "mergePaused".to_string(),
                    "converting" => "convertPaused".to_string(),
                    other => other.to_string(),
                };
                t.to_public_task()
            };
            self.save_to_disk();
            self.emit_task_update(app, pub_t, true);
        }
    }

    pub fn resume_task<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        bili: Arc<BiliState>,
        id: &str,
    ) {
        if let Some(task_arc) = self.find_task(id) {
            self.spawn_worker(app.clone(), bili, task_arc, true);
        }
    }

    pub fn retry_task<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        bili: Arc<BiliState>,
        id: &str,
    ) {
        if let Some(flag) = self.cancel_flags.lock().unwrap().remove(id) {
            flag.store(true, Ordering::SeqCst);
        }
        if let Some(task_arc) = self.find_task(id) {
            {
                let mut t = task_arc.lock().unwrap();
                t.status = "waiting".to_string();
                t.error = None;
                t.download_progress = 0;
                t.merge_progress = 0;
                t.convert_progress = 0;
            }
            self.save_to_disk();
            self.broadcast_full(app);
            self.spawn_worker(app.clone(), bili, task_arc, false);
        }
    }

    pub async fn cancel_task<R: Runtime>(self: &Arc<Self>, app: &AppHandle<R>, id: &str) {
        self.pending_updates.lock().unwrap().remove(id);
        if let Some(flag) = self.cancel_flags.lock().unwrap().remove(id) {
            flag.store(true, Ordering::SeqCst);
        }
        let removed = {
            let mut lock = self.tasks.write().unwrap();
            lock.iter()
                .position(|t| t.lock().unwrap().id == id)
                .map(|pos| lock.remove(pos))
        };
        if let Some(task_arc) = removed {
            let temp_dir = task_arc.lock().unwrap().temp_dir.clone();
            if !temp_dir.is_empty() {
                let _ = tokio::fs::remove_dir_all(&temp_dir).await;
            }
        }
        self.save_to_disk();
        self.broadcast_full(app);
    }

    pub async fn clear_tasks<R: Runtime>(self: &Arc<Self>, app: &AppHandle<R>) {
        self.pending_updates.lock().unwrap().clear();
        {
            let mut flags = self.cancel_flags.lock().unwrap();
            for (_, flag) in flags.drain() {
                flag.store(true, Ordering::SeqCst);
            }
        }
        let all_tasks: Vec<Arc<Mutex<FullMediaDownloadTask>>> = {
            let mut lock = self.tasks.write().unwrap();
            lock.drain(..).collect()
        };
        for task_arc in all_tasks {
            let temp_dir = task_arc.lock().unwrap().temp_dir.clone();
            if !temp_dir.is_empty() {
                let _ = tokio::fs::remove_dir_all(&temp_dir).await;
            }
        }
        self.save_to_disk();
        self.broadcast_full(app);
    }
}

#[tauri::command]
pub async fn download_get_list(
    mgr: tauri::State<'_, Arc<DownloadManager>>,
) -> Result<Vec<MediaDownloadTask>, String> {
    Ok(mgr.get_task_list())
}

#[tauri::command]
pub async fn download_add<R: Runtime>(
    app: AppHandle<R>,
    mgr: tauri::State<'_, Arc<DownloadManager>>,
    bili: tauri::State<'_, Arc<BiliState>>,
    task: MediaDownloadInfo,
) -> Result<(), String> {
    mgr.add_task(&app, bili.inner().clone(), task);
    Ok(())
}

#[tauri::command]
pub async fn download_add_list<R: Runtime>(
    app: AppHandle<R>,
    mgr: tauri::State<'_, Arc<DownloadManager>>,
    bili: tauri::State<'_, Arc<BiliState>>,
    tasks: Vec<MediaDownloadInfo>,
) -> Result<(), String> {
    for task in tasks {
        mgr.add_task(&app, bili.inner().clone(), task);
    }
    Ok(())
}

#[tauri::command]
pub async fn download_pause<R: Runtime>(
    app: AppHandle<R>,
    mgr: tauri::State<'_, Arc<DownloadManager>>,
    id: String,
) -> Result<(), String> {
    mgr.pause_task(&app, &id);
    Ok(())
}

#[tauri::command]
pub async fn download_resume<R: Runtime>(
    app: AppHandle<R>,
    mgr: tauri::State<'_, Arc<DownloadManager>>,
    bili: tauri::State<'_, Arc<BiliState>>,
    id: String,
) -> Result<(), String> {
    mgr.resume_task(&app, bili.inner().clone(), &id);
    Ok(())
}

#[tauri::command]
pub async fn download_retry<R: Runtime>(
    app: AppHandle<R>,
    mgr: tauri::State<'_, Arc<DownloadManager>>,
    bili: tauri::State<'_, Arc<BiliState>>,
    id: String,
) -> Result<(), String> {
    mgr.retry_task(&app, bili.inner().clone(), &id);
    Ok(())
}

#[tauri::command]
pub async fn download_cancel<R: Runtime>(
    app: AppHandle<R>,
    mgr: tauri::State<'_, Arc<DownloadManager>>,
    id: String,
) -> Result<(), String> {
    mgr.cancel_task(&app, &id).await;
    Ok(())
}

#[tauri::command]
pub async fn download_clear<R: Runtime>(
    app: AppHandle<R>,
    mgr: tauri::State<'_, Arc<DownloadManager>>,
) -> Result<(), String> {
    mgr.clear_tasks(&app).await;
    Ok(())
}
