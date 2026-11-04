use crate::bili::BiliState;
use crate::store::read_store_object;
use std::path::Path;
use std::sync::Arc;
use tauri::{AppHandle, Runtime};
use tauri_plugin_dialog::DialogExt;
use tauri_plugin_opener::OpenerExt;

#[tauri::command]
pub async fn select_directory<R: Runtime>(
    app: AppHandle<R>,
    state: tauri::State<'_, Arc<BiliState>>,
    title: Option<String>,
) -> Result<Option<String>, String> {
    let resolved_title = title
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .unwrap_or("选择目录")
        .to_string();

    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title(&resolved_title)
        .pick_folder(move |folder| {
            let path_str = folder
                .and_then(|f| f.into_path().ok())
                .map(|p| p.to_string_lossy().into_owned());
            let _ = tx.send(path_str);
        });

    let chosen = rx.await.map_err(|e| e.to_string())?;
    if let Some(ref dir) = chosen {
        state.register_local_dir(Path::new(dir));
    }
    Ok(chosen)
}

#[tauri::command]
pub async fn select_file<R: Runtime>(app: AppHandle<R>) -> Result<Option<String>, String> {
    let (tx, rx) = tokio::sync::oneshot::channel();
    app.dialog()
        .file()
        .set_title("选择文件")
        .pick_file(move |file| {
            let path_str = file
                .and_then(|f| f.into_path().ok())
                .map(|p| p.to_string_lossy().into_owned());
            let _ = tx.send(path_str);
        });

    let chosen = rx.await.map_err(|e| e.to_string())?;
    Ok(chosen)
}

#[tauri::command]
pub async fn open_directory<R: Runtime>(
    app: AppHandle<R>,
    path: Option<String>,
) -> Result<bool, String> {
    let target_dir = path
        .filter(|s| !s.trim().is_empty())
        .or_else(|| {
            read_store_object(&app, "app-settings")
                .ok()
                .and_then(|v| {
                    v.get("appSettings")?
                        .get("downloadPath")?
                        .as_str()
                        .map(|s| s.to_string())
                })
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| dirs::download_dir().map(|p| p.to_string_lossy().into_owned()))
        .unwrap_or_else(|| {
            std::env::current_dir()
                .unwrap_or_default()
                .join("downloads")
                .to_string_lossy()
                .into_owned()
        });

    match app.opener().open_path(&target_dir, None::<&str>) {
        Ok(()) => Ok(true),
        Err(err) => {
            log::error!("[dialog] open_directory failed for {target_dir}: {err}");
            Ok(false)
        }
    }
}

#[tauri::command]
pub async fn show_file_in_folder<R: Runtime>(
    app: AppHandle<R>,
    file_path: String,
) -> Result<bool, String> {
    if file_path.trim().is_empty() || !Path::new(&file_path).exists() {
        return Err("文件路径不存在".into());
    }
    app.opener()
        .reveal_item_in_dir(Path::new(&file_path))
        .map_err(|e| e.to_string())?;
    Ok(true)
}
