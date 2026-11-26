use anyhow::{Context, Result, ensure};
use serde_json::{Map, Value, json};
use std::fs::{self, File, OpenOptions};
use std::io::{ErrorKind, Write};
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use crate::AppSettings;

const APP_ID: &str = "com.biu.wood3n";
const KNOWN_STORES: &[&str] = &[
    "app-settings",
    "user-login-info",
    "lyrics-cache",
    "media-downloads",
    "shortcut-settings",
    "desktop-lyrics",
    "search-history",
    "play-queue",
];

pub fn data_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("BIU_DATA_DIR").filter(|path| !path.is_empty()) {
        return Ok(PathBuf::from(path));
    }
    Ok(dirs::data_dir()
        .context("OS application data directory unavailable")?
        .join(APP_ID))
}

pub fn default_app_settings_json() -> Value {
    json!({ "appSettings": AppSettings::default() })
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
        let old_obj = prev_obj
            .and_then(|p| p.get(key))
            .and_then(|v| v.as_object());
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

#[derive(Debug, Clone)]
pub struct Store {
    directory: PathBuf,
}

// All Store instances use the same lock for read-modify-write transactions in this process.
static WRITE_LOCK: Mutex<()> = Mutex::new(());

impl Store {
    pub fn open() -> Result<Self> {
        Ok(Self::new(data_dir()?))
    }

    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    pub fn directory(&self) -> &Path {
        &self.directory
    }

    fn path(&self, name: &str) -> Result<PathBuf> {
        ensure!(KNOWN_STORES.contains(&name), "unknown store name: {name}");
        Ok(self.directory.join(format!("{name}.json")))
    }

    pub fn read(&self, name: &str) -> Result<Value> {
        let path = self.path(name)?;
        let mut value = default_store_value(name);
        match fs::read(&path) {
            Ok(bytes) => {
                let disk: Map<String, Value> = serde_json::from_slice(&bytes)
                    .with_context(|| format!("read {}", path.display()))?;
                value
                    .as_object_mut()
                    .expect("store defaults are objects")
                    .extend(disk);
            }
            Err(err) if err.kind() == ErrorKind::NotFound => {}
            Err(err) => return Err(err).with_context(|| format!("read {}", path.display())),
        }
        Ok(value)
    }

    pub fn write(&self, name: &str, value: Value) -> Result<()> {
        self.merge(name, value, true)
    }

    pub fn merge(&self, name: &str, value: Value, mark_manual: bool) -> Result<()> {
        let path = self.path(name)?;
        if value.is_null() {
            return Ok(());
        }
        ensure!(value.is_object(), "store value must be a JSON object");
        let _lock = WRITE_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let mut current = self.read(name)?;
        let updates = if mark_manual && name == "lyrics-cache" {
            mark_manual_edits(&current, &value)
        } else {
            value
        };
        current
            .as_object_mut()
            .unwrap()
            .extend(updates.as_object().unwrap().clone());
        write_json(&path, &current)
    }

    /// Edit a store atomically, including removal of top-level keys.
    pub fn update<T>(
        &self,
        name: &str,
        edit: impl FnOnce(&mut Map<String, Value>) -> Result<T>,
    ) -> Result<T> {
        let path = self.path(name)?;
        let _lock = WRITE_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        let mut value = self.read(name)?;
        let result = edit(value.as_object_mut().unwrap())?;
        write_json(&path, &value)?;
        Ok(result)
    }

    pub fn clear(&self, name: &str) -> Result<()> {
        let path = self.path(name)?;
        let _lock = WRITE_LOCK.lock().unwrap_or_else(|err| err.into_inner());
        write_json(&path, &default_store_value(name))
    }

    pub fn app_settings(&self) -> Result<AppSettings> {
        Ok(serde_json::from_value(
            self.read("app-settings")?["appSettings"].clone(),
        )?)
    }

    pub fn save_app_settings(&self, settings: &AppSettings) -> Result<()> {
        self.write("app-settings", json!({ "appSettings": settings }))
    }
}

fn write_json(path: &Path, value: &Value) -> Result<()> {
    atomic_write(path, |file| {
        serde_json::to_writer_pretty(&mut *file, value)?;
        file.write_all(b"\n")?;
        Ok(())
    })
}

pub(crate) fn atomic_write(path: &Path, write: impl FnOnce(&mut File) -> Result<()>) -> Result<()> {
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    fs::create_dir_all(parent)?;
    let temp = parent.join(format!(".{}.part", uuid::Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temp)?;
    let result = (|| {
        write(&mut file)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temp, path)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(temp);
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_and_preserves_tauri_settings_with_null_and_fractional_values() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path());
        fs::write(
            dir.path().join("app-settings.json"),
            r##"{
                "appSettings": {
                    "closeWindowOption": "exit",
                    "displayMode": "card",
                    "ffmpegPath": null,
                    "reportPlayHistory": null,
                    "sideMenuWidth": 213.5,
                    "borderRadius": 8.0,
                    "desktopLyricsFontSize": 42.0,
                    "proxySettings": { "type": "http", "host": "localhost", "port": 8080.0 },
                    "sideMenuCollectionFolded": { "created": true, "collected": null },
                    "desktopLyricsStyle": {
                        "lineMode": "single",
                        "colorPreset": "custom",
                        "customPlayedColor": "#ffba08",
                        "effect": "shadow",
                        "opacity": 0.85,
                        "align": null,
                        "showTranslation": false
                    },
                    "futureSetting": { "optional": null }
                }
            }"##,
        )?;
        let settings = store.app_settings()?;
        assert_eq!(settings.close_window_option, "exit");
        assert_eq!(settings.display_mode, "card");
        assert_eq!(settings.ffmpeg_path, "");
        assert!(settings.report_play_history);
        assert_eq!(settings.side_menu_width, 213);
        assert_eq!(settings.border_radius, 8);
        assert_eq!(settings.desktop_lyrics_font_size, 42);
        assert_eq!(settings.proxy_settings.port, Some(8080));
        assert!(settings.side_menu_collection_folded.created);
        assert_eq!(settings.desktop_lyrics_style.align, "center");
        assert!(!settings.desktop_lyrics_style.show_translation);
        store.save_app_settings(&settings)?;
        let disk = store.read("app-settings")?;
        let settings = &disk["appSettings"];
        assert_eq!(settings["closeWindowOption"], "exit");
        assert_eq!(settings["displayMode"], "card");
        assert_eq!(settings["futureSetting"], json!({ "optional": null }));
        let style = &settings["desktopLyricsStyle"];
        for (key, value) in [
            ("lineMode", json!("single")),
            ("colorPreset", json!("custom")),
            ("customPlayedColor", json!("#ffba08")),
            ("effect", json!("shadow")),
            ("opacity", json!(0.85)),
        ] {
            assert_eq!(style[key], value);
        }
        Ok(())
    }

    #[test]
    fn existing_tauri_settings_and_manual_lyrics_round_trip() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path());
        fs::write(
            dir.path().join("app-settings.json"),
            r##"{
          "appSettings":{"audioQuality":"low","primaryColor":"#ff0000","futureField":12},
          "window":{"width":900}
        }"##,
        )?;
        let mut settings = store.app_settings()?;
        assert_eq!(settings.audio_quality, crate::AudioQuality::Low);
        assert!(settings.desktop_lyrics_style.karaoke);
        settings.border_radius = 12;
        store.save_app_settings(&settings)?;
        let disk = store.read("app-settings")?;
        assert_eq!(disk["window"]["width"], 900);
        assert_eq!(disk["appSettings"]["futureField"], 12);
        assert_eq!(disk["appSettings"]["borderRadius"], 12);
        store.merge(
            "lyrics-cache",
            json!({"BV:2":{"lyrics":"old","source":"auto"}}),
            false,
        )?;
        store.write(
            "lyrics-cache",
            json!({"BV:2":{"lyrics":"new","source":"auto"}}),
        )?;
        assert_eq!(store.read("lyrics-cache")?["BV:2"]["source"], "manual");
        Ok(())
    }
}
