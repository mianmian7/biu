use crate::image_loader::{fetch_render_image, CoverKey, CoverStore};
use crate::theme::BiuPalette;
use biu_audio::{
    AudioEngine, AudioEvent, AudioSource, MediaControlAction, MediaControlsBridge,
    MediaControlsConfig, PlayQueue, PlaybackSnapshot, PlaybackState, SPECTRUM_BARS,
};
use biu_core::auto_lyrics::{format_lrc, run_auto_lyrics, AutoLyrics};
use biu_core::bili::BiliClient;
use biu_core::download::{DownloadFormat, DownloadManager};
use biu_core::image_cache::{ImageCache, ThumbSize};
use biu_core::lyrics::{load_cached_lyrics, save_cached_lyrics, CachedLyrics, LyricsClient};
use biu_core::media_cache::{MediaCache, PreparedAudio};
use biu_core::store::Store;
use biu_core::{
    AppSettings, FavFolder, LyricsCandidate, LyricsData, MediaPage, PlayMode, TrackItem,
    TrackSource, UserInfo,
};
use gpui_kit::*;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CollectionKind {
    FavFolder,
    Season,
    Series,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    MusicRecommend,
    Search {
        query: String,
    },
    Collection {
        kind: CollectionKind,
        id: u64,
        mid: u64,
        title: String,
    },
    Later,
    History,
    FollowList,
    DynamicFeed,
    UserProfile {
        mid: u64,
        name: String,
    },
    LocalMusic,
    DownloadList,
    Settings,
}

#[derive(Debug, Clone)]
pub enum ActiveModal {
    None,
    Login,
    LyricsSearch,
    FavSelect { track: TrackItem },
    FavFolderEdit { folder: Option<FavFolder> },
    PlaylistDrawer,
    PageList,
}

#[derive(Debug, Clone)]
pub struct ToastMessage {
    pub text: String,
    pub is_error: bool,
    pub expires_at: Instant,
}

/// Shared background services (pure Rust, zero UI coupling).
pub struct Services {
    pub rt: tokio::runtime::Runtime,
    pub store: Store,
    pub bili: Arc<BiliClient>,
    pub media_cache: Arc<MediaCache>,
    pub image_cache: Arc<ImageCache>,
    pub lyrics: Arc<LyricsClient>,
    pub downloads: Arc<DownloadManager>,
    pub audio: AudioEngine,
    pub media_controls: MediaControlsBridge,
    pub media_control_rx: std::sync::Mutex<std::sync::mpsc::Receiver<MediaControlAction>>,
}

impl Services {
    pub fn new() -> anyhow::Result<Self> {
        let rt = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(16)
            .enable_all()
            .build()?;

        let store = Store::open()?;
        let data_dir = store.directory().to_path_buf();
        let cache_root = dirs::cache_dir()
            .unwrap_or_else(|| data_dir.clone())
            .join("com.biu.wood3n");
        let media_cache = Arc::new(MediaCache::new(cache_root.join("audio-cache"))?);
        let image_cache = Arc::new(ImageCache::new(cache_root.join("image-cache"))?);
        let bili = Arc::new(BiliClient::open(&store)?);
        let lyrics = Arc::new(LyricsClient::new()?);
        let downloads = DownloadManager::open(store.clone(), bili.clone())?;
        let audio = AudioEngine::new();

        if let Ok(settings) = store.app_settings()
            && !settings.ffmpeg_path.trim().is_empty() {
                audio.set_ffmpeg_path(Some(PathBuf::from(settings.ffmpeg_path.trim())));
            }

        let (media_controls, media_control_rx) =
            MediaControlsBridge::new(MediaControlsConfig::default());

        Ok(Self {
            rt,
            store,
            bili,
            media_cache,
            image_cache,
            lyrics,
            downloads,
            audio,
            media_controls,
            media_control_rx: std::sync::Mutex::new(media_control_rx),
        })
    }
}

/// Window handles managed by the single-process GPUI app.
#[derive(Default)]
pub struct WindowRegistry {
    pub main: Option<AnyWindowHandle>,
    pub desktop_lyrics: Option<AnyWindowHandle>,
    pub mini_player: Option<AnyWindowHandle>,
}

/// Global reactive state owned by GPUI.
pub struct AppState {
    pub services: Arc<Services>,
    pub windows: WindowRegistry,

    // Navigation & overlays
    pub route: Route,
    pub nav_history: Vec<Route>,
    pub nav_index: usize,
    pub modal: ActiveModal,
    pub is_full_screen: bool,
    pub sidebar_collapsed: bool,

    // Coarse-grained dirty versions so 100ms playback ticks never rebuild page trees
    pub ui_version: u64,
    pub playback_version: u64,
    cover_notify_scheduled: bool,

    // Settings & user
    pub settings: AppSettings,
    pub user: Option<UserInfo>,
    pub created_folders: Vec<FavFolder>,
    pub collected_folders: Vec<FavFolder>,
    pub search_history: Vec<String>,
    pub toast: Option<ToastMessage>,

    // Playback & queue
    pub queue: PlayQueue,
    pub snapshot: PlaybackSnapshot,
    pub spectrum: [f32; SPECTRUM_BARS],
    pub lyrics: LyricsData,
    pub lyrics_offset_secs: f64,
    pub lyrics_loading: bool,
    pub auto_lyrics_running: bool,
    pub current_video_pages: Vec<MediaPage>,
    pub current_audio_badge: Option<String>,
    pub current_track_liked: bool,
    pub resolve_request_id: u64,
    pub audio_generation: u64,
    pub active_prepared: Option<PreparedAudio>,
    pub current_track_audio_path: Option<PathBuf>,
    last_reported_heartbeat_track: Option<String>,
    last_mc_state: Option<PlaybackState>,
    last_mc_track_id: Option<String>,

    // Bounded cover image cache
    pub covers: CoverStore,
}

impl AppState {
    pub fn new(services: Arc<Services>, cx: &mut Context<Self>) -> Self {
        let settings = services.store.app_settings().unwrap_or_default();
        let sidebar_collapsed = settings.side_menu_collapsed;
        let search_history = load_search_history(&services.store);
        let snapshot = services.audio.snapshot();

        let mut queue = PlayQueue::default();
        if let Ok(val) = services.store.read("play-queue") {
            if let Some(items_val) = val.get("items")
                && let Ok(items) = serde_json::from_value::<Vec<TrackItem>>(items_val.clone()) {
                    let start = val.get("currentIndex").and_then(|v| v.as_u64()).map(|v| v as usize);
                    if !items.is_empty() {
                        queue.set_queue(items, start.unwrap_or(0));
                    }
                }
            if let Some(mode_val) = val.get("mode")
                && let Ok(mode) = serde_json::from_value::<PlayMode>(mode_val.clone()) {
                    queue.set_mode(mode);
                }
        }

        let state = Self {
            services,
            windows: WindowRegistry::default(),
            route: Route::MusicRecommend,
            nav_history: vec![Route::MusicRecommend],
            nav_index: 0,
            modal: ActiveModal::None,
            is_full_screen: false,
            sidebar_collapsed,
            ui_version: 1,
            playback_version: 1,
            cover_notify_scheduled: false,
            settings,
            user: None,
            created_folders: Vec::new(),
            collected_folders: Vec::new(),
            search_history,
            toast: None,
            queue,
            snapshot,
            spectrum: [0.0; SPECTRUM_BARS],
            lyrics: LyricsData::default(),
            lyrics_offset_secs: 0.0,
            lyrics_loading: false,
            auto_lyrics_running: false,
            current_video_pages: Vec::new(),
            current_audio_badge: None,
            current_track_liked: false,
            resolve_request_id: 0,
            audio_generation: 0,
            active_prepared: None,
            current_track_audio_path: None,
            last_reported_heartbeat_track: None,
            last_mc_state: None,
            last_mc_track_id: None,
            covers: CoverStore::default(),
        };

        state.start_background_loops(cx);
        state
    }

    pub fn palette(&self, window: &Window) -> BiuPalette {
        let system_dark = matches!(
            window.appearance(),
            WindowAppearance::Dark | WindowAppearance::VibrantDark
        );
        BiuPalette::from_settings(
            &self.settings.theme_mode,
            &self.settings.primary_color,
            self.settings.border_radius,
            system_dark,
        )
    }

    pub fn current_track(&self) -> Option<&TrackItem> {
        self.queue.current()
    }

    pub fn notify_ui(&mut self, cx: &mut Context<Self>) {
        self.ui_version = self.ui_version.wrapping_add(1);
        self.playback_version = self.playback_version.wrapping_add(1);
        cx.notify();
    }

    pub fn notify_playback(&mut self, cx: &mut Context<Self>) {
        self.playback_version = self.playback_version.wrapping_add(1);
        cx.notify();
    }

    pub fn covers_version(&self) -> u64 {
        self.covers.version()
    }

    fn schedule_cover_notify(&mut self, cx: &mut Context<Self>) {
        if self.cover_notify_scheduled {
            return;
        }
        self.cover_notify_scheduled = true;
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            cx.background_executor()
                .timer(Duration::from_millis(45))
                .await;
            let _ = this.update(cx, |state, cx| {
                state.cover_notify_scheduled = false;
                cx.notify();
            });
        })
        .detach();
    }

    fn sync_pinned_covers(&mut self) {
        let mut keys = Vec::new();
        if let Some(track) = self.queue.current()
            && !track.cover.is_empty()
        {
            keys.push(CoverKey::thumb(track.cover.clone(), ThumbSize::Tiny));
            keys.push(CoverKey::thumb(track.cover.clone(), ThumbSize::Card));
            keys.push(CoverKey::thumb(track.cover.clone(), ThumbSize::Cover));
            keys.push(CoverKey::blurred(track.cover.clone()));
        }
        if let Some(ref u) = self.user
            && !u.face.is_empty()
        {
            keys.push(CoverKey::thumb(u.face.clone(), ThumbSize::Avatar));
        }
        self.covers.set_pinned(keys);
    }

    pub fn can_go_back(&self) -> bool {
        self.nav_index > 0
    }

    pub fn can_go_forward(&self) -> bool {
        self.nav_index + 1 < self.nav_history.len()
    }

    pub fn go_back(&mut self, cx: &mut Context<Self>) {
        if self.nav_index == 0 {
            return;
        }
        self.nav_index -= 1;
        self.route = self.nav_history[self.nav_index].clone();
        if self.is_full_screen {
            self.is_full_screen = false;
            self.services.audio.set_waveform_enabled(false);
        }
        self.notify_ui(cx);
    }

    pub fn go_forward(&mut self, cx: &mut Context<Self>) {
        if self.nav_index + 1 >= self.nav_history.len() {
            return;
        }
        self.nav_index += 1;
        self.route = self.nav_history[self.nav_index].clone();
        if self.is_full_screen {
            self.is_full_screen = false;
            self.services.audio.set_waveform_enabled(false);
        }
        self.notify_ui(cx);
    }

    pub fn navigate(&mut self, route: Route, cx: &mut Context<Self>) {
        if let Route::Search { ref query } = route {
            let trimmed = query.trim();
            if !trimmed.is_empty() {
                self.push_search_history(trimmed.to_string());
            }
        }
        if self.route != route {
            self.nav_history.truncate(self.nav_index + 1);
            self.nav_history.push(route.clone());
            if self.nav_history.len() > 50 {
                self.nav_history.remove(0);
            }
            self.nav_index = self.nav_history.len().saturating_sub(1);
        }
        self.route = route;
        if self.is_full_screen {
            self.is_full_screen = false;
            self.services.audio.set_waveform_enabled(false);
        }
        self.notify_ui(cx);
    }

    pub fn open_modal(&mut self, modal: ActiveModal, cx: &mut Context<Self>) {
        self.modal = modal;
        self.notify_ui(cx);
    }

    pub fn close_modal(&mut self, cx: &mut Context<Self>) {
        self.modal = ActiveModal::None;
        self.notify_ui(cx);
    }

    pub fn toggle_sidebar(&mut self, cx: &mut Context<Self>) {
        self.sidebar_collapsed = !self.sidebar_collapsed;
        let collapsed = self.sidebar_collapsed;
        self.update_settings(move |s| s.side_menu_collapsed = collapsed, cx);
    }

    pub fn toggle_full_screen(&mut self, cx: &mut Context<Self>) {
        self.is_full_screen = !self.is_full_screen;
        self.services.audio.set_waveform_enabled(self.is_full_screen);
        if self.is_full_screen
            && let Some(track) = self.current_track().cloned()
            && !track.cover.is_empty()
        {
            self.ensure_cover(&track.cover, ThumbSize::Cover, false, cx);
            self.ensure_cover(&track.cover, ThumbSize::BlurBg, true, cx);
        }
        self.notify_ui(cx);
    }

    pub fn show_toast(&mut self, text: impl Into<String>, is_error: bool, cx: &mut Context<Self>) {
        self.toast = Some(ToastMessage {
            text: text.into(),
            is_error,
            expires_at: Instant::now() + Duration::from_secs(3),
        });
        self.notify_ui(cx);
    }

    pub fn update_settings(&mut self, f: impl FnOnce(&mut AppSettings), cx: &mut Context<Self>) {
        let prev_proxy = self.settings.proxy_settings.clone();
        let prev_ffmpeg = self.settings.ffmpeg_path.clone();
        f(&mut self.settings);
        let _ = self.services.store.save_app_settings(&self.settings);
        if self.settings.proxy_settings != prev_proxy {
            let _ = self
                .services
                .bili
                .update_proxy(Some(self.settings.proxy_settings.clone()));
        }
        if self.settings.ffmpeg_path != prev_ffmpeg {
            let p = self.settings.ffmpeg_path.trim();
            self.services
                .audio
                .set_ffmpeg_path((!p.is_empty()).then(|| PathBuf::from(p)));
        }
        self.notify_ui(cx);
    }

    pub fn push_search_history(&mut self, term: String) {
        self.search_history.retain(|s| s != &term);
        self.search_history.insert(0, term);
        self.search_history.truncate(20);
        let _ = self.services.store.write(
            "search-history",
            serde_json::json!({ "items": self.search_history }),
        );
    }

    pub fn remove_search_history(&mut self, term: &str, cx: &mut Context<Self>) {
        let before = self.search_history.len();
        self.search_history.retain(|s| s != term);
        if self.search_history.len() != before {
            let _ = self.services.store.write(
                "search-history",
                serde_json::json!({ "items": self.search_history }),
            );
            self.notify_ui(cx);
        }
    }

    pub fn clear_search_history(&mut self, cx: &mut Context<Self>) {
        self.search_history.clear();
        let _ = self
            .services
            .store
            .write("search-history", serde_json::json!({ "items": [] }));
        self.notify_ui(cx);
    }

    pub fn persist_queue(&self) {
        let val = serde_json::json!({
            "items": self.queue.items(),
            "currentIndex": self.queue.current_index(),
            "mode": self.queue.mode(),
        });
        let _ = self.services.store.write("play-queue", val);
    }

    pub fn refresh_user_and_folders(&mut self, cx: &mut Context<Self>) {
        let services = self.services.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let handle = services.rt.spawn({
                let services = services.clone();
                async move {
                    services.bili.inject_auth_cookies().await;
                    let nav = services.bili.get_nav().await.ok();
                    let (created, collected) = if let Some(ref u) = nav {
                        if u.is_login && u.mid > 0 {
                            let c1 = services
                                .bili
                                .get_created_fav_folders(u.mid)
                                .await
                                .unwrap_or_default();
                            let c2 = services
                                .bili
                                .get_collected_fav_folders(u.mid, 1)
                                .await
                                .map(|p| p.items)
                                .unwrap_or_default();
                            (c1, c2)
                        } else {
                            (Vec::new(), Vec::new())
                        }
                    } else {
                        (Vec::new(), Vec::new())
                    };
                    (nav, created, collected)
                }
            });

            if let Ok((nav, created, collected)) = handle.await {
                let _ = this.update(cx, |state, cx| {
                    state.user = nav.filter(|u| u.is_login);
                    state.created_folders = created;
                    state.collected_folders = collected;
                    state.sync_pinned_covers();
                    if let Some(face) = state.user.as_ref().map(|u| u.face.clone())
                        && !face.is_empty()
                    {
                        state.ensure_cover(&face, ThumbSize::Avatar, false, cx);
                    }
                    state.notify_ui(cx);
                });
            }
        })
        .detach();
    }

    /// Clear all cached cover textures and explicitly release them from GPUI's GPU sprite atlas.
    pub fn clear_covers(&mut self, cx: &mut Context<Self>) {
        let dropped = self.covers.clear();
        for img in dropped {
            cx.drop_image(img, None);
        }
    }

    /// Ensure a cover image is loaded into the bounded GPU `CoverStore`.
    pub fn ensure_cover(
        &mut self,
        url: &str,
        size: ThumbSize,
        blurred: bool,
        cx: &mut Context<Self>,
    ) -> Option<Arc<RenderImage>> {
        if url.trim().is_empty() {
            return None;
        }
        let key = if blurred {
            CoverKey::blurred(url)
        } else {
            CoverKey::thumb(url, size)
        };
        if let Some(img) = self.covers.get(&key) {
            self.covers.touch(&key);
            return Some(img);
        }
        if !self.covers.should_fetch(&key) {
            return None;
        }
        self.covers.mark_in_flight(key.clone());
        let epoch = self.covers.clear_epoch();
        let services = self.services.clone();
        let url_owned = url.to_string();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = services
                .rt
                .spawn({
                    let cache = services.image_cache.clone();
                    async move { fetch_render_image(&cache, &url_owned, size, blurred).await }
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                match res {
                    Ok(Ok(img)) => {
                        let evicted = state.covers.insert(epoch, key, img);
                        for old in evicted {
                            cx.drop_image(old, None);
                        }
                        // Coalesce burst cover arrivals into a single UI repaint (~22Hz max)
                        // instead of rebuilding the entire page tree per thumbnail.
                        state.schedule_cover_notify(cx);
                    }
                    _ => state.covers.mark_failed(epoch, &key),
                }
            });
        })
        .detach();
        None
    }

    /// Prefetch a batch of covers without blocking rendering.
    pub fn prefetch_covers(
        &mut self,
        items: impl IntoIterator<Item = (String, ThumbSize)>,
        cx: &mut Context<Self>,
    ) {
        for (url, size) in items {
            if !url.is_empty() {
                self.ensure_cover(&url, size, false, cx);
            }
        }
    }

    pub fn play_queue(
        &mut self,
        tracks: Vec<TrackItem>,
        start_index: usize,
        cx: &mut Context<Self>,
    ) {
        if tracks.is_empty() {
            return;
        }
        let idx = start_index.min(tracks.len().saturating_sub(1));
        self.queue.set_queue(tracks, idx);
        self.persist_queue();
        if let Some(track) = self.queue.current().cloned() {
            self.start_playback_for_track(track, cx);
        }
    }

    pub fn play_track(&mut self, track: TrackItem, cx: &mut Context<Self>) {
        self.queue.play_or_insert(track.clone());
        self.persist_queue();
        self.start_playback_for_track(track, cx);
    }

    pub fn play_next(&mut self, track: TrackItem, cx: &mut Context<Self>) {
        if self.queue.is_empty() {
            self.play_track(track, cx);
            return;
        }
        self.queue.insert_next(track);
        self.persist_queue();
        self.show_toast("已添加到下一首播放", false, cx);
    }

    pub fn append_to_queue(&mut self, tracks: Vec<TrackItem>, cx: &mut Context<Self>) {
        let count = tracks.len();
        if count == 0 {
            return;
        }
        self.queue.append(tracks);
        self.persist_queue();
        self.show_toast(format!("已加入播放列表 ({count})"), false, cx);
    }

    pub fn jump_to_queue_index(&mut self, idx: usize, cx: &mut Context<Self>) {
        if let Some(track) = self.queue.jump_to(idx).cloned() {
            self.persist_queue();
            self.start_playback_for_track(track, cx);
        }
    }

    pub fn remove_from_queue(&mut self, idx: usize, cx: &mut Context<Self>) {
        let Some(removed) = self.queue.remove(idx) else {
            return;
        };
        self.persist_queue();
        if removed.was_current {
            if let Some(next) = self.queue.current().cloned() {
                self.start_playback_for_track(next, cx);
            } else {
                self.invalidate_active_playback();
            }
        }
        self.notify_ui(cx);
    }

    pub fn clear_queue(&mut self, cx: &mut Context<Self>) {
        self.queue.clear();
        self.invalidate_active_playback();
        self.persist_queue();
        self.notify_ui(cx);
    }

    fn invalidate_active_playback(&mut self) {
        self.resolve_request_id = self.resolve_request_id.wrapping_add(1);
        self.audio_generation = 0;
        if let Some(prev) = self.active_prepared.take() {
            prev.abort();
        }
        self.current_track_audio_path = None;
        self.services.audio.stop();
        self.services.media_controls.set_stopped();
        self.last_mc_state = Some(PlaybackState::Stopped);
        self.lyrics = LyricsData::default();
        self.lyrics_offset_secs = 0.0;
        self.lyrics_loading = false;
        self.current_video_pages.clear();
        self.current_audio_badge = None;
        self.current_track_liked = false;
    }

    pub fn open_track_in_bilibili(&mut self, track: &TrackItem, cx: &mut Context<Self>) {
        let url = match &track.source {
            TrackSource::BiliVideo { bvid, .. } if !bvid.is_empty() => {
                format!("https://www.bilibili.com/video/{bvid}")
            }
            TrackSource::BiliAudio { sid } if *sid > 0 => {
                format!("https://www.bilibili.com/audio/au{sid}")
            }
            _ => {
                self.show_toast("本地文件没有 B 站链接", false, cx);
                return;
            }
        };
        if let Err(err) = open::that_detached(&url) {
            self.show_toast(format!("打开浏览器失败: {err}"), true, cx);
        }
    }

    pub fn adjust_lyrics_offset(&mut self, delta: f64, cx: &mut Context<Self>) {
        let next = if delta == 0.0 {
            0.0
        } else {
            ((self.lyrics_offset_secs + delta) * 10.0).round() / 10.0
        };
        self.lyrics_offset_secs = next.clamp(-30.0, 30.0);
        self.show_toast(
            format!("歌词偏移 {:+.1}s", self.lyrics_offset_secs),
            false,
            cx,
        );
        self.notify_playback(cx);
    }

    pub fn switch_video_part(&mut self, page: &MediaPage, cx: &mut Context<Self>) {
        let Some(current) = self.current_track().cloned() else {
            return;
        };
        let TrackSource::BiliVideo { bvid, aid, .. } = &current.source else {
            return;
        };
        let part_track = TrackItem {
            id: format!("{bvid}:{}", page.cid),
            title: current.title.clone(),
            artist: current.artist.clone(),
            artist_mid: current.artist_mid,
            cover: current.cover.clone(),
            duration_secs: if page.duration > 0.0 {
                page.duration
            } else {
                current.duration_secs
            },
            source: TrackSource::BiliVideo {
                bvid: bvid.clone(),
                aid: *aid,
                cid: page.cid,
                part_title: (!page.part.is_empty()).then(|| page.part.clone()),
            },
        };
        self.play_track(part_track, cx);
    }

    pub fn queue_all_video_parts(&mut self, cx: &mut Context<Self>) {
        let Some(current) = self.current_track().cloned() else {
            return;
        };
        let TrackSource::BiliVideo { bvid, aid, .. } = &current.source else {
            return;
        };
        if self.current_video_pages.is_empty() {
            return;
        }
        let tracks: Vec<TrackItem> = self
            .current_video_pages
            .iter()
            .map(|page| TrackItem {
                id: format!("{bvid}:{}", page.cid),
                title: if page.part.is_empty() {
                    format!("{} · P{}", current.title, page.page)
                } else {
                    format!("{} · P{} {}", current.title, page.page, page.part)
                },
                artist: current.artist.clone(),
                artist_mid: current.artist_mid,
                cover: current.cover.clone(),
                duration_secs: page.duration,
                source: TrackSource::BiliVideo {
                    bvid: bvid.clone(),
                    aid: *aid,
                    cid: page.cid,
                    part_title: (!page.part.is_empty()).then(|| page.part.clone()),
                },
            })
            .collect();
        self.append_to_queue(tracks, cx);
    }

    pub fn toggle_like_current_track(&mut self, cx: &mut Context<Self>) {
        if self.user.is_none() {
            self.open_modal(ActiveModal::Login, cx);
            return;
        }
        let Some(TrackSource::BiliVideo { bvid, .. }) =
            self.current_track().map(|t| t.source.clone())
        else {
            return;
        };
        let target_like = !self.current_track_liked;
        self.current_track_liked = target_like;
        self.notify_ui(cx);

        let services = self.services.clone();
        let expected_bvid = bvid.clone();
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = services
                .rt
                .spawn({
                    let bili = services.bili.clone();
                    async move { bili.archive_like(&bvid, target_like).await }
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                let same = matches!(
                    state.current_track().map(|t| &t.source),
                    Some(TrackSource::BiliVideo { bvid, .. }) if bvid == &expected_bvid
                );
                if !same {
                    return;
                }
                match res {
                    Ok(Ok(())) => {
                        state.show_toast(
                            if target_like { "已点赞" } else { "已取消点赞" },
                            false,
                            cx,
                        );
                    }
                    Ok(Err(e)) => {
                        state.current_track_liked = !target_like;
                        state.show_toast(format!("点赞失败: {e}"), true, cx);
                    }
                    Err(e) => {
                        state.current_track_liked = !target_like;
                        state.show_toast(format!("点赞任务异常: {e}"), true, cx);
                    }
                }
            });
        })
        .detach();
    }

    pub fn toggle_play(&mut self, cx: &mut Context<Self>) {
        if self.snapshot.state == PlaybackState::Idle
            && let Some(track) = self.queue.current().cloned()
        {
            self.start_playback_for_track(track, cx);
            return;
        }
        self.services.audio.toggle_play();
        self.snapshot = self.services.audio.snapshot();
        self.notify_ui(cx);
    }

    pub fn next_track(&mut self, auto_advance: bool, cx: &mut Context<Self>) {
        if let Some(track) = self.queue.next_track(auto_advance).cloned() {
            self.persist_queue();
            self.start_playback_for_track(track, cx);
        } else {
            self.services.audio.stop();
            self.notify_ui(cx);
        }
    }

    pub fn prev_track(&mut self, cx: &mut Context<Self>) {
        if let Some(track) = self.queue.prev_track().cloned() {
            self.persist_queue();
            self.start_playback_for_track(track, cx);
        }
    }

    pub fn seek(&mut self, secs: f64, cx: &mut Context<Self>) {
        let target = secs.max(0.0);
        self.services.audio.seek(target);
        self.snapshot = self.services.audio.snapshot();
        let playing = self.snapshot.state == PlaybackState::Playing;
        self.services.media_controls.set_playback(playing, target);
        self.notify_playback(cx);
    }

    pub fn set_volume(&mut self, volume: f32, cx: &mut Context<Self>) {
        self.services.audio.set_volume(volume.clamp(0.0, 1.0));
        self.snapshot = self.services.audio.snapshot();
        self.notify_playback(cx);
    }

    pub fn adjust_volume(&mut self, delta: f32, cx: &mut Context<Self>) {
        let next = (self.snapshot.volume + delta).clamp(0.0, 1.0);
        self.set_volume(next, cx);
    }

    pub fn toggle_mute(&mut self, cx: &mut Context<Self>) {
        self.services.audio.set_muted(!self.snapshot.muted);
        self.snapshot = self.services.audio.snapshot();
        self.notify_playback(cx);
    }

    pub fn set_rate(&mut self, rate: f32, cx: &mut Context<Self>) {
        self.services.audio.set_rate(rate.clamp(0.5, 2.0));
        self.snapshot = self.services.audio.snapshot();
        self.notify_playback(cx);
    }

    pub fn toggle_play_mode(&mut self, cx: &mut Context<Self>) {
        self.queue.toggle_mode();
        self.persist_queue();
        self.notify_ui(cx);
    }

    pub fn queue_download(
        &mut self,
        track: TrackItem,
        format: DownloadFormat,
        cx: &mut Context<Self>,
    ) {
        let title = track.title.clone();
        match self.services.downloads.queue(track, format) {
            Ok(_) => self.show_toast(format!("已加入下载队列: {title}"), false, cx),
            Err(err) => self.show_toast(format!("下载失败: {err}"), true, cx),
        }
    }

    pub fn start_playback_for_track(&mut self, track: TrackItem, cx: &mut Context<Self>) {
        if let Some(prev) = self.active_prepared.take() {
            prev.abort();
        }
        self.resolve_request_id = self.resolve_request_id.wrapping_add(1);
        let req_id = self.resolve_request_id;
        self.current_track_audio_path = None;
        self.last_reported_heartbeat_track = None;
        self.lyrics = LyricsData::default();
        self.lyrics_offset_secs = 0.0;
        self.lyrics_loading = true;
        self.current_audio_badge = None;
        self.current_track_liked = false;
        let keep_pages = matches!(
            &track.source,
            TrackSource::BiliVideo { bvid, .. }
                if !self.current_video_pages.is_empty()
                    && self.queue.items().iter().any(|t| matches!(&t.source, TrackSource::BiliVideo { bvid: b, .. } if b == bvid))
        );
        if !keep_pages {
            self.current_video_pages.clear();
        }

        self.sync_pinned_covers();
        if !track.cover.is_empty() {
            self.ensure_cover(&track.cover, ThumbSize::Tiny, false, cx);
            if self.is_full_screen {
                self.ensure_cover(&track.cover, ThumbSize::Cover, false, cx);
                self.ensure_cover(&track.cover, ThumbSize::BlurBg, true, cx);
            }
        }

        self.load_lyrics_for_track(&track, req_id, cx);

        if let TrackSource::BiliVideo { ref bvid, .. } = track.source
            && !bvid.is_empty()
        {
            let services_meta = self.services.clone();
            let bvid_owned = bvid.clone();
            let logged_in = self.user.is_some();
            cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
                let res = services_meta
                    .rt
                    .spawn({
                        let bili = services_meta.bili.clone();
                        let bvid = bvid_owned.clone();
                        async move {
                            let pages = bili.get_video_pagelist(&bvid).await.ok();
                            let liked = if logged_in {
                                bili.archive_has_like(&bvid).await.unwrap_or(false)
                            } else {
                                false
                            };
                            (pages, liked)
                        }
                    })
                    .await;
                if let Ok((pages, liked)) = res {
                    let _ = this.update(cx, |state, cx| {
                        if state.resolve_request_id != req_id {
                            return;
                        }
                        if let Some(pages) = pages {
                            state.current_video_pages = pages;
                        }
                        state.current_track_liked = liked;
                        state.notify_ui(cx);
                    });
                }
            })
            .detach();
        }

        let services = self.services.clone();
        let quality = self.settings.audio_quality;
        let report_history = self.settings.report_play_history;

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let resolve_res = services
                .rt
                .spawn({
                    let services = services.clone();
                    let track = track.clone();
                    async move { resolve_audio_source(&services, &track, quality, report_history).await }
                })
                .await;

            let _ = this.update(cx, |state, cx| {
                if state.resolve_request_id != req_id {
                    if let Ok(Ok((_, Some(prep), _, _))) = resolve_res {
                        prep.abort();
                    }
                    return;
                }
                match resolve_res {
                    Ok(Ok((source, prepared, file_path, audio_badge))) => {
                        state.active_prepared = prepared;
                        state.current_track_audio_path = file_path;
                        state.current_audio_badge = audio_badge;
                        let audio_gen = state.services.audio.play(source, 0.0);
                        state.audio_generation = audio_gen;
                        state.snapshot = state.services.audio.snapshot();
                        state.update_os_media_metadata(&track);
                    }
                    Ok(Err(err)) => {
                        state.show_toast(format!("无法播放《{}》: {err}", track.title), true, cx);
                    }
                    Err(err) => {
                        state.show_toast(format!("播放任务异常: {err}"), true, cx);
                    }
                }
                state.notify_ui(cx);
            });
        })
        .detach();
        self.notify_ui(cx);
    }

    fn load_lyrics_for_track(
        &mut self,
        track: &TrackItem,
        req_id: u64,
        cx: &mut Context<Self>,
    ) {
        let services = self.services.clone();
        let track_id = track.id.clone();
        let (primary_key, legacy_key) = lyrics_cache_keys(track);
        let query = clean_lyrics_query(&track.title, &track.artist);

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = services
                .rt
                .spawn({
                    let services = services.clone();
                    async move {
                        for key in std::iter::once(primary_key.as_str())
                            .chain(legacy_key.as_deref())
                        {
                            if let Ok(Some(cached)) = load_cached_lyrics(&services.store, key) {
                                let data = cached.data();
                                if !data.lines.is_empty() {
                                    return Ok::<LyricsData, anyhow::Error>(data);
                                }
                            }
                        }
                        let candidates = services
                            .lyrics
                            .search_netease(&query, 5)
                            .await
                            .unwrap_or_default();
                        if let Some(first) = candidates.first()
                            && let Ok(data) = services.lyrics.fetch(first).await
                            && !data.lines.is_empty()
                        {
                            let cached = lyrics_data_to_cached(&data, 0.0);
                            let _ = save_cached_lyrics(
                                &services.store,
                                &primary_key,
                                &cached,
                                false,
                            );
                            return Ok(data);
                        }
                        let lrclib = services
                            .lyrics
                            .search_lrclib(&query)
                            .await
                            .unwrap_or_default();
                        if let Some(first) = lrclib.first()
                            && let Ok(data) = services.lyrics.fetch(first).await
                            && !data.lines.is_empty()
                        {
                            let cached = lyrics_data_to_cached(&data, 0.0);
                            let _ = save_cached_lyrics(
                                &services.store,
                                &primary_key,
                                &cached,
                                false,
                            );
                            return Ok(data);
                        }
                        Ok(LyricsData::default())
                    }
                })
                .await;

            let _ = this.update(cx, |state, cx| {
                let matches_current = state
                    .current_track()
                    .is_some_and(|t| t.id == track_id);
                if matches_current && state.resolve_request_id == req_id {
                    state.lyrics_loading = false;
                    if let Ok(Ok(data)) = res {
                        state.lyrics = data;
                    }
                    state.notify_playback(cx);
                }
            });
        })
        .detach();
    }

    pub fn apply_manual_lyrics(&mut self, candidate: LyricsCandidate, cx: &mut Context<Self>) {
        let Some(track) = self.current_track().cloned() else {
            return;
        };
        let services = self.services.clone();
        let expected_track_id = track.id.clone();
        let (primary_key, _) = lyrics_cache_keys(&track);
        self.lyrics_loading = true;
        cx.notify();

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = services
                .rt
                .spawn({
                    let services = services.clone();
                    async move {
                        let data = services.lyrics.fetch(&candidate).await?;
                        let cached = lyrics_data_to_cached(&data, 0.0);
                        let _ = save_cached_lyrics(&services.store, &primary_key, &cached, true);
                        Ok::<LyricsData, anyhow::Error>(data)
                    }
                })
                .await;
            let _ = this.update(cx, |state, cx| {
                let still_current = state
                    .current_track()
                    .is_some_and(|t| t.id == expected_track_id);
                if !still_current {
                    return;
                }
                state.lyrics_loading = false;
                match res {
                    Ok(Ok(data)) => {
                        state.lyrics = data;
                        if matches!(state.modal, ActiveModal::LyricsSearch) {
                            state.close_modal(cx);
                        }
                        state.show_toast("已应用新歌词", false, cx);
                    }
                    Ok(Err(e)) => state.show_toast(format!("歌词加载失败: {e}"), true, cx),
                    Err(e) => state.show_toast(format!("歌词任务异常: {e}"), true, cx),
                }
                state.notify_playback(cx);
            });
        })
        .detach();
    }

    pub fn trigger_auto_lyrics(&mut self, cx: &mut Context<Self>) {
        let Some(track) = self.current_track().cloned() else {
            return;
        };
        let audio_path = match self.resolved_complete_audio_path() {
            Ok(path) => path,
            Err(msg) => {
                self.show_toast(msg, true, cx);
                return;
            }
        };
        if self.auto_lyrics_running {
            return;
        }
        self.auto_lyrics_running = true;
        self.show_toast("正在执行声纹自动对齐歌词…", false, cx);
        let services = self.services.clone();
        let expected_track_id = track.id.clone();
        let (primary_key, _) = lyrics_cache_keys(&track);
        let ffmpeg_path = (!self.settings.ffmpeg_path.trim().is_empty())
            .then(|| PathBuf::from(self.settings.ffmpeg_path.trim()));

        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let res = services
                .rt
                .spawn(run_auto_lyrics_task(
                    services.clone(),
                    primary_key,
                    audio_path,
                    track.title.clone(),
                    track.artist.clone(),
                    ffmpeg_path,
                ))
                .await;

            let _ = this.update(cx, |state, cx| {
                state.auto_lyrics_running = false;
                let still_current = state
                    .current_track()
                    .is_some_and(|t| t.id == expected_track_id);
                if !still_current {
                    cx.notify();
                    return;
                }
                match res {
                    Ok(Ok(aligned)) => {
                        state.lyrics = aligned.lyrics;
                        state.show_toast(
                            format!(
                                "自动对齐成功（偏移 {:+.2}s · 置信度 {:.0}%）",
                                aligned.offset_secs,
                                aligned.confidence * 100.0
                            ),
                            false,
                            cx,
                        );
                    }
                    Ok(Err(e)) => state.show_toast(format!("自动对齐失败: {e}"), true, cx),
                    Err(e) => state.show_toast(format!("自动对齐任务异常: {e}"), true, cx),
                }
                state.notify_playback(cx);
            });
        })
        .detach();
    }

    fn resolved_complete_audio_path(&self) -> Result<PathBuf, &'static str> {
        if let Some(ref prepared) = self.active_prepared {
            match prepared {
                PreparedAudio::Ready { path, .. } if path.exists() => return Ok(path.clone()),
                PreparedAudio::Streaming { path, done, .. } => {
                    if !done.load(std::sync::atomic::Ordering::Acquire) {
                        return Err("请等待当前音频缓冲完成后再自动对齐");
                    }
                    let committed = path.with_extension("audio");
                    if committed.exists() {
                        return Ok(committed);
                    }
                    if path.exists() {
                        return Ok(path.clone());
                    }
                }
                _ => {}
            }
        }
        if let Some(ref p) = self.current_track_audio_path {
            if p.exists() {
                return Ok(p.clone());
            }
            let committed = p.with_extension("audio");
            if committed.exists() {
                return Ok(committed);
            }
        }
        Err("请等待当前音频缓冲完成后再自动对齐")
    }

    fn update_os_media_metadata(&mut self, track: &TrackItem) {
        let dur = if track.duration_secs > 0.0 {
            Some(track.duration_secs)
        } else {
            None
        };
        self.services
            .media_controls
            .set_metadata(&track.title, &track.artist, None, dur);
        self.last_mc_track_id = Some(track.id.clone());
    }

    fn start_background_loops(&self, cx: &mut Context<Self>) {
        // Adaptive ticker: 100ms while playing or visible; 500ms while paused & hidden in tray.
        cx.spawn(async move |this: WeakEntity<Self>, cx: &mut AsyncApp| {
            let mut audio_events = this
                .read_with(cx, |s, _| s.services.audio.subscribe_events())
                .ok();
            loop {
                let tick_ms = this
                    .read_with(cx, |s, _| {
                        let any_window = s.windows.main.is_some()
                            || s.windows.mini_player.is_some()
                            || s.windows.desktop_lyrics.is_some();
                        let playing = s.snapshot.state == PlaybackState::Playing;
                        if playing || any_window || s.toast.is_some() {
                            100
                        } else {
                            500
                        }
                    })
                    .unwrap_or(500);
                cx.background_executor()
                    .timer(Duration::from_millis(tick_ms))
                    .await;

                // Drain audio events non-blockingly
                let mut ended_gen: Option<u64> = None;
                let mut err_msg: Option<(u64, String)> = None;
                if let Some(ref mut rx) = audio_events {
                    while let Ok(ev) = rx.try_recv() {
                        match ev {
                            AudioEvent::TrackEnded { generation } => ended_gen = Some(generation),
                            AudioEvent::Error {
                                generation,
                                message,
                            } => err_msg = Some((generation, message)),
                            AudioEvent::StateChanged(_) => {}
                        }
                    }
                }

                let ok = this.update(cx, |state, cx| {
                    let prev_pos = state.snapshot.position_secs;
                    let prev_state = state.snapshot.state;
                    state.snapshot = state.services.audio.snapshot();
                    let playing = state.snapshot.state == PlaybackState::Playing;

                    if state.is_full_screen && playing {
                        state.spectrum = state.services.audio.spectrum();
                    }

                    // Drain OS media key actions
                    let mut actions = Vec::new();
                    if let Ok(rx) = state.services.media_control_rx.lock() {
                        while let Ok(action) = rx.try_recv() {
                            actions.push(action);
                        }
                    }
                    for action in actions {
                        match action {
                            MediaControlAction::Play => state.services.audio.resume(),
                            MediaControlAction::Pause => state.services.audio.pause(),
                            MediaControlAction::Toggle => state.toggle_play(cx),
                            MediaControlAction::Next => state.next_track(false, cx),
                            MediaControlAction::Previous => state.prev_track(cx),
                            MediaControlAction::Stop => state.services.audio.stop(),
                            MediaControlAction::Seek(pos) => state.seek(pos, cx),
                            MediaControlAction::SeekBy(delta) => {
                                let p = (state.snapshot.position_secs + delta).max(0.0);
                                state.seek(p, cx);
                            }
                            MediaControlAction::SetVolume(v) => state.set_volume(v as f32, cx),
                            MediaControlAction::Raise | MediaControlAction::Quit => {}
                        }
                    }

                    if state.last_mc_state != Some(state.snapshot.state) {
                        match state.snapshot.state {
                            PlaybackState::Playing => state
                                .services
                                .media_controls
                                .set_playback(true, state.snapshot.position_secs),
                            PlaybackState::Paused => state
                                .services
                                .media_controls
                                .set_playback(false, state.snapshot.position_secs),
                            PlaybackState::Idle | PlaybackState::Stopped => {
                                state.services.media_controls.set_stopped()
                            }
                            PlaybackState::Loading | PlaybackState::Ended | PlaybackState::Error => {
                            }
                        }
                        state.last_mc_state = Some(state.snapshot.state);
                    }

                    if let Some(ref toast) = state.toast
                        && Instant::now() >= toast.expires_at
                    {
                        state.toast = None;
                        state.notify_ui(cx);
                    }

                    if let Some(ended_generation) = ended_gen
                        && state.audio_generation > 0
                        && ended_generation == state.audio_generation
                    {
                        state.next_track(true, cx);
                    }
                    if let Some((err_gen, msg)) = err_msg
                        && (state.audio_generation == 0 || err_gen == state.audio_generation)
                    {
                        state.show_toast(format!("播放出错: {msg}"), true, cx);
                    }

                    if state.snapshot.state != prev_state {
                        state.notify_ui(cx);
                    } else if playing || (state.snapshot.position_secs - prev_pos).abs() > 0.05 {
                        state.notify_playback(cx);
                    }
                });

                if ok.is_err() {
                    break;
                }
            }
        })
        .detach();
    }
}

fn lyrics_cache_keys(track: &TrackItem) -> (String, Option<String>) {
    match &track.source {
        TrackSource::BiliVideo { bvid, cid, .. } if *cid > 0 => (
            format!("{bvid}-{cid}"),
            Some(track.id.clone()),
        ),
        _ => (track.id.clone(), None),
    }
}

async fn run_auto_lyrics_task(
    services: Arc<Services>,
    track_key: String,
    audio_path: PathBuf,
    title: String,
    artist: String,
    ffmpeg_path: Option<PathBuf>,
) -> anyhow::Result<biu_core::AutoLyricsResult> {
    let auto_client = AutoLyrics {
        bili: &services.bili,
        lyrics: &services.lyrics,
        media_cache: &services.media_cache,
        ffmpeg: ffmpeg_path.as_deref(),
    };
    let aligned = run_auto_lyrics(&auto_client, &audio_path, &title, &artist).await?;
    let cached = lyrics_data_to_cached(&aligned.lyrics, 0.0);
    let _ = save_cached_lyrics(&services.store, &track_key, &cached, false);
    Ok(aligned)
}

fn lyrics_data_to_cached(data: &LyricsData, offset_ms: f64) -> CachedLyrics {
    let main_lrc = format_lrc(&data.lines, |l| Some(l.text.as_str()));
    let t_lrc = format_lrc(&data.lines, |l| l.translation.as_deref());
    let r_lrc = format_lrc(&data.lines, |l| l.romaji.as_deref());
    CachedLyrics {
        lyrics: (!main_lrc.is_empty()).then_some(main_lrc),
        t_lyrics: (!t_lrc.is_empty()).then_some(t_lrc),
        r_lyrics: (!r_lrc.is_empty()).then_some(r_lrc),
        yrc: None,
        offset: offset_ms,
        source: data.source.clone(),
        extra: Default::default(),
    }
}

async fn resolve_audio_source(
    services: &Services,
    track: &TrackItem,
    quality: biu_core::AudioQuality,
    report_history: bool,
) -> anyhow::Result<(AudioSource, Option<PreparedAudio>, Option<PathBuf>, Option<String>)> {
    match &track.source {
        TrackSource::LocalFile { path, .. } => {
            let p = PathBuf::from(path);
            if !p.exists() {
                anyhow::bail!("本地文件不存在: {path}");
            }
            Ok((AudioSource::File(p.clone()), None, Some(p), None))
        }
        TrackSource::BiliAudio { sid } => {
            let info = services.bili.get_audio_playurl(*sid).await?;
            let badge = info
                .streams
                .first()
                .and_then(|s| (s.id == 3 || s.codecs == "flac").then(|| "无损".to_string()));
            let url = info
                .urls
                .first()
                .ok_or_else(|| anyhow::anyhow!("未获取到音频直链"))?;
            let (src, prep, path) = prepare_stream_from_url(services, url).await?;
            Ok((src, prep, path, badge))
        }
        TrackSource::BiliVideo { bvid, aid, cid, .. } => {
            let (resolved_aid, resolved_cid) = if *cid > 0 {
                (*aid, *cid)
            } else {
                let view = services.bili.get_video_view(bvid).await?;
                let first_cid = view
                    .pages
                    .first()
                    .map(|p| p.cid)
                    .or(match view.track.source {
                        TrackSource::BiliVideo { cid, .. } => Some(cid),
                        _ => None,
                    })
                    .unwrap_or(0);
                let resolved_aid = match view.track.source {
                    TrackSource::BiliVideo { aid, .. } => aid,
                    _ => *aid,
                };
                (resolved_aid, first_cid)
            };
            if resolved_cid == 0 {
                anyhow::bail!("无法解析视频分 P cid");
            }
            if report_history && resolved_aid > 0 {
                let bili = services.bili.clone();
                tokio::spawn(async move {
                    let _ = bili.report_heartbeat(resolved_aid, resolved_cid, 0).await;
                });
            }
            let info = services.bili.get_playurl(bvid, resolved_cid, quality).await?;
            let mut last_err = None;
            let mut tried = std::collections::HashSet::new();
            for stream in &info.streams {
                let badge = match stream.id {
                    30251 => Some("无损".to_string()),
                    30250 => Some("杜比".to_string()),
                    _ => None,
                };
                for url in &stream.urls {
                    if !tried.insert(url.as_str().to_string()) {
                        continue;
                    }
                    match prepare_stream_from_url(services, url).await {
                        Ok((src, prep, path)) => return Ok((src, prep, path, badge)),
                        Err(e) => last_err = Some(e),
                    }
                }
            }
            for url in &info.urls {
                if !tried.insert(url.as_str().to_string()) {
                    continue;
                }
                match prepare_stream_from_url(services, url).await {
                    Ok((src, prep, path)) => return Ok((src, prep, path, None)),
                    Err(e) => last_err = Some(e),
                }
            }
            Err(last_err.unwrap_or_else(|| anyhow::anyhow!("未获取到可用音频流")))
        }
    }
}

async fn prepare_stream_from_url(
    services: &Services,
    url: &url::Url,
) -> anyhow::Result<(AudioSource, Option<PreparedAudio>, Option<PathBuf>)> {
    let bili = services.bili.clone();
    let prepared = services
        .media_cache
        .prepare_audio_file(url.clone(), services.bili.media_client(), move |u| {
            bili.is_media_url_allowed(u)
        })
        .await?;

    match &prepared {
        PreparedAudio::Ready { path, .. } => {
            let p = path.clone();
            Ok((AudioSource::File(p.clone()), Some(prepared), Some(p)))
        }
        PreparedAudio::Streaming {
            path,
            total_bytes,
            done,
            aborted,
            error,
        } => {
            let p = path.clone();
            let src = AudioSource::StreamingFile {
                path: p.clone(),
                total_bytes: (*total_bytes > 0).then_some(*total_bytes),
                done: done.clone(),
                aborted: aborted.clone(),
                error: error.clone(),
            };
            Ok((src, Some(prepared), Some(p)))
        }
    }
}

fn load_search_history(store: &Store) -> Vec<String> {
    store
        .read("search-history")
        .ok()
        .and_then(|v| v.get("items").cloned())
        .and_then(|v| serde_json::from_value(v).ok())
        .unwrap_or_default()
}

fn clean_lyrics_query(title: &str, artist: &str) -> String {
    let mut cleaned = title.to_string();
    for pair in [('【', '】'), ('[', ']'), ('(', ')'), ('（', '）'), ('《', '》')] {
        while let Some(start) = cleaned.find(pair.0) {
            if let Some(end) = cleaned[start..].find(pair.1) {
                cleaned.replace_range(start..start + end + pair.1.len_utf8(), " ");
            } else {
                break;
            }
        }
    }
    let cleaned = cleaned.split_whitespace().collect::<Vec<_>>().join(" ");
    let base = if cleaned.is_empty() {
        title.trim().to_string()
    } else {
        cleaned
    };
    if artist.trim().is_empty() || base.contains(artist.trim()) {
        base
    } else {
        format!("{base} {}", artist.trim())
    }
}
