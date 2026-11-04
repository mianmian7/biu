use hmac::{Hmac, Mac};
use reqwest::header::{HeaderMap, HeaderName, HeaderValue};
use reqwest_cookie_store::CookieStoreMutex;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::Sha256;
use std::collections::{HashMap, HashSet};
use std::fs::{self, OpenOptions};
use std::io::BufReader;
use std::path::{Path, PathBuf};
use std::sync::{Arc, RwLock};
use std::time::{SystemTime, UNIX_EPOCH};
use url::Url;

pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
pub const BILI_REFERER: &str = "https://www.bilibili.com/";
pub const BILI_ORIGIN: &str = "https://www.bilibili.com";

#[derive(Debug, Clone, Deserialize, Serialize, Default)]
pub struct ProxySettings {
    #[serde(rename = "type")]
    pub proxy_type: String,
    pub host: Option<String>,
    pub port: Option<u16>,
    pub username: Option<String>,
    pub password: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct BiliRequestInput {
    pub method: Option<String>,
    pub url: String,
    pub params: Option<Value>,
    pub body: Option<String>,
    pub headers: Option<HashMap<String, String>>,
    #[serde(default, alias = "timeoutMs")]
    pub timeout_ms: Option<u64>,
}

#[derive(Debug, Serialize)]
pub struct BiliRequestOutput {
    pub status: u16,
    pub headers: HashMap<String, String>,
    pub body: String,
}

#[derive(Debug, Deserialize)]
pub struct CookieSetInput {
    pub name: String,
    pub value: String,
    #[serde(rename = "expirationDate")]
    pub expiration_date: Option<i64>,
}

pub struct BiliState {
    pub cookies: Arc<CookieStoreMutex>,
    pub cookie_path: PathBuf,
    pub client: RwLock<reqwest::Client>,
    pub allowed_media_hosts: RwLock<HashSet<String>>,
}

impl BiliState {
    pub fn new(cookie_path: PathBuf, initial_proxy: Option<ProxySettings>) -> Result<Self, String> {
        let store = load_cookie_store(&cookie_path);
        let cookies = Arc::new(CookieStoreMutex::new(store));
        let client = build_reqwest_client(cookies.clone(), initial_proxy.as_ref())?;
        Ok(Self {
            cookies,
            cookie_path,
            client: RwLock::new(client),
            allowed_media_hosts: RwLock::new(HashSet::new()),
        })
    }

    pub fn client(&self) -> reqwest::Client {
        self.client.read().unwrap().clone()
    }

    pub fn update_proxy(&self, proxy: Option<ProxySettings>) -> Result<(), String> {
        let new_client = build_reqwest_client(self.cookies.clone(), proxy.as_ref())?;
        *self.client.write().unwrap() = new_client;
        Ok(())
    }

    pub fn save_cookies(&self) -> Result<(), String> {
        save_cookie_store(&self.cookies, &self.cookie_path)
    }

    pub fn get_cookie(&self, key: &str) -> Option<String> {
        let url = Url::parse("https://www.bilibili.com/").ok()?;
        let store = self.cookies.lock().unwrap();
        for (name, val) in store.get_request_values(&url) {
            if name == key {
                return Some(val.to_string());
            }
        }
        for c in store.iter_unexpired() {
            if c.name() == key {
                return Some(c.value().to_string());
            }
        }
        None
    }

    pub fn set_cookie(
        &self,
        name: &str,
        value: &str,
        expiration_date: Option<i64>,
    ) -> Result<(), String> {
        let url = Url::parse("https://www.bilibili.com/").map_err(|e| e.to_string())?;
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_secs() as i64;
        let max_age_part = match expiration_date {
            Some(exp) => {
                let diff = (exp - now).max(1);
                format!("; Max-Age={diff}")
            }
            None => "; Max-Age=31536000".to_string(),
        };
        let raw = format!(
            "{name}={value}; Domain=.bilibili.com; Path=/; Secure; SameSite=None{max_age_part}"
        );
        {
            let mut store = self.cookies.lock().unwrap();
            store.parse(&raw, &url).map_err(|e| e.to_string())?;
        }
        self.save_cookies()
    }

    pub fn register_media_host(&self, host: &str) {
        let lower = host.to_ascii_lowercase();
        if !lower.is_empty() {
            self.allowed_media_hosts.write().unwrap().insert(lower);
        }
    }

    pub fn is_media_url_allowed(&self, u: &Url) -> bool {
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
        if self.allowed_media_hosts.read().unwrap().contains(&lower) {
            return true;
        }
        is_bili_audio_cdn_host(&lower)
    }
}

pub fn is_bili_api_url_allowed(u: &Url) -> bool {
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
    const ALLOWED_SUFFIXES: &[&str] = &[
        "bilibili.com",
        "hdslb.com",
        "biliapi.net",
        "biliapi.com",
        "bilivideo.com",
        "bilivideo.cn",
    ];
    ALLOWED_SUFFIXES
        .iter()
        .any(|suffix| lower == *suffix || lower.ends_with(&format!(".{suffix}")))
}

pub fn is_bili_audio_cdn_host(host: &str) -> bool {
    let lower = host.to_ascii_lowercase();
    const CDN_SUFFIXES: &[&str] = &[
        "bilivideo.com",
        "bilivideo.cn",
        "hdslb.com",
        "biliapi.net",
        "bilicdn1.com",
    ];
    if CDN_SUFFIXES
        .iter()
        .any(|suffix| lower == *suffix || lower.ends_with(&format!(".{suffix}")))
    {
        return true;
    }
    // Specific Bilibili Akamai audio CDN hosts (not wildcard *.akamaized.net)
    lower == "upos-hz-mirrorakam.akamaized.net"
        || (lower.starts_with("upos-") && lower.ends_with(".akamaized.net"))
}

pub fn extract_and_register_playurl_hosts(state: &BiliState, json_body: &Value) {
    let mut urls = Vec::new();
    if let Some(data) = json_body.get("data") {
        // DASH audio / flac / dolby / video urls
        if let Some(dash) = data.get("dash") {
            for key in ["audio", "video"] {
                if let Some(arr) = dash.get(key).and_then(|v| v.as_array()) {
                    for item in arr {
                        collect_urls_from_dash_item(item, &mut urls);
                    }
                }
            }
            if let Some(flac_audio) = dash.get("flac").and_then(|f| f.get("audio")) {
                collect_urls_from_dash_item(flac_audio, &mut urls);
            }
            if let Some(dolby_arr) = dash
                .get("dolby")
                .and_then(|d| d.get("audio"))
                .and_then(|a| a.as_array())
            {
                for item in dolby_arr {
                    collect_urls_from_dash_item(item, &mut urls);
                }
            }
        }
        // audio/music-service-c/web/url cdns
        if let Some(cdns) = data.get("cdns").and_then(|v| v.as_array()) {
            for cdn in cdns {
                if let Some(s) = cdn.as_str() {
                    urls.push(s.to_string());
                }
            }
        }
    }
    for u_str in urls {
        if let Ok(parsed) = Url::parse(&u_str) {
            if let Some(host) = parsed.host_str() {
                state.register_media_host(host);
            }
        }
    }
}

fn collect_urls_from_dash_item(item: &Value, out: &mut Vec<String>) {
    for k in ["baseUrl", "base_url"] {
        if let Some(s) = item.get(k).and_then(|v| v.as_str()) {
            out.push(s.to_string());
        }
    }
    for k in ["backupUrl", "backup_url"] {
        if let Some(arr) = item.get(k).and_then(|v| v.as_array()) {
            for u in arr {
                if let Some(s) = u.as_str() {
                    out.push(s.to_string());
                }
            }
        }
    }
}

fn load_cookie_store(path: &Path) -> cookie_store::CookieStore {
    if !path.exists() {
        return cookie_store::CookieStore::default();
    }
    let Ok(file) = fs::File::open(path) else {
        return cookie_store::CookieStore::default();
    };
    let reader = BufReader::new(file);
    cookie_store::serde::json::load(reader).unwrap_or_default()
}

fn save_cookie_store(cookies: &CookieStoreMutex, path: &Path) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut opts = OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts.open(path).map_err(|e| e.to_string())?;
    let store = cookies.lock().unwrap();
    cookie_store::serde::json::save_incl_expired_and_nonpersistent(&store, &mut file)
        .map_err(|e| e.to_string())?;
    Ok(())
}

pub fn build_reqwest_client(
    cookies: Arc<CookieStoreMutex>,
    proxy: Option<&ProxySettings>,
) -> Result<reqwest::Client, String> {
    let mut default_headers = HeaderMap::new();
    default_headers.insert(
        reqwest::header::REFERER,
        HeaderValue::from_static(BILI_REFERER),
    );
    default_headers.insert(
        reqwest::header::ORIGIN,
        HeaderValue::from_static(BILI_ORIGIN),
    );

    let mut builder = reqwest::Client::builder()
        .cookie_provider(cookies)
        .user_agent(USER_AGENT)
        .connect_timeout(std::time::Duration::from_secs(10))
        .default_headers(default_headers);

    if let Some(p) = proxy {
        let p_type = p.proxy_type.trim().to_ascii_lowercase();
        if matches!(p_type.as_str(), "http" | "socks4" | "socks5") {
            if let (Some(host), Some(port)) = (p.host.as_deref(), p.port) {
                let host = host.trim();
                if !host.is_empty() && port > 0 {
                    let scheme = match p_type.as_str() {
                        "http" => "http",
                        "socks4" => "socks4",
                        _ => "socks5",
                    };
                    let proxy_url = format!("{scheme}://{host}:{port}");
                    let mut req_proxy =
                        reqwest::Proxy::all(&proxy_url).map_err(|e| e.to_string())?;
                    if let Some(user) = p.username.as_deref().filter(|u| !u.is_empty()) {
                        let pass = p.password.as_deref().unwrap_or("");
                        req_proxy = req_proxy.basic_auth(user, pass);
                    }
                    builder = builder.proxy(req_proxy);
                } else {
                    builder = builder.no_proxy();
                }
            } else {
                builder = builder.no_proxy();
            }
        } else {
            builder = builder.no_proxy();
        }
    }

    builder.build().map_err(|e| e.to_string())
}

pub fn compute_bili_ticket_hexsign(ts: u64) -> String {
    type HmacSha256 = Hmac<Sha256>;
    let mut mac = HmacSha256::new_from_slice(b"XgwSnGZ1p").expect("HMAC accepts any key size");
    mac.update(format!("ts{ts}").as_bytes());
    let bytes = mac.finalize().into_bytes();
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

pub async fn inject_auth_cookies(state: Arc<BiliState>) {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let need_ticket = state.get_cookie("bili_ticket").is_none();
    let need_buvid4 = state.get_cookie("buvid4").is_none();

    if need_ticket {
        let hexsign = compute_bili_ticket_hexsign(now);
        let url = format!(
            "https://api.bilibili.com/bapis/bilibili.api.ticket.v1.Ticket/GenWebTicket?key_id=ec02&hexsign={hexsign}&context%5Bts%5D={now}&csrf="
        );
        if let Ok(resp) = state.client().post(&url).send().await {
            if let Ok(val) = resp.json::<Value>().await {
                if let Some(ticket) = val
                    .get("data")
                    .and_then(|d| d.get("ticket"))
                    .and_then(|t| t.as_str())
                {
                    let exp = (now + 259260) as i64;
                    let _ = state.set_cookie("bili_ticket", ticket, Some(exp));
                    let _ = state.set_cookie("bili_ticket_expires", &exp.to_string(), Some(exp));
                }
            }
        }
    }

    if need_buvid4 {
        let url = "https://api.bilibili.com/x/frontend/finger/spi_v2";
        if let Ok(resp) = state.client().get(url).send().await {
            if let Ok(val) = resp.json::<Value>().await {
                if let Some(b3) = val
                    .get("data")
                    .and_then(|d| d.get("b_3"))
                    .and_then(|s| s.as_str())
                {
                    if state.get_cookie("buvid3").is_none() {
                        let exp = (now + 30 * 24 * 3600) as i64;
                        let _ = state.set_cookie("buvid3", b3, Some(exp));
                    }
                }
                if let Some(b4) = val
                    .get("data")
                    .and_then(|d| d.get("b_4"))
                    .and_then(|s| s.as_str())
                {
                    let exp = (now + 30 * 24 * 3600) as i64;
                    let _ = state.set_cookie("buvid4", b4, Some(exp));
                }
            }
        }
    }
}

pub fn append_query_params(url: &mut Url, params: &Value) {
    let Some(obj) = params.as_object() else {
        return;
    };
    let mut pairs = url.query_pairs_mut();
    for (k, v) in obj {
        match v {
            Value::Null => {}
            Value::String(s) => {
                pairs.append_pair(k, s);
            }
            Value::Bool(b) => {
                pairs.append_pair(k, if *b { "true" } else { "false" });
            }
            Value::Number(n) => {
                pairs.append_pair(k, &n.to_string());
            }
            Value::Array(arr) => {
                for item in arr {
                    if !item.is_null() {
                        let s = match item {
                            Value::String(s) => s.clone(),
                            other => other.to_string(),
                        };
                        pairs.append_pair(k, &s);
                    }
                }
            }
            other => {
                pairs.append_pair(k, &other.to_string());
            }
        }
    }
}

pub fn sanitize_reqwest_error(e: &reqwest::Error) -> String {
    let mut flags = Vec::new();
    if e.is_timeout() {
        flags.push("timeout");
    }
    if e.is_connect() {
        flags.push("connect");
    }
    let mut clean = e.to_string();
    if let Some(idx) = clean.find(" for url (") {
        clean.truncate(idx);
    }
    if flags.is_empty() {
        clean
    } else {
        format!("{} ({})", clean, flags.join(","))
    }
}

pub async fn execute_bili_request(
    state: &BiliState,
    input: BiliRequestInput,
) -> Result<BiliRequestOutput, String> {
    let mut url = Url::parse(&input.url).map_err(|e| format!("invalid url: {e}"))?;
    if !is_bili_api_url_allowed(&url) {
        return Err(format!(
            "url host not allowed: {}",
            url.host_str().unwrap_or("")
        ));
    }

    if let Some(params) = &input.params {
        append_query_params(&mut url, params);
    }

    let method_str = input
        .method
        .as_deref()
        .unwrap_or("GET")
        .trim()
        .to_ascii_uppercase();
    let method = reqwest::Method::from_bytes(method_str.as_bytes()).map_err(|e| e.to_string())?;
    let timeout_ms = input.timeout_ms.unwrap_or(15_000).max(1);
    let req_timeout = std::time::Duration::from_millis(timeout_ms);

    let build_request = || {
        let mut req = state
            .client()
            .request(method.clone(), url.clone())
            .timeout(req_timeout);

        if let Some(headers) = &input.headers {
            for (k, v) in headers {
                let lower = k.to_ascii_lowercase();
                // Prevent overriding security-critical headers unless safe
                if matches!(lower.as_str(), "host" | "cookie" | "content-length") {
                    continue;
                }
                if let (Ok(name), Ok(val)) = (
                    HeaderName::from_bytes(k.as_bytes()),
                    HeaderValue::from_str(v),
                ) {
                    req = req.header(name, val);
                }
            }
        }

        if let Some(body) = &input.body {
            req = req.body(body.clone());
        }
        req
    };

    let first_attempt = build_request().send().await;
    let resp = match first_attempt {
        Ok(r) => r,
        Err(e) if method == reqwest::Method::GET && (e.is_connect() || e.is_timeout()) => {
            log::warn!(
                "[bili_request] {} {}{} retrying once after error: {}",
                method_str,
                url.host_str().unwrap_or(""),
                url.path(),
                sanitize_reqwest_error(&e)
            );
            match build_request().send().await {
                Ok(r) => r,
                Err(e2) => {
                    let msg = sanitize_reqwest_error(&e2);
                    log::warn!(
                        "[bili_request] {} {}{} failed: {}",
                        method_str,
                        url.host_str().unwrap_or(""),
                        url.path(),
                        msg
                    );
                    return Err(msg);
                }
            }
        }
        Err(e) => {
            let msg = sanitize_reqwest_error(&e);
            log::warn!(
                "[bili_request] {} {}{} failed: {}",
                method_str,
                url.host_str().unwrap_or(""),
                url.path(),
                msg
            );
            return Err(msg);
        }
    };
    let status = resp.status().as_u16();
    let mut out_headers = HashMap::new();
    let has_set_cookie = resp.headers().contains_key(reqwest::header::SET_COOKIE);
    for (k, v) in resp.headers() {
        let name = k.as_str().to_ascii_lowercase();
        // Do not leak Set-Cookie raw values into renderer logs/headers if not needed
        if name == "set-cookie" {
            continue;
        }
        if let Ok(val_str) = v.to_str() {
            out_headers.insert(name, val_str.to_string());
        }
    }

    let body = match resp.text().await {
        Ok(t) => t,
        Err(e) => {
            let msg = sanitize_reqwest_error(&e);
            log::warn!(
                "[bili_request] {} {}{} read body failed: {}",
                method_str,
                url.host_str().unwrap_or(""),
                url.path(),
                msg
            );
            return Err(msg);
        }
    };

    // Persist cookies whenever Set-Cookie was received or on login/refresh endpoints
    if has_set_cookie || url.path().contains("login") || url.path().contains("cookie") {
        let _ = state.save_cookies();
    }

    // If this was a playurl or audio stream url response, extract and allowlist returned CDN hosts
    if url.path().contains("playurl") || url.path().contains("music-service-c") {
        if let Ok(parsed_json) = serde_json::from_str::<Value>(&body) {
            extract_and_register_playurl_hosts(state, &parsed_json);
        }
    }

    Ok(BiliRequestOutput {
        status,
        headers: out_headers,
        body,
    })
}

#[tauri::command]
pub async fn bili_request(
    state: tauri::State<'_, Arc<BiliState>>,
    request: BiliRequestInput,
) -> Result<BiliRequestOutput, String> {
    execute_bili_request(&state, request).await
}

#[tauri::command]
pub async fn cookie_get(
    state: tauri::State<'_, Arc<BiliState>>,
    key: String,
) -> Result<Option<String>, String> {
    Ok(state.get_cookie(&key))
}

#[tauri::command]
pub async fn cookie_set(
    state: tauri::State<'_, Arc<BiliState>>,
    payload: CookieSetInput,
) -> Result<(), String> {
    state.set_cookie(&payload.name, &payload.value, payload.expiration_date)
}

#[tauri::command]
pub async fn proxy_set(
    state: tauri::State<'_, Arc<BiliState>>,
    proxy_settings: ProxySettings,
) -> Result<(), String> {
    state.update_proxy(Some(proxy_settings))
}

pub fn parse_proxy_settings_value(val: Option<Value>) -> Option<ProxySettings> {
    val.and_then(|v| serde_json::from_value(v).ok())
}
