use crate::bili::{USER_AGENT, allowlist_redirect_policy};
use crate::store::Store;
use crate::{LyricLine, LyricWord, LyricsCandidate, LyricsData, LyricsProvider};
use anyhow::{Context, Result, bail, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value, json};
use std::time::Duration;
use url::Url;

pub mod matcher;
mod romaji;
pub use romaji::kana_to_romaji;

const NETEASE: &str = "https://interface.music.163.com";
const QQMUSIC: &str = "https://c.y.qq.com";
const LRCLIB: &str = "https://lrclib.net";

#[derive(Clone)]
pub struct LyricsClient {
    client: reqwest::Client,
}

impl LyricsClient {
    pub fn new() -> Result<Self> {
        let client = reqwest::Client::builder()
            .user_agent(USER_AGENT)
            .pool_max_idle_per_host(4)
            .pool_idle_timeout(Duration::from_secs(30))
            .connect_timeout(Duration::from_secs(8))
            .timeout(Duration::from_secs(10))
            .redirect(allowlist_redirect_policy(|url: &Url| {
                matches!(url.scheme(), "http" | "https")
                    && url.username().is_empty()
                    && url.password().is_none()
                    && matches!(
                        url.host_str(),
                        Some("interface.music.163.com" | "c.y.qq.com" | "lrclib.net")
                    )
            }))
            .build()?;
        Ok(Self { client })
    }

    async fn get(&self, base: &str, path: &str, query: &[(&str, &str)]) -> Result<Value> {
        let mut request = self.client.get(format!("{base}{path}")).query(query);
        if base == NETEASE {
            request = request
                .header("Referer", "https://music.163.com/")
                .header("Origin", "https://music.163.com");
        } else if base == QQMUSIC {
            request = request
                .header("Referer", "https://y.qq.com/")
                .header("Origin", "https://y.qq.com");
        } else {
            request = request.header("User-Agent", "got (https://github.com/sindresorhus/got)");
        }
        for attempt in 0..3 {
            let next = request
                .try_clone()
                .expect("GET has no streaming body")
                .send()
                .await;
            match next {
                Ok(resp) if attempt < 2 && resp.status().is_server_error() => {
                    tokio::time::sleep(Duration::from_millis(300)).await;
                }
                Err(err) if attempt < 2 && (err.is_connect() || err.is_timeout()) => {
                    tokio::time::sleep(Duration::from_millis(200)).await;
                }
                result => {
                    return Ok(result
                        .map_err(reqwest::Error::without_url)?
                        .error_for_status()
                        .map_err(reqwest::Error::without_url)?
                        .json()
                        .await
                        .map_err(reqwest::Error::without_url)?);
                }
            }
        }
        unreachable!("the final attempt always returns")
    }

    pub async fn search_netease(&self, query: &str, limit: u32) -> Result<Vec<LyricsCandidate>> {
        let data = self
            .get(
                NETEASE,
                "/api/search/get",
                &[
                    ("s", query),
                    ("type", "1"),
                    ("offset", "0"),
                    ("limit", &limit.to_string()),
                ],
            )
            .await?;
        ensure!(
            data["code"] == 200,
            "NetEase search failed with code {}",
            data["code"]
        );
        Ok(data["result"]["songs"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|song| {
                let artists = song["artists"].as_array().or_else(|| song["ar"].as_array());
                Some(LyricsCandidate {
                    id: song["id"].as_u64()?,
                    title: song["name"].as_str()?.to_owned(),
                    artist: artists
                        .into_iter()
                        .flatten()
                        .filter_map(|a| a["name"].as_str())
                        .collect::<Vec<_>>()
                        .join(" / "),
                    album: song["album"]["name"]
                        .as_str()
                        .or_else(|| song["al"]["name"].as_str())
                        .unwrap_or_default()
                        .to_owned(),
                    duration_secs: song["duration"]
                        .as_f64()
                        .or_else(|| song["dt"].as_f64())
                        .map(|ms| ms / 1000.0),
                    provider: LyricsProvider::NetEase,
                    provider_key: None,
                    aliases: ["alias", "aliases", "transNames", "tns"]
                        .into_iter()
                        .flat_map(|key| song[key].as_array().into_iter().flatten())
                        .filter_map(|value| value.as_str().map(str::to_owned))
                        .collect(),
                })
            })
            .collect())
    }

    pub async fn search_qq(&self, query: &str, limit: u32) -> Result<Vec<LyricsCandidate>> {
        let limit_str = limit.clamp(1, 30).to_string();
        let data = self
            .get(
                QQMUSIC,
                "/soso/fcgi-bin/client_search_cp",
                &[
                    ("w", query),
                    ("format", "json"),
                    ("p", "1"),
                    ("n", &limit_str),
                ],
            )
            .await?;
        ensure!(
            data["code"] == 0,
            "QQ Music search failed with code {}",
            data["code"]
        );
        Ok(data["data"]["song"]["list"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(|song| {
                let songmid = song["songmid"].as_str()?.trim();
                if songmid.is_empty() {
                    return None;
                }
                let id = song["songid"].as_u64().unwrap_or(0);
                let title = unescape_html(song["songname"].as_str()?);
                let artist = song["singer"]
                    .as_array()
                    .into_iter()
                    .flatten()
                    .filter_map(|s| s["name"].as_str())
                    .map(unescape_html)
                    .collect::<Vec<_>>()
                    .join(" / ");
                let album = song["albumname"]
                    .as_str()
                    .map(unescape_html)
                    .unwrap_or_default();
                let duration_secs = song["interval"].as_f64().filter(|d| *d > 0.0);
                Some(LyricsCandidate {
                    id,
                    title,
                    artist,
                    album,
                    duration_secs,
                    provider: LyricsProvider::QqMusic,
                    provider_key: Some(songmid.to_owned()),
                    aliases: Vec::new(),
                })
            })
            .collect())
    }

    pub async fn search_lrclib(&self, query: &str) -> Result<Vec<LyricsCandidate>> {
        let data = self.get(LRCLIB, "/api/search", &[("q", query)]).await?;
        Ok(data
            .as_array()
            .context("invalid LRCLIB search response")?
            .iter()
            .filter_map(|song| {
                Some(LyricsCandidate {
                    id: song["id"].as_u64()?,
                    title: song["trackName"]
                        .as_str()
                        .or_else(|| song["name"].as_str())?
                        .to_owned(),
                    artist: song["artistName"].as_str().unwrap_or_default().to_owned(),
                    album: song["albumName"].as_str().unwrap_or_default().to_owned(),
                    duration_secs: song["duration"].as_f64(),
                    provider: LyricsProvider::Lrclib,
                    provider_key: None,
                    aliases: Vec::new(),
                })
            })
            .collect())
    }

    pub async fn fetch_netease(&self, id: u64) -> Result<LyricsData> {
        let data = self
            .get(
                NETEASE,
                "/api/song/lyric",
                &[
                    ("id", &id.to_string()),
                    ("tv", "-1"),
                    ("lv", "-1"),
                    ("rv", "-1"),
                    ("kv", "-1"),
                    ("_nmclfl", "1"),
                    ("yv", "-1"),
                    ("ytv", "-1"),
                    ("yrv", "-1"),
                ],
            )
            .await?;
        ensure!(
            data["code"] == 200,
            "NetEase lyrics failed with code {}",
            data["code"]
        );
        let text = |key: &str| data[key]["lyric"].as_str().unwrap_or_default();
        let mut lyrics = parse_lyrics(text("lrc"), text("yrc"), text("tlyric"), text("romalrc"));
        if lyrics.lines.iter().any(|line| !line.words.is_empty()) {
            apply_companions(&mut lyrics.lines, text("ytlrc"), false);
            apply_companions(&mut lyrics.lines, text("yromalrc"), true);
        }
        lyrics.source = Some("netease".into());
        Ok(lyrics)
    }

    pub async fn fetch_lrclib(&self, id: u64) -> Result<LyricsData> {
        let data = self.get(LRCLIB, &format!("/api/get/{id}"), &[]).await?;
        let mut lines = parse_lrc(data["syncedLyrics"].as_str().unwrap_or_default());
        if lines.is_empty()
            && let Some(plain) = data["plainLyrics"]
                .as_str()
                .filter(|text| !text.trim().is_empty())
        {
            lines.push(LyricLine {
                text: plain.trim().to_owned(),
                ..LyricLine::default()
            });
        }
        add_romaji(&mut lines);
        Ok(LyricsData {
            lines,
            source: Some("lrclib".into()),
        })
    }

    pub async fn fetch_qq(&self, songmid: &str) -> Result<LyricsData> {
        let data = self
            .get(
                QQMUSIC,
                "/lyric/fcgi-bin/fcg_query_lyric_new.fcg",
                &[("songmid", songmid), ("format", "json"), ("nobase64", "1")],
            )
            .await?;
        ensure!(
            data["code"] == 0,
            "QQ Music lyrics failed with code {}",
            data["code"]
        );
        let raw_lrc = unescape_html(data["lyric"].as_str().unwrap_or_default());
        let raw_trans = unescape_html(data["trans"].as_str().unwrap_or_default());
        let cleaned_trans = raw_trans
            .lines()
            .filter(|line| {
                let text = line
                    .rsplit_once(']')
                    .map(|(_, rest)| rest.trim())
                    .unwrap_or("");
                !text.is_empty() && text != "//"
            })
            .collect::<Vec<_>>()
            .join("\n");
        let mut lyrics = parse_lyrics(&raw_lrc, "", &cleaned_trans, "");
        lyrics.source = Some("qqmusic".into());
        Ok(lyrics)
    }

    pub async fn fetch(&self, candidate: &LyricsCandidate) -> Result<LyricsData> {
        match candidate.provider {
            LyricsProvider::NetEase => self.fetch_netease(candidate.id).await,
            LyricsProvider::QqMusic => {
                let mid = candidate
                    .provider_key
                    .as_deref()
                    .filter(|s| !s.is_empty())
                    .context("QQ Music candidate missing songmid")?;
                self.fetch_qq(mid).await
            }
            LyricsProvider::Lrclib => self.fetch_lrclib(candidate.id).await,
        }
    }
}

pub(crate) fn unescape_html(input: &str) -> String {
    if !input.contains('&') {
        return input.to_owned();
    }
    input
        .replace("&apos;", "'")
        .replace("&#39;", "'")
        .replace("&#039;", "'")
        .replace("&quot;", "\"")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&nbsp;", " ")
        .replace("&amp;", "&")
}

/// Lines and word starts are in seconds from the start of the audio.
pub fn parse_lrc(source: &str) -> Vec<LyricLine> {
    let offset = source
        .lines()
        .rev()
        .find_map(|row| {
            let row = row.trim().trim_start_matches('\u{feff}');
            row.strip_prefix("[offset:")
                .or_else(|| row.strip_prefix("[OFFSET:"))?
                .strip_suffix(']')?
                .trim()
                .parse::<i64>()
                .ok()
        })
        .unwrap_or_default() as f64
        / 1000.0;
    let mut lines = Vec::new();
    for row in source.lines() {
        let mut rest = row.trim().trim_start_matches('\u{feff}');
        let mut times = Vec::new();
        while let Some(tag) = rest.strip_prefix('[').and_then(|s| s.split_once(']')) {
            let Some(time) = lrc_time(tag.0) else { break };
            times.push(time);
            rest = tag.1;
        }
        let text = rest.trim();
        if !text.is_empty() {
            lines.extend(times.into_iter().map(|t| LyricLine {
                start_secs: (t + offset).max(0.0),
                text: text.to_owned(),
                ..LyricLine::default()
            }));
        }
    }
    lines.sort_by(|a, b| a.start_secs.total_cmp(&b.start_secs));
    lines
}

fn lrc_time(tag: &str) -> Option<f64> {
    let (minutes, rest) = tag.split_once(':')?;
    let (seconds, fraction) = rest.split_once(['.', ':']).unwrap_or((rest, ""));
    let seconds = seconds.parse::<u32>().ok()?;
    if seconds >= 60 || fraction.len() > 3 {
        return None;
    }
    let fraction_value = if fraction.is_empty() {
        0.0
    } else {
        f64::from(fraction.parse::<u32>().ok()?) / 10_f64.powi(fraction.len() as i32)
    };
    Some(minutes.parse::<u64>().ok()? as f64 * 60.0 + f64::from(seconds) + fraction_value)
}

pub fn parse_yrc(source: &str) -> Vec<LyricLine> {
    let mut lines = Vec::new();
    for row in source.lines() {
        let Some((line_tag, rest)) = row
            .trim_start_matches('\u{feff}')
            .trim()
            .strip_prefix('[')
            .and_then(|s| s.split_once(']'))
        else {
            continue;
        };
        let Some((start, duration)) = line_tag.split_once(',').and_then(|(start, duration)| {
            Some((start.parse::<i64>().ok()?, duration.parse::<u64>().ok()?))
        }) else {
            continue;
        };
        let mut markers = rest
            .match_indices('(')
            .filter_map(|(index, _)| {
                let (fields, _) = rest[index + 1..].split_once(')')?;
                let mut values = fields.split(',');
                let start = values.next()?.parse::<i64>().ok()?;
                let duration = values.next()?.parse::<u64>().ok()?;
                values.next()?.parse::<u32>().ok()?;
                values.next().is_none().then_some((
                    index,
                    index + 2 + fields.len(),
                    start,
                    duration,
                ))
            })
            .peekable();
        let mut words = Vec::new();
        while let Some((_, end, word_start, word_duration)) = markers.next() {
            let text = &rest[end..markers.peek().map_or(rest.len(), |next| next.0)];
            if !text.is_empty() {
                words.push(LyricWord {
                    start_secs: word_start as f64 / 1000.0,
                    duration_secs: word_duration as f64 / 1000.0,
                    text: text.to_owned(),
                });
            }
        }
        let line_start_secs = start as f64 / 1000.0;
        let line_duration_secs = duration as f64 / 1000.0;
        let relative = words
            .first()
            .is_some_and(|word| word.start_secs < line_start_secs - 0.05)
            && words
                .iter()
                .all(|word| (0.0..=line_duration_secs).contains(&word.start_secs));
        for word in &mut words {
            let offset = if relative { line_start_secs } else { 0.0 };
            word.start_secs = (word.start_secs + offset).max(0.0);
        }
        let text = if words.is_empty() {
            rest.trim().to_owned()
        } else {
            words
                .iter()
                .map(|word| word.text.as_str())
                .collect::<String>()
                .trim()
                .to_owned()
        };
        if !text.is_empty() {
            lines.push(LyricLine {
                start_secs: line_start_secs.max(0.0),
                duration_secs: Some(line_duration_secs),
                text,
                words,
                ..LyricLine::default()
            });
        }
    }
    lines.sort_by(|a, b| a.start_secs.total_cmp(&b.start_secs));
    lines
}

pub fn parse_lyrics(lrc: &str, yrc: &str, translation: &str, romaji: &str) -> LyricsData {
    let mut lines = parse_yrc(yrc);
    if lines.is_empty() {
        lines = parse_lrc(lrc);
    }
    apply_companions(&mut lines, translation, false);
    apply_companions(&mut lines, romaji, true);
    add_romaji(&mut lines);
    LyricsData {
        lines,
        source: None,
    }
}

fn apply_companions(lines: &mut [LyricLine], companion: &str, romaji: bool) {
    let mut parsed = parse_lrc(companion);
    if parsed.is_empty() {
        parsed = parse_yrc(companion);
    }
    let mut from = 0;
    for line in lines {
        let tolerance = if line.words.is_empty() { 0.011 } else { 0.15 };
        from +=
            parsed[from..].partition_point(|other| other.start_secs < line.start_secs - tolerance);
        let best = parsed[from..]
            .iter()
            .take_while(|other| other.start_secs <= line.start_secs + tolerance)
            .enumerate()
            .min_by(|(_, a), (_, b)| {
                (a.start_secs - line.start_secs)
                    .abs()
                    .total_cmp(&(b.start_secs - line.start_secs).abs())
            });
        if let Some((offset, matched)) = best {
            let field = if romaji {
                &mut line.romaji
            } else {
                &mut line.translation
            };
            *field = Some(matched.text.clone());
            from += offset + 1;
        }
    }
}

fn add_romaji(lines: &mut [LyricLine]) {
    for line in lines {
        if line.romaji.is_none() && romaji::contains_kana(&line.text) {
            line.romaji = Some(kana_to_romaji(&line.text));
        }
    }
}

pub fn active_line_index(lines: &[LyricLine], current_time_secs: f64) -> Option<usize> {
    lines
        .partition_point(|line| line.start_secs <= current_time_secs)
        .checked_sub(1)
}

pub fn word_progress(word: &LyricWord, current_time_secs: f64) -> f64 {
    if current_time_secs.is_nan() || current_time_secs < word.start_secs {
        return 0.0;
    }
    if word.duration_secs <= 0.0 {
        return 1.0;
    }
    ((current_time_secs - word.start_secs) / word.duration_secs).clamp(0.0, 1.0)
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct CachedLyrics {
    pub lyrics: Option<String>,
    pub t_lyrics: Option<String>,
    pub r_lyrics: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub yrc: Option<String>,
    pub offset: f64,
    pub source: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl CachedLyrics {
    pub fn data(&self) -> LyricsData {
        let mut lyrics = parse_lyrics(
            self.lyrics.as_deref().unwrap_or_default(),
            self.yrc.as_deref().unwrap_or_default(),
            self.t_lyrics.as_deref().unwrap_or_default(),
            self.r_lyrics.as_deref().unwrap_or_default(),
        );
        lyrics.source = self.source.clone();
        shift_lyrics(&mut lyrics, self.offset / 1000.0);
        lyrics
    }
}

pub fn shift_lyrics(lyrics: &mut LyricsData, offset_secs: f64) {
    if !offset_secs.is_finite() {
        return;
    }
    for line in &mut lyrics.lines {
        line.start_secs = (line.start_secs + offset_secs).max(0.0);
        for word in &mut line.words {
            word.start_secs = (word.start_secs + offset_secs).max(0.0);
        }
    }
}

pub fn load_cached_lyrics(store: &Store, key: &str) -> Result<Option<CachedLyrics>> {
    let value = store.read("lyrics-cache")?;
    value
        .get(key)
        .filter(|entry| !entry.is_null())
        .map(|entry| serde_json::from_value(entry.clone()))
        .transpose()
        .map_err(Into::into)
}

pub fn save_cached_lyrics(
    store: &Store,
    key: &str,
    lyrics: &CachedLyrics,
    manual: bool,
) -> Result<()> {
    if key.is_empty() {
        bail!("lyrics cache key is empty");
    }
    store.merge("lyrics-cache", json!({ (key): lyrics }), manual)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lrc_tags_offset_and_companions() {
        let lrc = "[ar:artist]\n[offset:+250]\n[01:02.30][00:02:3] はい\n[00:66] bad";
        let lines = parse_lyrics(lrc, "", "[00:02.55] yes", "").lines;
        assert_eq!(lines.len(), 2);
        assert!((lines[0].start_secs - 2.55).abs() < 1e-9);
        assert_eq!(lines[0].translation.as_deref(), Some("yes"));
        assert_eq!(lines[0].romaji.as_deref(), Some("hai"));
        assert_eq!(active_line_index(&lines, 1.0), None);
        assert_eq!(active_line_index(&lines, 65.0), Some(1));
    }

    #[test]
    fn parses_yrc_word_timings_and_literal_parentheses() {
        let data = parse_lyrics(
            "",
            "[1000,800](1000,400,0)こ(aside)(1400,400,0)こ",
            "[00:01.00] here",
            "",
        );
        let line = &data.lines[0];
        assert_eq!(line.text, "こ(aside)こ");
        assert_eq!(line.translation.as_deref(), Some("here"));
        assert_eq!(line.words[1].text, "こ");
        assert!((word_progress(&line.words[1], 1.6) - 0.5).abs() < 1e-9);
        let alternate = parse_yrc("[-50,100](-50,100,0)lead\n[3000,800](0,400,0)go(400,400,0)now");
        assert_eq!(alternate[0].words[0].start_secs, 0.0);
        assert_eq!(alternate[1].words[1].start_secs, 3.4);
    }

    #[test]
    fn loads_tauri_cache_using_millisecond_offsets() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let store = Store::new(dir.path());
        store.merge("lyrics-cache", json!({"BV-9":{"lyrics":"[00:02.00]x","tLyrics":"[00:02.00]y","offset":-500,"source":"auto"}}), false)?;
        let cached = load_cached_lyrics(&store, "BV-9")?.unwrap();
        assert_eq!(cached.data().lines[0].start_secs, 1.5);
        assert_eq!(cached.data().lines[0].translation.as_deref(), Some("y"));
        Ok(())
    }
}
