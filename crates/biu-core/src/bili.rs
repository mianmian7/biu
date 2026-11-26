use crate::models::*;
use crate::store::{Store, atomic_write};
use anyhow::{Context, Result, bail, ensure};
use hmac::{Hmac, Mac};
use md5::{Digest, Md5};
use reqwest::{
    Client, RequestBuilder,
    header::{HeaderMap, HeaderValue},
};
use reqwest_cookie_store::CookieStoreMutex;
use serde::de::DeserializeOwned;
use serde_json::{Value, json};
use sha2::Sha256;
use std::collections::HashSet;
use std::fs::File;
use std::io::BufReader;
use std::path::PathBuf;
use std::sync::{Arc, RwLock};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use tokio::sync::Mutex;
use url::Url;

mod endpoints;

pub const USER_AGENT: &str = "Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/141.0.0.0 Safari/537.36";
pub const BILI_REFERER: &str = "https://www.bilibili.com/";
pub const BILI_ORIGIN: &str = "https://www.bilibili.com";

fn is_media_url_allowed(u: &Url, allowed_hosts: &HashSet<String>) -> bool {
    if !matches!(u.scheme(), "http" | "https") || !u.username().is_empty() || u.password().is_some()
    {
        return false;
    }
    let Some(host) = u.host_str() else {
        return false;
    };
    let lower = host.to_ascii_lowercase();
    allowed_hosts.contains(&lower) || is_bili_audio_cdn_host(&lower)
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

pub fn allowlist_redirect_policy(
    allowed: impl Fn(&Url) -> bool + Send + Sync + 'static,
) -> reqwest::redirect::Policy {
    reqwest::redirect::Policy::custom(move |attempt| {
        if attempt.previous().len() >= 10 {
            attempt.error("too many redirects")
        } else if allowed(attempt.url()) {
            attempt.follow()
        } else {
            let host = attempt.url().host_str().unwrap_or("").to_string();
            attempt.error(format!("redirect to disallowed host {host}"))
        }
    })
}

fn is_bili_redirect_allowed(u: &Url) -> bool {
    is_bili_api_url_allowed(u) || is_media_url_allowed(u, &HashSet::new())
}

pub fn build_reqwest_client(
    cookies: Arc<CookieStoreMutex>,
    proxy: Option<&ProxySettings>,
) -> Result<reqwest::Client, String> {
    build_client(
        cookies,
        proxy,
        allowlist_redirect_policy(is_bili_redirect_allowed),
    )
}

fn build_media_client(
    cookies: Arc<CookieStoreMutex>,
    proxy: Option<&ProxySettings>,
    allowed_hosts: Arc<RwLock<HashSet<String>>>,
) -> Result<reqwest::Client, String> {
    let policy =
        allowlist_redirect_policy(move |u| is_media_url_allowed(u, &allowed_hosts.read().unwrap()));
    build_client(cookies, proxy, policy)
}

fn build_client(
    cookies: Arc<CookieStoreMutex>,
    proxy: Option<&ProxySettings>,
    redirect: reqwest::redirect::Policy,
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
        .pool_max_idle_per_host(2)
        .pool_idle_timeout(Duration::from_secs(30))
        .cookie_provider(cookies)
        .user_agent(USER_AGENT)
        .connect_timeout(Duration::from_secs(10))
        .read_timeout(Duration::from_secs(30))
        .redirect(redirect)
        .default_headers(default_headers);

    if let Some(p) = proxy {
        let p_type = p.proxy_type.trim().to_ascii_lowercase();
        if matches!(p_type.as_str(), "http" | "socks4" | "socks5") {
            if let (Some(host), Some(port)) = (p.host.as_deref(), p.port) {
                let host = host.trim();
                if !host.is_empty() && port > 0 {
                    // A host must not be interpreted as URL credentials, a scheme, or a path.
                    url::Host::parse(host).map_err(|_| "invalid proxy host".to_string())?;
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

const MIXIN_KEY_ENC_TAB: [usize; 64] = [
    46, 47, 18, 2, 53, 8, 23, 32, 15, 50, 10, 31, 58, 3, 45, 35, 27, 43, 5, 49, 33, 9, 42, 19, 29,
    28, 14, 39, 12, 38, 41, 13, 37, 48, 7, 16, 24, 55, 40, 61, 26, 17, 0, 1, 60, 51, 30, 4, 22, 25,
    54, 21, 56, 59, 6, 63, 57, 62, 11, 36, 20, 34, 44, 52,
];

pub fn encode_uri_component(input: &str) -> String {
    let mut out = String::with_capacity(input.len());
    for b in input.bytes() {
        match b {
            b'A'..=b'Z'
            | b'a'..=b'z'
            | b'0'..=b'9'
            | b'-'
            | b'_'
            | b'.'
            | b'!'
            | b'~'
            | b'*'
            | b'\''
            | b'('
            | b')' => out.push(b as char),
            _ => out.push_str(&format!("%{b:02X}")),
        }
    }
    out
}

pub fn sign_wbi_params_with_keys(
    params: &[(String, String)],
    img_key: &str,
    sub_key: &str,
    wts: u64,
) -> Vec<(String, String)> {
    if img_key.is_empty() || sub_key.is_empty() {
        return params.to_vec();
    }
    let orig = format!("{img_key}{sub_key}");
    let orig_bytes = orig.as_bytes();
    let mut mixin_key = String::with_capacity(32);
    for &idx in MIXIN_KEY_ENC_TAB.iter().take(32) {
        if let Some(&b) = orig_bytes.get(idx) {
            mixin_key.push(b as char);
        }
    }

    let mut all_params: Vec<(String, String)> = params.to_vec();
    all_params.push(("wts".to_string(), wts.to_string()));
    all_params.sort_by(|a, b| a.0.cmp(&b.0));

    let query = all_params
        .iter()
        .map(|(k, v)| {
            let filtered: String = v
                .chars()
                .filter(|c| !matches!(c, '!' | '\'' | '(' | ')' | '*'))
                .collect();
            format!(
                "{}={}",
                encode_uri_component(k),
                encode_uri_component(&filtered)
            )
        })
        .collect::<Vec<_>>()
        .join("&");

    let mut hasher = Md5::new();
    hasher.update(format!("{query}{mixin_key}").as_bytes());
    let w_rid: String = hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect();

    let mut out = params.to_vec();
    out.push(("wts".to_string(), wts.to_string()));
    out.push(("w_rid".to_string(), w_rid));
    out
}

const API: &str = "https://api.bilibili.com";
const PASSPORT: &str = "https://passport.bilibili.com";
const NAV: &str = "/x/web-interface/nav";
const WBI_TTL: Duration = Duration::from_secs(30 * 60);

pub struct BiliClient {
    cookies: Arc<CookieStoreMutex>,
    cookie_path: PathBuf,
    clients: RwLock<(Client, Client)>,
    media_hosts: Arc<RwLock<HashSet<String>>>,
    wbi: Mutex<Option<(Instant, String, String)>>,
    auth_cookie_refresh: Mutex<Option<Instant>>,
}

impl BiliClient {
    pub fn open(store: &Store) -> Result<Self> {
        Self::new(
            store.directory().join("cookies.json"),
            Some(store.app_settings()?.proxy_settings),
        )
    }

    pub fn new(cookie_path: PathBuf, proxy: Option<ProxySettings>) -> Result<Self> {
        let jar = match File::open(&cookie_path) {
            Ok(file) => cookie_store::serde::json::load(BufReader::new(file))
                .map_err(|err| anyhow::anyhow!("invalid cookies.json: {err}"))?,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => Default::default(),
            Err(err) => return Err(err).context("open cookies.json"),
        };
        let cookies = Arc::new(CookieStoreMutex::new(jar));
        let media_hosts = Arc::new(RwLock::new(HashSet::new()));
        let clients = Self::build_clients(&cookies, &media_hosts, proxy.as_ref())?;
        Ok(Self {
            cookies,
            cookie_path,
            clients: RwLock::new(clients),
            media_hosts,
            wbi: Mutex::new(None),
            auth_cookie_refresh: Mutex::new(None),
        })
    }

    fn build_clients(
        cookies: &Arc<CookieStoreMutex>,
        hosts: &Arc<RwLock<HashSet<String>>>,
        proxy: Option<&ProxySettings>,
    ) -> Result<(Client, Client)> {
        Ok((
            build_reqwest_client(cookies.clone(), proxy).map_err(anyhow::Error::msg)?,
            build_media_client(cookies.clone(), proxy, hosts.clone())
                .map_err(anyhow::Error::msg)?,
        ))
    }

    fn client(&self) -> Client {
        self.clients.read().unwrap().0.clone()
    }

    pub fn media_client(&self) -> Client {
        self.clients.read().unwrap().1.clone()
    }

    pub fn update_proxy(&self, proxy: Option<ProxySettings>) -> Result<()> {
        *self.clients.write().unwrap() =
            Self::build_clients(&self.cookies, &self.media_hosts, proxy.as_ref())?;
        Ok(())
    }

    pub fn is_media_url_allowed(&self, url: &Url) -> bool {
        is_media_url_allowed(url, &self.media_hosts.read().unwrap())
    }

    pub fn save_cookies(&self) -> Result<()> {
        let jar = self.cookies.lock().unwrap();
        atomic_write(&self.cookie_path, |file| {
            cookie_store::serde::json::save_incl_expired_and_nonpersistent(&jar, file)
                .map_err(|err| anyhow::anyhow!("save cookies.json: {err}"))
        })
    }

    pub fn get_cookie(&self, name: &str) -> Option<String> {
        let url = Url::parse(BILI_REFERER).expect("valid Bilibili URL");
        let jar = self.cookies.lock().unwrap();
        jar.get_request_values(&url)
            .find_map(|(key, value)| (key == name).then(|| value.to_owned()))
            .or_else(|| {
                jar.iter_unexpired()
                    .find(|cookie| cookie.name() == name)
                    .map(|cookie| cookie.value().to_owned())
            })
    }

    pub fn set_cookie(&self, name: &str, value: &str, expires: Option<u64>) -> Result<()> {
        ensure!(
            !name.is_empty() && name.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_'),
            "invalid cookie name"
        );
        ensure!(!value.contains([';', '\r', '\n']), "invalid cookie value");
        let max_age = expires.map_or(31_536_000, |ts| ts.saturating_sub(unix_time()).max(1));
        let url = Url::parse(BILI_REFERER)?;
        let raw = format!(
            "{name}={value}; Domain=.bilibili.com; Path=/; Secure; SameSite=None; Max-Age={max_age}"
        );
        self.cookies
            .lock()
            .unwrap()
            .parse(&raw, &url)
            .map_err(|err| anyhow::anyhow!("set cookie {name}: {err}"))?;
        self.save_cookies()
    }

    /// Refresh web identity cookies at most once per concurrent request group.
    pub async fn inject_auth_cookies(&self) {
        let mut last_refresh = self.auth_cookie_refresh.lock().await;
        if last_refresh.is_some_and(|time| time.elapsed() < Duration::from_secs(60)) {
            return;
        }
        *last_refresh = Some(Instant::now());
        let now = unix_time();
        let ticket = async {
            if self.get_cookie("bili_ticket").is_some() {
                return;
            }
            let signed = compute_bili_ticket_hexsign(now);
            let request = self
                .client()
                .post(format!(
                    "{API}/bapis/bilibili.api.ticket.v1.Ticket/GenWebTicket"
                ))
                .query(&[
                    ("key_id", "ec02"),
                    ("hexsign", &signed),
                    ("context[ts]", &now.to_string()),
                    ("csrf", ""),
                ]);
            if let Ok(reply) = checked_body(request).await
                && let Some(value) = reply["data"]["ticket"].as_str()
            {
                let expires = now + 259_260;
                self.set_auth_cookie("bili_ticket", value, expires);
                self.set_auth_cookie("bili_ticket_expires", &expires.to_string(), expires);
            }
        };
        let buvid = async {
            if self.get_cookie("buvid4").is_some() && self.get_cookie("buvid3").is_some() {
                return;
            }
            let request = self.client().get(format!("{API}/x/frontend/finger/spi_v2"));
            if let Ok(reply) = checked_body(request).await {
                for (name, key) in [("buvid3", "b_3"), ("buvid4", "b_4")] {
                    if self.get_cookie(name).is_none()
                        && let Some(value) = reply["data"][key].as_str()
                    {
                        self.set_auth_cookie(name, value, now + 30 * 24 * 3600);
                    }
                }
            }
        };
        tokio::join!(ticket, buvid);
    }

    async fn refresh_session_cookies(&self) {
        if self.get_cookie("SESSDATA").is_some() {
            self.inject_auth_cookies().await;
        }
    }

    fn set_auth_cookie(&self, name: &str, value: &str, expires: u64) {
        if let Err(err) = self.set_cookie(name, value, Some(expires)) {
            log::warn!("failed to persist Bilibili cookie {name}: {err}");
        }
    }

    async fn nav_data(&self) -> Result<Value> {
        let reply = fetch_body(self.client().get(format!("{API}{NAV}"))).await?;
        if reply["code"] == -101 && reply["data"].is_object() {
            return Ok(reply["data"].clone());
        }
        check_code(reply).map(|body| body["data"].clone())
    }

    async fn signed(&self, params: &Value) -> Result<Vec<(String, String)>> {
        let mut cache = self.wbi.lock().await;
        if cache
            .as_ref()
            .is_none_or(|(time, _, _)| time.elapsed() >= WBI_TTL)
        {
            let nav = self.nav_data().await?;
            let (img, sub) = extract_wbi_keys(&nav)?;
            *cache = Some((Instant::now(), img, sub));
        }
        let (_, img, sub) = cache.as_ref().unwrap();
        Ok(sign_wbi_params_with_keys(
            &parameters(params),
            img,
            sub,
            unix_time(),
        ))
    }

    async fn get(&self, base: &str, path: &str, params: Value, wbi: bool) -> Result<Value> {
        self.refresh_session_cookies().await;
        let query = if wbi {
            self.signed(&params).await?
        } else {
            parameters(&params)
        };
        let request = self.client().get(format!("{base}{path}")).query(&query);
        let mut body = checked_body(request).await?;
        Ok(body.get_mut("data").map(Value::take).unwrap_or(Value::Null))
    }

    async fn post(
        &self,
        base: &str,
        path: &str,
        form: Value,
        signed_query: Option<Value>,
    ) -> Result<Value> {
        self.refresh_session_cookies().await;
        let csrf = self
            .get_cookie("bili_jct")
            .context("Bilibili login required")?;
        let mut fields = parameters(&form);
        let key = if base == PASSPORT { "biliCSRF" } else { "csrf" };
        fields.push((key.into(), csrf));
        let mut request = self.client().post(format!("{base}{path}"));
        if let Some(query) = signed_query {
            request = request.query(&self.signed(&query).await?);
        }
        let (boundary, body) = multipart_fields(&fields);
        let mut reply = checked_body(
            request
                .header(
                    "Content-Type",
                    format!("multipart/form-data; boundary={boundary}"),
                )
                .body(body),
        )
        .await?;
        Ok(reply
            .get_mut("data")
            .map(Value::take)
            .unwrap_or(Value::Null))
    }

    fn registered_urls<'a>(&self, values: impl IntoIterator<Item = &'a Value>) -> Vec<Url> {
        let mut seen = HashSet::new();
        let mut hosts = self.media_hosts.write().unwrap();
        values
            .into_iter()
            .filter_map(Value::as_str)
            .filter_map(|value| Url::parse(value).ok())
            .filter(|url| {
                let Some(host) = url.host_str() else {
                    return false;
                };
                if !matches!(url.scheme(), "https" | "http")
                    || !matches!(url.host(), Some(url::Host::Domain(_)))
                    || !host.contains('.')
                    || !url.username().is_empty()
                    || url.password().is_some()
                    || url.port().is_some()
                {
                    return false;
                }
                hosts.insert(host.to_owned());
                seen.insert(url.clone())
            })
            .collect()
    }
}

fn multipart_fields(fields: &[(String, String)]) -> (String, String) {
    // Field names are endpoint constants. The random boundary is independent of user input.
    use std::fmt::Write;
    let boundary = format!("biu-{}", uuid::Uuid::new_v4());
    let mut body = String::new();
    for (key, value) in fields {
        write!(
            body,
            "--{boundary}\r\nContent-Disposition: form-data; name=\"{key}\"\r\n\r\n{value}\r\n"
        )
        .unwrap();
    }
    write!(body, "--{boundary}--\r\n").unwrap();
    (boundary, body)
}

fn unix_time() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs()
}

fn parameters(value: &Value) -> Vec<(String, String)> {
    value
        .as_object()
        .into_iter()
        .flatten()
        .filter(|(_, value)| !value.is_null())
        .map(|(key, value)| {
            (
                key.clone(),
                value
                    .as_str()
                    .map_or_else(|| value.to_string(), str::to_owned),
            )
        })
        .collect()
}

fn extract_wbi_keys(nav: &Value) -> Result<(String, String)> {
    fn key(value: &Value) -> Option<String> {
        let url = Url::parse(value.as_str()?).ok()?;
        let file = url.path_segments()?.next_back()?;
        let (name, _) = file.rsplit_once('.')?;
        (name.len() == 32 && name.bytes().all(|b| b.is_ascii_hexdigit())).then(|| name.to_owned())
    }
    let img = &nav["wbi_img"];
    Ok((
        key(&img["img_url"]).context("Bilibili nav has no WBI image key")?,
        key(&img["sub_url"]).context("Bilibili nav has no WBI sub key")?,
    ))
}

async fn fetch_body(request: RequestBuilder) -> Result<Value> {
    Ok(request
        .timeout(Duration::from_secs(20))
        .send()
        .await
        .map_err(reqwest::Error::without_url)?
        .error_for_status()
        .map_err(reqwest::Error::without_url)?
        .json()
        .await
        .map_err(reqwest::Error::without_url)?)
}

fn check_code(body: Value) -> Result<Value> {
    let code = body["code"]
        .as_i64()
        .context("Bilibili response has no status code")?;
    ensure!(
        code == 0,
        "Bilibili API error {code}: {}",
        field(&body, &["message", "msg"])
    );
    Ok(body)
}

async fn checked_body(request: RequestBuilder) -> Result<Value> {
    check_code(fetch_body(request).await?)
}

fn field<'a>(value: &'a Value, keys: &[&str]) -> &'a str {
    keys.iter()
        .filter_map(|key| value[*key].as_str())
        .find(|s| !s.is_empty())
        .unwrap_or_default()
}

fn number(value: &Value) -> Option<u64> {
    value.as_u64().or_else(|| value.as_str()?.parse().ok())
}

fn list(value: &Value) -> &[Value] {
    value.as_array().map(Vec::as_slice).unwrap_or_default()
}

fn decode<T: DeserializeOwned>(value: Value) -> Result<T> {
    Ok(serde_json::from_value(sanitize_fields(value))?)
}

fn sanitize_fields(mut value: Value) -> Value {
    match &mut value {
        Value::Array(items) => {
            for item in items {
                *item = sanitize_fields(item.take());
            }
        }
        Value::Object(fields) => {
            for (key, field) in fields.iter_mut() {
                if matches!(
                    key.as_str(),
                    "title" | "archive_title" | "music_title" | "part" | "uname" | "name"
                ) && let Some(text) = field.as_str()
                {
                    *field = Value::String(strip_html_tags(text));
                } else if field.is_object() || field.is_array() {
                    *field = sanitize_fields(field.take());
                }
            }
        }
        _ => {}
    }
    value
}

/// Strip HTML tags and decode the text entities used in Bilibili display fields.
pub fn strip_html_tags(input: &str) -> String {
    let mut output = String::with_capacity(input.len());
    let mut rest = input;
    while let Some(index) = rest.find(['<', '&']) {
        output.push_str(&rest[..index]);
        rest = &rest[index..];
        if rest.starts_with('<') {
            let next = rest.as_bytes().get(1).copied().unwrap_or_default();
            if (next.is_ascii_alphabetic() || matches!(next, b'/' | b'!'))
                && let Some(end) = rest[1..].find(['<', '>']).map(|i| i + 1)
                && rest.as_bytes()[end] == b'>'
            {
                rest = &rest[end + 1..];
                continue;
            }
        } else if let Some(end) = rest.bytes().take(17).position(|byte| byte == b';') {
            let entity = &rest[1..end];
            let decoded = match entity {
                "amp" => Some('&'),
                "lt" => Some('<'),
                "gt" => Some('>'),
                "quot" => Some('"'),
                "apos" => Some('\''),
                "nbsp" => Some(' '),
                _ => entity
                    .strip_prefix("#x")
                    .or_else(|| entity.strip_prefix("#X"))
                    .and_then(|s| u32::from_str_radix(s, 16).ok())
                    .or_else(|| entity.strip_prefix('#')?.parse().ok())
                    .and_then(char::from_u32),
            };
            if let Some(character) = decoded {
                output.push(character);
                rest = &rest[end + 1..];
                continue;
            }
        }
        output.push(rest.as_bytes()[0] as char);
        rest = &rest[1..];
    }
    output.push_str(rest);
    output
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn signs_known_wbi_vector() {
        let params = [
            ("foo".into(), "114".into()),
            ("bar".into(), "514".into()),
            ("baz".into(), "1919810".into()),
        ];
        let query = sign_wbi_params_with_keys(
            &params,
            "7cd084941338484aae1ad9425b84077c",
            "4932caff0ff746eab6f01bf08b70ac45",
            1_702_204_169,
        );
        assert_eq!(query.last().unwrap().1, "6149fdadf571698ca7e6a567265cd0ee");
    }

    #[test]
    fn strips_search_highlights_and_decodes_entities() {
        assert_eq!(
            strip_html_tags("1 < 2 &amp; <em class=\"keyword\">音乐</em> &#x266B;"),
            "1 < 2 & 音乐 ♫"
        );
        assert_eq!(strip_html_tags("a<em>日本</em> &bad;"), "a日本 &bad;");
        assert_eq!(strip_html_tags("x&lt;3"), "x<3");
    }

    #[test]
    fn persists_tauri_cookie_format_and_permissions() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let path = dir.path().join("cookies.json");
        let client = BiliClient::new(path.clone(), None)?;
        client.set_cookie("buvid3", "fixture", None)?;
        let restored = BiliClient::new(path.clone(), None)?;
        assert_eq!(restored.get_cookie("buvid3").as_deref(), Some("fixture"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(path)?.permissions().mode() & 0o777, 0o600);
        }
        Ok(())
    }
}
