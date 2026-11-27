//! Automatic lyrics matching and alignment for local or fully cached recordings.

use crate::bili::BiliClient;
use crate::download::find_ffmpeg;
use crate::lyrics::LyricsClient;
use crate::media_cache::{MediaCache, PreparedAudio};
use crate::{
    AudioQuality, AutoLyricsResult, LyricLine, LyricsCandidate, LyricsData, TrackItem, TrackSource,
};
use anyhow::{Context, Result, bail, ensure};
use futures::{StreamExt, stream};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::{Arc, atomic::Ordering};
use std::time::Duration;
use tokio::io::{AsyncReadExt, BufReader};

mod alignment;
mod features;
pub(crate) mod titles;

pub use alignment::{GlobalOffset, estimate_global_offset};
pub use features::{AudioFeatures, Fft, SAMPLE_RATE, extract_features};
pub use titles::{build_search_queries, build_title_candidates, clean_track_title};

use alignment::{align_lines, consensus_reference, verify_score};
use features::{FeatureExtractor, has_vocal_onset};
use titles::{has_word, normalize, strong_match, without_trailing_note};

const DECODE_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const MAX_AUDIO_SECS: usize = 30 * 60;

pub struct AutoLyrics<'a> {
    pub bili: &'a BiliClient,
    pub lyrics: &'a LyricsClient,
    pub media_cache: &'a Arc<MediaCache>,
    pub ffmpeg: Option<&'a Path>,
}

impl AutoLyrics<'_> {
    pub async fn run(
        &self,
        audio_path: &Path,
        title: &str,
        artist: &str,
    ) -> Result<AutoLyricsResult> {
        run_auto_lyrics(self, audio_path, title, artist).await
    }
}

/// Decode a recording, identify the release, and verify timestamps against studio audio.
pub async fn run_auto_lyrics(
    client: &AutoLyrics<'_>,
    audio_path: &Path,
    title: &str,
    artist: &str,
) -> Result<AutoLyricsResult> {
    ensure!(
        !title.trim().is_empty(),
        "a song title is needed for auto lyrics"
    );
    ensure!(
        !contains_any(
            title,
            &[
                "合集",
                "全集",
                "串烧",
                "串燒",
                "メドレー",
                "全曲",
                "教程",
                "教学",
                "教學",
                "乐谱",
                "樂譜"
            ]
        ) && !has_word(title, "medley")
            && !has_word(title, "playlist"),
        "title describes a collection or tutorial"
    );
    let ffmpeg = find_ffmpeg(client.ffmpeg)
        .await
        .context("ffmpeg unavailable for automatic lyrics")?;
    let (target, candidates) = tokio::try_join!(
        decode_audio_features(&ffmpeg, audio_path),
        search_candidates(client.lyrics, title, artist)
    )?;
    let target = Arc::new(target);
    let mut ranked = candidates
        .into_iter()
        .filter_map(|candidate| {
            candidate_score(&candidate, title, artist, target.duration_secs)
                .map(|score| (score, candidate))
        })
        .collect::<Vec<_>>();
    ranked.sort_by(|(a, _), (b, _)| b.cmp(a));
    ensure!(!ranked.is_empty(), "no lyrics candidate matched the title");
    let ambiguous = ranked.get(1).is_some_and(|(score, candidate)| {
        ranked[0].0 - score < 12
            && (normalize(&candidate.title) != normalize(&ranked[0].1.title)
                || normalize(&candidate.artist) != normalize(&ranked[0].1.artist))
    });
    let mut matches = Vec::new();
    let mut self_reference = None;
    for (index, (_, candidate)) in ranked.into_iter().take(3).enumerate() {
        let mut lyrics = match client.lyrics.fetch(&candidate).await {
            Ok(lyrics) => lyrics,
            Err(err) => {
                log::debug!("lyrics candidate {} failed: {err}", candidate.id);
                continue;
            }
        };
        if !lyrics.lines.iter().any(|line| line.start_secs > 0.0) {
            continue;
        }
        lyrics.lines.retain(|line| !is_credit(&line.text));
        let Some(first) = lyrics.lines.first().map(|line| line.start_secs) else {
            continue;
        };
        let Some(release_duration) = candidate.duration_secs.filter(|duration| *duration > 0.0)
        else {
            continue;
        };
        let sources = client
            .references(&candidate, &ffmpeg, first)
            .await
            .unwrap_or_else(|err| {
                log::debug!("reference search failed: {err}");
                Vec::new()
            });

        if sources.is_empty() {
            if index == 0
                && !ambiguous
                && !is_live(title)
                && (target.duration_secs - release_duration).abs() <= 2.0
                && has_vocal_onset(&target, first)
            {
                lyrics.source = Some("auto".into());
                self_reference = Some(AutoLyricsResult {
                    lyrics,
                    candidate,
                    reference: None,
                    offset_secs: 0.0,
                    confidence: 0.6,
                });
            }
            continue;
        }

        let target = target.clone();
        let (score, aligned) = tokio::task::spawn_blocking(move || {
            align_candidate(sources, &target, candidate, lyrics)
        })
        .await?;
        matches.push((score, aligned));
        if !ambiguous && score >= 0.5 {
            break;
        }
    }
    if matches.is_empty()
        && let Some(result) = self_reference
    {
        return Ok(result);
    }
    matches.sort_by(|(a, _), (b, _)| b.total_cmp(a));
    if let Some((score, result)) = matches.first() {
        let tied = matches.get(1).is_some_and(|(second, other)| {
            score - second < 0.05 && !same_lyrics(&result.lyrics, &other.lyrics)
        });
        if *score >= if ambiguous { 0.72 } else { 0.5 } && !tied {
            return Ok(matches.remove(0).1);
        }
    }
    bail!("no reference audio aligned confidently with the recording")
}

impl AutoLyrics<'_> {
    async fn references(
        &self,
        candidate: &LyricsCandidate,
        ffmpeg: &Path,
        first_line: f64,
    ) -> Result<Vec<(TrackItem, AudioFeatures)>> {
        let title = without_trailing_note(&candidate.title);
        let first_artist = candidate.artist.split(" / ").next().unwrap_or_default();
        let search = self
            .bili
            .search_videos_with_page_size(&format!("{title} {first_artist}"), "totalrank", 1, 50)
            .await?;
        let duration = candidate.duration_secs.unwrap_or_default();
        let mut tracks = search
            .items
            .into_iter()
            .filter(|track| {
                (track.duration_secs - duration).abs() <= 2.0 && strong_match(title, &track.title)
            })
            .collect::<Vec<_>>();
        tracks.sort_by(|a, b| {
            reference_score(b, duration)
                .cmp(&reference_score(a, duration))
                .then_with(|| {
                    (a.duration_secs - duration)
                        .abs()
                        .total_cmp(&(b.duration_secs - duration).abs())
                })
                .then_with(|| a.id.cmp(&b.id))
        });
        tracks.dedup_by(|a, b| a.id == b.id);
        let results = stream::iter(tracks.into_iter().take(3).map(|track| async move {
            let path = self.reference_path(&track).await?;
            let features = decode_audio_features(ffmpeg, &path).await?;
            ensure!(
                has_vocal_onset(&features, first_line),
                "reference vocal onset mismatch"
            );
            Ok::<_, anyhow::Error>((track, features))
        }))
        .buffered(2)
        .collect::<Vec<_>>()
        .await;
        Ok(results
            .into_iter()
            .filter_map(|result| {
                result
                    .map_err(|err| {
                        log::debug!("audio reference failed: {err}");
                    })
                    .ok()
            })
            .collect())
    }

    async fn reference_path(&self, track: &TrackItem) -> Result<PathBuf> {
        let TrackSource::BiliVideo { bvid, cid, .. } = &track.source else {
            bail!("audio reference is not a Bilibili video");
        };
        let info = self.bili.get_playurl(bvid, *cid, AudioQuality::Low).await?;
        let mut last_error = None;
        for url in info.streams.into_iter().flat_map(|stream| stream.urls) {
            let prepared = self
                .media_cache
                .prepare_audio_file(url, self.bili.media_client(), |url| {
                    self.bili.is_media_url_allowed(url)
                })
                .await;
            match prepared {
                Ok(prepared) => match wait_for_audio(prepared).await {
                    Ok(path) => return Ok(path),
                    Err(err) => last_error = Some(err),
                },
                Err(err) => last_error = Some(err),
            }
        }
        Err(last_error.unwrap_or_else(|| anyhow::anyhow!("reference has no playable audio URL")))
    }
}

async fn wait_for_audio(prepared: PreparedAudio) -> Result<PathBuf> {
    match prepared {
        PreparedAudio::Ready { path, .. } => Ok(path),
        PreparedAudio::Streaming {
            path,
            done,
            aborted,
            error,
            ..
        } => {
            tokio::time::timeout(Duration::from_secs(180), async {
                while !done.load(Ordering::Acquire) && !aborted.load(Ordering::Acquire) {
                    tokio::time::sleep(Duration::from_millis(40)).await;
                }
            })
            .await
            .context("audio reference download timed out")?;
            ensure!(
                !aborted.load(Ordering::Acquire),
                "audio reference download cancelled"
            );
            if let Some(error) = &*error.lock().unwrap() {
                bail!("{error}");
            }
            Ok(path.with_extension("audio"))
        }
    }
}

async fn search_candidates(
    client: &LyricsClient,
    title: &str,
    artist: &str,
) -> Result<Vec<LyricsCandidate>> {
    let mut queries = build_search_queries(title);
    if queries.is_empty() {
        queries.push(title.to_owned());
    }
    if !artist.trim().is_empty() {
        queries.insert(0, format!("{} {}", queries[0], artist.trim()));
    }
    let netease_futs: Vec<_> = queries
        .iter()
        .map(|query| client.search_netease(query, 10))
        .collect();
    let qq_futs: Vec<_> = queries
        .iter()
        .take(3)
        .map(|query| client.search_qq(query, 10))
        .collect();
    let netease = stream::iter(netease_futs)
        .buffered(3)
        .collect::<Vec<_>>();
    let qq = stream::iter(qq_futs).buffered(3).collect::<Vec<_>>();
    let (ne_results, qq_results, lrclib) =
        tokio::join!(netease, qq, client.search_lrclib(&queries[0]));
    let mut seen = HashSet::new();
    let mut candidates = Vec::new();
    for candidate in ne_results
        .into_iter()
        .chain(qq_results)
        .filter_map(Result::ok)
        .flatten()
    {
        let key = (
            candidate.provider,
            candidate
                .provider_key
                .clone()
                .unwrap_or_else(|| candidate.id.to_string()),
        );
        if seen.insert(key) {
            candidates.push(candidate);
        }
    }
    match lrclib {
        Ok(results) => {
            for candidate in results {
                let key = (candidate.provider, candidate.id.to_string());
                if seen.insert(key) {
                    candidates.push(candidate);
                }
            }
        }
        Err(err) if candidates.is_empty() => return Err(err),
        Err(err) => log::debug!("LRCLIB search failed: {err}"),
    }
    Ok(candidates)
}

fn candidate_score(
    candidate: &LyricsCandidate,
    title: &str,
    artist: &str,
    target_secs: f64,
) -> Option<i32> {
    let name = without_trailing_note(&candidate.title);
    let clean = clean_track_title(title);
    let exact = normalize(&clean) == normalize(name);
    let in_title = strong_match(name, title)
        || candidate
            .aliases
            .iter()
            .any(|alias| strong_match(without_trailing_note(alias), title));
    if !exact && !in_title {
        return None;
    }
    let artist_match = candidate.artist.split(" / ").any(|name| {
        strong_match(name, title)
            || strong_match(name, artist)
            || normalize(&crate::lyrics::kana_to_romaji(name)) == normalize(artist)
                && !artist.is_empty()
    });
    let difference = candidate
        .duration_secs
        .map_or(f64::INFINITY, |d| (d - target_secs).abs());
    let note = normalize(
        candidate
            .title
            .trim()
            .strip_prefix(name)
            .unwrap_or_default(),
    );
    let wrong_version = !note.is_empty() && !normalize(title).contains(&note);
    Some(
        40 + i32::from(exact) * 12
            + i32::from(artist_match) * 24
            + i32::from(difference <= 2.0) * 10
            - i32::from(wrong_version) * 15,
    )
}

fn reference_score(track: &TrackItem, duration: f64) -> i32 {
    let text = track.title.to_ascii_lowercase();
    let base = if (track.duration_secs - duration).abs() <= 1.0 {
        60
    } else {
        40
    };
    base + 25
        * i32::from(
            contains_any(
                &text,
                &[
                    "官方",
                    "原曲",
                    "cd音源",
                    "hi-res",
                    "hires",
                    "完整版",
                    "无损",
                ],
            ) || has_word(&text, "cd"),
        )
        + 15 * i32::from(contains_any(
            &text,
            &["罗马音", "假名", "歌词", "扒谱", "动态鼓谱", "总谱"],
        ))
        - 50 * i32::from(is_live(&text) || contains_any(&text, &["巡演", "切片"]))
        - 30 * i32::from(
            contains_any(
                &text,
                &["music video", "project diva", "游戏", "遊戲", "ゲーム"],
            ) || ["mv", "pv", "game"]
                .iter()
                .any(|word| has_word(&text, word)),
        )
        - 80 * i32::from(
            contains_any(
                &text,
                &[
                    "翻唱",
                    "试听",
                    "伴奏",
                    "无贝斯",
                    "无吉他",
                    "反应",
                    "教学",
                    "教學",
                ],
            ) || ["cover", "ai", "reaction"]
                .iter()
                .any(|word| has_word(&text, word)),
        )
}

fn contains_any(text: &str, patterns: &[&str]) -> bool {
    patterns.iter().any(|pattern| text.contains(pattern))
}

fn is_live(text: &str) -> bool {
    has_word(text, "live")
        || has_word(text, "cover")
        || contains_any(
            text,
            &[
                "现场",
                "現場",
                "音乐节",
                "音樂節",
                "翻唱",
                "カバー",
                "ライブ",
                "演唱会",
                "演唱會",
            ],
        )
}

fn is_credit(text: &str) -> bool {
    let text = text.trim().to_ascii_lowercase();
    if let Some((label, _)) = text.split_once([':', '：'])
        && [
            "作词",
            "作詞",
            "作曲",
            "编曲",
            "編曲",
            "制作人",
            "製作人",
            "监制",
            "混音",
            "母带",
            "和声",
            "吉他",
            "贝斯",
            "鼓",
            "键盘",
            "弦乐",
            "词",
            "曲",
            "lyrics",
            "lyricist",
            "composer",
            "arranger",
            "producer",
        ]
        .contains(&label.trim_end())
    {
        return true;
    }
    [
        "produced",
        "programmed",
        "programming",
        "recorded",
        "recording",
        "mixed",
        "mixing",
        "mastered",
        "mastering",
        "directed",
        "organized",
        "arranged",
        "composed",
        "written",
        "engineered",
        "edited",
        "vocal",
        "vocals",
        "chorus",
        "guitar",
        "guitars",
        "bass",
        "drums",
        "keyboard",
        "keyboards",
        "piano",
        "strings",
        "all instruments",
    ]
    .iter()
    .any(|prefix| {
        text.strip_prefix(prefix)
            .is_some_and(|rest| rest.starts_with([' ', ':', '：']))
    }) && (text.contains([':', '：']) || text.contains(" by ") || text.contains(" at "))
}

fn same_lyrics(a: &LyricsData, b: &LyricsData) -> bool {
    let text_set = |data: &LyricsData| {
        data.lines
            .iter()
            .map(|line| normalize(&line.text))
            .filter(|text| !text.is_empty())
            .collect::<HashSet<_>>()
    };
    let (a, b) = (text_set(a), text_set(b));
    let total = a.len().min(b.len());
    total > 0 && a.intersection(&b).count() * 5 >= total * 4
}

fn align_candidate(
    references: Vec<(TrackItem, AudioFeatures)>,
    target: &AudioFeatures,
    candidate: LyricsCandidate,
    mut lyrics: LyricsData,
) -> (f64, AutoLyricsResult) {
    let (tracks, features): (Vec<_>, Vec<_>) = references.into_iter().unzip();
    let index = consensus_reference(&features);
    let reference = &features[index];
    let times = lyrics
        .lines
        .iter()
        .map(|line| line.start_secs)
        .collect::<Vec<_>>();
    let anchors = [times[0], times[times.len() / 3], times[times.len() * 2 / 3]];
    let coarse = estimate_global_offset(reference, target, None, &anchors);
    let lines = align_lines(reference, target, &times, coarse);
    let score = if coarse.confidence >= 0.1 {
        verify_score(&lines)
    } else {
        0.0
    };
    for (line, aligned) in lyrics.lines.iter_mut().zip(lines) {
        let offset = if aligned.uncertain {
            coarse.offset_secs
        } else {
            aligned.time - line.start_secs
        };
        line.start_secs = (line.start_secs + offset).max(0.0);
        for word in &mut line.words {
            word.start_secs = (word.start_secs + offset).max(0.0);
        }
    }
    lyrics
        .lines
        .sort_by(|a, b| a.start_secs.total_cmp(&b.start_secs));
    lyrics.source = Some("auto".into());
    (
        score,
        AutoLyricsResult {
            lyrics,
            candidate,
            reference: Some(tracks[index].clone()),
            offset_secs: coarse.offset_secs,
            confidence: score,
        },
    )
}

/// Generate standard centisecond LRC for storing a shifted result with the existing app.
pub fn format_lrc(lines: &[LyricLine], text: impl Fn(&LyricLine) -> Option<&str>) -> String {
    use std::fmt::Write;
    let mut output = String::new();
    for line in lines {
        if let Some(text) = text(line).map(str::trim).filter(|text| !text.is_empty())
            && line.start_secs.is_finite()
        {
            let time = (line.start_secs.max(0.0) * 100.0).round() as u64;
            if !output.is_empty() {
                output.push('\n');
            }
            write!(
                output,
                "[{:02}:{:02}.{:02}]{text}",
                time / 6000,
                time / 100 % 60,
                time % 100
            )
            .unwrap();
        }
    }
    output
}

/// Decode to 16 kHz, mono, signed 16-bit PCM and extract features with bounded buffering.
pub async fn decode_audio_features(ffmpeg: &Path, path: &Path) -> Result<AudioFeatures> {
    ensure!(
        tokio::fs::metadata(path).await?.is_file(),
        "audio input is not a file"
    );
    let mut child = tokio::process::Command::new(ffmpeg)
        .kill_on_drop(true)
        .args([
            "-hide_banner",
            "-loglevel",
            "error",
            "-nostdin",
            "-protocol_whitelist",
            "file,pipe",
            "-i",
        ])
        .arg(path)
        .args([
            "-map", "0:a:0", "-ac", "1", "-ar", "16000", "-f", "s16le", "pipe:1",
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .context("start ffmpeg audio decode")?;
    let mut pipe = BufReader::new(child.stdout.take().context("missing ffmpeg PCM output")?);
    let (sender, mut chunks) = tokio::sync::mpsc::channel::<Vec<u8>>(2);
    let extraction = tokio::task::spawn_blocking(move || {
        let mut extractor = FeatureExtractor::new();
        while let Some(chunk) = chunks.blocking_recv() {
            extractor.push(
                chunk
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|&bytes| f32::from(i16::from_le_bytes(bytes)) / 32768.0),
            );
        }
        extractor.finish()
    });
    let read = tokio::time::timeout(DECODE_TIMEOUT, async {
        let mut total = 0;
        loop {
            let mut bytes = vec![0; 16_384];
            let mut used = 0;
            while used < bytes.len() {
                let read = pipe.read(&mut bytes[used..]).await?;
                if read == 0 {
                    break;
                }
                used += read;
            }
            if used == 0 {
                break;
            }
            ensure!(used % 2 == 0, "ffmpeg returned incomplete PCM");
            total += used;
            ensure!(
                total <= MAX_AUDIO_SECS * SAMPLE_RATE as usize * 2,
                "audio exceeds the 30-minute alignment limit"
            );
            bytes.truncate(used);
            sender
                .send(bytes)
                .await
                .context("PCM feature extractor stopped")?;
        }
        let status = child.wait().await?;
        ensure!(status.success(), "ffmpeg audio decode failed ({status})");
        Ok(())
    })
    .await
    .context("ffmpeg audio decode timed out")?;
    drop(sender);
    read?;
    let features = extraction.await?;
    ensure!(
        !features.cens.is_empty(),
        "audio does not contain enough samples for alignment"
    );
    Ok(features)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lyrics::shift_lyrics;

    #[test]
    fn radix_two_fft_identifies_bin_and_negative_frequency() {
        let fft = Fft::new(8).unwrap();
        let mut real =
            std::array::from_fn::<_, 8, _>(|n| (std::f32::consts::TAU * n as f32 / 8.0).cos());
        let mut imag = [0.0; 8];
        fft.transform(&mut real, &mut imag);
        assert!((real[1] - 4.0).abs() < 1e-5 && (real[7] - 4.0).abs() < 1e-5);
        assert!(real[2..7].iter().chain(&imag).all(|v| v.abs() < 1e-5));
        assert!(Fft::new(9).is_none());
    }

    fn signal() -> Vec<f32> {
        [0, 9, 5, 2, 8, 3, 11, 6, 5, 4, 10, 9, 0, 2]
            .into_iter()
            .enumerate()
            .flat_map(|(index, note)| {
                let freq = 250.0 * 2_f32.powf(note as f32 / 12.0);
                (0..SAMPLE_RATE / 2).map(move |i| {
                    let time = i as f32 / SAMPLE_RATE as f32;
                    (std::f32::consts::TAU * freq * time).sin()
                        * if index % 2 == 0 { 0.7 } else { 0.4 }
                })
            })
            .collect()
    }

    #[test]
    fn aligns_synthetic_offset_and_preserves_companion_timings() {
        let audio = signal();
        let mut delayed = vec![0.0; SAMPLE_RATE as usize * 2];
        delayed.extend(&audio);
        delayed.extend(vec![0.0; 8_000]);
        let reference = extract_features(&audio);
        let target = extract_features(&delayed);
        let result = estimate_global_offset(&reference, &target, None, &[1.0, 3.0, 5.0]);
        assert!((result.offset_secs - 2.0).abs() < 0.12, "{result:?}");
        assert!(result.confidence > 0.9, "{result:?}");
        let lines = align_lines(&reference, &target, &[1.0, 3.0, 5.0], result);
        assert!((lines[1].time - 5.0).abs() < 0.12, "{lines:?}");
        assert!(verify_score(&lines) > 0.9, "{lines:?}");
        let mut lyrics = crate::lyrics::parse_lyrics("[00:01.5]abc", "", "[00:01.5]def", "");
        shift_lyrics(&mut lyrics, 2.0);
        assert_eq!(
            format_lrc(&lyrics.lines, |line| line.translation.as_deref()),
            "[00:03.50]def"
        );
    }

    #[tokio::test]
    async fn decodes_real_wav_with_ffmpeg() -> Result<()> {
        let Some(ffmpeg) = find_ffmpeg(None).await else {
            ensure!(
                std::env::var_os("BIU_REQUIRE_FFMPEG_TEST").is_none(),
                "ffmpeg test requires a working executable"
            );
            return Ok(());
        };
        let dir = tempfile::tempdir()?;
        let wav = dir.path().join("audio.wav");
        let frames = SAMPLE_RATE as usize / 2;
        let mut bytes = Vec::new();
        bytes.extend(b"RIFF");
        bytes.extend((36 + frames as u32 * 2).to_le_bytes());
        bytes.extend(b"WAVEfmt ");
        bytes.extend(16u32.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(1u16.to_le_bytes());
        bytes.extend(SAMPLE_RATE.to_le_bytes());
        bytes.extend((SAMPLE_RATE * 2).to_le_bytes());
        bytes.extend(2u16.to_le_bytes());
        bytes.extend(16u16.to_le_bytes());
        bytes.extend(b"data");
        bytes.extend((frames as u32 * 2).to_le_bytes());
        for sample in signal().into_iter().take(frames) {
            bytes.extend(((sample * i16::MAX as f32) as i16).to_le_bytes());
        }
        tokio::fs::write(&wav, bytes).await?;
        let audio = decode_audio_features(&ffmpeg, &wav).await?;
        assert_eq!(audio.duration_secs, 0.5);
        assert!(audio.energy.iter().all(|energy| *energy > 0.1));
        Ok(())
    }
}
