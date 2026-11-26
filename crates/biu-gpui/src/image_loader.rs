use biu_core::image_cache::{blur_rgba_240x135, ImageCache, ThumbSize};
use gpui_kit::RenderImage;
use image::{Frame, RgbaImage};
use qrcode::{Color as QrColor, QrCode};
use smallvec::smallvec;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// Hard caps on decoded GPU-ready RenderImage instances kept in memory/atlas.
pub const MAX_RENDER_IMAGES: usize = 48;
pub const MAX_COVER_BYTES: usize = 12 * 1024 * 1024;
pub const MAX_LARGE_COVERS: usize = 2;
pub const MAX_BLUR_BGS: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct CoverKey {
    pub url: String,
    pub size: u8,
    pub blurred: bool,
}

impl CoverKey {
    pub fn thumb(url: impl Into<String>, size: ThumbSize) -> Self {
        Self {
            url: url.into(),
            size: thumb_tag(size),
            blurred: false,
        }
    }

    pub fn blurred(url: impl Into<String>) -> Self {
        Self {
            url: url.into(),
            size: thumb_tag(ThumbSize::BlurBg),
            blurred: true,
        }
    }

    fn is_large_cover(&self) -> bool {
        !self.blurred && self.size == 3
    }
}

fn thumb_tag(size: ThumbSize) -> u8 {
    match size {
        ThumbSize::Tiny => 0,
        ThumbSize::Card => 1,
        ThumbSize::BlurBg => 2,
        ThumbSize::Cover => 3,
    }
}

fn estimate_image_bytes(img: &RenderImage) -> usize {
    img.as_bytes(0).map_or(0, <[u8]>::len)
}

/// Convert RGBA image into GPUI's BGRA `RenderImage`.
pub fn rgba_to_render_image(img: &RgbaImage) -> Arc<RenderImage> {
    let (w, h) = img.dimensions();
    let mut bgra = img.as_raw().clone();
    for px in bgra.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let buf = RgbaImage::from_raw(w, h, bgra).expect("valid BGRA buffer dimensions");
    Arc::new(RenderImage::new(smallvec![Frame::new(buf)]))
}

/// Convert raw RGBA bytes (w x h x 4) into GPUI's BGRA `RenderImage`.
pub fn rgba_bytes_to_render_image(
    width: u32,
    height: u32,
    mut bytes: Vec<u8>,
) -> Option<Arc<RenderImage>> {
    if bytes.len() != (width as usize) * (height as usize) * 4 {
        return None;
    }
    for px in bytes.as_chunks_mut::<4>().0 {
        px.swap(0, 2);
    }
    let buf = RgbaImage::from_raw(width, height, bytes)?;
    Some(Arc::new(RenderImage::new(smallvec![Frame::new(buf)])))
}

/// Render a QR code matrix into a crisp GPU-ready `RenderImage` (no external webview/SVG needed).
pub fn render_qr_image(content: &str) -> Option<Arc<RenderImage>> {
    let code = QrCode::new(content.as_bytes()).ok()?;
    let module_count = code.width();
    let quiet = 4usize;
    let scale = 6usize;
    let dim = (module_count + quiet * 2) * scale;
    let mut bgra = vec![255u8; dim * dim * 4];

    for y in 0..module_count {
        for x in 0..module_count {
            if code[(x, y)] == QrColor::Dark {
                for sy in 0..scale {
                    for sx in 0..scale {
                        let px_x = (x + quiet) * scale + sx;
                        let px_y = (y + quiet) * scale + sy;
                        let idx = (px_y * dim + px_x) * 4;
                        bgra[idx] = 18;
                        bgra[idx + 1] = 18;
                        bgra[idx + 2] = 18;
                        bgra[idx + 3] = 255;
                    }
                }
            }
        }
    }

    let buf = RgbaImage::from_raw(dim as u32, dim as u32, bgra)?;
    Some(Arc::new(RenderImage::new(smallvec![Frame::new(buf)])))
}

struct StoredCover {
    image: Arc<RenderImage>,
    bytes: usize,
}

/// Bounded LRU store of GPU-ready `Arc<RenderImage>` covers with explicit byte & class limits.
/// Callers MUST pass returned evicted images to `cx.drop_image(img, None)` to reclaim GPU atlas memory.
#[derive(Default)]
pub struct CoverStore {
    entries: HashMap<CoverKey, StoredCover>,
    order: VecDeque<CoverKey>,
    in_flight: HashSet<CoverKey>,
    failed: HashSet<CoverKey>,
    total_bytes: usize,
    clear_epoch: u64,
}

impl CoverStore {
    pub fn clear_epoch(&self) -> u64 {
        self.clear_epoch
    }

    pub fn get(&self, key: &CoverKey) -> Option<Arc<RenderImage>> {
        self.entries.get(key).map(|entry| entry.image.clone())
    }

    pub fn should_fetch(&self, key: &CoverKey) -> bool {
        !key.url.is_empty()
            && !self.entries.contains_key(key)
            && !self.in_flight.contains(key)
            && !self.failed.contains(key)
    }

    pub fn mark_in_flight(&mut self, key: CoverKey) {
        self.in_flight.insert(key);
    }

    /// Insert an image if its `epoch` matches the current `clear_epoch`.
    /// Returns all `RenderImage`s that must be dropped from GPUI's GPU sprite atlas.
    pub fn insert(
        &mut self,
        epoch: u64,
        key: CoverKey,
        image: Arc<RenderImage>,
    ) -> Vec<Arc<RenderImage>> {
        self.in_flight.remove(&key);
        if epoch != self.clear_epoch {
            // Stale fetch completed after hide-to-tray clear; drop immediately.
            return vec![image];
        }
        self.failed.remove(&key);
        let mut evicted = Vec::new();
        let bytes = estimate_image_bytes(&image);

        self.order.retain(|k| k != &key);
        if let Some(old) = self.entries.remove(&key) {
            self.total_bytes = self.total_bytes.saturating_sub(old.bytes);
            evicted.push(old.image);
        }

        self.order.push_back(key.clone());
        self.total_bytes = self.total_bytes.saturating_add(bytes);
        self.entries.insert(key.clone(), StoredCover { image, bytes });

        // Enforce per-class limits for large covers and blurred backgrounds first.
        if key.is_large_cover() {
            self.evict_matching(MAX_LARGE_COVERS, |k| k.is_large_cover(), &mut evicted);
        } else if key.blurred {
            self.evict_matching(MAX_BLUR_BGS, |k| k.blurred, &mut evicted);
        }

        // Enforce global entry count and byte budget.
        while self.order.len() > MAX_RENDER_IMAGES
            || (self.total_bytes > MAX_COVER_BYTES && self.order.len() > 1)
        {
            if let Some(old_key) = self.order.pop_front()
                && let Some(old) = self.entries.remove(&old_key)
            {
                self.total_bytes = self.total_bytes.saturating_sub(old.bytes);
                evicted.push(old.image);
            }
        }
        evicted
    }

    fn evict_matching(
        &mut self,
        limit: usize,
        pred: impl Fn(&CoverKey) -> bool,
        evicted: &mut Vec<Arc<RenderImage>>,
    ) {
        let mut count = self.order.iter().filter(|k| pred(k)).count();
        while count > limit {
            if let Some(pos) = self.order.iter().position(&pred)
                && let Some(old_key) = self.order.remove(pos)
                && let Some(old) = self.entries.remove(&old_key)
            {
                self.total_bytes = self.total_bytes.saturating_sub(old.bytes);
                evicted.push(old.image);
                count -= 1;
            } else {
                break;
            }
        }
    }

    pub fn mark_failed(&mut self, epoch: u64, key: &CoverKey) {
        if epoch != self.clear_epoch {
            return;
        }
        self.in_flight.remove(key);
        if self.failed.len() > 256 {
            self.failed.clear();
        }
        self.failed.insert(key.clone());
    }

    /// Release all decoded textures (called when hiding to system tray to minimize RAM).
    /// Returns all previously stored images so the caller can drop them from GPUI's GPU atlas.
    pub fn clear(&mut self) -> Vec<Arc<RenderImage>> {
        self.clear_epoch = self.clear_epoch.wrapping_add(1);
        self.order.clear();
        self.in_flight.clear();
        self.total_bytes = 0;
        self.entries
            .drain()
            .map(|(_, stored)| stored.image)
            .collect()
    }
}

/// Fetch and decode a thumbnail or pre-blurred 240x135 ambient background via `biu-core::ImageCache`.
pub async fn fetch_render_image(
    cache: &ImageCache,
    url: &str,
    size: ThumbSize,
    blurred: bool,
) -> anyhow::Result<Arc<RenderImage>> {
    let rgba = cache
        .get(url, if blurred { ThumbSize::BlurBg } else { size })
        .await?;
    if blurred {
        let resized = if rgba.dimensions() == (240, 135) {
            rgba.as_raw().clone()
        } else {
            image::imageops::resize(&*rgba, 240, 135, image::imageops::FilterType::Triangle)
                .into_raw()
        };
        let blurred_bytes = blur_rgba_240x135(&resized)?;
        rgba_bytes_to_render_image(240, 135, blurred_bytes)
            .ok_or_else(|| anyhow::anyhow!("failed to build blurred RenderImage"))
    } else {
        Ok(rgba_to_render_image(&rgba))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dummy_render_image(w: u32, h: u32) -> Arc<RenderImage> {
        let img = RgbaImage::from_pixel(w, h, image::Rgba([10, 20, 30, 255]));
        rgba_to_render_image(&img)
    }

    #[test]
    fn evicts_large_covers_and_discards_stale_epoch_inserts() {
        let mut store = CoverStore::default();
        let e0 = store.clear_epoch();
        for i in 0..4 {
            let key = CoverKey::thumb(format!("https://i0.hdslb.com/{i}.jpg"), ThumbSize::Cover);
            let evicted = store.insert(e0, key, dummy_render_image(64, 64));
            if i >= MAX_LARGE_COVERS {
                assert!(!evicted.is_empty());
            }
        }
        assert_eq!(store.entries.len(), MAX_LARGE_COVERS);

        let cleared = store.clear();
        assert_eq!(cleared.len(), MAX_LARGE_COVERS);
        assert_eq!(store.total_bytes, 0);

        // Stale insert from old epoch must be rejected & returned for immediate drop.
        let stale_key = CoverKey::thumb("https://i0.hdslb.com/stale.jpg", ThumbSize::Tiny);
        let dropped = store.insert(e0, stale_key.clone(), dummy_render_image(16, 16));
        assert_eq!(dropped.len(), 1);
        assert!(store.get(&stale_key).is_none());
    }
}
