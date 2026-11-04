use serde_json::{json, Value};
#[cfg(test)]
use std::collections::HashMap;
#[cfg(test)]
use std::path::PathBuf;
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;

const KNOWN_STORES: &[&str] = &[
    "app-settings",
    "user-login-info",
    "media-downloads",
    "shortcut-settings",
    "lyrics-cache",
    "desktop-lyrics",
];

pub fn store_filename(name: &str) -> Result<String, String> {
    if !KNOWN_STORES.contains(&name) {
        return Err(format!("unknown store name: {name}"));
    }
    Ok(format!("{name}.json"))
}

pub fn default_app_settings_json() -> Value {
    let download_path = dirs::download_dir()
        .map(|p| p.to_string_lossy().into_owned())
        .unwrap_or_default();
    json!({
        "appSettings": {
            "autoStart": false,
            "closeWindowOption": "hide",
            "fontFamily": "system-ui",
            "borderRadius": 8,
            "downloadPath": download_path,
            "primaryColor": "#1ed760",
            "backgroundColor": "",
            "audioQuality": "auto",
            "hiddenMenuKeys": [],
            "displayMode": "list",
            "ffmpegPath": "",
            "themeMode": "system",
            "pageTransition": "none",
            "showSearchHistory": true,
            "proxySettings": {
                "type": "none",
                "host": "",
                "username": "",
                "password": ""
            },
            "sideMenuCollapsed": false,
            "sideMenuWidth": 200,
            "sideMenuCollectionFolded": {
                "created": false,
                "collected": false
            },
            "reportPlayHistory": true,
            "localMusicDirs": [],
            "lyricsFontSize": 20,
            "showTopLyrics": true,
            "showTopLyricsTranslation": true,
            "showDesktopLyrics": false,
            "desktopLyricsLocked": true,
            "desktopLyricsFontSize": 36,
            "desktopLyricsStyle": {
                "layout": "double",
                "align": "center",
                "showTranslation": true,
                "showRomaji": true,
                "karaoke": true,
                "playedColor": "#1ed760",
                "unplayedColor": "#ffffff",
                "strokeColor": "#000000"
            }
        }
    })
}

pub fn default_shortcut_settings_json() -> Value {
    json!({
        "enableGlobalShortcuts": true,
        "shortcuts": [
            { "id": "togglePlay", "name": "播放/暂停", "shortcut": "CommandOrControl+P" },
            { "id": "prev", "name": "上一首", "shortcut": "CommandOrControl+Left" },
            { "id": "next", "name": "下一首", "shortcut": "CommandOrControl+Right" },
            { "id": "volumeUp", "name": "音量加", "shortcut": "CommandOrControl+Up" },
            { "id": "volumeDown", "name": "音量减", "shortcut": "CommandOrControl+Down" },
            { "id": "toggleMiniMode", "name": "切换mini/完整模式", "shortcut": "CommandOrControl+M" }
        ],
        "globalShortcuts": [
            { "id": "togglePlay", "name": "播放/暂停", "shortcut": "CommandOrControl+Alt+P" },
            { "id": "prev", "name": "上一首", "shortcut": "CommandOrControl+Alt+Left" },
            { "id": "next", "name": "下一首", "shortcut": "CommandOrControl+Alt+Right" },
            { "id": "volumeUp", "name": "音量加", "shortcut": "CommandOrControl+Alt+Up" },
            { "id": "volumeDown", "name": "音量减", "shortcut": "CommandOrControl+Alt+Down" },
            { "id": "toggleMiniMode", "name": "切换mini/完整模式", "shortcut": "CommandOrControl+Alt+M" }
        ]
    })
}

pub fn default_store_value(name: &str) -> Value {
    match name {
        "app-settings" => default_app_settings_json(),
        "shortcut-settings" => default_shortcut_settings_json(),
        _ => json!({}),
    }
}

pub fn mark_manual_edits(previous: &Value, incoming: &Value) -> Value {
    let Some(inc_obj) = incoming.as_object() else {
        return incoming.clone();
    };
    let prev_obj = previous.as_object();
    let mut out = serde_json::Map::new();
    for (key, entry) in inc_obj {
        let Some(entry_obj) = entry.as_object() else {
            out.insert(key.clone(), entry.clone());
            continue;
        };
        let old_obj = prev_obj.and_then(|p| p.get(key)).and_then(|v| v.as_object());
        let old_offset = old_obj
            .and_then(|o| o.get("offset"))
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let new_offset = entry_obj
            .get("offset")
            .and_then(|v| v.as_f64())
            .unwrap_or(0.0);
        let edited = old_obj.and_then(|o| o.get("lyrics")) != entry_obj.get("lyrics")
            || old_obj.and_then(|o| o.get("tLyrics")) != entry_obj.get("tLyrics")
            || old_obj.and_then(|o| o.get("rLyrics")) != entry_obj.get("rLyrics")
            || (old_offset - new_offset).abs() > f64::EPSILON;
        if edited {
            let mut updated = entry_obj.clone();
            updated.insert("source".to_string(), json!("manual"));
            out.insert(key.clone(), Value::Object(updated));
        } else {
            out.insert(key.clone(), entry.clone());
        }
    }
    Value::Object(out)
}

pub fn read_store_object<R: Runtime>(app: &AppHandle<R>, name: &str) -> Result<Value, String> {
    let filename = store_filename(name)?;
    let store = app.store(&filename).map_err(|e| e.to_string())?;
    let mut map = serde_json::Map::new();
    if let Some(defaults) = default_store_value(name).as_object() {
        for (k, v) in defaults {
            map.insert(k.clone(), v.clone());
        }
    }
    for (k, v) in store.entries() {
        map.insert(k, v);
    }
    Ok(Value::Object(map))
}

#[tauri::command]
pub fn store_get<R: Runtime>(app: AppHandle<R>, name: String) -> Result<Value, String> {
    read_store_object(&app, &name)
}

#[tauri::command]
pub fn store_set<R: Runtime>(app: AppHandle<R>, name: String, value: Value) -> Result<(), String> {
    let filename = store_filename(&name)?;
    if value.is_null() {
        return Ok(());
    }
    let final_val = if name == "lyrics-cache" {
        let prev = read_store_object(&app, &name).unwrap_or_else(|_| json!({}));
        mark_manual_edits(&prev, &value)
    } else {
        value
    };
    let Some(obj) = final_val.as_object() else {
        return Err("store value must be a JSON object".into());
    };
    let store = app.store(&filename).map_err(|e| e.to_string())?;
    for (k, v) in obj {
        store.set(k.clone(), v.clone());
    }
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn store_clear<R: Runtime>(app: AppHandle<R>, name: String) -> Result<bool, String> {
    let filename = store_filename(&name)?;
    let store = app.store(&filename).map_err(|e| e.to_string())?;
    store.clear();
    if let Some(defaults) = default_store_value(&name).as_object() {
        for (k, v) in defaults {
            store.set(k.clone(), v.clone());
        }
    }
    store.save().map_err(|e| e.to_string())?;
    Ok(true)
}

pub fn get_close_window_option<R: Runtime>(app: &AppHandle<R>) -> String {
    read_store_object(app, "app-settings")
        .ok()
        .and_then(|v| {
            v.get("appSettings")
                .and_then(|s| s.get("closeWindowOption"))
                .and_then(|c| c.as_str())
                .map(|s| s.to_string())
        })
        .unwrap_or_else(|| "hide".into())
}

pub fn get_saved_proxy_settings<R: Runtime>(app: &AppHandle<R>) -> Option<Value> {
    read_store_object(app, "app-settings")
        .ok()
        .and_then(|v| v.get("appSettings")?.get("proxySettings").cloned())
}

/// Pure file-backed store helpers for unit tests without a running Tauri AppHandle.
#[cfg(test)]
pub fn file_store_get(dir: &std::path::Path, name: &str) -> Result<Value, String> {
    let filename = store_filename(name)?;
    let path: PathBuf = dir.join(filename);
    let mut map = serde_json::Map::new();
    if let Some(defaults) = default_store_value(name).as_object() {
        for (k, v) in defaults {
            map.insert(k.clone(), v.clone());
        }
    }
    if path.exists() {
        let bytes = std::fs::read(&path).map_err(|e| e.to_string())?;
        let disk: HashMap<String, Value> =
            serde_json::from_slice(&bytes).map_err(|e| e.to_string())?;
        for (k, v) in disk {
            map.insert(k, v);
        }
    }
    Ok(Value::Object(map))
}

#[cfg(test)]
pub fn file_store_set(dir: &std::path::Path, name: &str, value: Value) -> Result<(), String> {
    let filename = store_filename(name)?;
    if value.is_null() {
        return Ok(());
    }
    let current = file_store_get(dir, name).unwrap_or_else(|_| json!({}));
    let final_val = if name == "lyrics-cache" {
        mark_manual_edits(&current, &value)
    } else {
        value
    };
    let Some(obj) = final_val.as_object() else {
        return Err("store value must be a JSON object".into());
    };
    let mut merged = current.as_object().cloned().unwrap_or_default();
    for (k, v) in obj {
        merged.insert(k.clone(), v.clone());
    }
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&merged).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(filename), bytes).map_err(|e| e.to_string())?;
    Ok(())
}

#[cfg(test)]
pub fn file_store_clear(dir: &std::path::Path, name: &str) -> Result<bool, String> {
    let filename = store_filename(name)?;
    let defaults = default_store_value(name);
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(&defaults).map_err(|e| e.to_string())?;
    std::fs::write(dir.join(filename), bytes).map_err(|e| e.to_string())?;
    Ok(true)
}
