use crate::store::{read_store_object, store_set};
use serde_json::json;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tauri_plugin_global_shortcut::{GlobalShortcutExt, ShortcutState};

pub fn register_all_shortcuts_internal<R: Runtime>(app: &AppHandle<R>) {
    let _ = app.global_shortcut().unregister_all();

    let Ok(store_val) = read_store_object(app, "shortcut-settings") else {
        return;
    };
    let enable = store_val
        .get("enableGlobalShortcuts")
        .and_then(|v| v.as_bool())
        .unwrap_or(true);
    if !enable {
        log::info!("[shortcut] global shortcuts disabled in settings");
        return;
    }

    let Some(arr) = store_val
        .get("globalShortcuts")
        .and_then(|v| v.as_array())
        .cloned()
    else {
        return;
    };

    let mut updated = Vec::with_capacity(arr.len());
    let mut registered_count = 0usize;

    for mut item in arr {
        let id = item
            .get("id")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string();
        let key = item
            .get("shortcut")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if !id.is_empty() && !key.is_empty() {
            let id_for_cb = id.clone();
            let res =
                app.global_shortcut()
                    .on_shortcut(key.as_str(), move |app_handle, _sc, event| {
                        if event.state == ShortcutState::Pressed {
                            log::info!("[shortcut] triggered: {id_for_cb}");
                            if let Some(w) = app_handle.get_webview_window("main") {
                                let _ = w.emit("shortcut:triggered", &id_for_cb);
                            }
                        }
                    });
            match res {
                Ok(()) => {
                    registered_count += 1;
                    if let Some(obj) = item.as_object_mut() {
                        obj.insert("isConflict".to_string(), json!(false));
                    }
                }
                Err(e) => {
                    log::warn!("[shortcut] failed to register {id} ({key}): {e}");
                    if let Some(obj) = item.as_object_mut() {
                        obj.insert("isConflict".to_string(), json!(true));
                    }
                }
            }
        }
        updated.push(item);
    }

    let total = updated.len();
    let _ = store_set(
        app.clone(),
        "shortcut-settings".to_string(),
        json!({ "globalShortcuts": updated }),
    );
    log::info!("[shortcut] registered {registered_count}/{total} global shortcuts");
}

#[tauri::command]
pub async fn shortcut_register<R: Runtime>(
    app: AppHandle<R>,
    id: String,
    accelerator: String,
) -> Result<bool, String> {
    if let Ok(store_val) = read_store_object(&app, "shortcut-settings") {
        if let Some(arr) = store_val.get("globalShortcuts").and_then(|v| v.as_array()) {
            for item in arr {
                if item.get("id").and_then(|v| v.as_str()) == Some(id.as_str()) {
                    if let Some(old_key) = item
                        .get("shortcut")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                    {
                        let _ = app.global_shortcut().unregister(old_key);
                    }
                }
            }
        }
    }

    let trimmed = accelerator.trim();
    if trimmed.is_empty() {
        return Ok(false);
    }

    let id_for_cb = id.clone();
    match app
        .global_shortcut()
        .on_shortcut(trimmed, move |app_handle, _sc, event| {
            if event.state == ShortcutState::Pressed {
                log::info!("[shortcut] triggered: {id_for_cb}");
                if let Some(w) = app_handle.get_webview_window("main") {
                    let _ = w.emit("shortcut:triggered", &id_for_cb);
                }
            }
        }) {
        Ok(()) => {
            log::info!("[shortcut] registered {id} -> {trimmed}");
            Ok(true)
        }
        Err(e) => {
            log::warn!("[shortcut] failed to register {id} ({trimmed}): {e}");
            Ok(false)
        }
    }
}

#[tauri::command]
pub async fn shortcut_unregister<R: Runtime>(app: AppHandle<R>, id: String) -> Result<(), String> {
    if let Ok(store_val) = read_store_object(&app, "shortcut-settings") {
        if let Some(arr) = store_val.get("globalShortcuts").and_then(|v| v.as_array()) {
            for item in arr {
                if item.get("id").and_then(|v| v.as_str()) == Some(id.as_str()) {
                    if let Some(old_key) = item
                        .get("shortcut")
                        .and_then(|v| v.as_str())
                        .map(|s| s.trim())
                        .filter(|s| !s.is_empty())
                    {
                        let _ = app.global_shortcut().unregister(old_key);
                        log::info!("[shortcut] unregistered {id} ({old_key})");
                    }
                }
            }
        }
    }
    Ok(())
}

#[tauri::command]
pub async fn shortcut_register_all<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    register_all_shortcuts_internal(&app);
    Ok(())
}

#[tauri::command]
pub async fn shortcut_unregister_all<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let _ = app.global_shortcut().unregister_all();
    if let Ok(store_val) = read_store_object(&app, "shortcut-settings") {
        if let Some(arr) = store_val
            .get("globalShortcuts")
            .and_then(|v| v.as_array())
            .cloned()
        {
            let cleared: Vec<_> = arr
                .into_iter()
                .map(|mut item| {
                    if let Some(obj) = item.as_object_mut() {
                        obj.insert("isConflict".to_string(), json!(false));
                    }
                    item
                })
                .collect();
            let _ = store_set(
                app,
                "shortcut-settings".to_string(),
                json!({ "globalShortcuts": cleared }),
            );
        }
    }
    log::info!("[shortcut] unregistered all global shortcuts");
    Ok(())
}
