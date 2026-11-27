//! Smart multi-provider lyrics matcher tailored for Bilibili music videos,
//! multi-P compilations, Bilibili audio, and local audio files.
//!
//! Pipeline:
//! 1. Multi-P aware title extraction (prefers meaningful `part_title` over compilation title).
//! 2. Noise-stripped query generation (extracts `《歌名》` / `「歌名」`, removes `【Hi-Res】` / `4K` / `无损`,
//!    and never forces a Bilibili re-uploader name into the primary query).
//! 3. Parallel fan-out across NetEase Cloud Music, QQ Music, and LRCLIB.
//! 4. Multi-signal candidate scoring (title exact/token overlap, artist match, duration proximity,
//!    version/modifier alignment, and ringtone penalty).
//! 5. Verified lyric fetch & richness ranking (rejects empty/instrumental/credit-only LRC;
//!    prefers synced YRC + translation when scores tie).
//! 6. Automatic fallback to Bilibili CC subtitles (`/x/player/wbi/v2`) when no music catalog
//!    candidate matches confidently.

use super::{LyricsClient, kana_to_romaji};
use crate::auto_lyrics::titles::{
    build_search_queries, clean_track_title, has_word, normalize, strong_match,
    without_trailing_note,
};
use crate::bili::BiliClient;
use crate::{LyricsCandidate, LyricsData, LyricsProvider, TrackItem, TrackSource};
use futures::{StreamExt, stream};
use std::collections::HashSet;

/// Bump this whenever auto-matching heuristics improve significantly so stale
/// low-quality auto-cached entries upgrade automatically (manual entries are untouched).
pub const SMART_MATCHER_VERSION: u32 = 2;

#[derive(Debug, Clone)]
pub struct TrackQueryInfo {
    pub title: String,
    pub part_title: Option<String>,
    pub artist: String,
    pub duration_secs: Option<f64>,
}

impl TrackQueryInfo {
    pub fn from_track(track: &TrackItem, duration_hint: Option<f64>) -> Self {
        let part_title = match &track.source {
            TrackSource::BiliVideo { part_title, .. } => part_title.clone(),
            _ => None,
        };
        let duration_secs = duration_hint
            .filter(|d| d.is_finite() && *d > 0.0)
            .or_else(|| (track.duration_secs.is_finite() && track.duration_secs > 0.0).then_some(track.duration_secs));
        Self {
            title: track.title.clone(),
            part_title,
            artist: track.artist.clone(),
            duration_secs,
        }
    }

    pub fn effective_title(&self) -> String {
        effective_search_title(&self.title, self.part_title.as_deref())
    }
}

#[derive(Debug, Clone)]
pub struct SmartLyricsMatch {
    pub lyrics: LyricsData,
    pub matched_candidate: Option<LyricsCandidate>,
    pub score: i32,
}

/// Choose the most informative title for lyrics lookup, preferring a meaningful
/// multi-P `part_title` over a compilation/album video title.
pub fn effective_search_title(title: &str, part_title: Option<&str>) -> String {
    if let Some(part) = part_title.map(strip_leading_track_index)
        && is_meaningful_part_title(&part)
    {
        return part;
    }
    strip_leading_track_index(title)
}

fn strip_leading_track_index(raw: &str) -> String {
    let s = raw.trim();
    // Strip leading patterns like "01. ", "1、", "P03 - ", "[02] ", "【P5】"
    let mut rest = s;
    for (l, r) in [('【', '】'), ('[', ']'), ('(', ')'), ('（', '）')] {
        if let Some(after_l) = rest.strip_prefix(l)
            && let Some((inner, after_r)) = after_l.split_once(r)
        {
            let inner_trim = inner.trim().to_ascii_lowercase();
            let is_part_tag = inner_trim
                .strip_prefix('p')
                .unwrap_or(&inner_trim)
                .chars()
                .all(|c| c.is_ascii_digit())
                && !inner_trim.is_empty();
            if is_part_tag && !after_r.trim().is_empty() {
                rest = after_r.trim();
            }
        }
    }
    let lower = rest.to_ascii_lowercase();
    if let Some(after_p) = lower.strip_prefix('p') {
        let digits = after_p
            .bytes()
            .take_while(|b| b.is_ascii_digit())
            .count();
        if digits > 0 {
            let after_num = &rest[1 + digits..];
            let trimmed = after_num.trim_start_matches([' ', '.', '、', '-', '_', '—', ':', '：']);
            if !trimmed.is_empty() && after_num.len() != trimmed.len() {
                return trimmed.trim().to_owned();
            }
        }
    }
    let digits = rest.bytes().take_while(|b| b.is_ascii_digit()).count();
    if digits > 0 && digits <= 3 {
        let after_num = &rest[digits..];
        let trimmed = after_num.trim_start_matches([' ', '.', '、', '-', '_', '—', ')']);
        if !trimmed.is_empty() && after_num.len() != trimmed.len() {
            return trimmed.trim().to_owned();
        }
    }
    rest.to_owned()
}

fn is_meaningful_part_title(part: &str) -> bool {
    let trimmed = part.trim();
    if trimmed.chars().count() < 2 {
        return false;
    }
    let lower = trimmed.to_ascii_lowercase();
    let generic = [
        "完整版", "原版", "mv", "pv", "live", "音频", "视频", "纯享", "纯享版", "正片", "花絮",
        "1080p", "4k", "hi-res", "hires", "flac", "mp3", "audio", "video", "full",
    ];
    if generic.contains(&lower.as_str()) {
        return false;
    }
    if lower
        .strip_prefix('p')
        .unwrap_or(&lower)
        .chars()
        .all(|c| c.is_ascii_digit() || matches!(c, ' ' | '.' | '-' | '_'))
    {
        return false;
    }
    true
}

/// Build clean, prioritized search queries without letting Bilibili re-uploader names
/// poison the primary query.
pub fn smart_search_queries(info: &TrackQueryInfo) -> Vec<String> {
    let primary_title = info.effective_title();
    let mut queries = Vec::new();
    let mut push = |q: String| {
        let q = q.trim().to_owned();
        let key = normalize(&q);
        if key.is_empty() || q.encode_utf16().count() > 36 {
            return;
        }
        if !queries.iter().any(|existing: &String| normalize(existing) == key) {
            queries.push(q);
        }
    };

    let base_candidates = build_search_queries(&primary_title);
    for c in base_candidates.iter().take(3) {
        push(c.clone());
    }

    // If multi-P part_title was used, also try combining it with an artist/album token from the main video title.
    if info.part_title.is_some() && primary_title != info.title {
        let parent_candidates = build_search_queries(&info.title);
        if let (Some(song), Some(parent_hint)) = (base_candidates.first(), parent_candidates.first())
            && normalize(song) != normalize(parent_hint)
            && parent_hint.chars().count() <= 12
        {
            push(format!("{song} {parent_hint}"));
        }
    }

    // Add a secondary query with `artist` (helpful when UP主 IS the real singer/official channel),
    // but NEVER make it the sole query so re-uploader names can't break matching.
    let artist = info.artist.trim();
    if !artist.is_empty()
        && artist.chars().count() <= 16
        && !looks_like_reuploader(artist)
        && let Some(first) = base_candidates.first()
        && !normalize(first).contains(&normalize(artist))
    {
        push(format!("{first} {artist}"));
    }

    let cleaned = clean_track_title(&primary_title);
    if !cleaned.is_empty() {
        push(cleaned);
    }
    drop(push);
    if queries.is_empty() && !primary_title.trim().is_empty() {
        queries.push(primary_title.trim().to_owned());
    }
    queries.truncate(4);
    queries
}

fn looks_like_reuploader(artist: &str) -> bool {
    let lower = artist.to_ascii_lowercase();
    [
        "搬运", "音乐馆", "音乐台", "音乐社", "电台", "合集", "歌单", "无损", "hi-res",
        "hires", "剪辑", "字幕组", "烤肉", "分享", "安利", "小站", "放映", "频道", "官方号",
        "每日", "精选", "音乐君", "推歌",
    ]
    .iter()
    .any(|kw| lower.contains(kw))
}

/// Multi-signal candidate scoring combining title match, artist match, duration proximity,
/// version modifier compatibility, and source quality.
pub fn score_candidate(candidate: &LyricsCandidate, info: &TrackQueryInfo) -> i32 {
    let effective = info.effective_title();
    let full_context = if info.part_title.is_some() {
        format!("{} {}", effective, info.title)
    } else {
        effective.clone()
    };
    let clean_eff = clean_track_title(&effective);
    let cand_base = without_trailing_note(&candidate.title);
    let cand_norm = normalize(cand_base);
    let eff_norm = normalize(&clean_eff);
    let raw_eff_norm = normalize(&effective);

    if cand_norm.is_empty() {
        return -100;
    }

    // 1. Title relevance (0..=55)
    let mut title_score = if eff_norm == cand_norm || raw_eff_norm == cand_norm {
        55
    } else if strong_match(cand_base, &effective) {
        46
    } else if candidate
        .aliases
        .iter()
        .any(|a| {
            let ab = without_trailing_note(a);
            !ab.is_empty() && (normalize(ab) == eff_norm || strong_match(ab, &effective))
        })
    {
        44
    } else if !eff_norm.is_empty() && (cand_norm.contains(&eff_norm) || eff_norm.contains(&cand_norm))
    {
        let ratio = (cand_norm.chars().count().min(eff_norm.chars().count()) as f64)
            / (cand_norm.chars().count().max(eff_norm.chars().count()).max(1) as f64);
        if ratio >= 0.5 {
            34
        } else {
            18
        }
    } else {
        let overlap = char_bigram_overlap(&eff_norm, &cand_norm);
        if overlap >= 0.55 {
            (overlap * 38.0).round() as i32
        } else if strong_match(cand_base, &full_context) {
            30
        } else {
            -25
        }
    };

    // Guard against matching super short generic 1-char titles accidentally
    if cand_norm.chars().count() == 1 && eff_norm != cand_norm {
        title_score = title_score.min(10);
    }

    // 2. Artist relevance (0..=25)
    let artist_match = candidate.artist.split(" / ").any(|singer| {
        let singer = singer.trim();
        let singer_base = without_trailing_note(singer);
        if singer_base.is_empty() {
            return false;
        }
        let s_norm = normalize(singer_base);
        strong_match(singer_base, &full_context)
            || strong_match(singer_base, &info.artist)
            || (!info.artist.is_empty() && normalize(&kana_to_romaji(singer_base)) == normalize(&info.artist))
            || (!s_norm.is_empty()
                && s_norm.chars().count() >= 2
                && (normalize(&full_context).contains(&s_norm)
                    || normalize(&info.artist).contains(&s_norm)))
    });
    let artist_score = if artist_match { 25 } else { 0 };

    // 3. Duration proximity (-40..=28)
    let duration_score = match (info.duration_secs, candidate.duration_secs) {
        (Some(target), Some(cand_dur)) if target > 0.0 && cand_dur > 0.0 => {
            if target > 90.0 && cand_dur < 45.0 {
                -40 // ringtone / short preview clip
            } else {
                let diff = (target - cand_dur).abs();
                if diff <= 2.0 {
                    28
                } else if diff <= 5.0 {
                    20
                } else if diff <= 12.0 {
                    10
                } else if diff <= 30.0 {
                    2
                } else if diff <= 60.0 {
                    -10
                } else {
                    -26
                }
            }
        }
        _ => 0,
    };

    // 4. Version/modifier penalty (avoid DJ/伴奏/Live/翻唱 unless requested by video title)
    let cand_lower = candidate.title.to_ascii_lowercase();
    let title_lower = full_context.to_ascii_lowercase();
    let mut modifier_penalty = 0;
    for (kw, pen) in [
        ("伴奏", 35),
        ("instrumental", 35),
        ("off vocal", 35),
        ("karaoke", 30),
        ("dj版", 25),
        ("remix", 18),
        ("翻唱", 18),
        ("cover", 14),
        ("live", 14),
        ("现场", 14),
        ("钢琴版", 20),
        ("吉他版", 18),
    ] {
        let cand_has = if kw.is_ascii() {
            has_word(&cand_lower, kw) || cand_lower.contains(kw)
        } else {
            cand_lower.contains(kw)
        };
        let title_has = if kw.is_ascii() {
            has_word(&title_lower, kw) || title_lower.contains(kw)
        } else {
            title_lower.contains(kw)
        };
        if cand_has && !title_has {
            modifier_penalty += pen;
        }
    }
    let note = normalize(
        candidate
            .title
            .trim()
            .strip_prefix(cand_base)
            .unwrap_or_default(),
    );
    if !note.is_empty() && !normalize(&full_context).contains(&note) && modifier_penalty == 0 {
        modifier_penalty += 6;
    }

    // 5. Provider richness tie-breaker
    let provider_bonus = match candidate.provider {
        LyricsProvider::NetEase => 3,
        LyricsProvider::QqMusic => 2,
        LyricsProvider::Lrclib => 0,
    };

    title_score + artist_score + duration_score - modifier_penalty + provider_bonus
}

fn char_bigram_overlap(a: &str, b: &str) -> f64 {
    let ac: Vec<char> = a.chars().collect();
    let bc: Vec<char> = b.chars().collect();
    if ac.len() < 2 || bc.len() < 2 {
        return 0.0;
    }
    let set_a: HashSet<(char, char)> = ac.windows(2).map(|w| (w[0], w[1])).collect();
    let set_b: HashSet<(char, char)> = bc.windows(2).map(|w| (w[0], w[1])).collect();
    let inter = set_a.intersection(&set_b).count();
    let denom = set_a.len().min(set_b.len()).max(1);
    inter as f64 / denom as f64
}

/// Returns true if the fetched lyrics contain real synced lyric lines (not empty,
/// not an instrumental placeholder, not credit-only metadata).
pub fn is_meaningful_lyrics(data: &LyricsData) -> bool {
    if data.lines.is_empty() {
        return false;
    }
    let has_timing = data.lines.iter().any(|l| l.start_secs > 0.0);
    if !has_timing && data.lines.len() < 3 {
        return false;
    }
    let instrumental_markers = [
        "纯音乐，请欣赏",
        "纯音乐,请欣赏",
        "此歌曲为没有填词的纯音乐",
        "该歌曲为纯音乐",
        "no lyrics",
        "instrumental",
    ];
    let sung_lines = data
        .lines
        .iter()
        .filter(|l| {
            let t = l.text.trim();
            if t.is_empty() || t == "♪" || t == "♫" {
                return false;
            }
            let lower = t.to_ascii_lowercase();
            if instrumental_markers.iter().any(|m| lower.contains(m)) {
                return false;
            }
            !is_metadata_line(&lower)
        })
        .count();
    sung_lines >= 3
}

fn is_metadata_line(lower: &str) -> bool {
    if let Some((prefix, _)) = lower.split_once([':', '：']) {
        let p = prefix.trim();
        return matches!(
            p,
            "作词" | "作詞" | "作曲" | "编曲" | "編曲" | "制作人" | "製作人" | "监制" | "混音"
                | "母带" | "和声" | "吉他" | "贝斯" | "鼓" | "键盘" | "弦乐" | "词" | "曲"
                | "演唱" | "原唱" | "翻唱" | "策划" | "录音" | "发行" | "op" | "sp"
                | "lyrics" | "composer" | "arranger" | "producer"
        );
    }
    false
}

/// Search across NetEase, QQ Music, and LRCLIB in parallel for a manual user query,
/// returning deduplicated candidates sorted by relevance.
pub async fn search_all_providers(
    client: &LyricsClient,
    query: &str,
    info: Option<&TrackQueryInfo>,
) -> (Vec<LyricsCandidate>, Vec<String>) {
    let (netease, qq, lrclib) = tokio::join!(
        client.search_netease(query, 15),
        client.search_qq(query, 15),
        client.search_lrclib(query),
    );
    let mut items = Vec::new();
    let mut errors = Vec::new();
    for (name, res) in [("网易云", netease), ("QQ音乐", qq), ("LRCLIB", lrclib)] {
        match res {
            Ok(mut list) => items.append(&mut list),
            Err(err) => errors.push(format!("{name}：{err}")),
        }
    }
    if let Some(info) = info {
        items.sort_by_key(|a| std::cmp::Reverse(score_candidate(a, info)));
    }
    (items, errors)
}

/// Best-effort automatic lyrics matching across NetEase, QQ Music, LRCLIB, and Bilibili CC subtitles.
pub async fn auto_match_lyrics(
    lyrics: &LyricsClient,
    bili: Option<&BiliClient>,
    track: &TrackItem,
    duration_hint: Option<f64>,
) -> Option<SmartLyricsMatch> {
    let info = TrackQueryInfo::from_track(track, duration_hint);
    let queries = smart_search_queries(&info);
    if queries.is_empty() {
        return None;
    }

    // Parallel fan-out: NetEase & QQ Music for top 3 queries, LRCLIB for top 2 queries.
    let mut futs: Vec<futures::future::BoxFuture<'_, Vec<LyricsCandidate>>> = Vec::new();
    for (idx, q) in queries.iter().take(3).enumerate() {
        let q1 = q.clone();
        futs.push(Box::pin(async move {
            lyrics.search_netease(&q1, 10).await.unwrap_or_default()
        }));
        let q2 = q.clone();
        futs.push(Box::pin(async move {
            lyrics.search_qq(&q2, 10).await.unwrap_or_default()
        }));
        if idx < 2 {
            let q3 = q.clone();
            futs.push(Box::pin(async move {
                lyrics.search_lrclib(&q3).await.unwrap_or_default()
            }));
        }
    }

    let batches: Vec<Vec<LyricsCandidate>> = stream::iter(futs).buffer_unordered(6).collect().await;

    let mut seen: HashSet<(LyricsProvider, String)> = HashSet::new();
    let mut scored: Vec<(i32, LyricsCandidate)> = Vec::new();
    for batch in batches {
        for cand in batch {
            let key = (
                cand.provider,
                cand.provider_key
                    .clone()
                    .unwrap_or_else(|| cand.id.to_string()),
            );
            if seen.insert(key) {
                let s = score_candidate(&cand, &info);
                if s >= 34 {
                    scored.push((s, cand));
                }
            }
        }
    }
    scored.sort_by(|(a, _), (b, _)| b.cmp(a));

    // Fetch top 4 highest-scoring candidates in parallel and pick the richest valid synced lyrics.
    let top: Vec<(i32, LyricsCandidate)> = scored.into_iter().take(4).collect();
    let fetched: Vec<(i32, LyricsCandidate, LyricsData)> = stream::iter(top.into_iter().map(
        |(base_score, cand)| async move {
            let data = lyrics.fetch(&cand).await.ok()?;
            if !is_meaningful_lyrics(&data) {
                return None;
            }
            let has_timing = data.lines.iter().any(|l| l.start_secs > 0.0);
            let has_yrc = data.lines.iter().any(|l| !l.words.is_empty());
            let has_trans = data.lines.iter().any(|l| l.translation.is_some());
            let richness = i32::from(has_timing) * 14
                + i32::from(has_yrc) * 6
                + i32::from(has_trans) * 4;
            Some((base_score + richness, cand, data))
        },
    ))
    .buffer_unordered(4)
    .filter_map(|x| async move { x })
    .collect()
    .await;

    let best_music = fetched.into_iter().max_by_key(|(total, _, _)| *total);

    // If a music catalog match is high-confidence, return it directly.
    if let Some((score, cand, data)) = best_music {
        if score >= 56 {
            return Some(SmartLyricsMatch {
                lyrics: data,
                matched_candidate: Some(cand),
                score,
            });
        }
        // Medium confidence: check if Bilibili video has exact CC subtitles first;
        // if not, still use our medium-confidence music lyrics!
        if let (Some(bili), TrackSource::BiliVideo { bvid, cid, .. }) = (bili, &track.source)
            && !bvid.is_empty()
            && let Ok(Some(cc)) = bili.get_video_subtitles(bvid, *cid).await
            && is_meaningful_lyrics(&cc)
        {
            return Some(SmartLyricsMatch {
                lyrics: cc,
                matched_candidate: None,
                score: 70,
            });
        }
        return Some(SmartLyricsMatch {
            lyrics: data,
            matched_candidate: Some(cand),
            score,
        });
    }

    // No music catalog hit at all -> fallback to Bilibili CC subtitles.
    if let (Some(bili), TrackSource::BiliVideo { bvid, cid, .. }) = (bili, &track.source)
        && !bvid.is_empty()
        && let Ok(Some(cc)) = bili.get_video_subtitles(bvid, *cid).await
        && is_meaningful_lyrics(&cc)
    {
        return Some(SmartLyricsMatch {
            lyrics: cc,
            matched_candidate: None,
            score: 65,
        });
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::parse_lyrics;

    fn cand(
        provider: LyricsProvider,
        title: &str,
        artist: &str,
        dur: Option<f64>,
    ) -> LyricsCandidate {
        LyricsCandidate {
            id: 1,
            title: title.into(),
            artist: artist.into(),
            album: String::new(),
            duration_secs: dur,
            provider,
            provider_key: (provider == LyricsProvider::QqMusic).then(|| "mid1".into()),
            aliases: Vec::new(),
        }
    }

    #[test]
    fn extracts_quoted_song_and_skips_reuploader_name() {
        let info = TrackQueryInfo {
            title: "【Hi-Res无损】《晴天》周杰伦 动态歌词 4K".into(),
            part_title: None,
            artist: "音乐搬运小王".into(),
            duration_secs: Some(269.0),
        };
        let queries = smart_search_queries(&info);
        assert_eq!(queries[0], "晴天");
        assert!(
            !queries.iter().any(|q| q.contains("音乐搬运小王")),
            "re-uploader name must not pollute queries: {queries:?}"
        );
    }

    #[test]
    fn prefers_meaningful_multi_p_part_title() {
        let info = TrackQueryInfo {
            title: "周杰伦经典50首合集 无损音质".into(),
            part_title: Some("05. 七里香".into()),
            artist: "HiRes音乐馆".into(),
            duration_secs: Some(299.0),
        };
        assert_eq!(info.effective_title(), "七里香");
        let queries = smart_search_queries(&info);
        assert_eq!(queries[0], "七里香");

        // Generic part title falls back to main video title.
        assert_eq!(effective_search_title("《晴天》周杰伦", Some("P1")), "《晴天》周杰伦");
        assert_eq!(effective_search_title("《晴天》周杰伦", Some("完整版")), "《晴天》周杰伦");
    }

    #[test]
    fn scores_studio_track_above_ringtone_and_instrumental() {
        let info = TrackQueryInfo {
            title: "【无损】《晴天》周杰伦".into(),
            part_title: None,
            artist: "某UP主".into(),
            duration_secs: Some(269.0),
        };
        let studio = cand(LyricsProvider::QqMusic, "晴天", "周杰伦", Some(269.0));
        let ringtone = cand(LyricsProvider::NetEase, "晴天", "周杰伦", Some(28.0));
        let inst = cand(LyricsProvider::NetEase, "晴天 (伴奏)", "周杰伦", Some(269.0));
        let wrong = cand(LyricsProvider::NetEase, "晴天娃娃", "其他人", Some(210.0));

        assert!(score_candidate(&studio, &info) > score_candidate(&ringtone, &info));
        assert!(score_candidate(&studio, &info) > score_candidate(&inst, &info));
        assert!(score_candidate(&studio, &info) > score_candidate(&wrong, &info));
        assert!(score_candidate(&studio, &info) >= 90);
    }

    #[test]
    fn rejects_instrumental_and_credit_only_lyrics() {
        let inst = parse_lyrics("[00:00.00]纯音乐，请欣赏", "", "", "");
        assert!(!is_meaningful_lyrics(&inst));

        let credits = parse_lyrics(
            "[00:01.00]作词：周杰伦\n[00:02.00]作曲：周杰伦\n[00:03.00]编曲：周杰伦",
            "",
            "",
            "",
        );
        assert!(!is_meaningful_lyrics(&credits));

        let real = parse_lyrics(
            "[00:01.00]作词：周杰伦\n[00:28.00]故事的小黄花\n[00:32.00]从出生那年就飘着\n[00:35.00]童年的荡秋千",
            "",
            "",
            "",
        );
        assert!(is_meaningful_lyrics(&real));
    }

    #[tokio::test]
    #[ignore = "live network check against NetEase, QQ Music, and LRCLIB"]
    async fn live_auto_match_tricky_bilibili_titles() {
        let client = LyricsClient::new().unwrap();
        let cases = [
            TrackItem {
                id: "case-jay".into(),
                title: "【Hi-Res无损】《晴天》周杰伦 动态歌词 4K".into(),
                artist: "音乐搬运小王".into(),
                artist_mid: None,
                cover: String::new(),
                duration_secs: 269.0,
                source: TrackSource::BiliVideo {
                    bvid: String::new(),
                    aid: 0,
                    cid: 0,
                    part_title: None,
                },
            },
            TrackItem {
                id: "case-multip".into(),
                title: "周杰伦经典50首合集 无损音质".into(),
                artist: "HiRes音乐馆".into(),
                artist_mid: None,
                cover: String::new(),
                duration_secs: 299.0,
                source: TrackSource::BiliVideo {
                    bvid: String::new(),
                    aid: 0,
                    cid: 0,
                    part_title: Some("05. 七里香".into()),
                },
            },
            TrackItem {
                id: "case-jp".into(),
                title: "【中字】米津玄師 - Lemon MV".into(),
                artist: "某动漫剪辑".into(),
                artist_mid: None,
                cover: String::new(),
                duration_secs: 256.0,
                source: TrackSource::BiliVideo {
                    bvid: String::new(),
                    aid: 0,
                    cid: 0,
                    part_title: None,
                },
            },
        ];

        for track in &cases {
            let matched = auto_match_lyrics(&client, None, track, None)
                .await
                .unwrap_or_else(|| panic!("expected lyrics match for {}", track.id));
            let cand_desc = matched
                .matched_candidate
                .as_ref()
                .map(|c| format!("{} - {} ({:?})", c.title, c.artist, c.provider))
                .unwrap_or_else(|| "none".into());
            let sample = matched
                .lyrics
                .lines
                .iter()
                .find(|l| l.start_secs > 15.0)
                .or_else(|| matched.lyrics.lines.first())
                .map(|l| l.text.as_str())
                .unwrap_or("");
            println!(
                "[LIVE OK] {} => {} | score={} | lines={} | source={:?} | sample={:?}",
                track.id,
                cand_desc,
                matched.score,
                matched.lyrics.lines.len(),
                matched.lyrics.source,
                sample
            );
            assert!(matched.lyrics.lines.len() >= 10);
            assert!(is_meaningful_lyrics(&matched.lyrics));
        }
    }
}
