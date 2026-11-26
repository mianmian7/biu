use serde::{Deserialize, Serialize};

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
