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

const MAX_FILENAME_STEM_BYTES: usize = 200;
const FFMPEG_CONVERT_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_FFMPEG_STDERR_BYTES: usize = 16 * 1024;
const CHUNK_READ_IDLE_TIMEOUT: Duration = Duration::from_secs(30);

fn is_windows_reserved_name(name: &str) -> bool {
    let base = name.split('.').next().unwrap_or(name).trim();
    matches!(
        base.to_ascii_uppercase().as_str(),
        "CON"
            | "PRN"
            | "AUX"
            | "NUL"
            | "COM1"
            | "COM2"
            | "COM3"
            | "COM4"
            | "COM5"
            | "COM6"
            | "COM7"
            | "COM8"
            | "COM9"
            | "LPT1"
            | "LPT2"
            | "LPT3"
            | "LPT4"
            | "LPT5"
            | "LPT6"
            | "LPT7"
            | "LPT8"
            | "LPT9"
    )
}

fn truncate_utf8_bytes(s: &str, max_bytes: usize) -> &str {
    if s.len() <= max_bytes {
        return s;
    }
    let mut end = max_bytes;
    while end > 0 && !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
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
    let trimmed = collapsed.trim().trim_end_matches(['.', ' ']);
    let truncated =
        truncate_utf8_bytes(trimmed, MAX_FILENAME_STEM_BYTES).trim_end_matches(['.', ' ']);
    if truncated.is_empty() {
        return String::new();
    }
    if is_windows_reserved_name(truncated) {
        format!("_{truncated}")
    } else {
        truncated.to_string()
    }
}

#[cfg(test)]
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

pub fn commit_temp_output_no_overwrite(
    temp_output: &Path,
    desired_output: &Path,
) -> Result<PathBuf, String> {
    let parent = desired_output.parent().unwrap_or_else(|| Path::new("."));
    let stem = desired_output
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    let ext = desired_output
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();

    for i in 0..10000usize {
        let candidate = if i == 0 {
            desired_output.to_path_buf()
        } else {
            parent.join(format!("{stem} ({i}){ext}"))
        };

        #[cfg(unix)]
        {
            match fs::hard_link(temp_output, &candidate) {
                Ok(()) => {
                    let _ = fs::remove_file(temp_output);
                    return Ok(candidate);
                }
                Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(_) => {
                    // Fallback if hard_link is unsupported on this filesystem
                }
            }
        }

        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&candidate)
        {
            Ok(placeholder) => {
                drop(placeholder);
                if let Err(e) = fs::rename(temp_output, &candidate) {
                    if let Err(copy_err) = fs::copy(temp_output, &candidate) {
                        let _ = fs::remove_file(&candidate);
                        return Err(format!("Failed to commit output ({e}): {copy_err}"));
                    }
                    let _ = fs::remove_file(temp_output);
                }
                return Ok(candidate);
            }
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("Cannot find available output path".to_string())
}

pub fn build_task_temp_output_path(desired_output: &Path, task_id: &str) -> PathBuf {
    let parent = desired_output.parent().unwrap_or_else(|| Path::new("."));
    let stem = desired_output
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("download");
    let ext = desired_output
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| format!(".{e}"))
        .unwrap_or_default();
    let safe_id: String = task_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '-' || c == '_' {
                c
            } else {
                '_'
            }
        })
        .collect();
    parent.join(format!(".{stem}.{safe_id}.part{ext}"))
}

fn spawn_pipe_tail_reader<R: Read + Send + 'static>(
    pipe: Option<R>,
    max_bytes: usize,
) -> std::thread::JoinHandle<String> {
    std::thread::spawn(move || {
        let Some(mut reader) = pipe else {
            return String::new();
        };
        let mut tail = std::collections::VecDeque::<u8>::with_capacity(max_bytes);
        let mut buf = [0u8; 4096];
        while let Ok(n) = reader.read(&mut buf) {
            if n == 0 {
                break;
            }
            let slice = &buf[..n];
            if slice.len() >= max_bytes {
                tail.clear();
                tail.extend(slice[slice.len() - max_bytes..].iter().copied());
            } else {
                let overflow = (tail.len() + slice.len()).saturating_sub(max_bytes);
                for _ in 0..overflow {
                    tail.pop_front();
                }
                tail.extend(slice.iter().copied());
            }
        }
        let bytes: Vec<u8> = tail.into_iter().collect();
        String::from_utf8_lossy(&bytes).into_owned()
    })
}

#[allow(clippy::too_many_arguments)]
pub fn convert_with_ffmpeg_for_task(
    ffmpeg_path: &Path,
    output_file_type: &str,
    audio_temp_path: Option<&Path>,
    video_temp_path: Option<&Path>,
    output_path: &Path,
    task_id: &str,
    cancelled: &AtomicBool,
    timeout: Duration,
) -> Result<PathBuf, String> {
    if cancelled.load(Ordering::SeqCst) {
        return Err("Aborted".to_string());
    }
    let temp_output = build_task_temp_output_path(output_path, task_id);
    let _ = fs::remove_file(&temp_output);

    let mut args: Vec<String> = vec![
        "-n".to_string(),
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
    args.push(temp_output.to_string_lossy().into_owned());

    let mut cmd = Command::new(ffmpeg_path);
    cmd.args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }

    let mut child = cmd
        .spawn()
        .map_err(|e| format!("Failed to spawn ffmpeg: {e}"))?;

    let stdout_handle = spawn_pipe_tail_reader(child.stdout.take(), 4096);
    let stderr_handle = spawn_pipe_tail_reader(child.stderr.take(), MAX_FFMPEG_STDERR_BYTES);
    let start = Instant::now();

    loop {
        if cancelled.load(Ordering::SeqCst) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            let _ = fs::remove_file(&temp_output);
            return Err("Aborted".to_string());
        }
        if start.elapsed() >= timeout {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            let _ = fs::remove_file(&temp_output);
            return Err("ffmpeg convert timed out".to_string());
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                let _ = stdout_handle.join();
                let stderr_str = stderr_handle.join().unwrap_or_default();
                if status.success() {
                    if cancelled.load(Ordering::SeqCst) {
                        let _ = fs::remove_file(&temp_output);
                        return Err("Aborted".to_string());
                    }
                    match commit_temp_output_no_overwrite(&temp_output, output_path) {
                        Ok(final_output) => return Ok(final_output),
                        Err(e) => {
                            let _ = fs::remove_file(&temp_output);
                            return Err(e);
                        }
                    }
                }
                let _ = fs::remove_file(&temp_output);
                return Err(format!("ffmpeg exited {status}: {stderr_str}"));
            }
            Ok(None) => {
                std::thread::sleep(Duration::from_millis(25));
            }
            Err(e) => {
                let _ = child.kill();
                let _ = child.wait();
                let _ = stdout_handle.join();
                let _ = stderr_handle.join();
                let _ = fs::remove_file(&temp_output);
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

pub fn parse_content_range_start(header_val: &str) -> Option<(u64, u64)> {
    let s = header_val.trim();
    let rest = s
        .strip_prefix("bytes")
        .or_else(|| s.strip_prefix("BYTES"))
        .or_else(|| {
            if s.len() >= 5 && s[..5].eq_ignore_ascii_case("bytes") {
                Some(&s[5..])
            } else {
                None
            }
        })?
        .trim();
    let range_part = rest.split('/').next()?.trim();
    let (start_str, end_str) = range_part.split_once('-')?;
    let start = start_str.trim().parse::<u64>().ok()?;
    let end = end_str.trim().parse::<u64>().ok()?;
    if start <= end {
        Some((start, end))
    } else {
        None
    }
}

pub struct WorkerControl {
    pub stop_flag: Arc<AtomicBool>,
    pub stop_notify: Arc<tokio::sync::Notify>,
    pub done_rx: tokio::sync::watch::Receiver<bool>,
}

pub struct DownloadManager {
    pub tasks: RwLock<Vec<Arc<Mutex<FullMediaDownloadTask>>>>,
    pub workers: Mutex<HashMap<String, WorkerControl>>,
    pub generations: Mutex<HashMap<String, u64>>,
    pub semaphore: Arc<Semaphore>,
    pub pending_updates: Mutex<HashMap<String, MediaDownloadTask>>,
    pub last_flush: Mutex<Instant>,
    pub flush_scheduled: AtomicBool,
    pub store_path: PathBuf,
    pub chunk_idle_timeout: RwLock<Duration>,
}

impl DownloadManager {
    pub fn new(store_path: PathBuf) -> Arc<Self> {
        let mgr = Arc::new(Self {
            tasks: RwLock::new(Vec::new()),
            workers: Mutex::new(HashMap::new()),
            generations: Mutex::new(HashMap::new()),
            semaphore: Arc::new(Semaphore::new(TASK_CONCURRENCY)),
            pending_updates: Mutex::new(HashMap::new()),
            last_flush: Mutex::new(
                Instant::now()
                    .checked_sub(Duration::from_secs(10))
                    .unwrap_or_else(Instant::now),
            ),
            flush_scheduled: AtomicBool::new(false),
            store_path,
            chunk_idle_timeout: RwLock::new(CHUNK_READ_IDLE_TIMEOUT),
        });
        mgr.restore_from_disk();
        mgr
    }

    #[cfg(test)]
    pub fn set_chunk_idle_timeout(&self, timeout: Duration) {
        *self.chunk_idle_timeout.write().unwrap() = timeout;
    }

    pub fn next_generation(&self, id: &str) -> u64 {
        let mut gens = self.generations.lock().unwrap();
        let entry = gens.entry(id.to_string()).or_insert(0);
        *entry += 1;
        *entry
    }

    pub fn is_current_generation(&self, id: &str, generation: u64) -> bool {
        self.generations
            .lock()
            .unwrap()
            .get(id)
            .copied()
            .unwrap_or(0)
            == generation
    }

    async fn stop_and_wait_worker(&self, id: &str) -> u64 {
        let (gen, prev_worker) = {
            let gen = self.next_generation(id);
            let prev = self.workers.lock().unwrap().remove(id);
            (gen, prev)
        };
        if let Some(mut w) = prev_worker {
            w.stop_flag.store(true, Ordering::SeqCst);
            w.stop_notify.notify_waiters();
            while !*w.done_rx.borrow_and_update() {
                if w.done_rx.changed().await.is_err() {
                    break;
                }
            }
        }
        gen
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

    pub fn broadcast_full<R: Runtime>(&self, app: Option<&AppHandle<R>>) {
        let Some(app) = app else {
            return;
        };
        let data = self.get_task_list();
        let _ = app.emit(
            "download:sync",
            json!({
                "type": "full",
                "data": data,
            }),
        );
    }

    pub fn emit_task_update_if_gen<R: Runtime>(
        self: &Arc<Self>,
        app: Option<&AppHandle<R>>,
        task: MediaDownloadTask,
        generation: u64,
        force_now: bool,
    ) {
        if !self.is_current_generation(&task.id, generation) {
            return;
        }
        self.emit_task_update(app, task, force_now);
    }

    pub fn emit_task_update<R: Runtime>(
        self: &Arc<Self>,
        app: Option<&AppHandle<R>>,
        task: MediaDownloadTask,
        force_now: bool,
    ) {
        let Some(app) = app else {
            return;
        };
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

    fn resolve_save_dir<R: Runtime>(&self, app: Option<&AppHandle<R>>, bili: &BiliState) -> String {
        let configured = app
            .and_then(|a| read_store_object(a, "app-settings").ok())
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
        self.add_task_opt(Some(app), bili, info);
    }

    pub fn add_task_opt<R: Runtime>(
        self: &Arc<Self>,
        app: Option<&AppHandle<R>>,
        bili: Arc<BiliState>,
        info: MediaDownloadInfo,
    ) -> String {
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
        let gen = self.next_generation(&id);
        self.tasks.write().unwrap().push(task_arc.clone());
        self.save_to_disk();
        self.broadcast_full(app);
        self.spawn_worker_with_gen(app.cloned(), bili, task_arc, false, gen);
        id
    }

    pub fn spawn_worker_with_gen<R: Runtime>(
        self: &Arc<Self>,
        app: Option<AppHandle<R>>,
        bili: Arc<BiliState>,
        task_arc: Arc<Mutex<FullMediaDownloadTask>>,
        is_resume: bool,
        generation: u64,
    ) {
        let id = task_arc.lock().unwrap().id.clone();
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_notify = Arc::new(tokio::sync::Notify::new());
        let (done_tx, done_rx) = tokio::sync::watch::channel(false);

        if let Some(prev) = self.workers.lock().unwrap().insert(
            id.clone(),
            WorkerControl {
                stop_flag: stop_flag.clone(),
                stop_notify: stop_notify.clone(),
                done_rx,
            },
        ) {
            prev.stop_flag.store(true, Ordering::SeqCst);
            prev.stop_notify.notify_waiters();
        }

        let mgr = self.clone();
        let id_clone = id.clone();
        tauri::async_runtime::spawn(async move {
            let permit = tokio::select! {
                p = mgr.semaphore.clone().acquire_owned() => p,
                _ = stop_notify.notified() => {
                    let _ = done_tx.send(true);
                    return;
                }
            };
            let Ok(_permit) = permit else {
                let _ = done_tx.send(true);
                return;
            };
            if stop_flag.load(Ordering::SeqCst) || !mgr.is_current_generation(&id_clone, generation)
            {
                let _ = done_tx.send(true);
                return;
            }
            mgr.run_task_pipeline(
                app.as_ref(),
                &bili,
                &task_arc,
                &stop_flag,
                &stop_notify,
                is_resume,
                generation,
            )
            .await;
            if mgr.is_current_generation(&id_clone, generation) {
                mgr.save_to_disk();
            }
            let _ = done_tx.send(true);
        });
    }

    #[allow(clippy::too_many_arguments)]
    async fn run_task_pipeline<R: Runtime>(
        self: &Arc<Self>,
        app: Option<&AppHandle<R>>,
        bili: &Arc<BiliState>,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
        stop_flag: &Arc<AtomicBool>,
        stop_notify: &Arc<tokio::sync::Notify>,
        is_resume: bool,
        generation: u64,
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

        if !self.is_current_generation(&task_id, generation) || stop_flag.load(Ordering::SeqCst) {
            return;
        }

        if let Some(bvid) = bvid_opt.filter(|b| !b.is_empty() && !has_cid) {
            let view_fut = get_web_interface_view(bili, &bvid);
            let view_res = tokio::select! {
                r = view_fut => r,
                _ = stop_notify.notified() => return,
            };
            if stop_flag.load(Ordering::SeqCst) || !self.is_current_generation(&task_id, generation)
            {
                return;
            }
            match view_res {
                Ok(view_res) => {
                    let pages = view_res
                        .get("data")
                        .and_then(|d| d.get("pages"))
                        .and_then(|p| p.as_array())
                        .cloned()
                        .unwrap_or_default();
                    if pages.is_empty() {
                        self.fail_task(
                            app,
                            task_arc,
                            generation,
                            "无法获取视频分集信息".to_string(),
                        );
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
                            self.add_task_opt(
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
                        self.remove_task_self(app, &task_id, generation).await;
                        return;
                    }
                }
                Err(e) => {
                    self.fail_task(app, task_arc, generation, e);
                    return;
                }
            }
        }

        if stop_flag.load(Ordering::SeqCst) || !self.is_current_generation(&task_id, generation) {
            return;
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
                if !self.is_current_generation(&t.id, generation)
                    || stop_flag.load(Ordering::SeqCst)
                {
                    return;
                }
                t.status = "downloading".to_string();
                let pub_t = t.to_public_task();
                drop(t);
                self.emit_task_update_if_gen(app, pub_t, generation, true);
            }

            if let Err(err) = self
                .stage_download(app, bili, task_arc, stop_flag, stop_notify, generation)
                .await
            {
                if !stop_flag.load(Ordering::SeqCst)
                    && self.is_current_generation(&task_id, generation)
                {
                    self.fail_task(app, task_arc, generation, err);
                }
                return;
            }
            if stop_flag.load(Ordering::SeqCst) || !self.is_current_generation(&task_id, generation)
            {
                return;
            }
        }

        if start_from_stage == "download" || start_from_stage == "merge" {
            if let Err(err) = self.stage_merge(app, task_arc, stop_flag, generation).await {
                if !stop_flag.load(Ordering::SeqCst)
                    && self.is_current_generation(&task_id, generation)
                {
                    self.fail_task(app, task_arc, generation, err);
                }
                return;
            }
            if stop_flag.load(Ordering::SeqCst) || !self.is_current_generation(&task_id, generation)
            {
                return;
            }
        }

        if let Err(err) = self
            .stage_convert(app, bili, task_arc, stop_flag, generation)
            .await
        {
            if !stop_flag.load(Ordering::SeqCst) && self.is_current_generation(&task_id, generation)
            {
                self.fail_task(app, task_arc, generation, err);
            }
            return;
        }
        if stop_flag.load(Ordering::SeqCst) || !self.is_current_generation(&task_id, generation) {
            return;
        }

        let pub_t = {
            let mut t = task_arc.lock().unwrap();
            if !self.is_current_generation(&t.id, generation) || stop_flag.load(Ordering::SeqCst) {
                return;
            }
            t.status = "completed".to_string();
            t.convert_progress = 100;
            t.to_public_task()
        };
        self.emit_task_update_if_gen(app, pub_t, generation, true);
    }

    async fn stage_download<R: Runtime>(
        self: &Arc<Self>,
        app: Option<&AppHandle<R>>,
        bili: &Arc<BiliState>,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
        stop_flag: &Arc<AtomicBool>,
        stop_notify: &Arc<tokio::sync::Notify>,
        generation: u64,
    ) -> Result<(), String> {
        // 1. Check if URLs need refreshing
        let (need_urls, task_id) = {
            let t = task_arc.lock().unwrap();
            let need = if t.output_file_type == "audio" {
                !is_url_valid(t.audio_url.as_deref())
            } else {
                !is_url_valid(t.video_url.as_deref())
                    || (t.audio_codecs.is_some() && !is_url_valid(t.audio_url.as_deref()))
            };
            (need, t.id.clone())
        };

        if need_urls {
            tokio::select! {
                res = self.fetch_download_urls(app, bili, task_arc) => res?,
                _ = stop_notify.notified() => return Ok(()),
            }
        }
        if stop_flag.load(Ordering::SeqCst) || !self.is_current_generation(&task_id, generation) {
            return Ok(());
        }

        // 2. Fetch Content-Length for audio and/or video
        let (output_file_type, audio_url, video_url, id, temp_dir_str) = {
            let t = task_arc.lock().unwrap();
            (
                t.output_file_type.clone(),
                t.audio_url.clone(),
                t.video_url.clone(),
                t.id.clone(),
                t.temp_dir.clone(),
            )
        };

        let mut total_bytes = 0u64;
        let mut audio_total_bytes = 0u64;
        let mut video_total_bytes = 0u64;

        if let Some(a_url) = audio_url.as_deref().filter(|s| !s.is_empty()) {
            let len = tokio::select! {
                l = self.get_content_length(bili, a_url) => l,
                _ = stop_notify.notified() => return Ok(()),
            };
            if stop_flag.load(Ordering::SeqCst) || !self.is_current_generation(&id, generation) {
                return Ok(());
            }
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
            let len = tokio::select! {
                l = self.get_content_length(bili, v_url) => l,
                _ = stop_notify.notified() => return Ok(()),
            };
            if stop_flag.load(Ordering::SeqCst) || !self.is_current_generation(&id, generation) {
                return Ok(());
            }
            if len == 0 {
                return Err("can't get video file size".to_string());
            }
            video_total_bytes = len;
            total_bytes += len;
        }

        let temp_dir_path = if temp_dir_str.trim().is_empty() {
            std::env::temp_dir().join("biu-temp-downloader").join(&id)
        } else {
            PathBuf::from(&temp_dir_str)
        };
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
            if !self.is_current_generation(&t.id, generation) || stop_flag.load(Ordering::SeqCst) {
                return Ok(());
            }
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
        let chunk_abort_flag = Arc::new(AtomicBool::new(false));
        let chunk_abort_notify = Arc::new(tokio::sync::Notify::new());
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
                self.emit_task_update_if_gen(app, pub_t, generation, false);
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
            let app_clone = app.cloned();
            let mgr_clone = self.clone();
            let task_clone = task_arc.clone();
            let stop_clone = stop_flag.clone();
            let stop_notify_clone = stop_notify.clone();
            let abort_clone = chunk_abort_flag.clone();
            let abort_notify_clone = chunk_abort_notify.clone();
            let chunk_clone = chunk.clone();

            let handle = tauri::async_runtime::spawn(async move {
                let permit = tokio::select! {
                    p = sem.acquire_owned() => p,
                    _ = stop_notify_clone.notified() => return Ok(()),
                    _ = abort_notify_clone.notified() => return Ok(()),
                };
                let Ok(_permit) = permit else {
                    return Err("Semaphore closed".to_string());
                };
                if stop_clone.load(Ordering::SeqCst) || abort_clone.load(Ordering::SeqCst) {
                    return Ok(());
                }
                let res = mgr_clone
                    .download_single_chunk(
                        app_clone.as_ref(),
                        &bili_clone,
                        &task_clone,
                        &stop_clone,
                        &stop_notify_clone,
                        &abort_clone,
                        &abort_notify_clone,
                        generation,
                        idx,
                        &chunk_clone,
                        &url,
                        &chunk_path,
                        current_size,
                    )
                    .await;
                if res.is_err() {
                    abort_clone.store(true, Ordering::SeqCst);
                    abort_notify_clone.notify_waiters();
                }
                res
            });
            join_handles.push(handle);
        }

        let mut first_err: Option<String> = None;
        for h in join_handles {
            match h.await {
                Ok(Ok(())) => {}
                Ok(Err(e)) => {
                    chunk_abort_flag.store(true, Ordering::SeqCst);
                    chunk_abort_notify.notify_waiters();
                    if first_err.is_none() {
                        first_err = Some(e);
                    }
                }
                Err(e) => {
                    chunk_abort_flag.store(true, Ordering::SeqCst);
                    chunk_abort_notify.notify_waiters();
                    if first_err.is_none() {
                        first_err = Some(e.to_string());
                    }
                }
            }
        }

        if stop_flag.load(Ordering::SeqCst) || !self.is_current_generation(&id, generation) {
            return Ok(());
        }
        if let Some(err) = first_err {
            return Err(err);
        }

        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn download_single_chunk<R: Runtime>(
        self: &Arc<Self>,
        app: Option<&AppHandle<R>>,
        bili: &BiliState,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
        stop_flag: &Arc<AtomicBool>,
        stop_notify: &Arc<tokio::sync::Notify>,
        chunk_abort_flag: &Arc<AtomicBool>,
        chunk_abort_notify: &Arc<tokio::sync::Notify>,
        generation: u64,
        chunk_idx: usize,
        chunk: &MediaDownloadChunk,
        url: &str,
        dest_path: &Path,
        initial_counted_offset: u64,
    ) -> Result<(), String> {
        let expected_chunk_len = chunk.end - chunk.start + 1;
        let mut accounted_bytes = initial_counted_offset;
        let mut last_err = String::new();
        let idle_timeout = *self.chunk_idle_timeout.read().unwrap();

        for attempt in 0..3 {
            if stop_flag.load(Ordering::SeqCst) || chunk_abort_flag.load(Ordering::SeqCst) {
                return Ok(());
            }

            let mut actual_offset = fs::metadata(dest_path).map(|m| m.len()).unwrap_or(0);
            if actual_offset > expected_chunk_len {
                let _ = fs::remove_file(dest_path);
                actual_offset = 0;
            }

            // Reconcile task.downloaded_bytes with actual_offset on disk before starting this attempt
            if actual_offset != accounted_bytes {
                let mut t = task_arc.lock().unwrap();
                if actual_offset > accounted_bytes {
                    t.downloaded_bytes += actual_offset - accounted_bytes;
                } else {
                    t.downloaded_bytes = t
                        .downloaded_bytes
                        .saturating_sub(accounted_bytes - actual_offset);
                }
                accounted_bytes = actual_offset;
            }

            if actual_offset == expected_chunk_len {
                let mut t = task_arc.lock().unwrap();
                if let Some(c) = t.chunks.get_mut(chunk_idx) {
                    c.done = true;
                }
                return Ok(());
            }

            let req_start = chunk.start + actual_offset;
            let range_header = format!("bytes={req_start}-{}", chunk.end);

            let send_fut = bili
                .client()
                .get(url)
                .header(reqwest::header::ORIGIN, BILI_ORIGIN)
                .header(reqwest::header::REFERER, BILI_ORIGIN)
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .header(reqwest::header::RANGE, &range_header)
                .send();

            let resp_res = tokio::select! {
                r = send_fut => r,
                _ = stop_notify.notified() => return Ok(()),
                _ = chunk_abort_notify.notified() => return Ok(()),
                _ = tokio::time::sleep(idle_timeout) => {
                    last_err = "request timed out".to_string();
                    continue;
                }
            };

            if stop_flag.load(Ordering::SeqCst) || chunk_abort_flag.load(Ordering::SeqCst) {
                return Ok(());
            }

            let mut resp = match resp_res {
                Ok(r) => r,
                Err(e) => {
                    last_err = e.to_string();
                    if attempt < 2 {
                        tokio::select! {
                            _ = tokio::time::sleep(Duration::from_millis(300 * (attempt + 1))) => {}
                            _ = stop_notify.notified() => return Ok(()),
                            _ = chunk_abort_notify.notified() => return Ok(()),
                        }
                    }
                    continue;
                }
            };

            let status_code = resp.status().as_u16();
            if status_code != 206 {
                if status_code == 200 {
                    // Server ignored Range header; do not append full resource onto chunk
                    last_err = "server returned 200 OK instead of 206 Partial Content".to_string();
                } else {
                    last_err = format!("HTTP {}", resp.status());
                }
                if attempt < 2 {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(300 * (attempt + 1))) => {}
                        _ = stop_notify.notified() => return Ok(()),
                        _ = chunk_abort_notify.notified() => return Ok(()),
                    }
                }
                continue;
            }

            // Validate Content-Range start matches req_start
            let cr_valid = resp
                .headers()
                .get(reqwest::header::CONTENT_RANGE)
                .and_then(|v| v.to_str().ok())
                .and_then(parse_content_range_start)
                .is_some_and(|(cr_start, cr_end)| cr_start == req_start && cr_end == chunk.end);

            if !cr_valid {
                let cr_raw = resp
                    .headers()
                    .get(reqwest::header::CONTENT_RANGE)
                    .and_then(|v| v.to_str().ok())
                    .unwrap_or("<missing>");
                last_err = format!(
                    "invalid Content-Range {cr_raw} (expected bytes {req_start}-{})",
                    chunk.end
                );
                if attempt < 2 {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(300 * (attempt + 1))) => {}
                        _ = stop_notify.notified() => return Ok(()),
                        _ = chunk_abort_notify.notified() => return Ok(()),
                    }
                }
                continue;
            }

            let mut file_opts = tokio::fs::OpenOptions::new();
            file_opts.create(true).write(true);
            if actual_offset > 0 {
                file_opts.append(true);
            } else {
                file_opts.truncate(true);
            }
            let mut file = file_opts.open(dest_path).await.map_err(|e| e.to_string())?;

            let mut stream_err = None;
            let remaining_expected = expected_chunk_len - actual_offset;
            let mut bytes_in_attempt = 0u64;

            loop {
                if stop_flag.load(Ordering::SeqCst) || chunk_abort_flag.load(Ordering::SeqCst) {
                    let _ = file.flush().await;
                    return Ok(());
                }

                let next_chunk = tokio::select! {
                    c = resp.chunk() => c,
                    _ = stop_notify.notified() => {
                        let _ = file.flush().await;
                        return Ok(());
                    }
                    _ = chunk_abort_notify.notified() => {
                        let _ = file.flush().await;
                        return Ok(());
                    }
                    _ = tokio::time::sleep(idle_timeout) => {
                        stream_err = Some("chunk read idle timeout".to_string());
                        break;
                    }
                };

                match next_chunk {
                    Ok(Some(bytes)) => {
                        let len = bytes.len() as u64;
                        if bytes_in_attempt + len > remaining_expected {
                            stream_err = Some(format!(
                                "chunk {} exceeded expected size ({} > {})",
                                chunk.name,
                                actual_offset + bytes_in_attempt + len,
                                expected_chunk_len
                            ));
                            break;
                        }
                        if let Err(e) = file.write_all(&bytes).await {
                            stream_err = Some(e.to_string());
                            break;
                        }
                        bytes_in_attempt += len;
                        accounted_bytes += len;
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
                        self.emit_task_update_if_gen(app, pub_t, generation, false);
                    }
                    Ok(None) => break,
                    Err(e) => {
                        stream_err = Some(e.to_string());
                        break;
                    }
                }
            }
            let _ = file.flush().await;
            drop(file);

            if let Some(e) = stream_err {
                last_err = e;
                if attempt < 2 {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(200 * (attempt + 1))) => {}
                        _ = stop_notify.notified() => return Ok(()),
                        _ = chunk_abort_notify.notified() => return Ok(()),
                    }
                }
                continue;
            }

            let final_len = fs::metadata(dest_path).map(|m| m.len()).unwrap_or(0);
            if final_len != expected_chunk_len {
                if final_len > expected_chunk_len {
                    let _ = fs::remove_file(dest_path);
                }
                last_err = format!(
                    "chunk {} incomplete: got {final_len} bytes, expected {expected_chunk_len}",
                    chunk.name
                );
                if attempt < 2 {
                    tokio::select! {
                        _ = tokio::time::sleep(Duration::from_millis(200 * (attempt + 1))) => {}
                        _ = stop_notify.notified() => return Ok(()),
                        _ = chunk_abort_notify.notified() => return Ok(()),
                    }
                }
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
        app: Option<&AppHandle<R>>,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
        stop_flag: &Arc<AtomicBool>,
        generation: u64,
    ) -> Result<(), String> {
        let (temp_dir, audio_temp, video_temp, chunks, total_bytes, task_id) = {
            let mut t = task_arc.lock().unwrap();
            if !self.is_current_generation(&t.id, generation) || stop_flag.load(Ordering::SeqCst) {
                return Ok(());
            }
            t.status = "merging".to_string();
            (
                PathBuf::from(&t.temp_dir),
                t.audio_temp_path.clone().map(PathBuf::from),
                t.video_temp_path.clone().map(PathBuf::from),
                t.chunks.clone(),
                t.total_bytes,
                t.id.clone(),
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
            self.emit_task_update_if_gen(app, pub_t, generation, true);
        }

        if let Some(ref ap) = audio_temp {
            self.merge_chunks_by_type(
                app,
                task_arc,
                stop_flag,
                generation,
                "audio",
                &temp_dir,
                ap,
                &chunks,
                &mut merged_bytes,
                total_bytes,
            )?;
        }
        if stop_flag.load(Ordering::SeqCst) || !self.is_current_generation(&task_id, generation) {
            return Ok(());
        }
        if let Some(ref vp) = video_temp {
            self.merge_chunks_by_type(
                app,
                task_arc,
                stop_flag,
                generation,
                "video",
                &temp_dir,
                vp,
                &chunks,
                &mut merged_bytes,
                total_bytes,
            )?;
        }
        if stop_flag.load(Ordering::SeqCst) || !self.is_current_generation(&task_id, generation) {
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
        app: Option<&AppHandle<R>>,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
        stop_flag: &Arc<AtomicBool>,
        generation: u64,
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

        if start_index >= chunks.len() || stop_flag.load(Ordering::SeqCst) {
            return Ok(());
        }

        let mut out_file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(dest_path)
            .map_err(|e| e.to_string())?;

        let mut buf = vec![0u8; 64 * 1024];
        for c in chunks.iter().skip(start_index) {
            if stop_flag.load(Ordering::SeqCst) {
                return Ok(());
            }
            let chunk_path = temp_dir.join(&c.name);
            let mut in_file = fs::File::open(&chunk_path).map_err(|e| e.to_string())?;
            loop {
                if stop_flag.load(Ordering::SeqCst) {
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
                self.emit_task_update_if_gen(app, pub_t, generation, false);
            }
        }
        out_file.flush().map_err(|e| e.to_string())?;
        Ok(())
    }

    async fn stage_convert<R: Runtime>(
        self: &Arc<Self>,
        app: Option<&AppHandle<R>>,
        bili: &BiliState,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
        stop_flag: &Arc<AtomicBool>,
        generation: u64,
    ) -> Result<(), String> {
        let (
            task_id,
            output_file_type,
            title,
            audio_codecs,
            mut save_dir,
            temp_dir,
            audio_temp,
            video_temp,
        ) = {
            let mut t = task_arc.lock().unwrap();
            if !self.is_current_generation(&t.id, generation) || stop_flag.load(Ordering::SeqCst) {
                return Ok(());
            }
            t.status = "converting".to_string();
            let pub_t = t.to_public_task();
            let res = (
                t.id.clone(),
                t.output_file_type.clone(),
                t.title.clone(),
                t.audio_codecs.clone().unwrap_or_default(),
                t.save_dir.clone(),
                PathBuf::from(&t.temp_dir),
                t.audio_temp_path.clone().map(PathBuf::from),
                t.video_temp_path.clone().map(PathBuf::from),
            );
            drop(t);
            self.emit_task_update_if_gen(app, pub_t, generation, true);
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

        let user_ffmpeg = app.and_then(|a| user_ffmpeg_setting(a));
        let ffmpeg_bin = resolve_ffmpeg_path(user_ffmpeg.as_deref())?;
        let cancel_clone = stop_flag.clone();
        let out_type_clone = output_file_type.clone();
        let a_temp_clone = audio_temp.clone();
        let v_temp_clone = video_temp.clone();
        let desired_clone = desired_save_path.clone();
        let task_id_clone = task_id.clone();

        let final_save_path = tokio::task::spawn_blocking(move || {
            convert_with_ffmpeg_for_task(
                &ffmpeg_bin,
                &out_type_clone,
                a_temp_clone.as_deref(),
                v_temp_clone.as_deref(),
                &desired_clone,
                &task_id_clone,
                &cancel_clone,
                FFMPEG_CONVERT_TIMEOUT,
            )
        })
        .await
        .map_err(|e| e.to_string())??;

        if stop_flag.load(Ordering::SeqCst) || !self.is_current_generation(&task_id, generation) {
            let _ = fs::remove_file(&final_save_path);
            return Ok(());
        }

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
        self.emit_task_update_if_gen(app, pub_t, generation, true);

        let _ = fs::remove_dir_all(&temp_dir);
        Ok(())
    }

    async fn fetch_download_urls<R: Runtime>(
        &self,
        app: Option<&AppHandle<R>>,
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
            let user_store = app
                .and_then(|a| read_store_object(a, "user-login-info").ok())
                .unwrap_or_else(|| json!({}));
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
        app: Option<&AppHandle<R>>,
        task_arc: &Arc<Mutex<FullMediaDownloadTask>>,
        generation: u64,
        error: String,
    ) {
        let pub_t = {
            let mut t = task_arc.lock().unwrap();
            if !self.is_current_generation(&t.id, generation) {
                return;
            }
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
        self.emit_task_update_if_gen(app, pub_t, generation, true);
    }

    pub async fn pause_task<R: Runtime>(self: &Arc<Self>, app: &AppHandle<R>, id: &str) {
        self.pause_task_opt(Some(app), id).await;
    }

    pub async fn pause_task_opt<R: Runtime>(
        self: &Arc<Self>,
        app: Option<&AppHandle<R>>,
        id: &str,
    ) {
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
            self.stop_and_wait_worker(id).await;
            self.save_to_disk();
            self.emit_task_update(app, pub_t, true);
        } else {
            self.stop_and_wait_worker(id).await;
        }
    }

    pub async fn resume_task<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        bili: Arc<BiliState>,
        id: &str,
    ) {
        self.resume_task_opt(Some(app), bili, id).await;
    }

    pub async fn resume_task_opt<R: Runtime>(
        self: &Arc<Self>,
        app: Option<&AppHandle<R>>,
        bili: Arc<BiliState>,
        id: &str,
    ) {
        if let Some(task_arc) = self.find_task(id) {
            let gen = self.stop_and_wait_worker(id).await;
            self.spawn_worker_with_gen(app.cloned(), bili, task_arc, true, gen);
        }
    }

    pub async fn retry_task<R: Runtime>(
        self: &Arc<Self>,
        app: &AppHandle<R>,
        bili: Arc<BiliState>,
        id: &str,
    ) {
        self.retry_task_opt(Some(app), bili, id).await;
    }

    pub async fn retry_task_opt<R: Runtime>(
        self: &Arc<Self>,
        app: Option<&AppHandle<R>>,
        bili: Arc<BiliState>,
        id: &str,
    ) {
        if let Some(task_arc) = self.find_task(id) {
            let gen = self.stop_and_wait_worker(id).await;
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
            self.spawn_worker_with_gen(app.cloned(), bili, task_arc, false, gen);
        }
    }

    async fn remove_task_self<R: Runtime>(
        self: &Arc<Self>,
        app: Option<&AppHandle<R>>,
        id: &str,
        generation: u64,
    ) {
        if !self.is_current_generation(id, generation) {
            return;
        }
        self.pending_updates.lock().unwrap().remove(id);
        self.workers.lock().unwrap().remove(id);
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

    pub async fn cancel_task<R: Runtime>(self: &Arc<Self>, app: &AppHandle<R>, id: &str) {
        self.cancel_task_opt(Some(app), id).await;
    }

    pub async fn cancel_task_opt<R: Runtime>(
        self: &Arc<Self>,
        app: Option<&AppHandle<R>>,
        id: &str,
    ) {
        self.pending_updates.lock().unwrap().remove(id);
        let removed = {
            let mut lock = self.tasks.write().unwrap();
            lock.iter()
                .position(|t| t.lock().unwrap().id == id)
                .map(|pos| lock.remove(pos))
        };
        self.stop_and_wait_worker(id).await;
        if let Some(task_arc) = removed {
            let (temp_dir, save_dir, title) = {
                let t = task_arc.lock().unwrap();
                (t.temp_dir.clone(), t.save_dir.clone(), t.title.clone())
            };
            if !temp_dir.is_empty() {
                let _ = tokio::fs::remove_dir_all(&temp_dir).await;
            }
            self.cleanup_task_temp_outputs(&save_dir, &title, id);
        }
        self.save_to_disk();
        self.broadcast_full(app);
    }

    fn cleanup_task_temp_outputs(&self, save_dir: &str, title: &str, id: &str) {
        if save_dir.trim().is_empty() {
            return;
        }
        let sanitized = sanitize_filename(title);
        let base_name = if sanitized.is_empty() {
            "download".to_string()
        } else {
            sanitized
        };
        for ext in [".m4a", ".mp3", ".flac", ".mp4", ".mkv"] {
            let desired = Path::new(save_dir).join(format!("{base_name}{ext}"));
            let temp_out = build_task_temp_output_path(&desired, id);
            let _ = fs::remove_file(temp_out);
        }
    }

    pub async fn clear_tasks<R: Runtime>(self: &Arc<Self>, app: &AppHandle<R>) {
        self.clear_tasks_opt(Some(app)).await;
    }

    pub async fn clear_tasks_opt<R: Runtime>(self: &Arc<Self>, app: Option<&AppHandle<R>>) {
        self.pending_updates.lock().unwrap().clear();
        let all_tasks: Vec<Arc<Mutex<FullMediaDownloadTask>>> = {
            let mut lock = self.tasks.write().unwrap();
            lock.drain(..).collect()
        };
        let ids: Vec<String> = all_tasks
            .iter()
            .map(|t| t.lock().unwrap().id.clone())
            .collect();
        for id in &ids {
            self.stop_and_wait_worker(id).await;
        }
        for task_arc in all_tasks {
            let (id, temp_dir, save_dir, title) = {
                let t = task_arc.lock().unwrap();
                (
                    t.id.clone(),
                    t.temp_dir.clone(),
                    t.save_dir.clone(),
                    t.title.clone(),
                )
            };
            if !temp_dir.is_empty() {
                let _ = tokio::fs::remove_dir_all(&temp_dir).await;
            }
            self.cleanup_task_temp_outputs(&save_dir, &title, &id);
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
    mgr.pause_task(&app, &id).await;
    Ok(())
}

#[tauri::command]
pub async fn download_resume<R: Runtime>(
    app: AppHandle<R>,
    mgr: tauri::State<'_, Arc<DownloadManager>>,
    bili: tauri::State<'_, Arc<BiliState>>,
    id: String,
) -> Result<(), String> {
    mgr.resume_task(&app, bili.inner().clone(), &id).await;
    Ok(())
}

#[tauri::command]
pub async fn download_retry<R: Runtime>(
    app: AppHandle<R>,
    mgr: tauri::State<'_, Arc<DownloadManager>>,
    bili: tauri::State<'_, Arc<BiliState>>,
    id: String,
) -> Result<(), String> {
    mgr.retry_task(&app, bili.inner().clone(), &id).await;
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::AtomicUsize;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn parse_range_header(req: &str) -> Option<(u64, u64)> {
        for line in req.lines() {
            let lower = line.to_ascii_lowercase();
            if let Some(rest) = lower.strip_prefix("range:") {
                let val = rest.trim().strip_prefix("bytes=")?;
                let (s, e) = val.split_once('-')?;
                return Some((s.trim().parse().ok()?, e.trim().parse().ok()?));
            }
        }
        None
    }

    #[test]
    fn sanitize_filename_utf8_byte_truncation_and_windows_reserved_names() {
        // Windows reserved names (case-insensitive, with or without extension / trailing dots/spaces)
        assert_eq!(sanitize_filename("CON"), "_CON");
        assert_eq!(sanitize_filename("con.mp3"), "_con.mp3");
        assert_eq!(sanitize_filename("Prn . .. "), "_Prn");
        assert_eq!(sanitize_filename("aux"), "_aux");
        assert_eq!(sanitize_filename("NUL"), "_NUL");
        assert_eq!(sanitize_filename("COM1"), "_COM1");
        assert_eq!(sanitize_filename("com9.txt"), "_com9.txt");
        assert_eq!(sanitize_filename("LPT1"), "_LPT1");
        assert_eq!(sanitize_filename("lpt9..."), "_lpt9");
        // Non-reserved prefixes
        assert_eq!(sanitize_filename("CONSOLE"), "CONSOLE");
        assert_eq!(sanitize_filename("COM10"), "COM10");

        // UTF-8 byte truncation (<= 200 bytes without splitting multi-byte characters)
        // Each Chinese character '歌' is 3 bytes in UTF-8. 100 chars = 300 bytes.
        let long_cjk = "歌".repeat(100);
        let sanitized = sanitize_filename(&long_cjk);
        assert!(sanitized.len() <= MAX_FILENAME_STEM_BYTES);
        // 200 / 3 = 66 chars = 198 bytes
        assert_eq!(sanitized.len(), 198);
        assert_eq!(sanitized.chars().count(), 66);

        // Trailing dots and spaces after truncation are stripped
        let prefix = "a".repeat(198);
        let with_dot = format!("{prefix}.b");
        let sanitized_dot = sanitize_filename(&with_dot);
        assert!(!sanitized_dot.ends_with('.'));
        assert!(!sanitized_dot.ends_with(' '));
    }

    #[tokio::test]
    async fn chunk_resume_recalculates_range_after_midstream_disconnect() {
        let dir = std::env::temp_dir().join(format!("biu-dl-resume-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let payload: Vec<u8> = (0..1000u32).map(|i| (i % 251) as u8).collect();
        let payload_srv = payload.clone();
        let seen_ranges: Arc<Mutex<Vec<(u64, u64)>>> = Arc::new(Mutex::new(Vec::new()));
        let seen_srv = seen_ranges.clone();
        let call_count = Arc::new(AtomicUsize::new(0));
        let count_srv = call_count.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let srv = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = vec![0u8; 2048];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    continue;
                }
                let req = String::from_utf8_lossy(&buf[..n]);
                let (start, end) = parse_range_header(&req).expect("Range header missing");
                seen_srv.lock().unwrap().push((start, end));
                let idx = count_srv.fetch_add(1, Ordering::SeqCst);
                let total = payload_srv.len();

                if idx == 0 {
                    // First attempt: claim 206 for full range, but only send 400 bytes then drop connection
                    let slice = &payload_srv[start as usize..(start as usize + 400)];
                    let hdr = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{total}\r\nConnection: close\r\n\r\n",
                        end - start + 1
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.write_all(slice).await;
                    let _ = sock.shutdown().await;
                } else {
                    // Retry attempt: send remaining range accurately
                    let slice = &payload_srv[start as usize..=end as usize];
                    let hdr = format!(
                        "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{total}\r\nConnection: close\r\n\r\n",
                        slice.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.write_all(slice).await;
                    let _ = sock.shutdown().await;
                    break;
                }
            }
        });

        let bili = Arc::new(BiliState::new(dir.join("cookies.json"), None).unwrap());
        let mgr = DownloadManager::new(dir.join("downloads.json"));

        // Pre-populate 100 bytes on disk to also verify initial offset + mid-stream disconnect retry
        let chunk_path = dir.join("audio.part1");
        fs::write(&chunk_path, &payload[..100]).unwrap();

        let task = FullMediaDownloadTask {
            id: "t-resume".into(),
            output_file_type: "audio".into(),
            title: "Resume Test".into(),
            cover: None,
            created_time: Some(1),
            bvid: None,
            cid: None,
            sid: None,
            audio_url: None,
            audio_codecs: None,
            audio_bandwidth: None,
            video_url: None,
            video_resolution: None,
            video_frame_rate: None,
            total_bytes: 1000,
            audio_total_bytes: 1000,
            video_total_bytes: 0,
            downloaded_bytes: 100,
            download_progress: 10,
            merge_progress: 0,
            convert_progress: 0,
            status: "downloading".into(),
            file_name: None,
            temp_dir: dir.to_string_lossy().into_owned(),
            save_dir: dir.to_string_lossy().into_owned(),
            save_path: None,
            audio_temp_path: None,
            video_temp_path: None,
            chunks: vec![MediaDownloadChunk {
                chunk_type: "audio".into(),
                name: "audio.part1".into(),
                start: 0,
                end: 999,
                done: false,
            }],
            error: None,
        };
        let task_arc = Arc::new(Mutex::new(task));
        let gen = mgr.next_generation("t-resume");
        let stop_flag = Arc::new(AtomicBool::new(false));
        let stop_notify = Arc::new(tokio::sync::Notify::new());
        let abort_flag = Arc::new(AtomicBool::new(false));
        let abort_notify = Arc::new(tokio::sync::Notify::new());
        let chunk = task_arc.lock().unwrap().chunks[0].clone();
        let url = format!("http://{addr}/audio.m4s");

        mgr.download_single_chunk::<tauri::Wry>(
            None,
            &bili,
            &task_arc,
            &stop_flag,
            &stop_notify,
            &abort_flag,
            &abort_notify,
            gen,
            0,
            &chunk,
            &url,
            &chunk_path,
            100,
        )
        .await
        .unwrap();

        let _ = srv.await;
        let ranges = seen_ranges.lock().unwrap().clone();
        // Attempt 1 started at 100; wrote 400 bytes -> Attempt 2 recalculated Range from 500..=999
        assert_eq!(ranges, vec![(100, 999), (500, 999)]);
        let final_bytes = fs::read(&chunk_path).unwrap();
        assert_eq!(final_bytes.len(), 1000);
        assert_eq!(final_bytes, payload);
        let t = task_arc.lock().unwrap();
        assert_eq!(t.downloaded_bytes, 1000);
        assert!(t.chunks[0].done);

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn chunk_rejects_200_ignoring_range_and_marks_task_failed_not_cancelled() {
        let dir = std::env::temp_dir().join(format!("biu-dl-200-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        // Server responds to HEAD with Content-Length: 500, but responds to GET Range with 200 OK (ignoring Range)
        let srv = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let mut buf = vec![0u8; 2048];
                let n = sock.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    continue;
                }
                let req = String::from_utf8_lossy(&buf[..n]);
                if req.starts_with("HEAD ") {
                    let hdr = "HTTP/1.1 200 OK\r\nContent-Length: 500\r\nConnection: close\r\n\r\n";
                    let _ = sock.write_all(hdr.as_bytes()).await;
                } else {
                    let body = vec![b'A'; 500];
                    let hdr = format!(
                        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    let _ = sock.write_all(&body).await;
                }
                let _ = sock.shutdown().await;
            }
        });

        let bili = Arc::new(BiliState::new(dir.join("cookies.json"), None).unwrap());
        let mgr = DownloadManager::new(dir.join("downloads.json"));

        let url = format!("http://{addr}/audio.m4s?deadline=4102444800");
        let task = FullMediaDownloadTask {
            id: "t-200-fail".into(),
            output_file_type: "audio".into(),
            title: "200 Ignore Range".into(),
            cover: None,
            created_time: Some(1),
            bvid: None,
            cid: None,
            sid: None,
            audio_url: Some(url),
            audio_codecs: Some("aac".into()),
            audio_bandwidth: Some(128000),
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
            status: "waiting".into(),
            file_name: None,
            temp_dir: dir.join("temp").to_string_lossy().into_owned(),
            save_dir: dir.join("save").to_string_lossy().into_owned(),
            save_path: None,
            audio_temp_path: None,
            video_temp_path: None,
            chunks: Vec::new(),
            error: None,
        };
        let task_arc = Arc::new(Mutex::new(task));
        let gen = mgr.next_generation("t-200-fail");
        mgr.tasks.write().unwrap().push(task_arc.clone());
        mgr.spawn_worker_with_gen::<tauri::Wry>(None, bili, task_arc.clone(), false, gen);

        // Wait for worker to finish
        for _ in 0..100 {
            let status = task_arc.lock().unwrap().status.clone();
            if status == "failed" {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }

        srv.abort();
        let t = task_arc.lock().unwrap();
        assert_eq!(
            t.status, "failed",
            "task should be marked failed, not stuck in downloading"
        );
        let err_msg = t.error.as_deref().unwrap_or("");
        assert!(
            err_msg.contains("200 OK"),
            "expected error mentioning 200 OK, got: {err_msg}"
        );

        let _ = fs::remove_dir_all(&dir);
    }

    #[tokio::test]
    async fn slow_response_idle_timeout_and_pause_immediate_resume_waits_for_old_worker() {
        let dir = std::env::temp_dir().join(format!("biu-dl-pause-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        let payload: Vec<u8> = (0..512u32).map(|i| (i % 251) as u8).collect();
        let payload_srv = payload.clone();
        let get_calls = Arc::new(AtomicUsize::new(0));
        let get_calls_srv = get_calls.clone();
        let first_get_started = Arc::new(tokio::sync::Notify::new());
        let first_get_srv = first_get_started.clone();

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();

        let srv = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = listener.accept().await else {
                    break;
                };
                let payload_conn = payload_srv.clone();
                let get_calls_conn = get_calls_srv.clone();
                let first_get_conn = first_get_srv.clone();
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 2048];
                    let n = sock.read(&mut buf).await.unwrap_or(0);
                    if n == 0 {
                        return;
                    }
                    let req = String::from_utf8_lossy(&buf[..n]);
                    if req.starts_with("HEAD ") {
                        let hdr = format!(
                            "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
                            payload_conn.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.shutdown().await;
                        return;
                    }
                    let (start, end) = parse_range_header(&req).unwrap_or((0, 511));
                    let call_idx = get_calls_conn.fetch_add(1, Ordering::SeqCst);
                    if call_idx == 0 {
                        // First worker: send headers and 100 bytes, then stall so pause + resume happens while stalled
                        let hdr = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n",
                            end - start + 1,
                            payload_conn.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock
                            .write_all(&payload_conn[start as usize..start as usize + 100])
                            .await;
                        first_get_conn.notify_waiters();
                        tokio::time::sleep(Duration::from_secs(30)).await;
                    } else {
                        // Second worker (after pause + immediate resume): serve requested range immediately
                        let slice = &payload_conn[start as usize..=end as usize];
                        let hdr = format!(
                            "HTTP/1.1 206 Partial Content\r\nContent-Length: {}\r\nContent-Range: bytes {start}-{end}/{}\r\nConnection: close\r\n\r\n",
                            slice.len(),
                            payload_conn.len()
                        );
                        let _ = sock.write_all(hdr.as_bytes()).await;
                        let _ = sock.write_all(slice).await;
                        let _ = sock.shutdown().await;
                    }
                });
            }
        });

        let bili = Arc::new(BiliState::new(dir.join("cookies.json"), None).unwrap());
        let mgr = DownloadManager::new(dir.join("downloads.json"));
        mgr.set_chunk_idle_timeout(Duration::from_millis(300));

        let url = format!("http://{addr}/audio.m4s?deadline=4102444800");
        let temp_dir = dir.join("temp");
        let save_dir = dir.join("save");
        let task = FullMediaDownloadTask {
            id: "t-pause-resume".into(),
            output_file_type: "audio".into(),
            title: "Pause Resume Song".into(),
            cover: None,
            created_time: Some(1),
            bvid: None,
            cid: None,
            sid: None,
            audio_url: Some(url),
            audio_codecs: Some("aac".into()),
            audio_bandwidth: Some(128000),
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
            status: "waiting".into(),
            file_name: None,
            temp_dir: temp_dir.to_string_lossy().into_owned(),
            save_dir: save_dir.to_string_lossy().into_owned(),
            save_path: None,
            audio_temp_path: None,
            video_temp_path: None,
            chunks: Vec::new(),
            error: None,
        };
        let task_arc = Arc::new(Mutex::new(task));
        let gen1 = mgr.next_generation("t-pause-resume");
        mgr.tasks.write().unwrap().push(task_arc.clone());
        mgr.spawn_worker_with_gen::<tauri::Wry>(None, bili.clone(), task_arc.clone(), false, gen1);

        // Wait until worker 1 has written the first 100 bytes and is stalled on network read
        first_get_started.notified().await;
        tokio::time::sleep(Duration::from_millis(30)).await;

        // Pause and immediately resume; pause_task_opt must wake and wait for worker 1 to exit
        mgr.pause_task_opt::<tauri::Wry>(None, "t-pause-resume")
            .await;
        assert_eq!(task_arc.lock().unwrap().status, "downloadPaused");
        assert!(!mgr.is_current_generation("t-pause-resume", gen1));

        // Stale update from gen1 must be rejected
        mgr.emit_task_update_if_gen::<tauri::Wry>(
            None,
            MediaDownloadTask {
                id: "t-pause-resume".into(),
                output_file_type: "audio".into(),
                title: "Stale".into(),
                bvid: None,
                cid: None,
                sid: None,
                cover: None,
                audio_codecs: None,
                audio_bandwidth: None,
                video_resolution: None,
                video_frame_rate: None,
                save_path: None,
                status: "downloading".into(),
                total_bytes: None,
                downloaded_bytes: None,
                download_progress: Some(99),
                merge_progress: None,
                convert_progress: None,
                created_time: None,
                error: None,
            },
            gen1,
            false,
        );
        assert!(mgr
            .pending_updates
            .lock()
            .unwrap()
            .get("t-pause-resume")
            .is_none());

        // Immediately resume; verify stage_download + stage_merge finish with exact payload
        let gen2 = mgr.stop_and_wait_worker("t-pause-resume").await;
        let stop_flag2 = Arc::new(AtomicBool::new(false));
        let stop_notify2 = Arc::new(tokio::sync::Notify::new());
        mgr.stage_download::<tauri::Wry>(None, &bili, &task_arc, &stop_flag2, &stop_notify2, gen2)
            .await
            .unwrap();
        mgr.stage_merge::<tauri::Wry>(None, &task_arc, &stop_flag2, gen2)
            .await
            .unwrap();

        let merged_audio = fs::read(temp_dir.join("audio.m4s")).unwrap();
        assert_eq!(merged_audio, payload);

        // Also test slow response idle timeout when server stalls forever
        let slow_listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let slow_addr = slow_listener.local_addr().unwrap();
        let slow_srv = tokio::spawn(async move {
            loop {
                let Ok((mut sock, _)) = slow_listener.accept().await else {
                    break;
                };
                tokio::spawn(async move {
                    let mut buf = vec![0u8; 1024];
                    let _ = sock.read(&mut buf).await;
                    let hdr = "HTTP/1.1 206 Partial Content\r\nContent-Length: 100\r\nContent-Range: bytes 0-99/100\r\n\r\n";
                    let _ = sock.write_all(hdr.as_bytes()).await;
                    tokio::time::sleep(Duration::from_secs(30)).await;
                });
            }
        });

        mgr.set_chunk_idle_timeout(Duration::from_millis(120));
        let slow_chunk = MediaDownloadChunk {
            chunk_type: "audio".into(),
            name: "slow.part1".into(),
            start: 0,
            end: 99,
            done: false,
        };
        let slow_err = mgr
            .download_single_chunk::<tauri::Wry>(
                None,
                &bili,
                &task_arc,
                &stop_flag2,
                &stop_notify2,
                &Arc::new(AtomicBool::new(false)),
                &Arc::new(tokio::sync::Notify::new()),
                gen2,
                0,
                &slow_chunk,
                &format!("http://{slow_addr}/slow.m4s"),
                &dir.join("slow.part1"),
                0,
            )
            .await
            .unwrap_err();
        assert!(
            slow_err.contains("idle timeout"),
            "expected idle timeout error, got: {slow_err}"
        );

        srv.abort();
        slow_srv.abort();
        let _ = fs::remove_dir_all(&dir);
    }

    #[cfg(unix)]
    #[test]
    fn ffmpeg_stderr_flood_and_concurrent_same_name_downloads_do_not_overwrite() {
        use std::os::unix::fs::PermissionsExt;

        let dir = std::env::temp_dir().join(format!("biu-dl-ffmpeg-{}", uuid::Uuid::new_v4()));
        fs::create_dir_all(&dir).unwrap();

        // Fake ffmpeg script that writes >256KB to stderr and copies input file to output path (rejecting overwrite if -n and file exists)
        let fake_ffmpeg = dir.join("fake-ffmpeg.sh");
        fs::write(
            &fake_ffmpeg,
            r#"#!/bin/sh
dd if=/dev/zero bs=4096 count=80 2>/dev/null | tr '\0' 'S' >&2
in_file=""
out_file=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-i" ]; then
    shift
    in_file="$1"
  else
    out_file="$1"
  fi
  shift
done
if [ -e "$out_file" ]; then
  echo "Output file already exists" >&2
  exit 1
fi
cp "$in_file" "$out_file"
"#,
        )
        .unwrap();
        fs::set_permissions(&fake_ffmpeg, fs::Permissions::from_mode(0o755)).unwrap();

        let in_a = dir.join("in_a.m4s");
        let in_b = dir.join("in_b.m4s");
        fs::write(&in_a, b"content-from-task-A").unwrap();
        fs::write(&in_b, b"content-from-task-B").unwrap();

        let desired = dir.join("Same Song.m4a");
        // Pre-existing file to verify neither task overwrites it
        fs::write(&desired, b"existing-original").unwrap();

        let f1 = fake_ffmpeg.clone();
        let d1 = desired.clone();
        let t1 = std::thread::spawn(move || {
            convert_with_ffmpeg_for_task(
                &f1,
                "audio",
                Some(&in_a),
                None,
                &d1,
                "task-alpha",
                &AtomicBool::new(false),
                Duration::from_secs(10),
            )
            .unwrap()
        });

        let f2 = fake_ffmpeg.clone();
        let d2 = desired.clone();
        let t2 = std::thread::spawn(move || {
            convert_with_ffmpeg_for_task(
                &f2,
                "audio",
                Some(&in_b),
                None,
                &d2,
                "task-beta",
                &AtomicBool::new(false),
                Duration::from_secs(10),
            )
            .unwrap()
        });

        let out1 = t1.join().unwrap();
        let out2 = t2.join().unwrap();
        assert_ne!(out1, out2);
        assert_ne!(out1, desired);
        assert_ne!(out2, desired);
        assert_eq!(fs::read(&desired).unwrap(), b"existing-original");

        let mut names = vec![
            out1.file_name().unwrap().to_str().unwrap().to_string(),
            out2.file_name().unwrap().to_str().unwrap().to_string(),
        ];
        names.sort();
        assert_eq!(names, vec!["Same Song (1).m4a", "Same Song (2).m4a"]);

        let mut contents = vec![fs::read(&out1).unwrap(), fs::read(&out2).unwrap()];
        contents.sort();
        assert_eq!(
            contents,
            vec![
                b"content-from-task-A".to_vec(),
                b"content-from-task-B".to_vec()
            ]
        );

        let _ = fs::remove_dir_all(&dir);
    }
}
