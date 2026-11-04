use crate::bili::{append_query_params, sanitize_reqwest_error, USER_AGENT};
use reqwest::header::{HeaderMap, HeaderValue};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::Path;
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{AppHandle, Manager, Runtime};
use url::Url;

pub const NETEASE_REFERER: &str = "https://music.163.com/";
pub const NETEASE_ORIGIN: &str = "https://music.163.com";

pub const KUROMOJI_DICT_FILES: &[(&str, &str)] = &[
    (
        "base.dat.gz",
        "0803327762e1c93ca731e4319ab8343340f2806bb84941207782cde9d2d5a8eb",
    ),
    (
        "cc.dat.gz",
        "02b7631be0d4de3a1a75cd9f9cc51536e4f94c9e6b389b813e06ba0f6e7de765",
    ),
    (
        "check.dat.gz",
        "193ae0035fff6fe812b58d9ee730e7a7d7ee601d918481ce51075c58114f6cc9",
    ),
    (
        "tid.dat.gz",
        "d43d831cb6fb0f0a411739cd287a6d5e998e121a8daca614df14a81a0dcac586",
    ),
    (
        "tid_map.dat.gz",
        "33efd5ffd87a70f669add093fa39dee44341d58f940844ef107c8fd98bb795b2",
    ),
    (
        "tid_pos.dat.gz",
        "60dbfc99a6ab993f30c5dab648bec6ad7f9aaefa5c14e1843837d95e509f8895",
    ),
    (
        "unk.dat.gz",
        "f7f991cdeb9bfd3e9c0e4577cc50ee0815a11c508cccd444a9d3ab3c81521100",
    ),
    (
        "unk_char.dat.gz",
        "9a8e86fd9aff32d323fbb59f5a7006f05927a11f8173c90712cc56293aeb3225",
    ),
    (
        "unk_compat.dat.gz",
        "50f60aa29bc2e86c2903ab8c825bb6fa604d2b294d96941c1d3924259791899d",
    ),
    (
        "unk_invoke.dat.gz",
        "6b210889548457c3006913afd12c8b525562255f2709e404604be9614a25e94c",
    ),
    (
        "unk_map.dat.gz",
        "6df12460e5477230bb6fd9641def918b699fc0a8868016b6c9f794488630509b",
    ),
    (
        "unk_pos.dat.gz",
        "5b183a29f281acc7e0542beca47b83f7985047c0a2d27e78a66f32276be5ad11",
    ),
];

pub fn is_lyrics_url_allowed(u: &Url) -> bool {
    if !matches!(u.scheme(), "http" | "https") {
        return false;
    }
    if !u.username().is_empty() || u.password().is_some() {
        return false;
    }
    let Some(host) = u.host_str() else {
        return false;
    };
    let lower = host.to_ascii_lowercase();
    matches!(lower.as_str(), "interface.music.163.com" | "lrclib.net")
}

fn lyrics_http_client() -> &'static reqwest::Client {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();
    CLIENT.get_or_init(|| {
        reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .connect_timeout(Duration::from_secs(10))
            .timeout(Duration::from_secs(10))
            .build()
            .expect("failed to build lyrics reqwest client")
    })
}

fn netease_headers() -> HeaderMap {
    let mut headers = HeaderMap::new();
    headers.insert(
        reqwest::header::REFERER,
        HeaderValue::from_static(NETEASE_REFERER),
    );
    headers.insert(
        reqwest::header::ORIGIN,
        HeaderValue::from_static(NETEASE_ORIGIN),
    );
    headers
}

async fn fetch_lyrics_json(url: Url, headers: Option<HeaderMap>) -> Result<Value, String> {
    if !is_lyrics_url_allowed(&url) {
        return Err(format!(
            "lyrics host not allowed: {}",
            url.host_str().unwrap_or("")
        ));
    }

    let client = lyrics_http_client();
    let build_req = || {
        let mut req = client.get(url.clone());
        if let Some(h) = &headers {
            req = req.headers(h.clone());
        }
        req
    };

    let mut attempt = 0u8;
    let resp = loop {
        attempt += 1;
        match build_req().send().await {
            Ok(r) if r.status().is_server_error() && attempt <= 2 => {
                log::warn!(
                    "[lyrics] GET {}{} returned HTTP {}, retrying (attempt {attempt})",
                    url.host_str().unwrap_or(""),
                    url.path(),
                    r.status().as_u16()
                );
                tokio::time::sleep(Duration::from_millis(300)).await;
            }
            Ok(r) => break r,
            Err(e) if (e.is_connect() || e.is_timeout()) && attempt <= 2 => {
                log::warn!(
                    "[lyrics] GET {}{} retrying after error: {}",
                    url.host_str().unwrap_or(""),
                    url.path(),
                    sanitize_reqwest_error(&e)
                );
                tokio::time::sleep(Duration::from_millis(200)).await;
            }
            Err(e) => {
                let msg = sanitize_reqwest_error(&e);
                log::warn!(
                    "[lyrics] GET {}{} failed: {}",
                    url.host_str().unwrap_or(""),
                    url.path(),
                    msg
                );
                return Err(msg);
            }
        }
    };

    if !resp.status().is_success() {
        let status = resp.status().as_u16();
        log::warn!(
            "[lyrics] GET {}{} returned HTTP {}",
            url.host_str().unwrap_or(""),
            url.path(),
            status
        );
        return Err(format!("HTTP {status}"));
    }

    resp.json::<Value>().await.map_err(|e| {
        let msg = sanitize_reqwest_error(&e);
        log::warn!(
            "[lyrics] GET {}{} invalid json: {}",
            url.host_str().unwrap_or(""),
            url.path(),
            msg
        );
        msg
    })
}

#[tauri::command]
pub async fn lyrics_search_netease(params: Value) -> Result<Value, String> {
    let mut url =
        Url::parse("https://interface.music.163.com/api/search/get").map_err(|e| e.to_string())?;
    append_query_params(&mut url, &params);
    fetch_lyrics_json(url, Some(netease_headers())).await
}

#[tauri::command]
pub async fn lyrics_get_netease(params: Value) -> Result<Value, String> {
    let mut url =
        Url::parse("https://interface.music.163.com/api/song/lyric").map_err(|e| e.to_string())?;
    let mut merged = json!({
        "tv": -1,
        "lv": -1,
        "rv": -1,
        "kv": -1,
        "_nmclfl": 1
    });
    if let (Some(base), Some(inc)) = (merged.as_object_mut(), params.as_object()) {
        for (k, v) in inc {
            base.insert(k.clone(), v.clone());
        }
    }
    append_query_params(&mut url, &merged);
    fetch_lyrics_json(url, Some(netease_headers())).await
}

#[tauri::command]
pub async fn lyrics_search_lrclib(params: Value) -> Result<Value, String> {
    let mut url = Url::parse("https://lrclib.net/api/search").map_err(|e| e.to_string())?;
    append_query_params(&mut url, &params);
    let mut headers = HeaderMap::new();
    headers.insert(
        reqwest::header::USER_AGENT,
        HeaderValue::from_static("got (https://github.com/sindresorhus/got)"),
    );
    fetch_lyrics_json(url, Some(headers)).await
}

pub fn sha256_hex(bytes: &[u8]) -> String {
    let digest = Sha256::digest(bytes);
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn verify_dict_file(path: &Path, expected_sha256: &str) -> bool {
    let Ok(bytes) = fs::read(path) else {
        return false;
    };
    sha256_hex(&bytes).eq_ignore_ascii_case(expected_sha256)
}

pub fn is_valid_kuromoji_dict_filename(name: &str) -> bool {
    KUROMOJI_DICT_FILES.iter().any(|(f, _)| *f == name)
}

pub fn read_kuromoji_dict_file(app_data_dir: &Path, name: &str) -> Result<Vec<u8>, u16> {
    if !is_valid_kuromoji_dict_filename(name) {
        return Err(403);
    }
    let path = app_data_dir.join("kuromoji-dict").join(name);
    fs::read(&path).map_err(|_| 404)
}

pub async fn ensure_kuromoji_dict_in_dir(
    dict_dir: &Path,
    mirror_bases: &[&str],
    tarball_url: Option<&str>,
) -> Result<(), String> {
    static DOWNLOAD_LOCK: OnceLock<tokio::sync::Mutex<()>> = OnceLock::new();
    let lock = DOWNLOAD_LOCK.get_or_init(|| tokio::sync::Mutex::new(()));
    let _guard = lock.lock().await;

    fs::create_dir_all(dict_dir).map_err(|e| e.to_string())?;

    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(30))
        .build()
        .map_err(|e| e.to_string())?;

    let mut missing: Vec<(&str, &str)> = Vec::new();
    for (filename, expected_sha256) in KUROMOJI_DICT_FILES {
        let target_path = dict_dir.join(filename);
        if verify_dict_file(&target_path, expected_sha256) {
            continue;
        }

        let mut downloaded = false;
        for base in mirror_bases {
            let url_str = format!("{}/{filename}", base.trim_end_matches('/'));
            let Ok(parsed_url) = Url::parse(&url_str) else {
                continue;
            };
            let host = parsed_url.host_str().unwrap_or("unknown");
            let path = parsed_url.path();

            let resp = match client.get(parsed_url.clone()).send().await {
                Ok(r) => r,
                Err(e) => {
                    log::warn!(
                        "[kuromoji] GET {host}{path} failed: {}",
                        sanitize_reqwest_error(&e)
                    );
                    continue;
                }
            };

            if !resp.status().is_success() {
                log::warn!(
                    "[kuromoji] GET {host}{path} returned HTTP {}",
                    resp.status().as_u16()
                );
                continue;
            }

            let Ok(bytes) = resp.bytes().await else {
                log::warn!("[kuromoji] GET {host}{path} failed to read body");
                continue;
            };

            let actual_sha256 = sha256_hex(&bytes);
            if !actual_sha256.eq_ignore_ascii_case(expected_sha256) {
                log::warn!(
                    "[kuromoji] sha256 mismatch for {filename} from {host}: expected {expected_sha256}, got {actual_sha256}"
                );
                continue;
            }

            let tmp_path = dict_dir.join(format!("{filename}.tmp"));
            fs::write(&tmp_path, &bytes).map_err(|e| e.to_string())?;
            fs::rename(&tmp_path, &target_path).map_err(|e| e.to_string())?;
            log::info!(
                "[kuromoji] downloaded and verified {filename} from {host} (sha256={actual_sha256})"
            );
            downloaded = true;
            break;
        }

        if !downloaded {
            missing.push((*filename, *expected_sha256));
        }
    }

    if missing.is_empty() {
        return Ok(());
    }

    let Some(tarball) = tarball_url else {
        return Err(format!(
            "failed to download and verify kuromoji dictionary files: {}",
            missing
                .iter()
                .map(|(f, _)| *f)
                .collect::<Vec<_>>()
                .join(", ")
        ));
    };
    extract_dict_from_tarball(dict_dir, tarball, &missing).await
}

/// Fallback for mirrors (npmmirror) that only serve the package tarball, not single files.
async fn extract_dict_from_tarball(
    dict_dir: &Path,
    tarball_url: &str,
    missing: &[(&str, &str)],
) -> Result<(), String> {
    let client = reqwest::Client::builder()
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .timeout(Duration::from_secs(180))
        .build()
        .map_err(|e| e.to_string())?;
    let parsed = Url::parse(tarball_url).map_err(|e| e.to_string())?;
    let host = parsed.host_str().unwrap_or("unknown").to_string();
    let resp = client.get(parsed).send().await.map_err(|e| {
        format!(
            "kuromoji tarball from {host}: {}",
            sanitize_reqwest_error(&e)
        )
    })?;
    if !resp.status().is_success() {
        return Err(format!(
            "kuromoji tarball from {host} returned HTTP {}",
            resp.status().as_u16()
        ));
    }
    let bytes = resp
        .bytes()
        .await
        .map_err(|e| format!("kuromoji tarball from {host}: {e}"))?;

    let mut remaining: Vec<(&str, &str)> = missing.to_vec();
    let mut archive = tar::Archive::new(flate2::read::GzDecoder::new(&bytes[..]));
    for entry in archive.entries().map_err(|e| e.to_string())? {
        let mut entry = entry.map_err(|e| e.to_string())?;
        let path = entry.path().map_err(|e| e.to_string())?.into_owned();
        let Some(name) = path
            .strip_prefix("package/dict")
            .ok()
            .and_then(|p| p.to_str())
            .map(str::to_owned)
        else {
            continue;
        };
        let Some(idx) = remaining.iter().position(|(f, _)| *f == name) else {
            continue;
        };
        let mut data = Vec::new();
        std::io::Read::read_to_end(&mut entry, &mut data).map_err(|e| e.to_string())?;
        let (filename, expected) = remaining[idx];
        let actual = sha256_hex(&data);
        if !actual.eq_ignore_ascii_case(expected) {
            log::warn!("[kuromoji] sha256 mismatch for {filename} in tarball from {host}");
            continue;
        }
        let target_path = dict_dir.join(filename);
        let tmp_path = dict_dir.join(format!("{filename}.tmp"));
        fs::write(&tmp_path, &data).map_err(|e| e.to_string())?;
        fs::rename(&tmp_path, &target_path).map_err(|e| e.to_string())?;
        log::info!("[kuromoji] extracted and verified {filename} from {host} tarball");
        remaining.remove(idx);
    }

    if remaining.is_empty() {
        Ok(())
    } else {
        Err(format!(
            "kuromoji tarball from {host} lacks verified files: {}",
            remaining
                .iter()
                .map(|(f, _)| *f)
                .collect::<Vec<_>>()
                .join(", ")
        ))
    }
}

#[tauri::command]
pub async fn kuromoji_ensure_dict<R: Runtime>(app: AppHandle<R>) -> Result<(), String> {
    let app_data_dir = app
        .path()
        .app_data_dir()
        .unwrap_or_else(|_| std::env::temp_dir().join("com.biu.wood3n"));
    let dict_dir = app_data_dir.join("kuromoji-dict");
    // npmmirror refuses single-file access for kuromoji (unpkg whitelist), so it is used via the tarball.
    const DEFAULT_MIRRORS: &[&str] = &[
        "https://cdn.jsdelivr.net/npm/kuromoji@0.1.2/dict",
        "https://unpkg.com/kuromoji@0.1.2/dict",
    ];
    const TARBALL: &str = "https://registry.npmmirror.com/kuromoji/-/kuromoji-0.1.2.tgz";
    ensure_kuromoji_dict_in_dir(&dict_dir, DEFAULT_MIRRORS, Some(TARBALL)).await
}
