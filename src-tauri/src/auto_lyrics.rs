use crate::bili::{BiliState, BILI_REFERER, USER_AGENT};
use crate::download::{
    clear_resolved_ffmpeg_path, encode_params_wbi, get_dash_url, get_web_interface_view,
    resolve_ffmpeg_path, user_ffmpeg_setting,
};
use crate::store::read_store_object;
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::collections::{HashMap, VecDeque};
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tauri::ipc::Response;
use tauri::{AppHandle, Runtime};
use tauri_plugin_store::StoreExt;
use url::Url;

pub const SAMPLE_RATE: u32 = 22050;
const MAX_CACHED_AUDIO_BUFFERS: usize = 8;

const NETEASE_SEARCH_URL: &str = "https://interface.music.163.com/api/search/get";
const NETEASE_LYRIC_URL: &str = "https://interface.music.163.com/api/song/lyric";
const NETEASE_REFERER: &str = "https://music.163.com/";
const NETEASE_ORIGIN: &str = "https://music.163.com";

pub fn is_netease_api_url_allowed(u: &Url) -> bool {
    if u.scheme() != "https" {
        return false;
    }
    if !u.username().is_empty() || u.password().is_some() {
        return false;
    }
    matches!(
        u.host_str().map(|h| h.to_ascii_lowercase()).as_deref(),
        Some("interface.music.163.com" | "music.163.com")
    )
}

type AudioBufferMap = (VecDeque<String>, HashMap<String, Vec<u8>>);

pub struct AutoLyricsState {
    pub references_path: PathBuf,
    pub references_cache: Mutex<serde_json::Map<String, Value>>,
    pub audio_buffers: Mutex<AudioBufferMap>,
    pub netease_client: reqwest::Client,
}

impl AutoLyricsState {
    pub fn new(references_path: PathBuf) -> Result<Self, String> {
        let initial_refs = if references_path.exists() {
            fs::read(&references_path)
                .ok()
                .and_then(|b| serde_json::from_slice::<Value>(&b).ok())
                .and_then(|v| v.as_object().cloned())
                .unwrap_or_default()
        } else {
            serde_json::Map::new()
        };

        let netease_client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(10))
            .build()
            .map_err(|e| e.to_string())?;

        Ok(Self {
            references_path,
            references_cache: Mutex::new(initial_refs),
            audio_buffers: Mutex::new((VecDeque::new(), HashMap::new())),
            netease_client,
        })
    }

    pub fn store_audio_buffer(&self, bytes: Vec<u8>) -> String {
        let token = format!("biu-audio-token:{}", uuid::Uuid::new_v4());
        let mut lock = self.audio_buffers.lock().unwrap();
        while lock.0.len() >= MAX_CACHED_AUDIO_BUFFERS {
            if let Some(oldest) = lock.0.pop_front() {
                lock.1.remove(&oldest);
            }
        }
        lock.0.push_back(token.clone());
        lock.1.insert(token.clone(), bytes);
        token
    }

    pub fn take_audio_buffer(&self, token: &str) -> Option<Vec<u8>> {
        let mut lock = self.audio_buffers.lock().unwrap();
        lock.0.retain(|k| k != token);
        lock.1.remove(token)
    }

    pub fn get_all_references(&self) -> Value {
        Value::Object(self.references_cache.lock().unwrap().clone())
    }

    pub fn set_reference(&self, id: String, value: Value) -> Result<(), String> {
        let snapshot = {
            let mut lock = self.references_cache.lock().unwrap();
            lock.insert(id, value);
            Value::Object(lock.clone())
        };
        if let Some(parent) = self.references_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let bytes = serde_json::to_vec_pretty(&snapshot).map_err(|e| e.to_string())?;
        fs::write(&self.references_path, bytes).map_err(|e| e.to_string())?;
        Ok(())
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SongCandidate {
    pub id: i64,
    pub name: String,
    pub artists: Vec<String>,
    pub duration: f64,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NeteaseLyricResult {
    pub lrc: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tlyric: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub romalrc: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct BiliVideoSearchItem {
    pub bvid: String,
    pub title: String,
    pub duration: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoPageInfo {
    pub cid: i64,
    pub part: String,
    pub duration: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct VideoViewInfo {
    pub bvid: String,
    pub cid: i64,
    pub title: String,
    pub duration: f64,
    pub tid: i64,
    pub owner_name: String,
    pub pages: Vec<VideoPageInfo>,
}

pub fn parse_duration_seconds(val: &Value) -> f64 {
    if let Some(n) = val.as_f64() {
        return n;
    }
    if let Some(s) = val.as_str() {
        return s.split(':').fold(0.0, |acc, part| {
            acc * 60.0 + part.trim().parse::<f64>().unwrap_or(0.0)
        });
    }
    0.0
}

pub fn strip_html_tags(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    let mut in_tag = false;
    for ch in input.chars() {
        if ch == '<' {
            in_tag = true;
        } else if ch == '>' && in_tag {
            in_tag = false;
        } else if !in_tag {
            out.push(ch);
        }
    }
    out
}

pub fn decode_audio_to_s16le(ffmpeg_path: &Path, data: &[u8]) -> Result<Vec<u8>, String> {
    let mut cmd = Command::new(ffmpeg_path);
    cmd.args([
        "-hide_banner",
        "-loglevel",
        "error",
        "-i",
        "pipe:0",
        "-f",
        "s16le",
        "-ac",
        "1",
        "-ar",
        &SAMPLE_RATE.to_string(),
        "pipe:1",
    ])
    .stdin(Stdio::piped())
    .stdout(Stdio::piped())
    .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x0800_0000);
    }

    let mut child = cmd.spawn().map_err(|e| {
        clear_resolved_ffmpeg_path();
        format!("decode-unavailable: {e}")
    })?;

    let mut stdin = child
        .stdin
        .take()
        .ok_or_else(|| "Failed to open ffmpeg stdin".to_string())?;
    let input_vec = data.to_vec();
    let writer_handle = std::thread::spawn(move || {
        let _ = stdin.write_all(&input_vec);
    });

    let mut pcm_bytes = Vec::new();
    if let Some(mut stdout) = child.stdout.take() {
        stdout
            .read_to_end(&mut pcm_bytes)
            .map_err(|e| e.to_string())?;
    }
    let mut stderr_str = String::new();
    if let Some(mut stderr) = child.stderr.take() {
        let _ = stderr.read_to_string(&mut stderr_str);
    }

    let _ = writer_handle.join();
    let status = child.wait().map_err(|e| e.to_string())?;
    if !status.success() {
        let code = status.code().unwrap_or(-1);
        return Err(format!("ffmpeg exited {code}: {stderr_str}"));
    }
    if pcm_bytes.len() < 2 || pcm_bytes.len() % 2 != 0 {
        return Err("ffmpeg returned invalid PCM".to_string());
    }
    Ok(pcm_bytes)
}

#[tauri::command]
pub async fn auto_lyrics_view(
    bili: tauri::State<'_, Arc<BiliState>>,
    bvid: String,
) -> Result<VideoViewInfo, String> {
    let res = get_web_interface_view(&bili, &bvid).await?;
    let code = res.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    let message = res.get("message").and_then(|v| v.as_str()).unwrap_or("");
    let Some(data) = res.get("data").filter(|_| code == 0) else {
        return Err(format!("Bilibili view: {code} {message}"));
    };

    let pages = data
        .get("pages")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .map(|p| VideoPageInfo {
                    cid: p.get("cid").and_then(|v| v.as_i64()).unwrap_or(0),
                    part: p
                        .get("part")
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string(),
                    duration: p.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0),
                })
                .collect()
        })
        .unwrap_or_default();

    Ok(VideoViewInfo {
        bvid: data
            .get("bvid")
            .and_then(|v| v.as_str())
            .unwrap_or(&bvid)
            .to_string(),
        cid: data.get("cid").and_then(|v| v.as_i64()).unwrap_or(0),
        title: data
            .get("title")
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        duration: data.get("duration").and_then(|v| v.as_f64()).unwrap_or(0.0),
        tid: data.get("tid").and_then(|v| v.as_i64()).unwrap_or(0),
        owner_name: data
            .get("owner")
            .and_then(|o| o.get("name"))
            .and_then(|v| v.as_str())
            .unwrap_or("")
            .to_string(),
        pages,
    })
}

#[tauri::command]
pub async fn auto_lyrics_search_netease(
    auto_state: tauri::State<'_, Arc<AutoLyricsState>>,
    query: String,
) -> Result<Vec<SongCandidate>, String> {
    let mut url = Url::parse(NETEASE_SEARCH_URL).map_err(|e| e.to_string())?;
    if !is_netease_api_url_allowed(&url) {
        return Err("Netease URL not allowed".to_string());
    }
    url.query_pairs_mut()
        .append_pair("s", &query)
        .append_pair("type", "1")
        .append_pair("limit", "20")
        .append_pair("offset", "0");

    let mut last_err = String::new();
    for attempt in 0..=3 {
        let resp_res = auto_state
            .netease_client
            .get(url.clone())
            .header(reqwest::header::REFERER, NETEASE_REFERER)
            .header(reqwest::header::ORIGIN, NETEASE_ORIGIN)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header("X-Real-IP", "116.25.146.177")
            .header("X-Forwarded-For", "116.25.146.177")
            .send()
            .await;

        match resp_res {
            Ok(resp) => match resp.json::<Value>().await {
                Ok(body) => {
                    let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
                    if code != 200 {
                        return Err(format!("Netease search: {code}"));
                    }
                    let songs = body
                        .get("result")
                        .and_then(|r| r.get("songs"))
                        .and_then(|s| s.as_array())
                        .cloned()
                        .unwrap_or_default();

                    let mut out = Vec::new();
                    for song in songs {
                        let Some(id) = song.get("id").and_then(|v| v.as_i64()).filter(|&n| n != 0)
                        else {
                            continue;
                        };
                        let Some(name) = song
                            .get("name")
                            .and_then(|v| v.as_str())
                            .filter(|s| !s.is_empty())
                        else {
                            continue;
                        };
                        let Some(duration_ms) = song
                            .get("duration")
                            .and_then(|v| v.as_f64())
                            .filter(|&d| d > 0.0)
                        else {
                            continue;
                        };
                        let artists = song
                            .get("artists")
                            .and_then(|v| v.as_array())
                            .map(|arr| {
                                arr.iter()
                                    .filter_map(|a| {
                                        a.get("name")
                                            .and_then(|v| v.as_str())
                                            .filter(|s| !s.is_empty())
                                            .map(|s| s.to_string())
                                    })
                                    .collect()
                            })
                            .unwrap_or_default();
                        let mut aliases = Vec::new();
                        for key in ["alias", "transNames"] {
                            if let Some(arr) = song.get(key).and_then(|v| v.as_array()) {
                                for item in arr {
                                    if let Some(s) = item.as_str() {
                                        aliases.push(s.to_string());
                                    }
                                }
                            }
                        }
                        out.push(SongCandidate {
                            id,
                            name: name.to_string(),
                            artists,
                            duration: duration_ms / 1000.0,
                            aliases,
                        });
                    }
                    return Ok(out);
                }
                Err(e) => last_err = e.to_string(),
            },
            Err(e) => last_err = e.to_string(),
        }
        if attempt < 3 {
            tokio::time::sleep(Duration::from_millis(200 * (attempt + 1))).await;
        }
    }
    Err(last_err)
}

#[tauri::command]
pub async fn auto_lyrics_get_netease_lyric(
    auto_state: tauri::State<'_, Arc<AutoLyricsState>>,
    song_id: i64,
) -> Result<NeteaseLyricResult, String> {
    let mut url = Url::parse(NETEASE_LYRIC_URL).map_err(|e| e.to_string())?;
    if !is_netease_api_url_allowed(&url) {
        return Err("Netease URL not allowed".to_string());
    }
    url.query_pairs_mut()
        .append_pair("id", &song_id.to_string())
        .append_pair("tv", "-1")
        .append_pair("lv", "-1")
        .append_pair("rv", "-1")
        .append_pair("kv", "-1")
        .append_pair("_nmclfl", "1");

    let mut last_err = String::new();
    for attempt in 0..=3 {
        let resp_res = auto_state
            .netease_client
            .get(url.clone())
            .header(reqwest::header::REFERER, NETEASE_REFERER)
            .header(reqwest::header::ORIGIN, NETEASE_ORIGIN)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .header("X-Real-IP", "116.25.146.177")
            .header("X-Forwarded-For", "116.25.146.177")
            .send()
            .await;

        match resp_res {
            Ok(resp) => match resp.json::<Value>().await {
                Ok(body) => {
                    let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
                    if code != 200 {
                        return Err(format!("Netease lyric: {code}"));
                    }
                    let lrc = body
                        .get("lrc")
                        .and_then(|l| l.get("lyric"))
                        .and_then(|v| v.as_str())
                        .unwrap_or("")
                        .to_string();
                    let tlyric = body
                        .get("tlyric")
                        .and_then(|l| l.get("lyric"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    let romalrc = body
                        .get("romalrc")
                        .and_then(|l| l.get("lyric"))
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string());
                    return Ok(NeteaseLyricResult {
                        lrc,
                        tlyric,
                        romalrc,
                    });
                }
                Err(e) => last_err = e.to_string(),
            },
            Err(e) => last_err = e.to_string(),
        }
        if attempt < 3 {
            tokio::time::sleep(Duration::from_millis(200 * (attempt + 1))).await;
        }
    }
    Err(last_err)
}

#[tauri::command]
pub async fn auto_lyrics_search_bili_videos(
    bili: tauri::State<'_, Arc<BiliState>>,
    keyword: String,
) -> Result<Vec<BiliVideoSearchItem>, String> {
    let parse_search_items = |body: &Value| -> Vec<BiliVideoSearchItem> {
        let results = body
            .get("data")
            .and_then(|d| d.get("result"))
            .and_then(|r| r.as_array())
            .cloned()
            .unwrap_or_default();
        let mut items = Vec::with_capacity(results.len());
        for item in results {
            let Some(bvid) = item
                .get("bvid")
                .and_then(|v| v.as_str())
                .filter(|s| !s.is_empty())
            else {
                continue;
            };
            let raw_title = item.get("title").and_then(|v| v.as_str()).unwrap_or("");
            let duration = item
                .get("duration")
                .map(parse_duration_seconds)
                .unwrap_or(0.0);
            items.push(BiliVideoSearchItem {
                bvid: bvid.to_string(),
                title: strip_html_tags(raw_title),
                duration,
            });
        }
        items
    };

    for attempt in 0..=2u32 {
        let base_params = vec![
            ("search_type".to_string(), "video".to_string()),
            ("keyword".to_string(), keyword.clone()),
            ("page".to_string(), "1".to_string()),
            ("page_size".to_string(), "50".to_string()),
            ("order".to_string(), "totalrank".to_string()),
        ];
        let signed = encode_params_wbi(&bili, &base_params).await;
        let mut url = Url::parse("https://api.bilibili.com/x/web-interface/wbi/search/type")
            .map_err(|e| e.to_string())?;
        {
            let mut pairs = url.query_pairs_mut();
            for (k, v) in &signed {
                pairs.append_pair(k, v);
            }
        }
        let resp = bili
            .client()
            .get(url)
            .header(reqwest::header::REFERER, BILI_REFERER)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .timeout(Duration::from_secs(15))
            .send()
            .await
            .map_err(|e| e.to_string())?;
        let body = resp.json::<Value>().await.map_err(|e| e.to_string())?;

        if body
            .get("data")
            .and_then(|d| d.get("v_voucher"))
            .and_then(|v| v.as_str())
            .is_some_and(|s| !s.is_empty())
        {
            // Try non-WBI search endpoint before retrying or failing
            let mut fallback_url =
                Url::parse("https://api.bilibili.com/x/web-interface/search/type")
                    .map_err(|e| e.to_string())?;
            {
                let mut pairs = fallback_url.query_pairs_mut();
                for (k, v) in &base_params {
                    pairs.append_pair(k, v);
                }
            }
            if let Ok(fb_resp) = bili
                .client()
                .get(fallback_url)
                .header(reqwest::header::REFERER, "https://search.bilibili.com/")
                .header(reqwest::header::USER_AGENT, USER_AGENT)
                .timeout(Duration::from_secs(15))
                .send()
                .await
            {
                if let Ok(fb_body) = fb_resp.json::<Value>().await {
                    let fb_code = fb_body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
                    let fb_voucher = fb_body
                        .get("data")
                        .and_then(|d| d.get("v_voucher"))
                        .and_then(|v| v.as_str())
                        .is_some_and(|s| !s.is_empty());
                    if fb_code == 0 && !fb_voucher {
                        return Ok(parse_search_items(&fb_body));
                    }
                }
            }

            if attempt == 2 {
                return Err("Bilibili search: v_voucher after 2 retries".to_string());
            }
            tokio::time::sleep(Duration::from_millis(500 * 2u64.pow(attempt))).await;
            continue;
        }

        let code = body.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
        if code != 0 {
            let msg = body.get("message").and_then(|v| v.as_str()).unwrap_or("");
            return Err(format!("Bilibili search: {code} {msg}"));
        }

        return Ok(parse_search_items(&body));
    }
    Ok(Vec::new())
}

#[tauri::command]
pub async fn auto_lyrics_fetch_audio(
    bili: tauri::State<'_, Arc<BiliState>>,
    auto_state: tauri::State<'_, Arc<AutoLyricsState>>,
    bvid: String,
    cid: Option<i64>,
) -> Result<String, String> {
    let resolved_cid = match cid {
        Some(c) if c > 0 => c,
        _ => {
            let view = get_web_interface_view(&bili, &bvid).await?;
            let code = view.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
            let msg = view.get("message").and_then(|v| v.as_str()).unwrap_or("");
            view.get("data")
                .filter(|_| code == 0)
                .and_then(|d| d.get("cid"))
                .and_then(|v| v.as_i64())
                .ok_or_else(|| format!("Bilibili view: {code} {msg}"))?
        }
    };

    let playurl = get_dash_url(&bili, &bvid, &resolved_cid.to_string(), 16, Some(16)).await?;
    let code = playurl.get("code").and_then(|v| v.as_i64()).unwrap_or(-1);
    let has_voucher = playurl
        .get("data")
        .and_then(|d| d.get("v_voucher"))
        .and_then(|v| v.as_str())
        .is_some_and(|s| !s.is_empty());
    if code != 0 || has_voucher {
        let msg = if has_voucher {
            "v_voucher"
        } else {
            playurl
                .get("message")
                .and_then(|v| v.as_str())
                .unwrap_or("")
        };
        return Err(format!("Bilibili playurl: {code} {msg}"));
    }

    let mut audio_arr = playurl
        .get("data")
        .and_then(|d| d.get("dash"))
        .and_then(|d| d.get("audio"))
        .and_then(|a| a.as_array())
        .cloned()
        .unwrap_or_default();
    audio_arr.sort_by_key(|a| {
        a.get("bandwidth")
            .and_then(|v| v.as_u64())
            .unwrap_or(u64::MAX)
    });

    let first_audio = audio_arr
        .first()
        .ok_or_else(|| "Bilibili playurl: no DASH audio".to_string())?;
    let audio_url_str = first_audio
        .get("baseUrl")
        .or_else(|| first_audio.get("base_url"))
        .and_then(|v| v.as_str())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| "Bilibili playurl: no DASH audio".to_string())?;

    let parsed_url = Url::parse(audio_url_str).map_err(|e| e.to_string())?;
    if !bili.is_media_url_allowed(&parsed_url) {
        return Err(format!(
            "Audio CDN host not allowed: {}",
            parsed_url.host_str().unwrap_or("")
        ));
    }

    let referer = format!("https://www.bilibili.com/video/{bvid}");
    let mut last_err = String::new();
    for attempt in 0..=1 {
        let resp_res = bili
            .client()
            .get(parsed_url.clone())
            .header(reqwest::header::REFERER, &referer)
            .header(reqwest::header::USER_AGENT, USER_AGENT)
            .timeout(Duration::from_secs(30))
            .send()
            .await;
        match resp_res {
            Ok(resp) if resp.status().is_success() => match resp.bytes().await {
                Ok(bytes) => {
                    return Ok(auto_state.store_audio_buffer(bytes.to_vec()));
                }
                Err(e) => last_err = e.to_string(),
            },
            Ok(resp) => last_err = format!("HTTP {}", resp.status()),
            Err(e) => last_err = e.to_string(),
        }
        if attempt == 0 {
            tokio::time::sleep(Duration::from_millis(200)).await;
        }
    }
    Err(format!("Failed to download audio: {last_err}"))
}

#[tauri::command]
pub async fn auto_lyrics_decode_audio<R: Runtime>(
    app: AppHandle<R>,
    auto_state: tauri::State<'_, Arc<AutoLyricsState>>,
    token: String,
) -> Result<Response, String> {
    let audio_bytes = auto_state
        .take_audio_buffer(&token)
        .ok_or_else(|| "Audio buffer token expired or missing".to_string())?;

    let user_ffmpeg = user_ffmpeg_setting(&app);
    let ffmpeg_bin = resolve_ffmpeg_path(user_ffmpeg.as_deref())?;

    let pcm_s16le =
        tokio::task::spawn_blocking(move || decode_audio_to_s16le(&ffmpeg_bin, &audio_bytes))
            .await
            .map_err(|e| e.to_string())??;

    Ok(Response::new(pcm_s16le))
}

#[tauri::command]
pub fn auto_lyrics_state_snapshot<R: Runtime>(
    app: AppHandle<R>,
    auto_state: tauri::State<'_, Arc<AutoLyricsState>>,
) -> Result<Value, String> {
    let lyrics_cache = read_store_object(&app, "lyrics-cache").unwrap_or_else(|_| json!({}));
    let references = auto_state.get_all_references();
    Ok(json!({
        "lyricsCache": lyrics_cache,
        "references": references,
    }))
}

#[tauri::command]
pub fn auto_lyrics_cache_set<R: Runtime>(
    app: AppHandle<R>,
    key: String,
    value: Value,
) -> Result<(), String> {
    let store = app.store("lyrics-cache.json").map_err(|e| e.to_string())?;
    store.set(key, value);
    store.save().map_err(|e| e.to_string())?;
    Ok(())
}

#[tauri::command]
pub fn auto_lyrics_ref_set(
    auto_state: tauri::State<'_, Arc<AutoLyricsState>>,
    id: String,
    value: Value,
) -> Result<(), String> {
    auto_state.set_reference(id, value)
}
