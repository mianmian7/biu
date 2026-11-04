use crate::bili::BiliState;
use crate::store::read_store_object;
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;
use md5::{Digest, Md5};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::UNIX_EPOCH;
use tauri::{AppHandle, Runtime};

const AUDIO_EXTS: &[&str] = &["mp3", "flac", "wav", "m4a", "aac", "ogg", "wma", "aiff"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMusicItem {
    pub id: String,
    pub path: String,
    pub dir: String,
    pub title: String,
    pub size: u64,
    pub format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_time: Option<u64>,
}

fn is_supported_audio_ext(ext: &str) -> bool {
    let lower = ext.to_ascii_lowercase();
    AUDIO_EXTS.contains(&lower.as_str())
}

fn to_safe_title(file_path: &Path, meta_title: Option<&str>) -> String {
    if let Some(t) = meta_title.map(str::trim).filter(|s| !s.is_empty()) {
        return t.to_string();
    }
    file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

pub fn md5_hex_lower(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn scan_local_music_dirs(dirs: &[String], state: &BiliState) -> Vec<LocalMusicItem> {
    let mut result = Vec::new();
    for dir in dirs {
        if dir.trim().is_empty() {
            continue;
        }
        let dir_path = Path::new(dir);
        state.register_local_dir(dir_path);

        let Ok(entries) = fs::read_dir(dir_path) else {
            continue;
        };

        let mut files: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                continue;
            }
            let full_path = entry.path();
            let Some(ext) = full_path.extension().and_then(|s| s.to_str()) else {
                continue;
            };
            if is_supported_audio_ext(ext) {
                files.push(full_path);
            }
        }

        for file in files {
            let Ok(meta) = fs::metadata(&file) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let canonical_path = fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
            let canonical_str = canonical_path.to_string_lossy().to_lowercase();
            let id = md5_hex_lower(&canonical_str);

            let (meta_title, duration) = match lofty::read_from_path(&file) {
                Ok(tagged) => {
                    let dur = tagged.properties().duration().as_secs_f64();
                    let dur_opt = if dur.is_finite() && dur > 0.0 {
                        Some(dur)
                    } else {
                        None
                    };
                    let title_opt = tagged
                        .primary_tag()
                        .or_else(|| tagged.first_tag())
                        .and_then(|t| t.title().map(|c| c.into_owned()));
                    (title_opt, dur_opt)
                }
                Err(_) => (None, None),
            };

            let format = file
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            let created_time = meta
                .created()
                .or_else(|_| meta.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64);

            result.push(LocalMusicItem {
                id,
                path: file.to_string_lossy().into_owned(),
                dir: dir.clone(),
                title: to_safe_title(&file, meta_title.as_deref()),
                size: meta.len(),
                format,
                duration,
                created_time,
            });
        }
    }
    result
}

pub fn sync_allowed_local_dirs_from_store<R: Runtime>(app: &AppHandle<R>, state: &BiliState) {
    if let Ok(val) = read_store_object(app, "app-settings") {
        if let Some(settings) = val.get("appSettings") {
            if let Some(dirs) = settings.get("localMusicDirs").and_then(|v| v.as_array()) {
                for d in dirs {
                    if let Some(s) = d.as_str() {
                        if !s.trim().is_empty() {
                            state.register_local_dir(Path::new(s));
                        }
                    }
                }
            }
            if let Some(dl) = settings.get("downloadPath").and_then(|v| v.as_str()) {
                if !dl.trim().is_empty() {
                    state.register_local_dir(Path::new(dl));
                }
            }
        }
    }
}

#[tauri::command]
pub async fn scan_local_music<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, Arc<BiliState>>,
    dirs: Vec<String>,
) -> Result<Vec<LocalMusicItem>, String> {
    sync_allowed_local_dirs_from_store(&app, &state);
    let state_clone = state.inner().clone();
    tokio::task::spawn_blocking(move || scan_local_music_dirs(&dirs, &state_clone))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn delete_local_music_file<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, Arc<BiliState>>,
    file_path: String,
) -> Result<bool, String> {
    if file_path.trim().is_empty() {
        return Ok(false);
    }
    sync_allowed_local_dirs_from_store(&app, &state);
    let p = PathBuf::from(&file_path);
    if !state.is_local_file_allowed(&p) {
        log::warn!("[local-music] denied deleting non-allowlisted path: {file_path}");
        return Ok(false);
    }
    match tokio::fs::metadata(&p).await {
        Ok(meta) if meta.is_file() => match tokio::fs::remove_file(&p).await {
            Ok(()) => Ok(true),
            Err(err) => {
                log::error!("[local-music] delete file error: {file_path}: {err}");
                Ok(false)
            }
        },
        _ => Ok(false),
    }
}
