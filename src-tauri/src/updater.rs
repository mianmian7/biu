use crate::bili::{parse_proxy_settings_value, ProxySettings};
use crate::store::get_saved_proxy_settings;
use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::io::AsyncWriteExt;
use url::Url;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CodeSignatureKind {
    Certificate,
    Adhoc,
    Unsigned,
}

pub fn parse_codesign_output(stderr: &str) -> CodeSignatureKind {
    if stderr.contains("Signature=adhoc") {
        return CodeSignatureKind::Adhoc;
    }
    if stderr.contains("code object is not signed") {
        return CodeSignatureKind::Unsigned;
    }
    if stderr.contains("Authority=") {
        return CodeSignatureKind::Certificate;
    }
    CodeSignatureKind::Unsigned
}

pub fn classify_update_error(message: &str) -> &'static str {
    let lower = message.to_ascii_lowercase();
    if lower.contains("could not fetch a valid release json from the remote")
        || lower.contains("release json")
        || lower.contains("404")
        || lower.contains("targetnotfound")
        || lower.contains("targetsnotfound")
        || lower.contains("was not found in the response `platforms` object")
        || lower.contains("none of the fallback platforms")
        || lower.contains("err_updater_channel_file_not_found")
        || lower.contains("err_updater_latest_version_not_found")
        || lower.contains("err_updater_no_published_versions")
    {
        return "unpublished";
    }
    if lower.contains("dns error")
        || lower.contains("timed out")
        || lower.contains("connection refused")
        || lower.contains("connection reset")
        || lower.contains("network")
        || lower.contains("error sending request")
    {
        return "network";
    }
    "other"
}

pub fn is_signature_error(message: &str) -> bool {
    let lower = message.to_ascii_lowercase();
    lower.contains("signature")
        || lower.contains("minisign")
        || lower.contains("signed for version")
        || lower.contains("code requirement")
        || lower.contains("code signing")
}

#[derive(Debug, Clone)]
pub struct UpdateModeOptions<'a> {
    pub os: &'a str,
    pub is_dev: bool,
    pub portable_dir: Option<&'a str>,
    pub portable_file: Option<&'a str>,
    pub appimage: Option<&'a str>,
    pub exec_path: &'a str,
    pub code_signature_kind: Option<CodeSignatureKind>,
    pub bundle_parent_writable: Option<bool>,
}

pub fn resolve_update_mode(opts: &UpdateModeOptions<'_>) -> &'static str {
    if opts.is_dev {
        return "manual";
    }
    match opts.os {
        "windows" | "win32" => {
            if opts.portable_dir.is_some_and(|s| !s.is_empty())
                || opts.portable_file.is_some_and(|s| !s.is_empty())
            {
                "manual"
            } else {
                "auto"
            }
        }
        "linux" => {
            if opts.appimage.is_some_and(|s| !s.is_empty()) {
                "auto"
            } else {
                "manual"
            }
        }
        "macos" | "darwin" => {
            if opts.exec_path.contains("/AppTranslocation/")
                || opts.exec_path.starts_with("/Volumes/")
            {
                return "manual";
            }
            if opts.code_signature_kind == Some(CodeSignatureKind::Certificate)
                && opts.bundle_parent_writable == Some(true)
            {
                "auto"
            } else {
                "manual"
            }
        }
        _ => "manual",
    }
}

fn is_dir_writable(dir: &Path) -> bool {
    let Ok(meta) = std::fs::metadata(dir) else {
        return false;
    };
    if meta.permissions().readonly() {
        return false;
    }
    #[cfg(unix)]
    {
        use std::ffi::CString;
        use std::os::unix::ffi::OsStrExt;
        if let Ok(c_path) = CString::new(dir.as_os_str().as_bytes()) {
            return unsafe { libc::access(c_path.as_ptr(), libc::W_OK) == 0 };
        }
    }
    true
}

pub fn detect_runtime_update_mode() -> &'static str {
    if let Ok(forced) = std::env::var("BIU_FORCE_UPDATE_MODE") {
        if forced == "auto" {
            return "auto";
        }
        if forced == "manual" {
            return "manual";
        }
    }
    let is_dev = cfg!(debug_assertions)
        && std::env::var_os("BIU_UPDATE_ENDPOINT").is_none()
        && std::env::var_os("BIU_UPDATE_FEED_URL").is_none();
    if is_dev {
        return "manual";
    }
    let exec_path = std::env::current_exe()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();
    let portable_dir = std::env::var("PORTABLE_EXECUTABLE_DIR").ok();
    let portable_file = std::env::var("PORTABLE_EXECUTABLE_FILE").ok();
    let appimage = std::env::var("APPIMAGE").ok();

    let (code_signature_kind, bundle_parent_writable) = if cfg!(target_os = "macos") {
        let exe = Path::new(&exec_path);
        let bundle_path = exe
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent());
        if let Some(bundle) = bundle_path {
            let sig_kind = Command::new("/usr/bin/codesign")
                .args(["-dv", "--verbose=2"])
                .arg(bundle)
                .output()
                .map(|o| parse_codesign_output(&String::from_utf8_lossy(&o.stderr)))
                .unwrap_or(CodeSignatureKind::Unsigned);
            let writable = bundle.parent().is_some_and(is_dir_writable);
            (Some(sig_kind), Some(writable))
        } else {
            (Some(CodeSignatureKind::Unsigned), Some(false))
        }
    } else {
        (None, None)
    };

    resolve_update_mode(&UpdateModeOptions {
        os: std::env::consts::OS,
        is_dev,
        portable_dir: portable_dir.as_deref(),
        portable_file: portable_file.as_deref(),
        appimage: appimage.as_deref(),
        exec_path: &exec_path,
        code_signature_kind,
        bundle_parent_writable,
    })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CheckAppUpdateResult {
    pub is_update_available: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest_version: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_notes: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub notice: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateAvailablePayload {
    pub latest_version: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub release_notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadProgressInfo {
    pub total: u64,
    pub delta: u64,
    pub transferred: u64,
    pub percent: f64,
    pub bytes_per_second: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadInfoPayload {
    pub file_path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct UpdateMessagePayload {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub process_info: Option<DownloadProgressInfo>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub download_info: Option<DownloadInfoPayload>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdaterPhase {
    Checking,
    Downloading,
    Downloaded,
    Installing,
}

pub struct UpdaterState {
    pub mode: Mutex<String>,
    pub phase: Mutex<UpdaterPhase>,
    pub pending_update: Mutex<Option<Update>>,
    pub downloaded_bytes: Mutex<Option<Vec<u8>>>,
    pub downloaded_file: Mutex<Option<PathBuf>>,
}

impl Default for UpdaterState {
    fn default() -> Self {
        let mode = detect_runtime_update_mode().to_string();
        log::info!("[updater] mode={mode}");
        Self {
            mode: Mutex::new(mode),
            phase: Mutex::new(UpdaterPhase::Checking),
            pending_update: Mutex::new(None),
            downloaded_bytes: Mutex::new(None),
            downloaded_file: Mutex::new(None),
        }
    }
}

fn emit_to_main<R: Runtime, S: Serialize + Clone>(app: &AppHandle<R>, event: &str, payload: S) {
    if let Some(w) = app.get_webview_window("main") {
        let _ = w.emit(event, payload);
    } else {
        let _ = app.emit(event, payload);
    }
}

fn proxy_url_from_settings(settings: Option<ProxySettings>) -> Option<Url> {
    let s = settings?;
    let scheme = match s.proxy_type.as_str() {
        "http" => "http",
        "socks4" => "socks4",
        "socks5" => "socks5",
        _ => return None,
    };
    let host_raw = s.host?;
    let host = host_raw.trim();
    let port = s.port?;
    if host.is_empty() || port == 0 {
        return None;
    }
    let raw = match (
        s.username.as_deref().filter(|u| !u.is_empty()),
        s.password.as_deref().filter(|p| !p.is_empty()),
    ) {
        (Some(u), Some(p)) => format!("{scheme}://{u}:{p}@{host}:{port}"),
        (Some(u), None) => format!("{scheme}://{u}@{host}:{port}"),
        _ => format!("{scheme}://{host}:{port}"),
    };
    Url::parse(&raw).ok()
}

fn build_updater<R: Runtime>(app: &AppHandle<R>) -> Result<tauri_plugin_updater::Updater, String> {
    let mut builder = app.updater_builder();
    if let Ok(endpoint) =
        std::env::var("BIU_UPDATE_ENDPOINT").or_else(|_| std::env::var("BIU_UPDATE_FEED_URL"))
    {
        let trimmed = endpoint.trim();
        if !trimmed.is_empty() {
            let url = Url::parse(trimmed).map_err(|e| e.to_string())?;
            builder = builder.endpoints(vec![url]).map_err(|e| e.to_string())?;
        }
    }
    if let Ok(target) = std::env::var("BIU_UPDATER_TARGET") {
        let trimmed = target.trim();
        if !trimmed.is_empty() {
            builder = builder.target(trimmed);
        }
    }
    if let Ok(exe_override) = std::env::var("BIU_UPDATER_EXECUTABLE_PATH") {
        let trimmed = exe_override.trim();
        if !trimmed.is_empty() {
            builder = builder.executable_path(trimmed);
        }
    } else if cfg!(target_os = "linux") {
        if let Ok(appimage) = std::env::var("APPIMAGE") {
            let trimmed = appimage.trim();
            if !trimmed.is_empty() {
                builder = builder.executable_path(trimmed);
            }
        }
    }
    let saved_proxy = parse_proxy_settings_value(get_saved_proxy_settings(app));
    if let Some(proxy_url) = proxy_url_from_settings(saved_proxy) {
        builder = builder.proxy(proxy_url);
    }
    builder.build().map_err(|e| e.to_string())
}

fn current_mode<R: Runtime>(app: &AppHandle<R>) -> String {
    app.try_state::<Arc<UpdaterState>>()
        .map(|s| s.mode.lock().unwrap().clone())
        .unwrap_or_else(|| detect_runtime_update_mode().to_string())
}

fn downgrade_to_manual<R: Runtime>(app: &AppHandle<R>) {
    if let Some(s) = app.try_state::<Arc<UpdaterState>>() {
        *s.mode.lock().unwrap() = "manual".to_string();
        log::warn!("[updater] mode=manual (native signature validation failed)");
    }
}

pub async fn perform_check<R: Runtime>(
    app: &AppHandle<R>,
    is_background: bool,
) -> CheckAppUpdateResult {
    let Some(state) = app.try_state::<Arc<UpdaterState>>() else {
        return CheckAppUpdateResult {
            is_update_available: false,
            latest_version: None,
            release_notes: None,
            error: Some("Updater state not initialized".into()),
            notice: None,
        };
    };

    if is_background && *state.phase.lock().unwrap() == UpdaterPhase::Downloaded {
        return CheckAppUpdateResult {
            is_update_available: true,
            latest_version: state
                .pending_update
                .lock()
                .unwrap()
                .as_ref()
                .map(|u| u.version.clone()),
            release_notes: state
                .pending_update
                .lock()
                .unwrap()
                .as_ref()
                .and_then(|u| u.body.clone()),
            error: None,
            notice: None,
        };
    }

    let updater = match build_updater(app) {
        Ok(u) => u,
        Err(err) => {
            log::warn!("[updater] failed to build updater: {err}");
            return CheckAppUpdateResult {
                is_update_available: false,
                latest_version: None,
                release_notes: None,
                error: Some(err),
                notice: None,
            };
        }
    };

    match updater.check().await {
        Ok(Some(update)) => {
            let version = update.version.clone();
            let notes = update.body.clone();
            *state.pending_update.lock().unwrap() = Some(update);

            let mode = current_mode(app);
            log::info!(
                "[updater] check_app_update: update-available version={version} mode={mode}"
            );
            emit_to_main(
                app,
                "app:update-available",
                UpdateAvailablePayload {
                    latest_version: version.clone(),
                    release_notes: notes.clone(),
                },
            );

            if mode == "auto" && std::env::var_os("BIU_UPDATER_E2E_DRIVE").is_none() {
                let phase = *state.phase.lock().unwrap();
                if phase != UpdaterPhase::Downloading && phase != UpdaterPhase::Downloaded {
                    *state.phase.lock().unwrap() = UpdaterPhase::Downloading;
                    let app_clone = app.clone();
                    tauri::async_runtime::spawn(async move {
                        let _ = download_update_internal(&app_clone).await;
                    });
                }
            }

            CheckAppUpdateResult {
                is_update_available: true,
                latest_version: Some(version),
                release_notes: notes,
                error: None,
                notice: None,
            }
        }
        Ok(None) => CheckAppUpdateResult {
            is_update_available: false,
            latest_version: None,
            release_notes: None,
            error: None,
            notice: None,
        },
        Err(err) => {
            let msg = err.to_string();
            let kind = classify_update_error(&msg);
            if is_background {
                log::warn!("[updater] background check: {kind} ({msg})");
            } else {
                log::warn!("[updater] manual check: {kind} ({msg})");
            }
            if kind == "unpublished" {
                CheckAppUpdateResult {
                    is_update_available: false,
                    latest_version: None,
                    release_notes: None,
                    error: None,
                    notice: Some("新版本正在发布，请几分钟后再试".into()),
                }
            } else {
                CheckAppUpdateResult {
                    is_update_available: false,
                    latest_version: None,
                    release_notes: None,
                    error: Some(msg),
                    notice: None,
                }
            }
        }
    }
}

async fn download_manual_installer_to_disk<R: Runtime>(
    app: &AppHandle<R>,
    update: &Update,
    mode: &str,
) -> Result<PathBuf, String> {
    let download_dir = dirs::download_dir()
        .or_else(|| app.path().app_cache_dir().ok())
        .unwrap_or_else(std::env::temp_dir);
    let _ = std::fs::create_dir_all(&download_dir);

    let file_name = update
        .download_url
        .path_segments()
        .and_then(|mut s| s.next_back())
        .filter(|n| !n.is_empty())
        .map(|s| s.to_string())
        .unwrap_or_else(|| format!("Biu-{}-update.bin", update.version));
    let dest_path = download_dir.join(&file_name);

    let mut client_builder = reqwest::Client::builder();
    let saved_proxy = parse_proxy_settings_value(get_saved_proxy_settings(app));
    if let Some(proxy_url) = proxy_url_from_settings(saved_proxy) {
        if let Ok(proxy) = reqwest::Proxy::all(proxy_url.as_str()) {
            client_builder = client_builder.proxy(proxy);
        }
    }
    let client = client_builder.build().map_err(|e| e.to_string())?;
    let resp = client
        .get(update.download_url.clone())
        .send()
        .await
        .map_err(|e| e.to_string())?;
    if !resp.status().is_success() {
        return Err(format!(
            "Download request failed with status: {}",
            resp.status()
        ));
    }

    let total = resp.content_length().unwrap_or(0);
    let mut file = tokio::fs::File::create(&dest_path)
        .await
        .map_err(|e| e.to_string())?;
    let mut transferred: u64 = 0;
    let started = Instant::now();
    let mut stream = resp.bytes_stream();

    while let Some(chunk_res) = stream.next().await {
        let chunk = chunk_res.map_err(|e| e.to_string())?;
        file.write_all(&chunk).await.map_err(|e| e.to_string())?;
        let delta = chunk.len() as u64;
        transferred += delta;
        let elapsed = started.elapsed().as_secs_f64().max(0.001);
        let bps = transferred as f64 / elapsed;
        let percent = if total > 0 {
            ((transferred as f64 / total as f64) * 100.0).min(100.0)
        } else {
            0.0
        };
        emit_to_main(
            app,
            "app:update-message",
            UpdateMessagePayload {
                status: "downloading".into(),
                mode: Some(mode.to_string()),
                process_info: Some(DownloadProgressInfo {
                    total,
                    delta,
                    transferred,
                    percent,
                    bytes_per_second: bps,
                }),
                download_info: None,
                error: None,
            },
        );
    }
    file.flush().await.map_err(|e| e.to_string())?;
    Ok(dest_path)
}

pub async fn download_update_internal<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let Some(state) = app.try_state::<Arc<UpdaterState>>() else {
        return Err("Updater state not initialized".into());
    };

    let mut maybe_update = state.pending_update.lock().unwrap().clone();
    if maybe_update.is_none() {
        let updater = build_updater(app)?;
        if let Ok(Some(found)) = updater.check().await {
            *state.pending_update.lock().unwrap() = Some(found.clone());
            maybe_update = Some(found);
        }
    }
    let Some(update) = maybe_update else {
        return Err("没有可下载的更新".into());
    };

    *state.phase.lock().unwrap() = UpdaterPhase::Downloading;
    let mode = current_mode(app);
    log::info!(
        "[updater] download_app_update: starting version={} mode={} url={}",
        update.version,
        mode,
        update.download_url
    );

    if mode == "manual" {
        match download_manual_installer_to_disk(app, &update, &mode).await {
            Ok(path) => {
                *state.phase.lock().unwrap() = UpdaterPhase::Downloaded;
                *state.downloaded_file.lock().unwrap() = Some(path.clone());
                log::info!(
                    "[updater] onDownloadAppProgress: status=downloaded mode=manual file={}",
                    path.display()
                );
                emit_to_main(
                    app,
                    "app:update-message",
                    UpdateMessagePayload {
                        status: "downloaded".into(),
                        mode: Some(mode),
                        process_info: None,
                        download_info: Some(DownloadInfoPayload {
                            file_path: path.to_string_lossy().to_string(),
                        }),
                        error: None,
                    },
                );
                return Ok(());
            }
            Err(err) => {
                *state.phase.lock().unwrap() = UpdaterPhase::Checking;
                emit_to_main(
                    app,
                    "app:update-message",
                    UpdateMessagePayload {
                        status: "error".into(),
                        mode: Some(mode),
                        process_info: None,
                        download_info: None,
                        error: Some(err.clone()),
                    },
                );
                return Err(err);
            }
        }
    }

    let mut transferred: u64 = 0;
    let mut logged_first_chunk = false;
    let started = Instant::now();
    let app_for_chunk = app.clone();
    let mode_for_chunk = mode.clone();

    let download_res = update
        .download(
            move |chunk_len, content_len| {
                let delta = chunk_len as u64;
                transferred += delta;
                let total = content_len.unwrap_or(transferred);
                let elapsed = started.elapsed().as_secs_f64().max(0.001);
                let bps = transferred as f64 / elapsed;
                let percent = if total > 0 {
                    ((transferred as f64 / total as f64) * 100.0).min(100.0)
                } else {
                    0.0
                };
                if !logged_first_chunk || transferred == total {
                    logged_first_chunk = true;
                    log::info!(
                        "[updater] onDownloadAppProgress: status=downloading transferred={transferred}/{total} ({percent:.1}%)"
                    );
                }
                emit_to_main(
                    &app_for_chunk,
                    "app:update-message",
                    UpdateMessagePayload {
                        status: "downloading".into(),
                        mode: Some(mode_for_chunk.clone()),
                        process_info: Some(DownloadProgressInfo {
                            total,
                            delta,
                            transferred,
                            percent,
                            bytes_per_second: bps,
                        }),
                        download_info: None,
                        error: None,
                    },
                );
            },
            || {},
        )
        .await;

    match download_res {
        Ok(bytes) => {
            let cache_dir = app
                .path()
                .app_cache_dir()
                .unwrap_or_else(|_| std::env::temp_dir());
            let _ = std::fs::create_dir_all(&cache_dir);
            let cached_path = cache_dir.join(format!("Biu-{}-update.bin", update.version));
            let _ = std::fs::write(&cached_path, &bytes);

            log::info!(
                "[updater] onDownloadAppProgress: status=downloaded mode={} bytes={} file={}",
                mode,
                bytes.len(),
                cached_path.display()
            );
            *state.downloaded_bytes.lock().unwrap() = Some(bytes);
            *state.downloaded_file.lock().unwrap() = Some(cached_path.clone());
            *state.phase.lock().unwrap() = UpdaterPhase::Downloaded;

            emit_to_main(
                app,
                "app:update-message",
                UpdateMessagePayload {
                    status: "downloaded".into(),
                    mode: Some(mode),
                    process_info: None,
                    download_info: Some(DownloadInfoPayload {
                        file_path: cached_path.to_string_lossy().to_string(),
                    }),
                    error: None,
                },
            );
            Ok(())
        }
        Err(err) => {
            let msg = err.to_string();
            let mut active_mode = mode;
            if is_signature_error(&msg) {
                downgrade_to_manual(app);
                active_mode = "manual".to_string();
            }
            *state.phase.lock().unwrap() = UpdaterPhase::Checking;
            emit_to_main(
                app,
                "app:update-message",
                UpdateMessagePayload {
                    status: "error".into(),
                    mode: Some(active_mode),
                    process_info: None,
                    download_info: None,
                    error: Some(msg.clone()),
                },
            );
            Err(msg)
        }
    }
}

pub fn install_downloaded_update_on_exit<R: Runtime>(app: &AppHandle<R>) {
    let Some(state) = app.try_state::<Arc<UpdaterState>>() else {
        return;
    };
    if *state.mode.lock().unwrap() != "auto" {
        return;
    }
    let phase = *state.phase.lock().unwrap();
    if phase != UpdaterPhase::Downloaded {
        return;
    }
    let maybe_update = state.pending_update.lock().unwrap().clone();
    let maybe_bytes = state.downloaded_bytes.lock().unwrap().take();
    if let (Some(update), Some(bytes)) = (maybe_update, maybe_bytes) {
        *state.phase.lock().unwrap() = UpdaterPhase::Installing;
        let update = update.restart_after_install(false);
        if let Err(err) = update.install(bytes) {
            log::warn!("[updater] failed to install update on exit: {err}");
        }
    }
}

pub fn setup_updater<R: Runtime>(app: &AppHandle<R>) {
    app.manage(Arc::new(UpdaterState::default()));
    let app_handle = app.clone();
    if std::env::var_os("BIU_UPDATER_E2E_DRIVE").is_some() {
        tauri::async_runtime::spawn(async move {
            tokio::time::sleep(Duration::from_millis(2000)).await;
            if let Some(w) = app_handle.get_webview_window("main") {
                let script = r#"
(async () => {
  const logInfo = (message) => window.__TAURI_INTERNALS__.invoke("frontend_log", { level: "info", message });
  try {
    for (let i = 0; i < 50 && !window.electron; i++) {
      await new Promise((r) => setTimeout(r, 100));
    }
    const mode = await window.electron.getUpdateMode();
    await logInfo(`[e2e-frontend] getUpdateMode() => ${mode}`);
    window.electron.onUpdateAvailable((info) => {
      void logInfo(`[e2e-frontend] event onUpdateAvailable => ${JSON.stringify(info)}`);
    });
    let sawDownloading = false;
    let resolveDownloaded;
    const downloadedPromise = new Promise((resolve) => { resolveDownloaded = resolve; });
    window.electron.onDownloadAppProgress((msg) => {
      if (msg.status === "downloading" && !sawDownloading) {
        sawDownloading = true;
        void logInfo(`[e2e-frontend] event onDownloadAppProgress(downloading) => ${JSON.stringify(msg)}`);
      } else if (msg.status === "downloaded") {
        void logInfo(`[e2e-frontend] event onDownloadAppProgress(downloaded) => ${JSON.stringify(msg)}`);
        resolveDownloaded(msg);
      } else if (msg.status === "error") {
        void logInfo(`[e2e-frontend] event onDownloadAppProgress(error) => ${JSON.stringify(msg)}`);
        resolveDownloaded(msg);
      }
    });
    const checkRes = await window.electron.checkAppUpdate();
    await logInfo(`[e2e-frontend] checkAppUpdate() => ${JSON.stringify(checkRes)}`);
    await window.electron.downloadAppUpdate();
    await downloadedPromise;
    await logInfo(`[e2e-frontend] calling quitAndInstall()`);
    await window.electron.quitAndInstall();
  } catch (err) {
    await logInfo(`[e2e-frontend] error => ${String(err)}`);
  }
})();
"#;
                let _ = w.eval(script);
            }
        });
        return;
    }
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_millis(1500)).await;
        let _ = perform_check(&app_handle, true).await;
        let mut interval = tokio::time::interval(Duration::from_secs(60 * 60));
        // Skip immediate first tick
        interval.tick().await;
        loop {
            interval.tick().await;
            let _ = perform_check(&app_handle, true).await;
        }
    });
}

#[tauri::command]
pub async fn check_app_update<R: Runtime>(
    app: AppHandle<R>,
) -> Result<CheckAppUpdateResult, String> {
    Ok(perform_check(&app, false).await)
}

#[tauri::command]
pub async fn get_update_mode<R: Runtime>(app: AppHandle<R>) -> Result<String, String> {
    Ok(current_mode(&app))
}

#[tauri::command]
pub async fn download_app_update<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    download_update_internal(&app).await
}

#[tauri::command]
pub async fn quit_and_install<R: Runtime>(app: AppHandle<R>) -> Result<bool, String> {
    let Some(state) = app.try_state::<Arc<UpdaterState>>() else {
        return Ok(false);
    };
    if *state.mode.lock().unwrap() != "auto" {
        return Ok(false);
    }
    let maybe_update = state.pending_update.lock().unwrap().clone();
    let maybe_bytes = state.downloaded_bytes.lock().unwrap().clone();
    let (Some(update), Some(bytes)) = (maybe_update, maybe_bytes) else {
        return Ok(false);
    };

    *state.phase.lock().unwrap() = UpdaterPhase::Installing;
    log::info!(
        "[updater] quit_and_install: installing version={} bytes={}",
        update.version,
        bytes.len()
    );
    let update = update.restart_after_install(true);
    match update.install(&bytes) {
        Ok(()) => {
            *state.downloaded_bytes.lock().unwrap() = None;
            log::info!(
                "[updater] quit_and_install: binary replaced, restarting into version={}",
                update.version
            );
            if let Ok(relaunch_marker) = std::env::var("BIU_UPDATER_RELAUNCH_MARKER") {
                let _ = std::fs::write(relaunch_marker, &update.version);
            }
            app.request_restart();
            Ok(true)
        }
        Err(err) => {
            let msg = err.to_string();
            let mut active_mode = current_mode(&app);
            if is_signature_error(&msg) {
                downgrade_to_manual(&app);
                active_mode = "manual".to_string();
            }
            *state.phase.lock().unwrap() = UpdaterPhase::Checking;
            emit_to_main(
                &app,
                "app:update-message",
                UpdateMessagePayload {
                    status: "error".into(),
                    mode: Some(active_mode),
                    process_info: None,
                    download_info: None,
                    error: Some(msg),
                },
            );
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolve_update_mode_matches_platform_rules() {
        // Dev mode always manual
        assert_eq!(
            resolve_update_mode(&UpdateModeOptions {
                os: "windows",
                is_dev: true,
                portable_dir: None,
                portable_file: None,
                appimage: None,
                exec_path: "C:\\Biu\\Biu.exe",
                code_signature_kind: None,
                bundle_parent_writable: None,
            }),
            "manual"
        );

        // Windows installed vs portable
        assert_eq!(
            resolve_update_mode(&UpdateModeOptions {
                os: "windows",
                is_dev: false,
                portable_dir: None,
                portable_file: None,
                appimage: None,
                exec_path: "C:\\Biu\\Biu.exe",
                code_signature_kind: None,
                bundle_parent_writable: None,
            }),
            "auto"
        );
        assert_eq!(
            resolve_update_mode(&UpdateModeOptions {
                os: "windows",
                is_dev: false,
                portable_dir: Some("C:\\Portable"),
                portable_file: None,
                appimage: None,
                exec_path: "C:\\Portable\\Biu.exe",
                code_signature_kind: None,
                bundle_parent_writable: None,
            }),
            "manual"
        );

        // Linux AppImage vs deb/rpm
        assert_eq!(
            resolve_update_mode(&UpdateModeOptions {
                os: "linux",
                is_dev: false,
                portable_dir: None,
                portable_file: None,
                appimage: Some("/home/user/Biu.AppImage"),
                exec_path: "/tmp/.mount_Biu/usr/bin/biu",
                code_signature_kind: None,
                bundle_parent_writable: None,
            }),
            "auto"
        );
        assert_eq!(
            resolve_update_mode(&UpdateModeOptions {
                os: "linux",
                is_dev: false,
                portable_dir: None,
                portable_file: None,
                appimage: None,
                exec_path: "/usr/bin/biu",
                code_signature_kind: None,
                bundle_parent_writable: None,
            }),
            "manual"
        );

        // macOS certificate + writable vs Volumes / AppTranslocation / adhoc
        assert_eq!(
            resolve_update_mode(&UpdateModeOptions {
                os: "macos",
                is_dev: false,
                portable_dir: None,
                portable_file: None,
                appimage: None,
                exec_path: "/Applications/Biu.app/Contents/MacOS/Biu",
                code_signature_kind: Some(CodeSignatureKind::Certificate),
                bundle_parent_writable: Some(true),
            }),
            "auto"
        );
        assert_eq!(
            resolve_update_mode(&UpdateModeOptions {
                os: "macos",
                is_dev: false,
                portable_dir: None,
                portable_file: None,
                appimage: None,
                exec_path: "/Volumes/Biu/Biu.app/Contents/MacOS/Biu",
                code_signature_kind: Some(CodeSignatureKind::Certificate),
                bundle_parent_writable: Some(true),
            }),
            "manual"
        );
        assert_eq!(
            resolve_update_mode(&UpdateModeOptions {
                os: "macos",
                is_dev: false,
                portable_dir: None,
                portable_file: None,
                appimage: None,
                exec_path:
                    "/private/var/folders/xx/AppTranslocation/UUID/d/Biu.app/Contents/MacOS/Biu",
                code_signature_kind: Some(CodeSignatureKind::Certificate),
                bundle_parent_writable: Some(true),
            }),
            "manual"
        );
        assert_eq!(
            resolve_update_mode(&UpdateModeOptions {
                os: "macos",
                is_dev: false,
                portable_dir: None,
                portable_file: None,
                appimage: None,
                exec_path: "/Applications/Biu.app/Contents/MacOS/Biu",
                code_signature_kind: Some(CodeSignatureKind::Adhoc),
                bundle_parent_writable: Some(true),
            }),
            "manual"
        );
    }

    #[test]
    fn codesign_and_error_classification() {
        assert_eq!(
            parse_codesign_output("Executable=/Applications/Biu.app\nSignature=adhoc\n"),
            CodeSignatureKind::Adhoc
        );
        assert_eq!(
            parse_codesign_output("Authority=Biu Self-Signed Code Signing\n"),
            CodeSignatureKind::Certificate
        );
        assert_eq!(
            parse_codesign_output("code object is not signed at all"),
            CodeSignatureKind::Unsigned
        );

        assert_eq!(
            classify_update_error("Could not fetch a valid release JSON from the remote"),
            "unpublished"
        );
        assert_eq!(
            classify_update_error(
                "the platform `linux-aarch64` was not found in the response `platforms` object"
            ),
            "unpublished"
        );
        assert_eq!(classify_update_error("connection timed out"), "network");
        assert!(is_signature_error(
            "Signature verification failed: minisign"
        ));
    }
}
