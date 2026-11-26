use crate::{LocalMusicItem, TrackItem, TrackSource};
use lofty::file::{AudioFile, TaggedFileExt};
use lofty::tag::Accessor;
use md5::{Digest, Md5};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

const AUDIO_EXTS: &[&str] = &["mp3", "flac", "wav", "m4a", "aac", "ogg", "wma", "aiff"];

fn is_supported_audio_ext(ext: &str) -> bool {
    let lower = ext.to_ascii_lowercase();
    AUDIO_EXTS.contains(&lower.as_str())
}

fn to_safe_title(file_path: &Path, meta_title: Option<&str>) -> String {
    if let Some(t) = meta_title.map(str::trim).filter(|s| !s.is_empty()) {
        return t.to_string();
    }
    file_path
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("unknown")
        .to_string()
}

pub fn md5_hex_lower(input: &str) -> String {
    let mut hasher = Md5::new();
    hasher.update(input.as_bytes());
    let digest = hasher.finalize();
    digest.iter().map(|b| format!("{b:02x}")).collect()
}

pub fn scan_local_music_dirs(dirs: &[String]) -> Vec<LocalMusicItem> {
    let mut result = Vec::new();
    for dir in dirs {
        if dir.trim().is_empty() {
            continue;
        }
        let dir_path = Path::new(dir);

        let Ok(entries) = fs::read_dir(dir_path) else {
            continue;
        };

        let mut files: Vec<PathBuf> = Vec::new();
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else {
                continue;
            };
            if ft.is_dir() {
                continue;
            }
            let full_path = entry.path();
            let Some(ext) = full_path.extension().and_then(|s| s.to_str()) else {
                continue;
            };
            if is_supported_audio_ext(ext) {
                files.push(full_path);
            }
        }

        for file in files {
            let Ok(meta) = fs::metadata(&file) else {
                continue;
            };
            if !meta.is_file() {
                continue;
            }
            let canonical_path = fs::canonicalize(&file).unwrap_or_else(|_| file.clone());
            let canonical_str = canonical_path.to_string_lossy().to_lowercase();
            let id = md5_hex_lower(&canonical_str);

            let (meta_title, artist, duration) = match lofty::read_from_path(&file) {
                Ok(tagged) => {
                    let dur = tagged.properties().duration().as_secs_f64();
                    let dur_opt = if dur.is_finite() && dur > 0.0 {
                        Some(dur)
                    } else {
                        None
                    };
                    let title_opt = tagged
                        .primary_tag()
                        .or_else(|| tagged.first_tag())
                        .and_then(|t| t.title().map(|c| c.into_owned()));
                    let artist = tagged
                        .primary_tag()
                        .or_else(|| tagged.first_tag())
                        .and_then(Accessor::artist)
                        .map(|s| s.into_owned())
                        .unwrap_or_default();
                    (title_opt, artist, dur_opt)
                }
                Err(_) => (None, String::new(), None),
            };

            let format = file
                .extension()
                .and_then(|s| s.to_str())
                .unwrap_or("")
                .to_ascii_lowercase();

            let created_time = meta
                .created()
                .or_else(|_| meta.modified())
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                .map(|d| d.as_millis() as u64);

            result.push(LocalMusicItem {
                id,
                path: file.to_string_lossy().into_owned(),
                dir: dir.clone(),
                title: to_safe_title(&file, meta_title.as_deref()),
                artist,
                size: meta.len(),
                format,
                duration,
                created_time,
            });
        }
    }
    result.sort_by(|a, b| a.path.cmp(&b.path));
    result.dedup_by(|a, b| a.path == b.path);
    result
}

impl From<LocalMusicItem> for TrackItem {
    fn from(item: LocalMusicItem) -> Self {
        Self {
            id: format!("local:{}", item.id),
            title: item.title,
            artist: item.artist,
            artist_mid: None,
            cover: String::new(),
            duration_secs: item.duration.unwrap_or_default(),
            source: TrackSource::LocalFile {
                path: item.path,
                format: item.format,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scans_a_real_wave_file_and_builds_a_track() -> anyhow::Result<()> {
        let directory = tempfile::tempdir()?;
        let wave = directory.path().join("Recorded Song.WAV");
        let data_size = 8_000u32;
        let mut data = Vec::new();
        data.extend(b"RIFF");
        data.extend((36 + data_size).to_le_bytes());
        data.extend(b"WAVEfmt ");
        data.extend(16u32.to_le_bytes());
        data.extend(1u16.to_le_bytes());
        data.extend(1u16.to_le_bytes());
        data.extend(8_000u32.to_le_bytes());
        data.extend(16_000u32.to_le_bytes());
        data.extend(2u16.to_le_bytes());
        data.extend(16u16.to_le_bytes());
        data.extend(b"data");
        data.extend(data_size.to_le_bytes());
        data.resize(44 + data_size as usize, 0);
        fs::write(&wave, data)?;
        fs::write(directory.path().join("ignore.txt"), "ignore")?;

        let paths = [directory.path().to_string_lossy().into_owned()];
        let mut tracks = scan_local_music_dirs(&paths);
        assert_eq!(tracks.len(), 1);
        let source = tracks.pop().unwrap();
        assert_eq!(source.duration, Some(0.5));
        assert_eq!(source.format, "wav");
        let track = TrackItem::from(source);
        assert_eq!(track.title, "Recorded Song");
        assert!(track.id.starts_with("local:"));
        assert_eq!(track.duration_secs, 0.5);
        Ok(())
    }
}
