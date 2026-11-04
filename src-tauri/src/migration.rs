use crate::bili::BiliState;
use crate::store::{default_store_value, file_store_get, store_filename};
use hmac::{Hmac, Mac};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha512;
use std::collections::HashMap;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use tauri::{AppHandle, Runtime};

pub const MIGRATION_MARKER_FILE: &str = "migration.json";
pub const ELECTRON_EXPORT_FILE: &str = "tauri-migration.json";
/// Renderer `localStorage` from the bridge export, applied by the webview on its next start.
pub const LOCAL_STORAGE_FILE: &str = "migrated-local-storage.json";
/// Incomplete migrations are retried on later starts, at most this many times.
const MAX_MIGRATION_ATTEMPTS: u32 = 5;
pub const USER_LOGIN_INFO_ENCRYPTION_KEY: &str = "user-login-info";

const MIGRATABLE_STORES: &[&str] = &[
    "app-settings",
    "user-login-info",
    "media-downloads",
    "shortcut-settings",
    "lyrics-cache",
    "desktop-lyrics",
];

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ExportedCookie {
    pub name: String,
    pub value: String,
    #[serde(default)]
    pub domain: String,
    #[serde(default)]
    pub path: String,
    #[serde(rename = "expirationDate")]
    pub expiration_date: Option<f64>,
    #[serde(default)]
    pub secure: bool,
    #[serde(default, rename = "httpOnly")]
    pub http_only: bool,
    #[serde(rename = "sameSite")]
    pub same_site: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TauriMigrationExport {
    pub version: u32,
    #[serde(default, rename = "exportedAt")]
    pub exported_at: Option<u64>,
    #[serde(default, rename = "appVersion")]
    pub app_version: Option<String>,
    #[serde(default)]
    pub stores: HashMap<String, Value>,
    #[serde(default)]
    pub cookies: Vec<ExportedCookie>,
    #[serde(default, rename = "localStorage")]
    pub local_storage: Option<serde_json::Map<String, Value>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct MigrationMarker {
    pub version: u32,
    #[serde(rename = "migratedAt")]
    pub migrated_at: u64,
    pub source: String,
    #[serde(rename = "sourcePath")]
    pub source_path: Option<String>,
    #[serde(rename = "importedStores")]
    pub imported_stores: Vec<String>,
    #[serde(rename = "importedCookies")]
    pub imported_cookies: usize,
    /// False while some item failed; those items are listed in `pending` and retried.
    #[serde(default = "default_true")]
    pub complete: bool,
    #[serde(default)]
    pub pending: Vec<String>,
    /// Items already imported (or with nothing to import); retries skip them.
    #[serde(default)]
    pub done: Vec<String>,
    #[serde(default)]
    pub attempts: u32,
}

fn default_true() -> bool {
    true
}

pub fn candidate_electron_user_data_dirs() -> Vec<PathBuf> {
    let mut base_dirs: Vec<PathBuf> = Vec::new();

    #[cfg(target_os = "macos")]
    {
        if let Some(home) = std::env::var_os("HOME")
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
        {
            base_dirs.push(home.join("Library").join("Application Support"));
        }
        if let Some(cfg) = dirs::config_dir() {
            if !base_dirs.contains(&cfg) {
                base_dirs.push(cfg);
            }
        }
    }

    #[cfg(target_os = "windows")]
    {
        if let Some(appdata) = std::env::var_os("APPDATA")
            .map(PathBuf::from)
            .or_else(dirs::config_dir)
        {
            base_dirs.push(appdata);
        }
    }

    #[cfg(not(any(target_os = "macos", target_os = "windows")))]
    {
        if let Some(xdg) = std::env::var_os("XDG_CONFIG_HOME")
            .filter(|s| !s.is_empty())
            .map(PathBuf::from)
        {
            base_dirs.push(xdg);
        }
        if let Some(home) = std::env::var_os("HOME")
            .map(PathBuf::from)
            .or_else(dirs::home_dir)
        {
            let dot_config = home.join(".config");
            if !base_dirs.contains(&dot_config) {
                base_dirs.push(dot_config);
            }
        }
        if let Some(cfg) = dirs::config_dir() {
            if !base_dirs.contains(&cfg) {
                base_dirs.push(cfg);
            }
        }
    }

    let mut candidates = Vec::new();
    for base in base_dirs {
        for name in ["Biu", "biu"] {
            let candidate = base.join(name);
            if !candidates.contains(&candidate) {
                candidates.push(candidate);
            }
        }
    }
    candidates
}

fn has_migratable_electron_files(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    if dir.join(ELECTRON_EXPORT_FILE).is_file() {
        return true;
    }
    MIGRATABLE_STORES.iter().any(|name| {
        store_filename(name)
            .map(|f| dir.join(f).is_file())
            .unwrap_or(false)
    })
}

pub fn locate_electron_user_data_dir(candidates: &[PathBuf]) -> Option<PathBuf> {
    if let Some(found) = candidates
        .iter()
        .find(|d| d.join(ELECTRON_EXPORT_FILE).is_file())
    {
        return Some(found.clone());
    }
    candidates
        .iter()
        .find(|d| has_migratable_electron_files(d))
        .cloned()
}

// --- Pure Rust AES-256-CBC + PBKDF2-HMAC-SHA512 for conf / electron-store ---

const AES_SBOX: [u8; 256] = [
    0x63, 0x7c, 0x77, 0x7b, 0xf2, 0x6b, 0x6f, 0xc5, 0x30, 0x01, 0x67, 0x2b, 0xfe, 0xd7, 0xab, 0x76,
    0xca, 0x82, 0xc9, 0x7d, 0xfa, 0x59, 0x47, 0xf0, 0xad, 0xd4, 0xa2, 0xaf, 0x9c, 0xa4, 0x72, 0xc0,
    0xb7, 0xfd, 0x93, 0x26, 0x36, 0x3f, 0xf7, 0xcc, 0x34, 0xa5, 0xe5, 0xf1, 0x71, 0xd8, 0x31, 0x15,
    0x04, 0xc7, 0x23, 0xc3, 0x18, 0x96, 0x05, 0x9a, 0x07, 0x12, 0x80, 0xe2, 0xeb, 0x27, 0xb2, 0x75,
    0x09, 0x83, 0x2c, 0x1a, 0x1b, 0x6e, 0x5a, 0xa0, 0x52, 0x3b, 0xd6, 0xb3, 0x29, 0xe3, 0x2f, 0x84,
    0x53, 0xd1, 0x00, 0xed, 0x20, 0xfc, 0xb1, 0x5b, 0x6a, 0xcb, 0xbe, 0x39, 0x4a, 0x4c, 0x58, 0xcf,
    0xd0, 0xef, 0xaa, 0xfb, 0x43, 0x4d, 0x33, 0x85, 0x45, 0xf9, 0x02, 0x7f, 0x50, 0x3c, 0x9f, 0xa8,
    0x51, 0xa3, 0x40, 0x8f, 0x92, 0x9d, 0x38, 0xf5, 0xbc, 0xb6, 0xda, 0x21, 0x10, 0xff, 0xf3, 0xd2,
    0xcd, 0x0c, 0x13, 0xec, 0x5f, 0x97, 0x44, 0x17, 0xc4, 0xa7, 0x7e, 0x3d, 0x64, 0x5d, 0x19, 0x73,
    0x60, 0x81, 0x4f, 0xdc, 0x22, 0x2a, 0x90, 0x88, 0x46, 0xee, 0xb8, 0x14, 0xde, 0x5e, 0x0b, 0xdb,
    0xe0, 0x32, 0x3a, 0x0a, 0x49, 0x06, 0x24, 0x5c, 0xc2, 0xd3, 0xac, 0x62, 0x91, 0x95, 0xe4, 0x79,
    0xe7, 0xc8, 0x37, 0x6d, 0x8d, 0xd5, 0x4e, 0xa9, 0x6c, 0x56, 0xf4, 0xea, 0x65, 0x7a, 0xae, 0x08,
    0xba, 0x78, 0x25, 0x2e, 0x1c, 0xa6, 0xb4, 0xc6, 0xe8, 0xdd, 0x74, 0x1f, 0x4b, 0xbd, 0x8b, 0x8a,
    0x70, 0x3e, 0xb5, 0x66, 0x48, 0x03, 0xf6, 0x0e, 0x61, 0x35, 0x57, 0xb9, 0x86, 0xc1, 0x1d, 0x9e,
    0xe1, 0xf8, 0x98, 0x11, 0x69, 0xd9, 0x8e, 0x94, 0x9b, 0x1e, 0x87, 0xe9, 0xce, 0x55, 0x28, 0xdf,
    0x8c, 0xa1, 0x89, 0x0d, 0xbf, 0xe6, 0x42, 0x68, 0x41, 0x99, 0x2d, 0x0f, 0xb0, 0x54, 0xbb, 0x16,
];

const AES_INV_SBOX: [u8; 256] = [
    0x52, 0x09, 0x6a, 0xd5, 0x30, 0x36, 0xa5, 0x38, 0xbf, 0x40, 0xa3, 0x9e, 0x81, 0xf3, 0xd7, 0xfb,
    0x7c, 0xe3, 0x39, 0x82, 0x9b, 0x2f, 0xff, 0x87, 0x34, 0x8e, 0x43, 0x44, 0xc4, 0xde, 0xe9, 0xcb,
    0x54, 0x7b, 0x94, 0x32, 0xa6, 0xc2, 0x23, 0x3d, 0xee, 0x4c, 0x95, 0x0b, 0x42, 0xfa, 0xc3, 0x4e,
    0x08, 0x2e, 0xa1, 0x66, 0x28, 0xd9, 0x24, 0xb2, 0x76, 0x5b, 0xa2, 0x49, 0x6d, 0x8b, 0xd1, 0x25,
    0x72, 0xf8, 0xf6, 0x64, 0x86, 0x68, 0x98, 0x16, 0xd4, 0xa4, 0x5c, 0xcc, 0x5d, 0x65, 0xb6, 0x92,
    0x6c, 0x70, 0x48, 0x50, 0xfd, 0xed, 0xb9, 0xda, 0x5e, 0x15, 0x46, 0x57, 0xa7, 0x8d, 0x9d, 0x84,
    0x90, 0xd8, 0xab, 0x00, 0x8c, 0xbc, 0xd3, 0x0a, 0xf7, 0xe4, 0x58, 0x05, 0xb8, 0xb3, 0x45, 0x06,
    0xd0, 0x2c, 0x1e, 0x8f, 0xca, 0x3f, 0x0f, 0x02, 0xc1, 0xaf, 0xbd, 0x03, 0x01, 0x13, 0x8a, 0x6b,
    0x3a, 0x91, 0x11, 0x41, 0x4f, 0x67, 0xdc, 0xea, 0x97, 0xf2, 0xcf, 0xce, 0xf0, 0xb4, 0xe6, 0x73,
    0x96, 0xac, 0x74, 0x22, 0xe7, 0xad, 0x35, 0x85, 0xe2, 0xf9, 0x37, 0xe8, 0x1c, 0x75, 0xdf, 0x6e,
    0x47, 0xf1, 0x1a, 0x71, 0x1d, 0x29, 0xc5, 0x89, 0x6f, 0xb7, 0x62, 0x0e, 0xaa, 0x18, 0xbe, 0x1b,
    0xfc, 0x56, 0x3e, 0x4b, 0xc6, 0xd2, 0x79, 0x20, 0x9a, 0xdb, 0xc0, 0xfe, 0x78, 0xcd, 0x5a, 0xf4,
    0x1f, 0xdd, 0xa8, 0x33, 0x88, 0x07, 0xc7, 0x31, 0xb1, 0x12, 0x10, 0x59, 0x27, 0x80, 0xec, 0x5f,
    0x60, 0x51, 0x7f, 0xa9, 0x19, 0xb5, 0x4a, 0x0d, 0x2d, 0xe5, 0x7a, 0x9f, 0x93, 0xc9, 0x9c, 0xef,
    0xa0, 0xe0, 0x3b, 0x4d, 0xae, 0x2a, 0xf5, 0xb0, 0xc8, 0xeb, 0xbb, 0x3c, 0x83, 0x53, 0x99, 0x61,
    0x17, 0x2b, 0x04, 0x7e, 0xba, 0x77, 0xd6, 0x26, 0xe1, 0x69, 0x14, 0x63, 0x55, 0x21, 0x0c, 0x7d,
];

const RCON: [u8; 10] = [0x01, 0x02, 0x04, 0x08, 0x10, 0x20, 0x40, 0x80, 0x1b, 0x36];

fn gmul(mut a: u8, mut b: u8) -> u8 {
    let mut p = 0u8;
    for _ in 0..8 {
        if (b & 1) != 0 {
            p ^= a;
        }
        let hi = (a & 0x80) != 0;
        a <<= 1;
        if hi {
            a ^= 0x1b;
        }
        b >>= 1;
    }
    p
}

fn aes256_expand_key(key: &[u8; 32]) -> [[u8; 16]; 15] {
    let mut w = [[0u8; 4]; 60];
    for i in 0..8 {
        w[i] = [key[4 * i], key[4 * i + 1], key[4 * i + 2], key[4 * i + 3]];
    }
    for i in 8..60 {
        let mut temp = w[i - 1];
        if i % 8 == 0 {
            temp = [
                AES_SBOX[temp[1] as usize] ^ RCON[(i / 8) - 1],
                AES_SBOX[temp[2] as usize],
                AES_SBOX[temp[3] as usize],
                AES_SBOX[temp[0] as usize],
            ];
        } else if i % 8 == 4 {
            temp = [
                AES_SBOX[temp[0] as usize],
                AES_SBOX[temp[1] as usize],
                AES_SBOX[temp[2] as usize],
                AES_SBOX[temp[3] as usize],
            ];
        }
        let prev = w[i - 8];
        w[i] = [
            prev[0] ^ temp[0],
            prev[1] ^ temp[1],
            prev[2] ^ temp[2],
            prev[3] ^ temp[3],
        ];
    }
    let mut round_keys = [[0u8; 16]; 15];
    for r in 0..15 {
        for c in 0..4 {
            round_keys[r][4 * c..4 * c + 4].copy_from_slice(&w[4 * r + c]);
        }
    }
    round_keys
}

fn add_round_key(state: &mut [u8; 16], rk: &[u8; 16]) {
    for (s, k) in state.iter_mut().zip(rk.iter()) {
        *s ^= *k;
    }
}

fn inv_shift_rows(s: &mut [u8; 16]) {
    // Row 1: shift right by 1
    let t = s[13];
    s[13] = s[9];
    s[9] = s[5];
    s[5] = s[1];
    s[1] = t;
    // Row 2: shift right by 2
    s.swap(2, 10);
    s.swap(6, 14);
    // Row 3: shift right by 3 (left by 1)
    let t = s[3];
    s[3] = s[7];
    s[7] = s[11];
    s[11] = s[15];
    s[15] = t;
}

fn inv_sub_bytes(s: &mut [u8; 16]) {
    for b in s.iter_mut() {
        *b = AES_INV_SBOX[*b as usize];
    }
}

fn inv_mix_columns(s: &mut [u8; 16]) {
    for c in 0..4 {
        let i = 4 * c;
        let (a0, a1, a2, a3) = (s[i], s[i + 1], s[i + 2], s[i + 3]);
        s[i] = gmul(a0, 0x0e) ^ gmul(a1, 0x0b) ^ gmul(a2, 0x0d) ^ gmul(a3, 0x09);
        s[i + 1] = gmul(a0, 0x09) ^ gmul(a1, 0x0e) ^ gmul(a2, 0x0b) ^ gmul(a3, 0x0d);
        s[i + 2] = gmul(a0, 0x0d) ^ gmul(a1, 0x09) ^ gmul(a2, 0x0e) ^ gmul(a3, 0x0b);
        s[i + 3] = gmul(a0, 0x0b) ^ gmul(a1, 0x0d) ^ gmul(a2, 0x09) ^ gmul(a3, 0x0e);
    }
}

fn aes256_decrypt_block(block: &[u8; 16], rks: &[[u8; 16]; 15]) -> [u8; 16] {
    let mut state = *block;
    add_round_key(&mut state, &rks[14]);
    for r in (1..14).rev() {
        inv_shift_rows(&mut state);
        inv_sub_bytes(&mut state);
        add_round_key(&mut state, &rks[r]);
        inv_mix_columns(&mut state);
    }
    inv_shift_rows(&mut state);
    inv_sub_bytes(&mut state);
    add_round_key(&mut state, &rks[0]);
    state
}

fn pbkdf2_hmac_sha512_32(password: &[u8], salt: &[u8], iterations: u32) -> [u8; 32] {
    type HmacSha512 = Hmac<Sha512>;
    let mut mac = HmacSha512::new_from_slice(password).expect("HMAC accepts any key size");
    mac.update(salt);
    mac.update(&1u32.to_be_bytes());
    let mut u = mac.finalize().into_bytes();
    let mut t = u;
    for _ in 1..iterations {
        let mut mac = HmacSha512::new_from_slice(password).expect("HMAC accepts any key size");
        mac.update(&u);
        u = mac.finalize().into_bytes();
        for (ti, ui) in t.iter_mut().zip(u.iter()) {
            *ti ^= *ui;
        }
    }
    let mut out = [0u8; 32];
    out.copy_from_slice(&t[..32]);
    out
}

/// Decrypts an `electron-store` / `conf` encrypted config file.
///
/// Format written by `conf`:
/// `[16-byte random IV][0x3a (':')][AES-256-CBC ciphertext with PKCS#7 padding]`
/// where key = `crypto.pbkdf2Sync(encryptionKey, iv.toString("utf8"), 10_000, 32, "sha512")`.
/// Notice `iv.toString()` in Node.js converts the 16 raw bytes using UTF-8 with U+FFFD replacement!
pub fn decrypt_conf_store(raw: &[u8], encryption_key: &str) -> Result<Value, String> {
    // Also support plain JSON in case user-login-info.json wasn't encrypted
    if let Ok(plain) = serde_json::from_slice::<Value>(raw) {
        if plain.is_object() {
            return Ok(plain);
        }
    }
    if raw.len() < 17 || raw[16] != b':' {
        return Err("invalid conf encrypted header".into());
    }
    let iv: [u8; 16] = raw[..16].try_into().unwrap();
    let ciphertext = &raw[17..];
    if ciphertext.is_empty() || !ciphertext.len().is_multiple_of(16) {
        return Err("invalid ciphertext length".into());
    }
    let salt_utf8 = String::from_utf8_lossy(&iv);
    let key = pbkdf2_hmac_sha512_32(encryption_key.as_bytes(), salt_utf8.as_bytes(), 10_000);
    let rks = aes256_expand_key(&key);

    let mut plain = Vec::with_capacity(ciphertext.len());
    let mut prev_block = iv;
    for block in ciphertext.as_chunks::<16>().0 {
        let mut dec = aes256_decrypt_block(block, &rks);
        for (d, p) in dec.iter_mut().zip(prev_block.iter()) {
            *d ^= *p;
        }
        plain.extend_from_slice(&dec);
        prev_block = *block;
    }

    let pad_len = *plain.last().ok_or("empty plaintext")? as usize;
    if pad_len == 0 || pad_len > 16 || pad_len > plain.len() {
        return Err("invalid PKCS#7 padding".into());
    }
    if !plain[plain.len() - pad_len..]
        .iter()
        .all(|&b| b as usize == pad_len)
    {
        return Err("corrupted PKCS#7 padding bytes".into());
    }
    plain.truncate(plain.len() - pad_len);

    let val: Value =
        serde_json::from_slice(&plain).map_err(|e| format!("invalid decrypted JSON: {e}"))?;
    if !val.is_object() {
        return Err("decrypted store root is not a JSON object".into());
    }
    Ok(val)
}

/// Normalizes Electron store data to Tauri's store structure and merges into `existing` without
/// overwriting user data that already exists on the Tauri side.
pub fn merge_store_without_overwriting(
    name: &str,
    existing_disk: Option<&Value>,
    incoming: &Value,
) -> Option<Value> {
    let incoming_obj = incoming.as_object()?;
    if incoming_obj.is_empty() {
        return None;
    }

    match name {
        "app-settings" => {
            let inc_settings = incoming_obj
                .get("appSettings")
                .and_then(|v| v.as_object())
                .unwrap_or(incoming_obj);
            if inc_settings.is_empty() {
                return None;
            }
            if let Some(existing_obj) = existing_disk.and_then(|v| v.as_object()) {
                if let Some(existing_settings) =
                    existing_obj.get("appSettings").and_then(|v| v.as_object())
                {
                    // Tauri already has appSettings on disk: only fill missing keys
                    let mut merged_settings = existing_settings.clone();
                    let mut changed = false;
                    for (k, v) in inc_settings {
                        if !merged_settings.contains_key(k) {
                            merged_settings.insert(k.clone(), v.clone());
                            changed = true;
                        }
                    }
                    if !changed {
                        return None;
                    }
                    let mut out = existing_obj.clone();
                    out.insert("appSettings".to_string(), Value::Object(merged_settings));
                    return Some(Value::Object(out));
                }
            }
            // No existing appSettings on disk: start from defaults and overlay incoming
            let mut base = default_store_value("app-settings");
            if let Some(base_settings) = base
                .as_object_mut()
                .and_then(|o| o.get_mut("appSettings"))
                .and_then(|v| v.as_object_mut())
            {
                for (k, v) in inc_settings {
                    base_settings.insert(k.clone(), v.clone());
                }
            }
            Some(base)
        }
        "user-login-info" => {
            if let Some(existing_obj) = existing_disk.and_then(|v| v.as_object()) {
                let already_logged_in = existing_obj
                    .get("isLogin")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false)
                    || existing_obj.get("mid").is_some();
                if already_logged_in {
                    return None;
                }
            }
            let mut cleaned = incoming_obj.clone();
            cleaned.remove("__internal__");
            if cleaned.is_empty() {
                None
            } else {
                Some(Value::Object(cleaned))
            }
        }
        "media-downloads" => {
            let mut normalized = serde_json::Map::new();
            for (k, v) in incoming_obj {
                if k == "__internal__" {
                    continue;
                }
                if k == "downloads" {
                    if let Some(nested) = v.as_object() {
                        for (task_id, task_val) in nested {
                            normalized.insert(format!("downloads.{task_id}"), task_val.clone());
                        }
                    }
                } else if k.starts_with("downloads.") {
                    normalized.insert(k.clone(), v.clone());
                } else if v.is_object() {
                    normalized.insert(format!("downloads.{k}"), v.clone());
                }
            }
            if normalized.is_empty() {
                return None;
            }
            let mut out = existing_disk
                .and_then(|v| v.as_object())
                .cloned()
                .unwrap_or_default();
            // Check existing nested "downloads" object on disk as well
            let existing_nested_ids: std::collections::HashSet<String> = out
                .get("downloads")
                .and_then(|v| v.as_object())
                .map(|m| m.keys().map(|id| format!("downloads.{id}")).collect())
                .unwrap_or_default();
            let mut changed = false;
            for (k, v) in normalized {
                if !out.contains_key(&k) && !existing_nested_ids.contains(&k) {
                    out.insert(k, v);
                    changed = true;
                }
            }
            if changed {
                Some(Value::Object(out))
            } else {
                None
            }
        }
        _ => {
            let mut cleaned = incoming_obj.clone();
            cleaned.remove("__internal__");
            if cleaned.is_empty() {
                return None;
            }
            if let Some(existing_obj) = existing_disk.and_then(|v| v.as_object()) {
                if !existing_obj.is_empty() {
                    let mut out = existing_obj.clone();
                    let mut changed = false;
                    for (k, v) in cleaned {
                        if !out.contains_key(&k) {
                            out.insert(k, v);
                            changed = true;
                        }
                    }
                    return if changed {
                        Some(Value::Object(out))
                    } else {
                        None
                    };
                }
            }
            Some(Value::Object(cleaned))
        }
    }
}

fn read_raw_disk_store(tauri_data_dir: &Path, name: &str) -> Option<Value> {
    let filename = store_filename(name).ok()?;
    let path = tauri_data_dir.join(filename);
    if !path.is_file() {
        return None;
    }
    let bytes = fs::read(path).ok()?;
    serde_json::from_slice::<Value>(&bytes).ok()
}

/// Reads one Electron store file: `Ok(None)` when it does not exist, `Err` when it exists
/// but cannot be read, parsed or decrypted.
fn read_raw_electron_store(electron_dir: &Path, store_name: &str) -> Result<Option<Value>, String> {
    let filename = store_filename(store_name)?;
    let src_file = electron_dir.join(&filename);
    if !src_file.is_file() {
        return Ok(None);
    }
    let raw_bytes = fs::read(&src_file).map_err(|e| format!("read {filename}: {e}"))?;
    if store_name == "user-login-info" {
        decrypt_conf_store(&raw_bytes, USER_LOGIN_INFO_ENCRYPTION_KEY)
            .map(Some)
            .map_err(|e| format!("decrypt {filename}: {e}"))
    } else {
        serde_json::from_slice::<Value>(&raw_bytes)
            .map(Some)
            .map_err(|e| format!("parse {filename}: {e}"))
    }
}

fn is_bilibili_domain(domain: &str) -> bool {
    let trimmed = domain.trim().trim_start_matches('.').to_ascii_lowercase();
    trimmed == "bilibili.com" || trimmed.ends_with(".bilibili.com")
}

/// Builds a `Set-Cookie` string and the URL it is set from, keeping the Electron cookie's
/// domain (host-only when it has no leading dot), path, Secure, HttpOnly and SameSite.
fn cookie_set_string(c: &ExportedCookie, now_secs: i64) -> Option<(String, url::Url)> {
    let host = c.domain.trim().trim_start_matches('.').to_ascii_lowercase();
    let host_only = !c.domain.trim().starts_with('.');
    let path = if c.path.starts_with('/') {
        c.path.as_str()
    } else {
        "/"
    };
    let url = url::Url::parse(&format!("https://{host}{path}")).ok()?;
    let mut raw = format!("{}={}; Path={path}", c.name, c.value);
    if !host_only {
        raw.push_str(&format!("; Domain={host}"));
    }
    if c.secure {
        raw.push_str("; Secure");
    }
    if c.http_only {
        raw.push_str("; HttpOnly");
    }
    match c.same_site.as_deref() {
        Some("no_restriction") => raw.push_str("; SameSite=None"),
        Some("lax") => raw.push_str("; SameSite=Lax"),
        Some("strict") => raw.push_str("; SameSite=Strict"),
        _ => {}
    }
    if let Some(exp) = c.expiration_date.map(|f| f.floor() as i64) {
        raw.push_str(&format!("; Max-Age={}", (exp - now_secs).max(1)));
    }
    Some((raw, url))
}

/// Imports unexpired bilibili cookies that the Tauri store does not already hold under the
/// same (domain, path, name). Returns (imported, failed).
pub fn import_cookies_into_bili_state(
    bili_state: &BiliState,
    cookies: &[ExportedCookie],
) -> (usize, usize) {
    let now_secs = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64;

    let mut imported = 0usize;
    let mut failed = 0usize;
    {
        let mut store = bili_state.cookies.lock().unwrap();
        for c in cookies {
            if c.name.trim().is_empty() || !is_bilibili_domain(&c.domain) {
                continue;
            }
            if matches!(c.expiration_date, Some(exp) if (exp.floor() as i64) <= now_secs) {
                continue;
            }
            let domain = c.domain.trim().trim_start_matches('.').to_ascii_lowercase();
            let path = if c.path.starts_with('/') {
                c.path.as_str()
            } else {
                "/"
            };
            if store.contains(&domain, path, &c.name) {
                continue;
            }
            match cookie_set_string(c, now_secs).map(|(raw, url)| store.parse(&raw, &url)) {
                Some(Ok(_)) => imported += 1,
                _ => {
                    log::warn!(
                        "[migration] failed to import cookie {} for {domain}",
                        c.name
                    );
                    failed += 1;
                }
            }
        }
    }
    if imported > 0 {
        if let Err(e) = bili_state.save_cookies() {
            log::warn!("[migration] failed to save imported cookies: {e}");
            failed += imported;
            imported = 0;
        }
    }
    (imported, failed)
}

fn write_local_storage_file(
    tauri_data_dir: &Path,
    entries: &serde_json::Map<String, Value>,
) -> Result<(), String> {
    let strings: serde_json::Map<String, Value> = entries
        .iter()
        .filter(|(_, v)| v.is_string())
        .map(|(k, v)| (k.clone(), v.clone()))
        .collect();
    if strings.is_empty() {
        return Ok(());
    }
    write_private_json(tauri_data_dir, LOCAL_STORAGE_FILE, &Value::Object(strings))
}

/// Returns the migrated `localStorage` entries waiting to be applied, or an empty map.
pub fn read_migrated_local_storage(tauri_data_dir: &Path) -> serde_json::Map<String, Value> {
    fs::read(tauri_data_dir.join(LOCAL_STORAGE_FILE))
        .ok()
        .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
        .and_then(|v| v.as_object().cloned())
        .unwrap_or_default()
}

fn read_marker(tauri_data_dir: &Path) -> Option<MigrationMarker> {
    let bytes = fs::read(tauri_data_dir.join(MIGRATION_MARKER_FILE)).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn write_migration_marker(tauri_data_dir: &Path, marker: &MigrationMarker) -> Result<(), String> {
    let value = serde_json::to_value(marker).map_err(|e| e.to_string())?;
    write_private_json(tauri_data_dir, MIGRATION_MARKER_FILE, &value)
}

/// Atomically writes a 0600 JSON file into `dir`.
fn write_private_json(dir: &Path, name: &str, value: &Value) -> Result<(), String> {
    fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    let marker_path = dir.join(name);
    let tmp_path = dir.join(format!(".{name}.tmp"));
    let bytes = serde_json::to_vec_pretty(value).map_err(|e| e.to_string())?;
    {
        let mut opts = OpenOptions::new();
        opts.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            opts.mode(0o600);
        }
        let mut file = opts.open(&tmp_path).map_err(|e| e.to_string())?;
        file.write_all(&bytes).map_err(|e| e.to_string())?;
    }
    fs::rename(&tmp_path, &marker_path).map_err(|e| e.to_string())?;
    Ok(())
}

/// Core migration routine that operates on directories and a store writer closure.
/// Used both by Tauri startup (`AppHandle` store writer) and unit tests (`file_store_set`).
pub fn migrate_from_candidates_with<F>(
    tauri_data_dir: &Path,
    electron_candidates: &[PathBuf],
    bili_state: &BiliState,
    mut write_store: F,
) -> Option<MigrationMarker>
where
    F: FnMut(&str, Value) -> Result<(), String>,
{
    let previous = read_marker(tauri_data_dir);
    if let Some(prev) = &previous {
        if prev.complete || prev.attempts >= MAX_MIGRATION_ATTEMPTS {
            log::info!(
                "[migration] already done (complete={}, attempts={}), skipping",
                prev.complete,
                prev.attempts
            );
            return None;
        }
        log::info!(
            "[migration] retrying incomplete migration (pending={:?})",
            prev.pending
        );
    } else if tauri_data_dir.join(MIGRATION_MARKER_FILE).exists() {
        log::warn!("[migration] unreadable marker, skipping migration");
        return None;
    }
    let attempts = previous.as_ref().map_or(0, |p| p.attempts) + 1;
    // Items finished on an earlier attempt are never imported again, so a retry cannot bring
    // back data (cookies, tokens, cache) the user removed after the first run.
    let mut done: Vec<String> = previous
        .as_ref()
        .map(|p| p.done.clone())
        .unwrap_or_default();
    let mut pending: Vec<String> = Vec::new();
    let is_done = |done: &Vec<String>, item: &str| done.iter().any(|d| d == item);

    let now_ms = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64;

    let Some(electron_dir) = locate_electron_user_data_dir(electron_candidates) else {
        log::info!("[migration] no Electron userData directory found, skipping migration");
        return None;
    };

    log::info!(
        "[migration] found Electron userData directory at {:?}",
        electron_dir
    );

    let export_path = electron_dir.join(ELECTRON_EXPORT_FILE);
    let mut imported_stores = Vec::new();
    let mut imported_cookies = 0usize;
    let mut source_kind = "raw-stores".to_string();

    let export_payload = if export_path.is_file() {
        match fs::read(&export_path)
            .ok()
            .and_then(|bytes| serde_json::from_slice::<TauriMigrationExport>(&bytes).ok())
        {
            Some(payload) if payload.version == 1 => Some(payload),
            Some(payload) => {
                log::warn!(
                    "[migration] unsupported tauri-migration.json version {}, falling back to raw store files",
                    payload.version
                );
                None
            }
            None => {
                // The export exists but cannot be read now: import stores from the raw files
                // and keep cookies and localStorage pending for the next start.
                log::warn!(
                    "[migration] failed to read tauri-migration.json, falling back to raw store files"
                );
                pending.push("export".to_string());
                None
            }
        }
    } else {
        None
    };

    // One store: Some(value) to merge, None when the source has nothing, Err when unreadable.
    let mut import_store = |store_name: &str,
                            incoming: Result<Option<Value>, String>,
                            done: &mut Vec<String>,
                            pending: &mut Vec<String>,
                            imported: &mut Vec<String>| {
        match incoming {
            Err(e) => {
                log::warn!("[migration] cannot read store {store_name}: {e}");
                pending.push(store_name.to_string());
            }
            Ok(None) => done.push(store_name.to_string()),
            Ok(Some(incoming_val)) => {
                let existing_disk = read_raw_disk_store(tauri_data_dir, store_name);
                match merge_store_without_overwriting(
                    store_name,
                    existing_disk.as_ref(),
                    &incoming_val,
                ) {
                    None => done.push(store_name.to_string()),
                    Some(merged) => match write_store(store_name, merged) {
                        Ok(()) => {
                            imported.push(store_name.to_string());
                            done.push(store_name.to_string());
                        }
                        Err(e) => {
                            log::warn!("[migration] failed to write store {store_name}: {e}");
                            pending.push(store_name.to_string());
                        }
                    },
                }
            }
        }
    };

    if let Some(export) = export_payload {
        source_kind = "tauri-migration.json".to_string();
        for &store_name in MIGRATABLE_STORES {
            if is_done(&done, store_name) {
                continue;
            }
            let incoming = match export.stores.get(store_name) {
                Some(v) => Ok(Some(v.clone())),
                // desktop-lyrics is not in the export's StoreNameMap; read its own file.
                None if store_name == "desktop-lyrics" => {
                    read_raw_electron_store(&electron_dir, store_name)
                }
                None => Ok(None),
            };
            import_store(
                store_name,
                incoming,
                &mut done,
                &mut pending,
                &mut imported_stores,
            );
        }
        if !is_done(&done, "cookies") {
            let (cookies_ok, cookies_failed) =
                import_cookies_into_bili_state(bili_state, &export.cookies);
            imported_cookies = cookies_ok;
            if cookies_failed > 0 {
                pending.push("cookies".to_string());
            } else {
                done.push("cookies".to_string());
            }
        }
        if !is_done(&done, "localStorage") {
            let staged = match &export.local_storage {
                Some(entries) => write_local_storage_file(tauri_data_dir, entries),
                None => Ok(()),
            };
            match staged {
                Ok(()) => done.push("localStorage".to_string()),
                Err(e) => {
                    log::warn!("[migration] failed to stage localStorage: {e}");
                    pending.push("localStorage".to_string());
                }
            }
        }
    } else {
        for &store_name in MIGRATABLE_STORES {
            if is_done(&done, store_name) {
                continue;
            }
            let incoming = read_raw_electron_store(&electron_dir, store_name);
            import_store(
                store_name,
                incoming,
                &mut done,
                &mut pending,
                &mut imported_stores,
            );
        }
    }

    let marker = MigrationMarker {
        version: 1,
        migrated_at: now_ms,
        source: source_kind.clone(),
        source_path: Some(electron_dir.to_string_lossy().into_owned()),
        imported_stores: imported_stores.clone(),
        imported_cookies,
        complete: pending.is_empty(),
        pending,
        done,
        attempts,
    };

    if let Err(e) = write_migration_marker(tauri_data_dir, &marker) {
        log::warn!("[migration] failed to write migration marker: {e}");
    } else {
        log::info!(
            "[migration] finished from {} (stores={:?}, cookies={}, complete={}, pending={:?})",
            source_kind,
            imported_stores,
            imported_cookies,
            marker.complete,
            marker.pending
        );
    }

    Some(marker)
}

#[cfg(test)]
pub fn migrate_from_candidates_in_dir(
    tauri_data_dir: &Path,
    electron_candidates: &[PathBuf],
    bili_state: &BiliState,
) -> Option<MigrationMarker> {
    migrate_from_candidates_with(
        tauri_data_dir,
        electron_candidates,
        bili_state,
        |name, val| crate::store::file_store_write(tauri_data_dir, name, val, false),
    )
}

pub fn run_startup_migration<R: Runtime>(
    app: &AppHandle<R>,
    tauri_data_dir: &Path,
    bili_state: &BiliState,
) -> Option<MigrationMarker> {
    let candidates = candidate_electron_user_data_dirs();
    let marker =
        migrate_from_candidates_with(tauri_data_dir, &candidates, bili_state, |name, val| {
            crate::store::write_store(app.clone(), name.to_string(), val, false)
        });
    if let Some(ref m) = marker {
        if m.imported_stores.iter().any(|s| s == "app-settings") {
            if let Ok(settings_val) = file_store_get(tauri_data_dir, "app-settings") {
                crate::store::sync_allowed_local_dirs_from_value(bili_state, &settings_val);
                let proxy = settings_val
                    .get("appSettings")
                    .and_then(|s| s.get("proxySettings"))
                    .cloned();
                let parsed_proxy = crate::bili::parse_proxy_settings_value(proxy);
                let _ = bili_state.update_proxy(parsed_proxy);
            }
        }
    }
    marker
}

fn app_data_dir<R: Runtime>(app: &AppHandle<R>) -> Result<PathBuf, String> {
    use tauri::Manager;
    app.path().app_data_dir().map_err(|e| e.to_string())
}

/// Migrated `localStorage` entries for the webview to apply before its stores load.
#[tauri::command]
pub fn migration_local_storage<R: Runtime>(
    app: AppHandle<R>,
) -> Result<serde_json::Map<String, Value>, String> {
    Ok(read_migrated_local_storage(&app_data_dir(&app)?))
}

/// Called by the webview once it has written the migrated entries.
#[tauri::command]
pub fn migration_local_storage_done<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    match fs::remove_file(app_data_dir(&app)?.join(LOCAL_STORAGE_FILE)) {
        Ok(()) => Ok(()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(e) => Err(e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::store::{file_store_set, file_store_write};
    use serde_json::json;
    use std::process::Command;

    #[test]
    fn migrate_from_tauri_migration_json_and_skip_expired_cookies_and_idempotent() {
        let root = std::env::temp_dir().join(format!("biu-mig-export-{}", uuid::Uuid::new_v4()));
        let electron_dir = root.join("electron-Biu");
        let tauri_dir = root.join("tauri-data");
        fs::create_dir_all(&electron_dir).unwrap();
        fs::create_dir_all(&tauri_dir).unwrap();

        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let export_json = json!({
            "version": 1,
            "exportedAt": 1730000000000u64,
            "appVersion": "1.16.1-autolyrics.7",
            "stores": {
                "app-settings": {
                    "appSettings": {
                        "primaryColor": "#fb7299",
                        "downloadPath": "/custom/downloads/path",
                        "lyricsFontSize": 28
                    }
                },
                "user-login-info": {
                    "isLogin": true,
                    "mid": 888888,
                    "uname": "migrated_tester"
                },
                "shortcut-settings": {
                    "enableGlobalShortcuts": false
                },
                "media-downloads": {
                    "downloads": {
                        "task-1": {
                            "id": "task-1",
                            "outputFileType": "m4a",
                            "title": "Test Song",
                            "bvid": "BV1xx411c7mD",
                            "cid": 123,
                            "status": "completed",
                            "createdTime": 1700000000000i64
                        }
                    }
                },
                "lyrics-cache": {
                    "BV1xx411c7mD": {
                        "lyrics": "[00:01.00]hello",
                        "offset": 0,
                        "source": "manual"
                    }
                }
            },
            "cookies": [
                {
                    "name": "SESSDATA",
                    "value": "fake_valid_sessdata",
                    "domain": ".bilibili.com",
                    "path": "/",
                    "expirationDate": (now_secs + 86400) as f64,
                    "secure": true,
                    "httpOnly": true,
                    "sameSite": "no_restriction"
                },
                {
                    "name": "bili_jct",
                    "value": "fake_expired_csrf",
                    "domain": ".bilibili.com",
                    "path": "/",
                    "expirationDate": (now_secs - 3600) as f64,
                    "secure": true,
                    "httpOnly": false
                }
            ]
        });

        fs::write(
            electron_dir.join(ELECTRON_EXPORT_FILE),
            serde_json::to_vec_pretty(&export_json).unwrap(),
        )
        .unwrap();

        let bili = BiliState::new(tauri_dir.join("cookies.json"), None).unwrap();
        let candidates = vec![electron_dir.clone()];

        let marker = migrate_from_candidates_in_dir(&tauri_dir, &candidates, &bili)
            .expect("first migration should run");
        assert_eq!(marker.source, "tauri-migration.json");
        assert_eq!(marker.imported_cookies, 1);
        assert!(marker.imported_stores.contains(&"app-settings".to_string()));
        assert!(marker
            .imported_stores
            .contains(&"user-login-info".to_string()));

        // Verify settings & downloadPath preserved
        let settings = file_store_get(&tauri_dir, "app-settings").unwrap();
        assert_eq!(settings["appSettings"]["primaryColor"], "#fb7299");
        assert_eq!(
            settings["appSettings"]["downloadPath"],
            "/custom/downloads/path"
        );
        assert_eq!(settings["appSettings"]["lyricsFontSize"], 28);

        // Verify user-login-info
        let user = file_store_get(&tauri_dir, "user-login-info").unwrap();
        assert_eq!(user["isLogin"], true);
        assert_eq!(user["mid"], 888888);

        // Verify valid cookie imported and expired cookie skipped
        assert_eq!(
            bili.get_cookie("SESSDATA").as_deref(),
            Some("fake_valid_sessdata")
        );
        assert_eq!(bili.get_cookie("bili_jct"), None);
        assert!(tauri_dir.join("cookies.json").is_file());
        assert!(tauri_dir.join(MIGRATION_MARKER_FILE).is_file());

        // Verify Electron source file untouched
        assert!(electron_dir.join(ELECTRON_EXPORT_FILE).is_file());

        // Second run must be a no-op because migration.json exists
        let export_json_modified = json!({
            "version": 1,
            "stores": {
                "app-settings": {
                    "appSettings": { "primaryColor": "#000000" }
                }
            },
            "cookies": []
        });
        fs::write(
            electron_dir.join(ELECTRON_EXPORT_FILE),
            serde_json::to_vec_pretty(&export_json_modified).unwrap(),
        )
        .unwrap();

        let second = migrate_from_candidates_in_dir(&tauri_dir, &candidates, &bili);
        assert!(second.is_none());
        let settings_after = file_store_get(&tauri_dir, "app-settings").unwrap();
        assert_eq!(settings_after["appSettings"]["primaryColor"], "#fb7299");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn migrate_without_export_file_decrypts_node_conf_fixture() {
        let root = std::env::temp_dir().join(format!("biu-mig-conf-{}", uuid::Uuid::new_v4()));
        let electron_dir = root.join("electron-Biu");
        let tauri_dir = root.join("tauri-data");
        fs::create_dir_all(&electron_dir).unwrap();
        fs::create_dir_all(&tauri_dir).unwrap();

        // Generate encrypted user-login-info.json using Node's crypto (matching conf's exact encryption format)
        let repo_root = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .parent()
            .unwrap()
            .to_path_buf();
        let node_bin = [
            "/home/ubuntu/.local/share/pnpm/nodejs/22.17.1/bin/node",
            "node",
        ]
        .into_iter()
        .find(|cand| {
            Command::new(cand)
                .arg("-v")
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        })
        .expect("node binary required for conf fixture test");

        let status = Command::new(node_bin)
            .current_dir(&repo_root)
            .env("ELECTRON_DIR", &electron_dir)
            .args([
                "--input-type=module",
                "-e",
                r##"
                import crypto from "node:crypto";
                import fs from "node:fs";
                import path from "node:path";
                const dir = process.env.ELECTRON_DIR;
                const payload = JSON.stringify({
                  isLogin: true,
                  mid: 424242,
                  uname: "conf_encrypted_user",
                  face: "https://i0.hdslb.com/bfs/face/member/noface.jpg",
                }, null, "\t");
                const iv = crypto.randomBytes(16);
                const password = crypto.pbkdf2Sync("user-login-info", iv.toString(), 10_000, 32, "sha512");
                const cipher = crypto.createCipheriv("aes-256-cbc", password, iv);
                const encrypted = Buffer.concat([iv, Buffer.from(":"), cipher.update(Buffer.from(payload)), cipher.final()]);
                fs.writeFileSync(path.join(dir, "user-login-info.json"), encrypted);
                fs.writeFileSync(
                  path.join(dir, "app-settings.json"),
                  JSON.stringify({
                    appSettings: {
                      primaryColor: "#123456",
                      downloadPath: "/mnt/music",
                    },
                  })
                );
                "##,
            ])
            .status()
            .expect("failed to execute node fixture generator");
        assert!(status.success());

        // Verify the file on disk is actually encrypted (not plain JSON)
        let raw_encrypted = fs::read(electron_dir.join("user-login-info.json")).unwrap();
        assert!(serde_json::from_slice::<Value>(&raw_encrypted).is_err());
        assert_eq!(raw_encrypted[16], b':');

        let bili = BiliState::new(tauri_dir.join("cookies.json"), None).unwrap();
        let marker =
            migrate_from_candidates_in_dir(&tauri_dir, std::slice::from_ref(&electron_dir), &bili)
                .expect("raw store migration should succeed");

        assert_eq!(marker.source, "raw-stores");
        assert_eq!(marker.imported_cookies, 0);
        assert!(marker.imported_stores.contains(&"app-settings".to_string()));
        assert!(marker
            .imported_stores
            .contains(&"user-login-info".to_string()));

        let user = file_store_get(&tauri_dir, "user-login-info").unwrap();
        assert_eq!(user["isLogin"], true);
        assert_eq!(user["mid"], 424242);
        assert_eq!(user["uname"], "conf_encrypted_user");

        let settings = file_store_get(&tauri_dir, "app-settings").unwrap();
        assert_eq!(settings["appSettings"]["primaryColor"], "#123456");
        assert_eq!(settings["appSettings"]["downloadPath"], "/mnt/music");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn migrate_does_not_overwrite_existing_tauri_data() {
        let root =
            std::env::temp_dir().join(format!("biu-mig-nooverwrite-{}", uuid::Uuid::new_v4()));
        let electron_dir = root.join("electron-Biu");
        let tauri_dir = root.join("tauri-data");
        fs::create_dir_all(&electron_dir).unwrap();
        fs::create_dir_all(&tauri_dir).unwrap();

        let now_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        // Pre-populate Tauri user-login-info, app-settings, and SESSDATA cookie
        file_store_set(
            &tauri_dir,
            "user-login-info",
            json!({
                "isLogin": true,
                "mid": 111111,
                "uname": "existing_tauri_user"
            }),
        )
        .unwrap();
        fs::write(
            tauri_dir.join("app-settings.json"),
            serde_json::to_vec_pretty(&json!({
                "appSettings": {
                    "primaryColor": "#abcdef"
                }
            }))
            .unwrap(),
        )
        .unwrap();

        let bili = BiliState::new(tauri_dir.join("cookies.json"), None).unwrap();
        bili.set_cookie(
            "SESSDATA",
            "existing_tauri_sessdata",
            Some(now_secs + 86400),
        )
        .unwrap();

        // Electron export has different primaryColor, user, SESSDATA, plus a new key and cookie
        let export_json = json!({
            "version": 1,
            "stores": {
                "app-settings": {
                    "appSettings": {
                        "primaryColor": "#999999",
                        "downloadPath": "/filled/from/electron"
                    }
                },
                "user-login-info": {
                    "isLogin": true,
                    "mid": 999999,
                    "uname": "electron_old_user"
                }
            },
            "cookies": [
                {
                    "name": "SESSDATA",
                    "value": "electron_old_sessdata",
                    "domain": ".bilibili.com",
                    "path": "/",
                    "expirationDate": (now_secs + 86400) as f64,
                    "secure": true,
                    "httpOnly": true
                },
                {
                    "name": "DedeUserID",
                    "value": "111111",
                    "domain": ".bilibili.com",
                    "path": "/",
                    "expirationDate": (now_secs + 86400) as f64,
                    "secure": true,
                    "httpOnly": false
                }
            ]
        });
        fs::write(
            electron_dir.join(ELECTRON_EXPORT_FILE),
            serde_json::to_vec_pretty(&export_json).unwrap(),
        )
        .unwrap();

        let marker = migrate_from_candidates_in_dir(&tauri_dir, &[electron_dir], &bili)
            .expect("migration should run");
        // Only DedeUserID should be imported; SESSDATA must not be overwritten
        assert_eq!(marker.imported_cookies, 1);
        assert_eq!(
            bili.get_cookie("SESSDATA").as_deref(),
            Some("existing_tauri_sessdata")
        );
        assert_eq!(bili.get_cookie("DedeUserID").as_deref(), Some("111111"));

        // Existing user must not be overwritten
        let user = file_store_get(&tauri_dir, "user-login-info").unwrap();
        assert_eq!(user["mid"], 111111);
        assert_eq!(user["uname"], "existing_tauri_user");

        // Existing primaryColor kept, missing downloadPath filled
        let raw_settings: Value =
            serde_json::from_slice(&fs::read(tauri_dir.join("app-settings.json")).unwrap())
                .unwrap();
        assert_eq!(raw_settings["appSettings"]["primaryColor"], "#abcdef");
        assert_eq!(
            raw_settings["appSettings"]["downloadPath"],
            "/filled/from/electron"
        );

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn migrate_keeps_cookie_scope_lyrics_source_local_storage_and_retries_failures() {
        let root = std::env::temp_dir().join(format!("biu-mig-scope-{}", uuid::Uuid::new_v4()));
        let electron_dir = root.join("electron-Biu");
        let tauri_dir = root.join("tauri-data");
        fs::create_dir_all(&electron_dir).unwrap();
        fs::create_dir_all(&tauri_dir).unwrap();
        let exp = (SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
            + 86400) as f64;
        let export_json = json!({
            "version": 1,
            "stores": {
                "app-settings": { "appSettings": { "primaryColor": "#000000" } },
                "lyrics-cache": { "BV1": { "lyrics": "[00:01.00]a", "offset": 0, "source": "auto" } }
            },
            "cookies": [
                { "name": "SESSDATA", "value": "s", "domain": ".bilibili.com", "path": "/",
                  "expirationDate": exp, "secure": true, "httpOnly": true, "sameSite": "no_restriction" },
                { "name": "host_only", "value": "h", "domain": "www.bilibili.com", "path": "/x",
                  "expirationDate": exp, "secure": true, "httpOnly": false }
            ],
            "localStorage": { "user-token": "{\"state\":{}}", "not-a-string": 3 }
        });
        fs::write(
            electron_dir.join(ELECTRON_EXPORT_FILE),
            serde_json::to_vec(&export_json).unwrap(),
        )
        .unwrap();
        let bili = BiliState::new(tauri_dir.join("cookies.json"), None).unwrap();
        let candidates = vec![electron_dir.clone()];

        // First run: the app-settings write fails, so the marker stays incomplete.
        let marker = migrate_from_candidates_with(&tauri_dir, &candidates, &bili, |name, val| {
            if name == "app-settings" {
                Err("disk full".into())
            } else {
                file_store_write(&tauri_dir, name, val, false)
            }
        })
        .expect("first run");
        assert!(!marker.complete);
        assert_eq!(marker.pending, vec!["app-settings".to_string()]);
        assert_eq!(marker.imported_cookies, 2);

        // Cookie scope is preserved: host-only cookie keeps its host and path.
        {
            let store = bili.cookies.lock().unwrap();
            assert!(store.contains("bilibili.com", "/", "SESSDATA"));
            assert!(store.contains("www.bilibili.com", "/x", "host_only"));
            assert!(!store.contains("bilibili.com", "/", "host_only"));
            let root_url = url::Url::parse("https://api.bilibili.com/").unwrap();
            let sent: Vec<String> = store
                .get_request_values(&root_url)
                .map(|(n, _)| n.to_string())
                .collect();
            assert_eq!(sent, vec!["SESSDATA".to_string()]);
        }

        // Lyrics cache keeps its original source.
        let lyrics = file_store_get(&tauri_dir, "lyrics-cache").unwrap();
        assert_eq!(lyrics["BV1"]["source"], "auto");

        // Only string localStorage entries are staged.
        let staged = read_migrated_local_storage(&tauri_dir);
        assert_eq!(staged.len(), 1);
        assert_eq!(staged["user-token"], "{\"state\":{}}");

        // The user logs out before the retry: finished items must not come back.
        bili.cookies.lock().unwrap().clear();
        fs::remove_file(tauri_dir.join(LOCAL_STORAGE_FILE)).unwrap();

        // Second run retries only app-settings and completes.
        let marker =
            migrate_from_candidates_in_dir(&tauri_dir, &candidates, &bili).expect("retry runs");
        assert_eq!(bili.get_cookie("SESSDATA"), None);
        assert!(read_migrated_local_storage(&tauri_dir).is_empty());
        assert_eq!(marker.imported_stores, vec!["app-settings".to_string()]);
        assert!(marker.complete);
        assert_eq!(marker.attempts, 2);
        assert_eq!(marker.imported_cookies, 0);
        let settings = file_store_get(&tauri_dir, "app-settings").unwrap();
        assert_eq!(settings["appSettings"]["primaryColor"], "#000000");

        // Third run is skipped.
        assert!(migrate_from_candidates_in_dir(&tauri_dir, &candidates, &bili).is_none());
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn unreadable_sources_stay_pending() {
        let root = std::env::temp_dir().join(format!("biu-mig-bad-{}", uuid::Uuid::new_v4()));
        let electron_dir = root.join("electron-Biu");
        let tauri_dir = root.join("tauri-data");
        fs::create_dir_all(&electron_dir).unwrap();
        fs::create_dir_all(&tauri_dir).unwrap();
        // No export file; app-settings is corrupt, user-login-info cannot be decrypted.
        fs::write(electron_dir.join("app-settings.json"), b"{not json").unwrap();
        fs::write(electron_dir.join("user-login-info.json"), b"garbage").unwrap();
        fs::write(
            electron_dir.join("shortcut-settings.json"),
            b"{\"enableGlobalShortcuts\":false}",
        )
        .unwrap();
        let bili = BiliState::new(tauri_dir.join("cookies.json"), None).unwrap();
        let candidates = vec![electron_dir.clone()];

        let marker = migrate_from_candidates_in_dir(&tauri_dir, &candidates, &bili).unwrap();
        assert!(!marker.complete);
        assert!(marker.pending.contains(&"app-settings".to_string()));
        assert!(marker.pending.contains(&"user-login-info".to_string()));
        assert!(marker.done.contains(&"shortcut-settings".to_string()));

        // An unreadable export keeps the run incomplete so cookies can be retried later.
        fs::write(electron_dir.join(ELECTRON_EXPORT_FILE), b"{broken").unwrap();
        let marker = migrate_from_candidates_in_dir(&tauri_dir, &candidates, &bili).unwrap();
        assert!(!marker.complete);
        assert!(marker.pending.contains(&"export".to_string()));
        assert!(!marker.done.contains(&"cookies".to_string()));
        assert_eq!(marker.attempts, 2);
        let _ = fs::remove_dir_all(&root);
    }
}
