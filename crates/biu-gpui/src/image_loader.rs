use biu_core::image_cache::{blur_rgba_240x135, ImageCache, ThumbSize};
use gpui_kit::RenderImage;
use image::{Frame, RgbaImage};
use qrcode::{Color as QrColor, QrCode};
use smallvec::smallvec;
use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

/// Hard cap on decoded GPU-ready RenderImage instances kept in memory (~12-16 MB max).
pub const MAX_RENDER_IMAGES: usize = 48;

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
}

fn thumb_tag(size: ThumbSize) -> u8 {
    match size {
        ThumbSize::Tiny => 0,
        ThumbSize::Card => 1,
        ThumbSize::BlurBg => 2,
        ThumbSize::Cover => 3,
    }
}

/// Convert RGBA image into GPUI's BGRA `RenderImage`.
pub fn rgba_to_render_image(img: &RgbaImage) -> Arc<RenderImage> {
    let (w, h) = img.dimensions();
    let mut bgra = img.as_raw().clone();
    for px in bgra.chunks_exact_mut(4) {
        px.swap(0, 2);
    }
    let buf = RgbaImage::from_raw(w, h, bgra).expect("valid BGRA buffer dimensions");
    Arc::new(RenderImage::new(smallvec![Frame::new(buf)]))
}

/// Convert raw RGBA bytes (w x h x 4) into GPUI's BGRA `RenderImage`.
pub fn rgba_bytes_to_render_image(width: u32, height: u32, mut bytes: Vec<u8>) -> Option<Arc<RenderImage>> {
    if bytes.len() != (width as usize) * (height as usize) * 4 {
        return None;
    }
    for px in bytes.chunks_exact_mut(4) {
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

/// Bounded LRU store of GPU-ready `Arc<RenderImage>` covers.
#[derive(Default)]
pub struct CoverStore {
    entries: HashMap<CoverKey, Arc<RenderImage>>,
    order: VecDeque<CoverKey>,
    in_flight: HashSet<CoverKey>,
    failed: HashSet<CoverKey>,
}

impl CoverStore {
    pub fn get(&self, key: &CoverKey) -> Option<Arc<RenderImage>> {
        self.entries.get(key).cloned()
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

    pub fn insert(&mut self, key: CoverKey, image: Arc<RenderImage>) {
        self.in_flight.remove(&key);
        self.failed.remove(&key);
        self.order.retain(|k| k != &key);
        self.order.push_back(key.clone());
        self.entries.insert(key, image);
        while self.order.len() > MAX_RENDER_IMAGES {
            if let Some(evicted) = self.order.pop_front() {
                self.entries.remove(&evicted);
            }
        }
    }

    pub fn mark_failed(&mut self, key: &CoverKey) {
        self.in_flight.remove(key);
        if self.failed.len() > 256 {
            self.failed.clear();
        }
        self.failed.insert(key.clone());
    }

    /// Release all decoded textures (called when hiding to system tray to minimize RAM).
    pub fn clear(&mut self) {
        self.entries.clear();
        self.order.clear();
        self.in_flight.clear();
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }
}

/// Fetch and decode a thumbnail or pre-blurred 240x135 ambient background via `biu-core::ImageCache`.
pub async fn fetch_render_image(
    cache: &ImageCache,
    url: &str,
    size: ThumbSize,
    blurred: bool,
) -> anyhow::Result<Arc<RenderImage>> {
    let rgba = cache.get(url, if blurred { ThumbSize::BlurBg } else { size }).await?;
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
