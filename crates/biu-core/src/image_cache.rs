use crate::bili::{BILI_REFERER, USER_AGENT, allowlist_redirect_policy};
use crate::media_cache::{cache_files, remove_if_present};
use crate::store::atomic_write;
use anyhow::{Context, Result, ensure};
use futures::StreamExt;
use image::{ImageReader, RgbaImage, imageops::FilterType};
use sha2::{Digest, Sha256};
use std::collections::VecDeque;
use std::io::Cursor;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime};
use tokio::sync::{Mutex as AsyncMutex, Semaphore};
use url::Url;

const MAX_MEMORY_ENTRIES: usize = 96;
const MAX_MEMORY_BYTES: usize = 24 * 1024 * 1024;
const MAX_DISK_BYTES: u64 = 120 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 12 * 1024 * 1024;
const BLUR_WIDTH: usize = 240;
const BLUR_HEIGHT: usize = 135;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThumbSize {
    Tiny,
    Avatar,
    Card,
    BlurBg,
    Cover,
}

impl ThumbSize {
    pub fn dimensions(self) -> (u32, u32) {
        match self {
            Self::Tiny => (160, 90),
            Self::Avatar => (160, 160),
            Self::Card => (320, 180),
            Self::BlurBg => (240, 135),
            Self::Cover => (960, 720),
        }
    }
}

fn is_image_host(url: &Url) -> bool {
    url.scheme() == "https"
        && url.username().is_empty()
        && url.password().is_none()
        && url.port().is_none()
        && url.host_str().is_some_and(|host| {
            matches!(host, "hdslb.com" | "biliimg.com")
                || host.ends_with(".hdslb.com")
                || host.ends_with(".biliimg.com")
        })
}

fn original_path(url: &Url) -> String {
    let path = url
        .path()
        .split_once('@')
        .map_or(url.path(), |(path, _)| path);
    path.strip_suffix(".avif")
        .map_or_else(|| path.to_owned(), |base| format!("{base}.webp"))
}

/// Build a CDN thumbnail URL, replacing an existing Bilibili resize/format suffix.
pub fn thumbnail_url(input: &str, size: ThumbSize) -> Result<Url> {
    let mut url = if input.starts_with("//") {
        Url::parse(&format!("https:{input}"))?
    } else {
        Url::parse(input)?
    };
    if url.scheme() == "http" {
        url.set_scheme("https")
            .map_err(|()| anyhow::anyhow!("invalid image scheme"))?;
    }
    ensure!(
        is_image_host(&url),
        "image host is not a Bilibili image CDN"
    );
    let (width, height) = size.dimensions();
    url.set_path(&format!(
        "{}@{width}w_{height}h_1c.webp",
        original_path(&url)
    ));
    url.set_fragment(None);
    Ok(url)
}

struct ImageLru {
    entries: VecDeque<(String, Arc<RgbaImage>)>,
    bytes: usize,
}

impl ImageLru {
    fn get(&mut self, key: &str) -> Option<Arc<RgbaImage>> {
        let index = self.entries.iter().position(|(id, _)| id == key)?;
        let entry = self.entries.remove(index)?;
        let value = entry.1.clone();
        self.entries.push_back(entry);
        Some(value)
    }

    fn insert(&mut self, key: String, value: Arc<RgbaImage>) {
        if self.get(&key).is_some() {
            return;
        }
        let size = value.as_raw().len();
        if size > MAX_MEMORY_BYTES {
            return;
        }
        while self.entries.len() >= MAX_MEMORY_ENTRIES || self.bytes + size > MAX_MEMORY_BYTES {
            if let Some((_, oldest)) = self.entries.pop_front() {
                self.bytes -= oldest.as_raw().len();
            }
        }
        self.bytes += size;
        self.entries.push_back((key, value));
    }
}

pub struct ImageCache {
    directory: PathBuf,
    client: reqwest::Client,
    memory: Mutex<ImageLru>,
    disk: AsyncMutex<()>,
    fills: Semaphore,
}

impl ImageCache {
    pub fn new(directory: impl Into<PathBuf>) -> Result<Self> {
        let directory = directory.into();
        std::fs::create_dir_all(&directory)?;
        let client = reqwest::Client::builder()
            .redirect(allowlist_redirect_policy(is_image_host))
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(20))
            .pool_max_idle_per_host(2)
            .pool_idle_timeout(Duration::from_secs(30))
            .build()?;
        Ok(Self {
            directory,
            client,
            memory: Mutex::new(ImageLru {
                entries: VecDeque::new(),
                bytes: 0,
            }),
            disk: AsyncMutex::new(()),
            fills: Semaphore::new(6),
        })
    }

    /// Fetch and resize a bounded RGBA image, including unblurred pixels for `BlurBg`.
    pub async fn get(&self, input: &str, size: ThumbSize) -> Result<Arc<RgbaImage>> {
        let thumbnail = thumbnail_url(input, size)?;
        let key = format!("{:x}", Sha256::digest(thumbnail.as_str().as_bytes()));
        if let Some(image) = self.memory.lock().unwrap().get(&key) {
            return Ok(image);
        }
        let _permit = self.fills.acquire().await?;
        if let Some(image) = self.memory.lock().unwrap().get(&key) {
            return Ok(image);
        }
        let path = self.directory.join(format!("{key}.source"));
        let mut fallback = thumbnail.clone();
        fallback.set_path(&original_path(&thumbnail));
        let disk_bytes = {
            let _disk = self.disk.lock().await;
            read_cached(&path).await
        };
        let cached = match disk_bytes {
            Some(bytes) => tokio::task::spawn_blocking(move || decode_image(bytes, size))
                .await?
                .ok(),
            None => None,
        };
        let image = if let Some((image, _)) = cached {
            image
        } else {
            let mut fetched = None;
            let mut last_error = None;
            for url in [thumbnail, fallback] {
                match self.fetch(url).await {
                    Ok(bytes) => {
                        match tokio::task::spawn_blocking(move || decode_image(bytes, size)).await?
                        {
                            Ok(decoded) => {
                                fetched = Some(decoded);
                                break;
                            }
                            Err(err) => last_error = Some(err),
                        }
                    }
                    Err(err) => last_error = Some(err),
                }
            }
            let (image, bytes) =
                fetched.ok_or_else(|| last_error.expect("at least one image attempt"))?;
            let _disk = self.disk.lock().await;
            if let Err(err) = self.write_disk(path, bytes).await {
                log::warn!("image disk cache write failed: {err}");
            }
            image
        };
        let image = Arc::new(image);
        self.memory.lock().unwrap().insert(key, image.clone());
        Ok(image)
    }

    async fn fetch(&self, url: Url) -> Result<Vec<u8>> {
        let response = self
            .client
            .get(url)
            .header("Referer", BILI_REFERER)
            .send()
            .await
            .map_err(reqwest::Error::without_url)?
            .error_for_status()
            .map_err(reqwest::Error::without_url)?;
        ensure!(
            response
                .content_length()
                .is_none_or(|size| size <= MAX_RESPONSE_BYTES as u64),
            "image response too large"
        );
        let capacity = response.content_length().unwrap_or_default() as usize;
        let mut stream = response.bytes_stream();
        let mut bytes = Vec::with_capacity(capacity);
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.map_err(reqwest::Error::without_url)?;
            ensure!(
                bytes.len() + chunk.len() <= MAX_RESPONSE_BYTES,
                "image response too large"
            );
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }

    async fn write_disk(&self, path: PathBuf, bytes: Vec<u8>) -> Result<()> {
        let protected = path.clone();
        tokio::task::spawn_blocking(move || {
            atomic_write(&path, |file| {
                use std::io::Write;
                file.write_all(&bytes)?;
                Ok(())
            })
        })
        .await??;
        let mut entries = cache_files(&self.directory, &["source"]).await?;
        entries.sort_by_key(|entry| entry.modified);
        let mut total: u64 = entries.iter().map(|entry| entry.size).sum();
        for entry in entries {
            if total <= MAX_DISK_BYTES {
                break;
            }
            if entry.path != protected && remove_if_present(&entry.path).await.is_ok() {
                total -= entry.size;
            }
        }
        Ok(())
    }
}

async fn read_cached(path: &Path) -> Option<Vec<u8>> {
    let file = tokio::fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .await
        .ok()?;
    if file.metadata().await.ok()?.len() > MAX_RESPONSE_BYTES as u64 {
        return None;
    }
    let _ = file.into_std().await.set_modified(SystemTime::now());
    tokio::fs::read(path).await.ok()
}

fn decode_image(bytes: Vec<u8>, size: ThumbSize) -> Result<(RgbaImage, Vec<u8>)> {
    let mut reader = ImageReader::new(Cursor::new(&bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(4096);
    limits.max_image_height = Some(4096);
    limits.max_alloc = Some(80 * 1024 * 1024);
    reader.limits(limits);
    let source = reader.decode().context("decode Bilibili image")?;
    let (width, height) = size.dimensions();
    let image = if source.width() == width && source.height() == height {
        source.into_rgba8()
    } else {
        source
            .resize_to_fill(width, height, FilterType::Triangle)
            .into_rgba8()
    };
    Ok((image, bytes))
}

/// Three separable box filters approximate a Gaussian using two 240×135 RGBA buffers.
pub fn blur_rgba_240x135(input: &[u8]) -> Result<Vec<u8>> {
    ensure!(
        input.len() == BLUR_WIDTH * BLUR_HEIGHT * 4,
        "expected a 240x135 RGBA image"
    );
    let mut pixels = input.to_vec();
    let mut scratch = vec![0; input.len()];
    for radius in [6, 7, 8] {
        box_pass(
            &pixels,
            &mut scratch,
            BLUR_WIDTH,
            BLUR_HEIGHT,
            4,
            BLUR_WIDTH * 4,
            radius,
        );
        box_pass(
            &scratch,
            &mut pixels,
            BLUR_HEIGHT,
            BLUR_WIDTH,
            BLUR_WIDTH * 4,
            4,
            radius,
        );
    }
    Ok(pixels)
}

fn box_pass(
    input: &[u8],
    output: &mut [u8],
    length: usize,
    lines: usize,
    step: usize,
    line_step: usize,
    radius: usize,
) {
    let width = (2 * radius + 1) as u32;
    for line in 0..lines {
        let base = line * line_step;
        let mut sum = [0u32; 4];
        for position in 0..=2 * radius {
            let index = base + position.saturating_sub(radius) * step;
            for (channel, sum) in sum.iter_mut().enumerate() {
                *sum += u32::from(input[index + channel]);
            }
        }
        for position in 0..length {
            let output_index = base + position * step;
            let left = base + position.saturating_sub(radius) * step;
            let right = base + (position + radius + 1).min(length - 1) * step;
            for (channel, sum) in sum.iter_mut().enumerate() {
                output[output_index + channel] = ((*sum + width / 2) / width) as u8;
                *sum = *sum + u32::from(input[right + channel]) - u32::from(input[left + channel]);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rewrites_cdn_thumbnails_and_avif() -> Result<()> {
        let input = "//i0.hdslb.com/bfs/archive/a.jpg@640w.avif?key=1";
        assert_eq!(
            thumbnail_url(input, ThumbSize::Tiny)?.as_str(),
            "https://i0.hdslb.com/bfs/archive/a.jpg@160w_90h_1c.webp?key=1"
        );
        assert_eq!(
            thumbnail_url("https://i1.hdslb.com/a.avif", ThumbSize::Cover)?.path(),
            "/a.webp@960w_720h_1c.webp"
        );
        assert_eq!(
            thumbnail_url("https://i0.biliimg.com/bfs/face/a.jpg", ThumbSize::Avatar)?.as_str(),
            "https://i0.biliimg.com/bfs/face/a.jpg@160w_160h_1c.webp"
        );
        assert_eq!(
            thumbnail_url("https://i0.biliimg.com/bfs/face/a.jpg", ThumbSize::Card)?.as_str(),
            "https://i0.biliimg.com/bfs/face/a.jpg@320w_180h_1c.webp"
        );
        assert!(thumbnail_url("https://hdslb.com.example.org/a.png", ThumbSize::Card).is_err());
        assert!(thumbnail_url("https://biliimg.com.example.org/a.png", ThumbSize::Card).is_err());
        Ok(())
    }

    #[tokio::test]
    async fn gets_unblurred_background_pixels_from_disk() -> Result<()> {
        let dir = tempfile::tempdir()?;
        let cache = ImageCache::new(dir.path())?;
        let url = "https://i0.biliimg.com/cover.png";
        let thumbnail = thumbnail_url(url, ThumbSize::BlurBg)?;
        let key = format!("{:x}", Sha256::digest(thumbnail.as_str().as_bytes()));
        let (width, height) = ThumbSize::BlurBg.dimensions();
        let image = RgbaImage::from_fn(width, height, |x, _| {
            image::Rgba(if x < width / 2 {
                [0, 0, 0, 255]
            } else {
                [255; 4]
            })
        });
        image.save_with_format(
            dir.path().join(format!("{key}.source")),
            image::ImageFormat::Png,
        )?;
        assert_eq!(*cache.get(url, ThumbSize::BlurBg).await?, image);
        Ok(())
    }

    #[test]
    fn blurs_an_edge_symmetrically_and_preserves_alpha() -> Result<()> {
        let mut pixels = vec![255; BLUR_WIDTH * BLUR_HEIGHT * 4];
        for line in pixels.as_chunks_mut::<{ BLUR_WIDTH * 4 }>().0 {
            for pixel in line[..BLUR_WIDTH * 2].as_chunks_mut::<4>().0 {
                pixel[..3].fill(0);
            }
        }
        let blurred = blur_rgba_240x135(&pixels)?;
        let row = &blurred[0..BLUR_WIDTH * 4];
        assert_eq!(row[0], 0);
        assert_eq!(row[BLUR_WIDTH * 4 - 4], 255);
        assert!(row[119 * 4] > 80 && row[120 * 4] < 175);
        assert_eq!(u16::from(row[119 * 4]) + u16::from(row[120 * 4]), 255);
        assert!(
            blurred
                .as_chunks::<4>()
                .0
                .iter()
                .all(|pixel| pixel[3] == 255)
        );
        Ok(())
    }

    #[test]
    fn memory_cache_enforces_byte_and_entry_caps() {
        let mut lru = ImageLru {
            entries: VecDeque::new(),
            bytes: 0,
        };
        let large = Arc::new(RgbaImage::new(960, 720));
        for i in 0..12 {
            lru.insert(i.to_string(), Arc::new((*large).clone()));
        }
        assert_eq!(lru.entries.len(), 9);
        assert!(lru.bytes <= MAX_MEMORY_BYTES);
        assert!(lru.get("0").is_none());
        let small = Arc::new(RgbaImage::new(1, 1));
        for i in 12..130 {
            lru.insert(i.to_string(), small.clone());
        }
        assert_eq!(lru.entries.len(), MAX_MEMORY_ENTRIES);
    }
}
