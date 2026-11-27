use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};
use serde_json::{Map, Value};

/// Unified playable track representation across Bilibili videos, Bilibili audio (au), and local files.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct TrackItem {
    /// Stable unique identifier (`bvid:cid`, `au:<sid>`, or `local:<md5>`).
    pub id: String,
    pub title: String,
    pub artist: String,
    #[serde(default)]
    pub artist_mid: Option<u64>,
    #[serde(default)]
    pub cover: String,
    #[serde(default)]
    pub duration_secs: f64,
    pub source: TrackSource,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TrackSource {
    BiliVideo {
        bvid: String,
        #[serde(default)]
        aid: u64,
        #[serde(default)]
        cid: u64,
        #[serde(default)]
        part_title: Option<String>,
    },
    BiliAudio {
        sid: u64,
    },
    LocalFile {
        path: String,
        format: String,
    },
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "camelCase")]
pub enum PlayMode {
    #[default]
    Loop,
    Order,
    Single,
    Shuffle,
}

impl PlayMode {
    pub fn next(self) -> Self {
        match self {
            Self::Loop => Self::Order,
            Self::Order => Self::Single,
            Self::Single => Self::Shuffle,
            Self::Shuffle => Self::Loop,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Loop => "列表循环",
            Self::Order => "顺序播放",
            Self::Single => "单曲循环",
            Self::Shuffle => "随机播放",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub enum AudioQuality {
    #[default]
    Auto,
    Lossless,
    High,
    Medium,
    Low,
}

#[derive(Debug, Clone, Default, Serialize, PartialEq, Eq)]
pub struct ProxySettings {
    #[serde(rename = "type")]
    pub proxy_type: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub username: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub password: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct AppSettings {
    pub auto_start: bool,
    pub close_window_option: String,
    pub font_family: String,
    pub border_radius: u32,
    pub download_path: String,
    pub primary_color: String,
    pub background_color: String,
    pub audio_quality: AudioQuality,
    pub hidden_menu_keys: Vec<String>,
    pub display_mode: String,
    pub ffmpeg_path: String,
    pub theme_mode: String,
    pub page_transition: String,
    pub show_search_history: bool,
    pub proxy_settings: ProxySettings,
    pub side_menu_collapsed: bool,
    pub side_menu_width: u32,
    pub side_menu_collection_folded: CollectionFolded,
    pub report_play_history: bool,
    pub local_music_dirs: Vec<String>,
    pub lyrics_font_size: u32,
    pub show_top_lyrics: bool,
    pub show_top_lyrics_translation: bool,
    pub show_desktop_lyrics: bool,
    pub desktop_lyrics_locked: bool,
    pub desktop_lyrics_font_size: u32,
    pub desktop_lyrics_style: DesktopLyricsStyle,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for AppSettings {
    fn default() -> Self {
        Self {
            auto_start: false,
            close_window_option: "hide".into(),
            font_family: "system-ui".into(),
            border_radius: 8,
            download_path: dirs::download_dir()
                .map(|path| path.to_string_lossy().into_owned())
                .unwrap_or_default(),
            primary_color: "#1ed760".into(),
            background_color: String::new(),
            audio_quality: AudioQuality::Auto,
            hidden_menu_keys: Vec::new(),
            display_mode: "card".into(),
            ffmpeg_path: String::new(),
            theme_mode: "system".into(),
            page_transition: "none".into(),
            show_search_history: true,
            proxy_settings: ProxySettings {
                proxy_type: "none".into(),
                host: Some(String::new()),
                port: None,
                username: Some(String::new()),
                password: Some(String::new()),
            },
            side_menu_collapsed: false,
            side_menu_width: 200,
            side_menu_collection_folded: CollectionFolded::default(),
            report_play_history: true,
            local_music_dirs: Vec::new(),
            lyrics_font_size: 20,
            show_top_lyrics: true,
            show_top_lyrics_translation: true,
            show_desktop_lyrics: false,
            desktop_lyrics_locked: true,
            desktop_lyrics_font_size: 36,
            desktop_lyrics_style: DesktopLyricsStyle::default(),
            extra: Map::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct CollectionFolded {
    pub created: bool,
    pub collected: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopLyricsStyle {
    pub layout: String,
    pub align: String,
    pub show_translation: bool,
    pub show_romaji: bool,
    pub karaoke: bool,
    pub played_color: String,
    pub unplayed_color: String,
    pub stroke_color: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl Default for DesktopLyricsStyle {
    fn default() -> Self {
        Self {
            layout: "double".into(),
            align: "center".into(),
            show_translation: true,
            show_romaji: true,
            karaoke: true,
            played_color: "#1ed760".into(),
            unplayed_color: "#ffffff".into(),
            stroke_color: "#000000".into(),
            extra: Map::new(),
        }
    }
}

// Tauri settings are JSON from the web UI, where a setting can be null and
// numeric values can have a fractional part. Keep the defaults for invalid
// known values and leave unrecognized fields available for round-tripping.
struct SettingsFields(Map<String, Value>);

impl SettingsFields {
    fn deserialize<'de, D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let fields = match Value::deserialize(deserializer)? {
            Value::Object(fields) => fields,
            _ => Map::new(),
        };
        Ok(Self(fields))
    }

    fn read<T: DeserializeOwned>(&mut self, key: &str, field: &mut T) {
        if let Some(value) = self.0.remove(key).filter(|value| !value.is_null())
            && let Ok(value) = serde_json::from_value(value)
        {
            *field = value;
        }
    }

    fn unsigned<T: TryFrom<u32>>(&mut self, key: &str) -> Option<T> {
        let number = self.0.remove(key)?.as_f64()?;
        if (0.0..f64::from(u32::MAX) + 1.0).contains(&number) {
            T::try_from(number as u32).ok()
        } else {
            None
        }
    }

    fn read_unsigned(&mut self, key: &str, field: &mut u32) {
        if let Some(value) = self.unsigned(key) {
            *field = value;
        }
    }
}

impl<'de> Deserialize<'de> for AppSettings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = SettingsFields::deserialize(deserializer)?;
        let mut settings = Self::default();
        fields.read("autoStart", &mut settings.auto_start);
        fields.read("closeWindowOption", &mut settings.close_window_option);
        fields.read("fontFamily", &mut settings.font_family);
        fields.read_unsigned("borderRadius", &mut settings.border_radius);
        fields.read("downloadPath", &mut settings.download_path);
        fields.read("primaryColor", &mut settings.primary_color);
        fields.read("backgroundColor", &mut settings.background_color);
        fields.read("audioQuality", &mut settings.audio_quality);
        fields.read("hiddenMenuKeys", &mut settings.hidden_menu_keys);
        fields.read("displayMode", &mut settings.display_mode);
        fields.read("ffmpegPath", &mut settings.ffmpeg_path);
        fields.read("themeMode", &mut settings.theme_mode);
        fields.read("pageTransition", &mut settings.page_transition);
        fields.read("showSearchHistory", &mut settings.show_search_history);
        fields.read("proxySettings", &mut settings.proxy_settings);
        fields.read("sideMenuCollapsed", &mut settings.side_menu_collapsed);
        fields.read_unsigned("sideMenuWidth", &mut settings.side_menu_width);
        fields.read(
            "sideMenuCollectionFolded",
            &mut settings.side_menu_collection_folded,
        );
        fields.read("reportPlayHistory", &mut settings.report_play_history);
        fields.read("localMusicDirs", &mut settings.local_music_dirs);
        fields.read_unsigned("lyricsFontSize", &mut settings.lyrics_font_size);
        fields.read("showTopLyrics", &mut settings.show_top_lyrics);
        fields.read(
            "showTopLyricsTranslation",
            &mut settings.show_top_lyrics_translation,
        );
        fields.read("showDesktopLyrics", &mut settings.show_desktop_lyrics);
        fields.read("desktopLyricsLocked", &mut settings.desktop_lyrics_locked);
        fields.read_unsigned(
            "desktopLyricsFontSize",
            &mut settings.desktop_lyrics_font_size,
        );
        fields.read("desktopLyricsStyle", &mut settings.desktop_lyrics_style);
        settings.extra = fields.0;
        Ok(settings)
    }
}

impl<'de> Deserialize<'de> for DesktopLyricsStyle {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = SettingsFields::deserialize(deserializer)?;
        let mut style = Self::default();
        fields.read("layout", &mut style.layout);
        fields.read("align", &mut style.align);
        fields.read("showTranslation", &mut style.show_translation);
        fields.read("showRomaji", &mut style.show_romaji);
        fields.read("karaoke", &mut style.karaoke);
        fields.read("playedColor", &mut style.played_color);
        fields.read("unplayedColor", &mut style.unplayed_color);
        fields.read("strokeColor", &mut style.stroke_color);
        style.extra = fields.0;
        Ok(style)
    }
}

impl<'de> Deserialize<'de> for ProxySettings {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = SettingsFields::deserialize(deserializer)?;
        let mut proxy = Self::default();
        fields.read("type", &mut proxy.proxy_type);
        fields.read("host", &mut proxy.host);
        proxy.port = fields.unsigned("port");
        fields.read("username", &mut proxy.username);
        fields.read("password", &mut proxy.password);
        Ok(proxy)
    }
}

impl<'de> Deserialize<'de> for CollectionFolded {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let mut fields = SettingsFields::deserialize(deserializer)?;
        let mut folded = Self::default();
        fields.read("created", &mut folded.created);
        fields.read("collected", &mut folded.collected);
        Ok(folded)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ShortcutBinding {
    pub id: String,
    pub name: String,
    pub shortcut: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct WbiImages {
    pub img_url: String,
    pub sub_url: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct UserInfo {
    pub mid: u64,
    #[serde(rename = "uname", alias = "name", alias = "username")]
    pub name: String,
    pub face: String,
    pub sign: String,
    #[serde(rename = "isLogin")]
    pub is_login: bool,
    pub wbi_img: Option<WbiImages>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FavFolder {
    pub id: u64,
    pub fid: u64,
    pub mid: u64,
    pub title: String,
    pub intro: String,
    pub cover: String,
    pub media_count: u64,
    pub attr: u64,
    #[serde(rename = "type")]
    pub folder_type: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct MediaPage {
    pub cid: u64,
    pub page: u32,
    pub part: String,
    pub duration: f64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub total: Option<u64>,
    pub has_more: Option<bool>,
    pub next_offset: Option<String>,
}

#[derive(Debug, Clone)]
pub struct VideoInfo {
    pub track: TrackItem,
    pub pages: Vec<MediaPage>,
    pub description: String,
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub struct SearchResult<T = TrackItem> {
    pub items: Vec<T>,
    pub page: u64,
    pub page_size: u64,
    pub total: u64,
    pub pages: u64,
}

#[derive(Debug, Clone)]
pub struct HistoryItem {
    pub track: Option<TrackItem>,
    pub title: String,
    pub kid: String,
    pub view_at: u64,
    pub progress: i64,
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub struct DynamicItem {
    pub id: String,
    pub track: Option<TrackItem>,
    pub raw: Value,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct FollowingUser {
    pub mid: u64,
    #[serde(alias = "uname")]
    pub name: String,
    #[serde(alias = "upic")]
    pub face: String,
    #[serde(alias = "usign")]
    pub sign: String,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct BannerItem {
    #[serde(alias = "archive_title")]
    pub title: String,
    pub cover: String,
    pub jump_url: String,
    pub author: String,
    pub bvid: String,
    pub aid: u64,
    pub cid: u64,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

#[derive(Debug, Clone)]
pub struct RankItem {
    pub track: Option<TrackItem>,
    pub title: String,
    pub author: String,
    pub cover: String,
    pub score: f64,
    pub raw: Value,
}

#[derive(Debug, Clone)]
pub struct AudioStream {
    pub id: u64,
    pub codecs: String,
    pub bandwidth: u64,
    pub mime_type: String,
    pub urls: Vec<url::Url>,
}

#[derive(Debug, Clone)]
pub struct PlayUrlInfo {
    /// Ordered fallbacks for the selected representation.
    pub urls: Vec<url::Url>,
    /// Selected representation first, followed by lower preference representations.
    pub streams: Vec<AudioStream>,
    pub duration_secs: Option<f64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QrCodeSession {
    pub url: String,
    pub qrcode_key: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QrPollStatus {
    WaitingForScan,
    WaitingForConfirmation,
    Expired,
    LoggedIn { refresh_token: String },
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct LyricWord {
    pub start_secs: f64,
    pub duration_secs: f64,
    pub text: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct LyricLine {
    pub start_secs: f64,
    pub duration_secs: Option<f64>,
    pub text: String,
    pub translation: Option<String>,
    pub romaji: Option<String>,
    pub words: Vec<LyricWord>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
#[serde(default, rename_all = "camelCase")]
pub struct LyricsData {
    pub lines: Vec<LyricLine>,
    pub source: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum LyricsProvider {
    NetEase,
    QqMusic,
    Lrclib,
}

#[derive(Debug, Clone)]
pub struct LyricsCandidate {
    pub id: u64,
    pub title: String,
    pub artist: String,
    pub album: String,
    pub duration_secs: Option<f64>,
    pub provider: LyricsProvider,
    pub provider_key: Option<String>,
    pub aliases: Vec<String>,
}

#[derive(Debug, Clone)]
pub struct AutoLyricsResult {
    pub lyrics: LyricsData,
    pub candidate: LyricsCandidate,
    pub reference: Option<TrackItem>,
    pub offset_secs: f64,
    pub confidence: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct LocalMusicItem {
    pub id: String,
    pub path: String,
    pub dir: String,
    pub title: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub artist: String,
    pub size: u64,
    pub format: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_time: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DownloadStatus {
    #[serde(rename = "waiting", alias = "queued", alias = "pending")]
    Queued,
    Downloading,
    Merging,
    Converting,
    #[serde(
        rename = "downloadPaused",
        alias = "paused",
        alias = "mergePaused",
        alias = "convertPaused"
    )]
    Paused,
    #[serde(rename = "completed", alias = "done")]
    Done,
    Failed,
    #[serde(alias = "canceled")]
    Cancelled,
    #[serde(other)]
    Other,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DownloadTask {
    pub id: String,
    pub output_file_type: String,
    pub title: String,
    pub status: DownloadStatus,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bvid: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cid: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sid: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub cover: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub save_path: Option<String>,
    #[serde(default)]
    pub total_bytes: u64,
    #[serde(default)]
    pub downloaded_bytes: u64,
    #[serde(default)]
    pub download_progress: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub created_time: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(flatten)]
    pub extra: Map<String, Value>,
}

impl DownloadTask {
    pub fn progress(&self) -> f64 {
        f64::from(self.download_progress.min(100)) / 100.0
    }
}
