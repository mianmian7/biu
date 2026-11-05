import { addToast } from "@heroui/react";
import { shuffle } from "es-toolkit/array";
import { remove } from "es-toolkit/array";
import { uniqueId } from "es-toolkit/compat";
import { create } from "zustand";
import { persist } from "zustand/middleware";
import { immer } from "zustand/middleware/immer";

import { getPlayModeList, PlayMode } from "@/common/constants/audio";
import { getAudioUrl, getDashUrl, isUrlValid } from "@/common/utils/audio";
import log from "@/common/utils/logger";
import { beginPlayReport, endPlayReport, reportHeartbeat } from "@/common/utils/play-report";
import {
  anchorNow,
  getAnchorPositionMs,
  markPlaybackAnchor,
  resetPlaybackAnchor,
  usePlaybackAnchor,
} from "@/common/utils/playback-anchor";
import { stripHtml } from "@/common/utils/str";
import { formatUrlProtocol } from "@/common/utils/url";
import { isSubWindow } from "@/common/utils/window-role";
import { invokeTauri, toMediaUrl } from "@/platform";
import { getAudioSongInfo } from "@/service/audio-song-info";
import { getWebInterfaceView } from "@/service/web-interface-view";

import { usePlayProgress } from "./play-progress";

export type PlayDataType = "mv" | "audio";

export interface PlayData {
  id: string;
  /** 视频标题 */
  title: string;
  /** 类型 */
  type: PlayDataType;
  /** 视频id */
  bvid?: string;
  /** 音频id */
  sid?: number;
  /** 视频aid,部分视频操作需要，例如收藏 */
  aid?: string;
  /** 视频分集id */
  cid?: string;
  /** 视频封面 */
  cover?: string;
  /** UP name */
  ownerName?: string;
  /** up mid */
  ownerMid?: number;
  /** 是否为多集视频 */
  hasMultiPart?: boolean;
  /** 分集标题 */
  pageTitle?: string;
  /** 分集封面 */
  pageCover?: string;
  /** 分集id */
  pageIndex?: number;
  /** 视频总分集数 */
  totalPage?: number;
  /** 视频时长 单位为秒 */
  duration?: number;
  /** 视频音频url */
  audioUrl?: string;
  /** 视频url */
  videoUrl?: string;
  /** 是否为无损音频 */
  isLossless?: boolean;
  /** 是否为杜比音频 */
  isDolby?: boolean;
  /** 来源 */
  source?: "local" | "online";
}

interface State {
  // 播放/暂停
  isPlaying: boolean;
  // 静音
  isMuted: boolean;
  // 音量 0-1
  volume: number;
  // 播放模式
  playMode: PlayMode;
  // 播放速率（0.5x - 2.0x）
  rate: number;
  // 总时长（秒）
  duration: number | undefined;
  /** 播放队列 */
  list: PlayData[];
  /** 当前播放视频id */
  playId?: string;
  /** 下一个播放视频id */
  nextId?: string;
  /** 是否在随机播放模式下保持视频分集顺序 */
  shouldKeepPagesOrderInRandomPlayMode: boolean;
}

export interface PlayItem {
  type: PlayDataType;
  id?: string;
  source?: "local" | "online";
  audioUrl?: string;
  title: string;
  bvid?: string;
  sid?: number;
  cover?: string;
  ownerName?: string;
  ownerMid?: number;
}

interface Action {
  togglePlay: () => void;
  toggleMute: () => void;
  setVolume: (volume: number) => void; // 0-1
  togglePlayMode: () => void;
  setRate: (rate: number) => void; // 0.5-2.0
  seek: (s: number) => void;
  setShouldKeepPagesOrderInRandomPlayMode: (shouldKeep: boolean) => void;

  init: VoidFunction;
  play: (params: PlayItem) => Promise<void>;
  playListItem: (id: string) => Promise<void>;
  playList: (items: PlayItem[]) => Promise<void>;
  addToNext: (item: PlayItem) => void;
  addList: (items: PlayItem[]) => void;
  delPage: (id: string) => void;
  del: (id: string) => void;
  clear: () => void;
  next: () => Promise<void>;
  prev: () => Promise<void>;

  getAudio: () => HTMLAudioElement;
  getPlayItem: () => PlayData | undefined;
}

const idGenerator = () => `${Date.now()}${uniqueId()}`;

const getMVData = async (bvid: string) => {
  const res = await getWebInterfaceView({ bvid });
  const hasMultiPart = (res?.data?.pages?.length ?? 0) > 1;

  return (
    res?.data?.pages?.map(item => ({
      id: idGenerator(),
      type: "mv" as PlayDataType,
      bvid,
      aid: String(res?.data?.aid),
      cid: String(item.cid),
      title: res?.data?.title,
      cover: formatUrlProtocol(res?.data?.pic),
      ownerName: res?.data?.owner?.name,
      ownerMid: res?.data?.owner?.mid,
      hasMultiPart,

      pageIndex: item.page,
      pageTitle: hasMultiPart ? item.part : res?.data?.title,
      pageCover: hasMultiPart
        ? formatUrlProtocol(item.first_frame || res?.data?.pic)
        : formatUrlProtocol(res?.data?.pic),
      totalPage: res?.data?.pages?.length,
      duration: item.duration,
    })) || []
  );
};

const getAudioData = async (sid: number) => {
  const res = await getAudioSongInfo({ sid });

  return [
    {
      id: idGenerator(),
      type: "audio" as PlayDataType,
      sid,
      title: res?.data?.title || "",
      cover: formatUrlProtocol(res?.data?.cover || ""),
      duration: res?.data?.duration || 0,
      ownerName: res?.data?.author || "",
      ownerMid: res?.data?.uid || 0,
    },
  ];
};

const toastError = (title: string) => {
  addToast({
    title,
    color: "danger",
  });
};

const sanitizeTitle = (title: string) => stripHtml(title);

let forceSilentMode = false;

const syncSilentMode = () => {
  void invokeTauri<boolean>("is_silent_mode")
    .then(silent => {
      if (silent) {
        forceSilentMode = true;
        if (audio) {
          audio.muted = true;
        }
      }
    })
    .catch(() => {});
};

const handlePlayError = (error: any) => {
  const errorMsg = error?.message || error?.name || String(error || "");
  void invokeTauri("frontend_log", {
    level: "warn",
    message: `[handlePlayError] ${errorMsg}`,
  }).catch(() => {});
  // A newer load cancelled this play(): Chromium says "interrupted", WebKit throws AbortError.
  const superseded = error?.name === "AbortError" || errorMsg.includes("interrupted");
  if (!superseded && !errorMsg.includes("NotAllowed")) {
    toastError(error instanceof Error ? error.message : "获取播放链接失败");
  }
};

const updateMediaSession = ({ title, artist, cover }: { title: string; artist?: string; cover?: string }) => {
  if ("mediaSession" in navigator) {
    navigator.mediaSession.metadata = new MediaMetadata({
      title,
      artist,
      artwork: cover ? [{ src: formatUrlProtocol(cover)! }] : [],
    });
  }
};

const createAudio = (): HTMLAudioElement => {
  const audio = new Audio();
  audio.preload = "metadata";
  audio.controls = false;
  audio.crossOrigin = "anonymous";
  audio.onvolumechange = () => {
    if (forceSilentMode && !audio.muted) {
      audio.muted = true;
    }
  };
  return audio;
};

export const audio = createAudio();
syncSilentMode();

const updatePlaybackState = () => {
  if ("mediaSession" in navigator) {
    navigator.mediaSession.playbackState = audio.paused ? "paused" : "playing";
  }

  if (window.electron && window.electron.updatePlaybackState) {
    window.electron.updatePlaybackState(!audio.paused);
  }
};

const playAudioSafely = async () => {
  // Only the main window plays; sub windows load the same store but must stay silent.
  if (isSubWindow()) {
    return;
  }
  if (forceSilentMode) {
    audio.muted = true;
  }
  try {
    await audio.play();
  } catch (error) {
    if ((error as DOMException)?.name === "NotSupportedError") {
      const refreshed = await refreshCurrentAudioSource();
      if (refreshed) {
        try {
          await audio.play();
          return;
        } catch (retryError) {
          handlePlayError(retryError);
          return;
        }
      }
      return;
    }
    handlePlayError(error);
  }
};

/** 外推位置与真实播放位置相差超过该值时重新记录锚点 */
export const ANCHOR_DRIFT_LIMIT_MS = 120;
const MAX_DRIFT_LOGS_PER_TRACK = 20;
let driftLogCount = 0;

/**
 * 锚点只在媒体事件时记录，外推时钟会和音频时钟慢慢分开。
 * timeupdate 时对比两者，超过阈值就重新记录，桌面歌词的进度和换句才能对齐真实播放位置。
 */
const correctAnchorDrift = (media: HTMLMediaElement) => {
  const anchor = usePlaybackAnchor.getState();
  if (!anchor.running || media.paused) return;
  const drift = getAnchorPositionMs(anchor, 0, anchorNow()) - media.currentTime * 1000;
  if (Math.abs(drift) <= ANCHOR_DRIFT_LIMIT_MS) return;
  markPlaybackAnchor(media, true);
  if (driftLogCount < MAX_DRIFT_LOGS_PER_TRACK) {
    driftLogCount += 1;
    void invokeTauri("frontend_log", {
      level: "info",
      message: `[anchor] re-anchored after ${Math.round(drift)}ms drift at ${Math.round(media.currentTime * 1000)}ms`,
    }).catch(() => {});
  }
};

const updatePositionState = () => {
  if ("mediaSession" in navigator) {
    const dur = audio.duration;
    if (!Number.isNaN(dur) && dur !== Infinity) {
      navigator.mediaSession.setPositionState({
        duration: dur,
        playbackRate: audio.playbackRate,
        position: audio.currentTime,
      });
    }
  }
};

export const isSame = (
  item1?: { type: "mv" | "audio"; sid?: number; bvid?: string; source?: "local" | "online"; id?: string },
  item2?: { type: "mv" | "audio"; sid?: number; bvid?: string; source?: "local" | "online"; id?: string },
) => {
  if (!item1 || !item2) {
    return false;
  }
  if (item1.source === "local" || item2.source === "local") {
    return Boolean(item1.id) && Boolean(item2.id) && item1.id === item2.id;
  }
  if (item1.type !== item2.type) {
    return false;
  }
  if (item1.type === "mv") {
    return Boolean(item1.bvid) && Boolean(item2.bvid) && item1.bvid === item2.bvid;
  }
  if (item1.type === "audio") {
    return item1.sid !== undefined && item2.sid !== undefined && item1.sid === item2.sid;
  }
  return false;
};

const shouldReportPlayRecord = (item?: { type: PlayDataType; source?: "local" | "online" }) =>
  item?.type === "mv" && item?.source !== "local";

export const usePlayList = create<State & Action>()(
  persist(
    immer((set, get) => {
      const ensureAudioSrcValid = async () => {
        const { playId, list } = get();
        const currentPlayItem = list.find(item => item.id === playId);
        if (currentPlayItem?.source === "local" && currentPlayItem?.audioUrl) {
          const mediaUrl = await toMediaUrl(currentPlayItem.audioUrl);
          if (audio.src !== mediaUrl) {
            audio.src = mediaUrl;
          }
          const currentTime = usePlayProgress.getState().currentTime;
          if (typeof currentTime === "number" && currentTime > 0) {
            audio.currentTime = currentTime;
          }
          return;
        }
        if (isUrlValid(currentPlayItem?.audioUrl)) {
          const mediaUrl = await toMediaUrl(currentPlayItem.audioUrl);
          if (audio.src !== mediaUrl) {
            audio.src = mediaUrl;
          }
          const currentTime = usePlayProgress.getState().currentTime;
          if (typeof currentTime === "number" && currentTime > 0) {
            audio.currentTime = currentTime;
          }
          return;
        }

        if (currentPlayItem?.type === "mv" && currentPlayItem?.bvid && currentPlayItem?.cid) {
          const mvPlayData = await getDashUrl(currentPlayItem.bvid, currentPlayItem.cid);
          if (mvPlayData?.audioUrl) {
            if (audio.src !== mvPlayData.audioUrl) {
              audio.src = mvPlayData.audioUrl;
              const currentTime = usePlayProgress.getState().currentTime;
              if (typeof currentTime === "number") {
                audio.currentTime = currentTime;
              }
            }
            set(state => {
              const listItem = state.list.find(item => item.id === state.playId);
              if (listItem) {
                listItem.audioUrl = mvPlayData.audioUrl;
                listItem.videoUrl = mvPlayData.videoUrl;
                listItem.isLossless = mvPlayData.isLossless;
                listItem.isDolby = mvPlayData.isDolby;
              }
            });
          } else {
            log.error("无法获取音频播放链接", {
              type: "mv",
              bvid: currentPlayItem.bvid,
              cid: currentPlayItem.cid,
              title: currentPlayItem.title,
              mvPlayData,
            });
            toastError("无法获取音频播放链接");
          }
        }

        if (currentPlayItem?.type === "audio" && currentPlayItem?.sid) {
          const musicPlayData = await getAudioUrl(currentPlayItem.sid);
          if (musicPlayData?.audioUrl) {
            if (audio.src !== musicPlayData.audioUrl) {
              audio.src = musicPlayData.audioUrl;
              const currentTime = usePlayProgress.getState().currentTime;
              if (typeof currentTime === "number") {
                audio.currentTime = currentTime;
              }
            }
            set(state => {
              const listItem = state.list.find(item => item.id === state.playId);
              if (listItem) {
                listItem.audioUrl = musicPlayData.audioUrl;
                listItem.isLossless = musicPlayData.isLossless;
              }
            });
          } else {
            log.error("无法获取音频播放链接", {
              type: "audio",
              sid: currentPlayItem.sid,
              title: currentPlayItem.title,
              musicPlayData,
            });
            toastError("无法获取音频播放链接");
          }
        }
      };

      return {
        isPlaying: false,
        isMuted: false,
        volume: 0.5,
        playMode: PlayMode.Loop,
        rate: 1,
        duration: undefined,
        shouldKeepPagesOrderInRandomPlayMode: true,
        list: [],
        init: async () => {
          syncSilentMode();
          if (audio) {
            audio.volume = get().volume;
            audio.muted = forceSilentMode ? true : get().isMuted;
            audio.playbackRate = get().rate;
            audio.loop = get().playMode === PlayMode.Single;

            audio.ondurationchange = () => {
              const dur = audio.duration;
              if (!Number.isNaN(dur) && dur !== Infinity) {
                set({ duration: Math.round(dur * 100) / 100 });
                updatePositionState();
              }
            };

            audio.ontimeupdate = () => {
              correctAnchorDrift(audio);
              const currentTime = Math.round(audio.currentTime * 100) / 100;
              usePlayProgress.getState().setCurrentTime(currentTime);
              const playItem = get().getPlayItem?.();
              if (shouldReportPlayRecord(playItem)) {
                void reportHeartbeat(playItem, currentTime, audio.duration, 0);
              }
            };

            audio.onplaying = () => {
              markPlaybackAnchor(audio, true);
              markSwitchPlaying();
            };
            audio.onwaiting = () => markPlaybackAnchor(audio, false);
            audio.onseeking = () => markPlaybackAnchor(audio, false);
            audio.onloadstart = () => {
              driftLogCount = 0;
              resetPlaybackAnchor();
            };

            audio.onseeked = () => {
              updatePositionState();
              markPlaybackAnchor(audio, !audio.paused && audio.readyState >= 3);
            };

            audio.onratechange = () => {
              updatePositionState();
              markPlaybackAnchor(audio, usePlaybackAnchor.getState().running);
            };

            audio.onplay = () => {
              set({ isPlaying: true });
              updatePlaybackState();
              updatePositionState();
              const playItem = get().getPlayItem?.();
              if (shouldReportPlayRecord(playItem)) {
                void reportHeartbeat(playItem, audio.currentTime, audio.duration, 1);
              }
            };

            audio.onpause = () => {
              markPlaybackAnchor(audio, false);
              set({ isPlaying: false });
              updatePlaybackState();
              updatePositionState();
              const playItem = get().getPlayItem?.();
              if (shouldReportPlayRecord(playItem)) {
                void reportHeartbeat(playItem, audio.currentTime, audio.duration, 2);
              }
            };

            audio.onended = () => {
              if (get().playMode === PlayMode.Single) {
                return;
              }

              const playItem = get().getPlayItem?.();
              if (shouldReportPlayRecord(playItem)) {
                void reportHeartbeat(playItem, audio.duration, audio.duration, 4);
                endPlayReport();
              }

              const currentIndex = get().list.findIndex(item => item.id === get().playId);
              if (get().playMode === PlayMode.Sequence && currentIndex === get().list.length - 1) {
                audio.currentTime = 0;
                audio.pause();
                return;
              }

              get().next();
            };

            if ("mediaSession" in navigator) {
              navigator.mediaSession.setActionHandler("play", () => get().togglePlay());
              navigator.mediaSession.setActionHandler("pause", () => get().togglePlay());
              navigator.mediaSession.setActionHandler("previoustrack", () => get().prev());
              navigator.mediaSession.setActionHandler("nexttrack", () => {
                if (get().list.length > 1) {
                  get().next();
                }
              });
              navigator.mediaSession.setActionHandler("seekto", details => {
                if (details.seekTime) get().seek(Math.round(details.seekTime * 100) / 100);
                updatePositionState();
              });
              navigator.mediaSession.setActionHandler("seekbackward", details => {
                const offset = details?.seekOffset || 10;
                get().seek(Math.round((audio.currentTime - offset) * 100) / 100);
              });
              navigator.mediaSession.setActionHandler("seekforward", details => {
                const offset = details?.seekOffset || 10;
                get().seek(Math.round((audio.currentTime + offset) * 100) / 100);
              });
            }

            if (get().playId) {
              const playItem = get().list.find(item => item.id === get().playId);
              if (playItem) {
                await ensureAudioSrcValid();

                const localCurrentTime = usePlayProgress.getState().initCurrentTime();
                if (localCurrentTime) {
                  audio.currentTime = localCurrentTime;
                }

                updateMediaSession({
                  title: playItem.title,
                  artist: playItem.ownerName,
                  cover: playItem.pageCover,
                });
              }
            }
          }
        },
        toggleMute: () => {
          if (audio) {
            audio.muted = forceSilentMode ? true : !audio.muted;
          }
          set(s => ({ isMuted: !s.isMuted }));
        },
        setVolume: volume => {
          if (audio) {
            audio.volume = volume;
          }
          set(state => {
            state.volume = volume;
          });
        },
        togglePlayMode: () => {
          const playModeList = getPlayModeList();
          const currentIndex = playModeList.findIndex(item => item.value === get().playMode);
          const nextIndex = (currentIndex + 1) % playModeList.length;
          const nextPlayMode = playModeList[nextIndex].value;

          if (audio) {
            audio.loop = nextPlayMode === PlayMode.Single;
          }
          set(state => {
            state.playMode = nextPlayMode;
          });
        },
        setRate: rate => {
          if (audio) {
            audio.playbackRate = rate;
          }
          set(state => {
            state.rate = rate;
          });
        },
        seek: s => {
          // 先移动音频和锚点再更新进度，桌面歌词不会收到“新行号 + 旧锚点”的内容
          if (audio) {
            audio.currentTime = s;
            markPlaybackAnchor(audio, false);
          }
          usePlayProgress.getState().setCurrentTime(s);
        },
        togglePlay: async () => {
          if (!get().list?.length) {
            return;
          }

          if (!get().playId) {
            return;
          }

          if (audio.paused) {
            set(state => {
              state.isPlaying = true;
            });
            await ensureAudioSrcValid();
            await playAudioSafely();
          } else {
            audio.pause();
            set(state => {
              state.isPlaying = false;
            });
          }
        },
        setShouldKeepPagesOrderInRandomPlayMode: shouldKeep => {
          set({ shouldKeepPagesOrderInRandomPlayMode: shouldKeep });
        },
        play: async ({ type, bvid, sid, title, cover, ownerName, ownerMid, id, source, audioUrl }: PlayItem) => {
          const { list, playId } = get();
          const currentItem = list?.find(item => item.id === playId);
          const sanitizedTitle = sanitizeTitle(title);
          const candidate = { type, bvid, sid, source, id };

          // 当前正在播放，如果暂停了则播放
          if (isSame(currentItem, candidate)) {
            if (audio.paused) {
              await ensureAudioSrcValid();
              await playAudioSafely();
            }
            return;
          }

          // 列表已存在
          const existItem = list?.find(item => isSame(item, candidate));
          if (existItem) {
            // The playId subscription loads and plays the item. Loading it here too raced that
            // subscription for the same audio element and aborted one of the play() calls.
            set({ playId: existItem.id });
            return;
          }

          const isLocal = source === "local";
          // 新添加项
          let playItem: PlayData[] =
            isLocal && id
              ? [
                  {
                    id,
                    type,
                    source,
                    audioUrl,
                    title: sanitizedTitle,
                  },
                ]
              : [
                  {
                    id: idGenerator(),
                    type,
                    bvid,
                    sid,
                    title: sanitizedTitle,
                    cover: cover ? formatUrlProtocol(cover) : undefined,
                    ownerName,
                    ownerMid,
                  },
                ];
          // 补充缺失信息
          if (!isLocal && (!cover || !ownerName || !ownerMid)) {
            if (type === "mv" && bvid) {
              playItem = await getMVData(bvid);
            }

            if (type === "audio" && sid) {
              playItem = await getAudioData(sid);
            }
          }

          const nextPlayItem = playItem[0];
          if (!nextPlayItem) {
            toastError("播放失败：无法获取播放信息");
            return;
          }

          set(state => {
            state.list = [...state.list, ...playItem];
            state.playId = nextPlayItem.id;
          });
        },
        playListItem: async (id: string) => {
          if (get().playId === id) {
            return;
          }

          set(state => {
            state.playId = id;
            if (state.nextId === id) {
              state.nextId = undefined;
            }
          });
        },
        playList: async items => {
          const newList = items.map(item => ({
            ...item,
            title: sanitizeTitle(item.title),
            id: item.source === "local" && item.id ? item.id : idGenerator(),
          }));

          set(state => {
            state.list = newList;
            state.playId = newList[0].id;
          });
        },
        next: async () => {
          const { playMode, list, playId, nextId, shouldKeepPagesOrderInRandomPlayMode } = get();

          if (!list?.length) {
            return;
          }

          if (!playId) {
            return;
          }

          if (nextId) {
            set(state => {
              state.playId = nextId;
              state.nextId = undefined;
            });
            return;
          }

          const currentIndex = list.findIndex(item => item.id === playId);
          const nextIndex = (currentIndex + 1) % list.length;
          switch (playMode) {
            case PlayMode.Sequence:
            case PlayMode.Single:
            case PlayMode.Loop: {
              if (list.length === 1) {
                audio.currentTime = 0;
                await playAudioSafely();
                break;
              }

              set(state => {
                state.playId = list[nextIndex].id;
              });
              break;
            }
            case PlayMode.Random: {
              const currentPlayItem = list[currentIndex];

              if (list.length === 1) {
                audio.currentTime = 0;
                await playAudioSafely();
                break;
              }

              // 保持分集顺序，且当前为分集视频，且不是最后一集
              if (
                shouldKeepPagesOrderInRandomPlayMode &&
                currentPlayItem.pageIndex &&
                currentPlayItem.pageIndex !== currentPlayItem.totalPage
              ) {
                const nextPage = list.find(
                  item => item.bvid === currentPlayItem.bvid && item.pageIndex === currentPlayItem.pageIndex! + 1,
                );
                if (nextPage) {
                  set({ playId: nextPage.id });
                  break;
                }
              }

              const shuffledList = shuffle(list?.map(item => item.id));
              const currentIndexInShuffled = shuffledList.findIndex(shuffled => shuffled === playId);
              const nextShuffledIndex = (currentIndexInShuffled + 1) % shuffledList.length;
              set(state => {
                state.playId = shuffledList[nextShuffledIndex];
              });
              break;
            }
          }
        },
        prev: async () => {
          const { playId, list } = get();

          if (!list?.length) {
            return;
          }

          if (!playId) {
            return;
          }

          const currentIndex = list.findIndex(item => item.id === playId);
          if (currentIndex === -1) return;

          const prevIndex = (currentIndex - 1 + list.length) % list.length;

          set(state => {
            state.playId = list[prevIndex].id;
          });
        },
        addToNext: async ({ type, title, bvid, sid, cover, ownerName, ownerMid, id, source, audioUrl }) => {
          const { playId, nextId: currentNextId, list } = get();
          const currentItem = list.find(item => item.id === playId);
          const sanitizedTitle = sanitizeTitle(title);
          const candidate = { type, bvid, sid, source, id };
          // 如果当前正在播放，则不添加
          if (isSame(candidate, currentItem)) {
            return;
          }

          // 如果下一首就是要添加的，则不添加
          if (currentNextId) {
            const currentNextItem = list.find(item => item.id === currentNextId);
            if (isSame(candidate, currentNextItem)) {
              return;
            }
          }

          // 列表已存在
          const existItemIndex = list?.findIndex(item => isSame(item, candidate)) ?? -1;
          if (existItemIndex !== -1) {
            set(state => {
              state.nextId = list[existItemIndex].id;
              // 将已存在项移动到下一首
              const currentItemIndex = list.findIndex(item => item.id === playId);
              if (currentItemIndex !== existItemIndex - 1) {
                state.list.splice(existItemIndex, 1);
                state.list.splice(currentItemIndex, 0, list[existItemIndex]);
              }
            });
            return;
          }

          let nextPlayItem: PlayData[] =
            source === "local" && id
              ? [
                  {
                    id,
                    type,
                    bvid,
                    sid,
                    source,
                    audioUrl,
                    title: sanitizedTitle,
                    cover: cover ? formatUrlProtocol(cover) : undefined,
                    ownerName,
                    ownerMid,
                  },
                ]
              : [
                  {
                    id: idGenerator(),
                    type,
                    bvid,
                    sid,
                    title: sanitizedTitle,
                    cover: cover ? formatUrlProtocol(cover) : undefined,
                    ownerName,
                    ownerMid,
                  },
                ];
          if (source !== "local" && (!cover || !ownerName || !ownerMid)) {
            if (type === "mv" && bvid) {
              nextPlayItem = await getMVData(bvid);
            }

            if (type === "audio" && sid) {
              nextPlayItem = await getAudioData(sid);
            }
          }

          if (!nextPlayItem || nextPlayItem.length === 0) {
            toastError("添加失败：无法获取播放信息");
            return;
          }

          const nextId = nextPlayItem[0].id;
          // 空列表，直接播放
          if (list.length === 0) {
            set({
              playId: nextId,
              list: nextPlayItem,
            });
            return;
          }

          // 当前播放的是音频，则直接插入到其后面
          if (currentItem?.type === "audio") {
            set(state => {
              state.nextId = nextId;
              const currentItemIndex = list.findIndex(item => item.id === state.playId);
              state.list.splice(currentItemIndex + 1, 0, ...nextPlayItem);
            });
          }

          // 当前播放的是视频，找到最后一个分集的索引，插入到其后面
          if (currentItem?.type === "mv") {
            const currentMVLastPageIndex = list.findLastIndex(item =>
              isSame(item, { type: "mv", bvid: currentItem.bvid }),
            );
            set(state => {
              state.nextId = nextId;
              state.list.splice(currentMVLastPageIndex + 1, 0, ...nextPlayItem);
            });
          }
        },
        addList: async items => {
          const { list, playId } = get();
          if (list.length === 0) {
            get().playList(items);
            return;
          }

          const currentItem = list.find(item => item.id === playId);

          const paddingItems = items
            .filter(item => {
              if (currentItem && isSame(item, currentItem)) {
                return false;
              }
              return !list.some(existing => isSame(existing, item));
            })
            .map(item => ({
              ...item,
              title: sanitizeTitle(item.title),
              id: item.source === "local" && item.id ? item.id : idGenerator(),
            }));

          if (paddingItems.length === 0) {
            return;
          }

          set({
            list: [...list, ...paddingItems],
          });
        },
        delPage: async id => {
          if (get().list.length === 1) {
            get().clear();
            return;
          }

          if (id === get().playId) {
            try {
              await get().next();
            } catch (error) {
              handlePlayError(error);
            }
          }

          set(state => {
            const removeIndex = state.list.findIndex(item => item.id === id);
            if (removeIndex !== -1) {
              state.list.splice(removeIndex, 1);
            }
          });
        },
        del: async id => {
          if (get().list.length === 1) {
            get().clear();
            return;
          }

          const { playId, list } = get();
          const playItem = list.find(item => item.id === playId);
          const removedItem = list.find(item => item.id === id);

          if (isSame(playItem, removedItem)) {
            if (removedItem?.type === "audio") {
              try {
                await get().next();
              } catch (error) {
                handlePlayError(error);
              }
            } else {
              if (list.some(item => !isSame(item, removedItem))) {
                const lastIndex = list.findLastIndex(item => isSame(item, removedItem));
                if (lastIndex !== -1) {
                  const nextPlayIndex = (lastIndex + 1) % list.length;
                  set(state => {
                    state.playId = state.list[nextPlayIndex].id;
                  });
                }
              } else {
                get().clear();
                return;
              }
            }
          }

          set(state => {
            remove(state.list, item => isSame(item, removedItem));
          });
        },
        clear: () => {
          const currentPlayItem = get().getPlayItem?.();
          if (shouldReportPlayRecord(currentPlayItem)) {
            endPlayReport();
          }
          if (audio) {
            audio.src = "";
            if (!audio.paused) {
              audio.pause();
            }
          }
          set(state => {
            state.isPlaying = false;
            state.duration = undefined;
            state.list = [];
            state.playId = undefined;
            state.nextId = undefined;
          });
          usePlayProgress.getState().setCurrentTime(0);
        },
        getPlayItem: () => {
          const { playId, list } = get();
          const playItem = list.find(item => item.id === playId);
          return playItem;
        },
        getAudio: () => audio,
      };
    }),
    {
      name: "play-list-store",
      partialize: state => ({
        isMuted: state.isMuted,
        volume: state.volume,
        playMode: state.playMode,
        rate: state.rate,
        duration: state.duration,
        list: state.list,
        playId: state.playId,
        nextId: state.nextId,
        shouldKeepPagesOrderInRandomPlayMode: state.shouldKeepPagesOrderInRandomPlayMode,
      }),
    },
  ),
);

async function refreshCurrentAudioSource(): Promise<boolean> {
  const { getPlayItem } = usePlayList.getState?.() ?? {};
  const playItem = getPlayItem?.();

  if (!playItem) {
    return false;
  }

  try {
    if (playItem.type === "mv" && playItem.bvid && playItem.cid) {
      const mvPlayData = await getDashUrl(playItem.bvid, playItem.cid);
      if (mvPlayData?.audioUrl) {
        audio.src = mvPlayData.audioUrl;
        usePlayList.setState(state => {
          const listItem = state.list.find(item => item.id === state.playId);
          if (listItem) {
            listItem.audioUrl = mvPlayData.audioUrl;
            listItem.videoUrl = mvPlayData.videoUrl;
            listItem.isLossless = mvPlayData.isLossless;
            listItem.isDolby = mvPlayData.isDolby;
          }
        });
        return true;
      }
    }

    if (playItem.type === "audio" && playItem.sid) {
      const musicPlayData = await getAudioUrl(playItem.sid);
      if (musicPlayData?.audioUrl) {
        audio.src = musicPlayData.audioUrl;
        usePlayList.setState(state => {
          const listItem = state.list.find(item => item.id === state.playId);
          if (listItem) {
            listItem.audioUrl = musicPlayData.audioUrl;
            listItem.isLossless = musicPlayData.isLossless;
          }
        });
        return true;
      }
    }
  } catch (refreshError) {
    log.error("刷新播放链接失败", {
      playItem,
      refreshError,
    });
    handlePlayError(refreshError);
  }

  return false;
}

/** 切歌计时：从切歌开始到拿到播放链接、到开始出声各用了多久，写入日志便于排查切歌慢 */
let switchTiming: { startedAt: number; urlAt?: number } | undefined;
/** 补全信息后替换当前条目会再触发一次切歌，这次切歌沿用原来的计时 */
let continuingSwitch = false;

const logSwitchTiming = (message: string) =>
  void invokeTauri("frontend_log", { level: "info", message: `[switch] ${message}` }).catch(() => {});

function markSwitchPlaying() {
  if (!switchTiming) return;
  const { startedAt, urlAt } = switchTiming;
  switchTiming = undefined;
  const now = performance.now();
  const urlMs = urlAt === undefined ? "?" : Math.round(urlAt - startedAt);
  const loadMs = urlAt === undefined ? "?" : Math.round(now - urlAt);
  const item = usePlayList.getState().getPlayItem?.();
  const quality = item?.isLossless ? "lossless" : item?.isDolby ? "dolby" : "standard";
  logSwitchTiming(`playing after ${Math.round(now - startedAt)}ms (url ${urlMs}ms, load ${loadMs}ms, ${quality})`);
}

function resetAudioAndPlay(url: string) {
  if (switchTiming) switchTiming.urlAt = performance.now();
  audio.src = url;
  audio.currentTime = 0;
  audio.load();
  void playAudioSafely();
}

let playSwitchId = 0;

// 切换歌曲时，更新当前播放的歌曲信息
usePlayList.subscribe(async (state, prevState) => {
  if (state.playId !== prevState.playId) {
    // Each switch owns the audio element until a newer switch starts. After every await,
    // a stale switch must stop, or it would load the old track and write its URL into the new item.
    const switchId = ++playSwitchId;
    const isStale = () => switchId !== playSwitchId;
    if (!continuingSwitch || !switchTiming) {
      switchTiming = state.playId ? { startedAt: performance.now() } : undefined;
    }
    // 新歌的时长加载出来之前不显示上一首的时长
    if (state.duration !== undefined) usePlayList.setState({ duration: undefined });
    if (!state.playId) {
      const prevPlayItem = prevState.list.find(item => item.id === prevState.playId);
      if (shouldReportPlayRecord(prevPlayItem)) {
        endPlayReport();
      }
    }

    if (audio && !audio.paused) {
      audio.pause();
      audio.currentTime = 0;
    }
    // 切换歌曲
    if (state.playId) {
      const playItem = state.list.find(item => item.id === state.playId);
      if (playItem) {
        if (shouldReportPlayRecord(playItem)) {
          void beginPlayReport(playItem);
        }
      }
      if (playItem?.source === "local" && playItem?.audioUrl && audio.paused) {
        const mediaUrl = await toMediaUrl(playItem.audioUrl);
        if (!isStale()) resetAudioAndPlay(mediaUrl);
        return;
      }
      if (isUrlValid(playItem?.audioUrl) && audio.paused) {
        const mediaUrl = await toMediaUrl(playItem.audioUrl);
        if (!isStale()) resetAudioAndPlay(mediaUrl);
        return;
      }

      if (playItem?.type === "mv") {
        if (playItem?.bvid && playItem?.cid) {
          const mvPlayData = await getDashUrl(playItem.bvid, playItem.cid);
          if (isStale()) return;
          if (mvPlayData?.audioUrl) {
            resetAudioAndPlay(mvPlayData?.audioUrl);

            updateMediaSession({
              title: playItem.pageTitle || playItem.title,
              artist: playItem.ownerName,
              cover: playItem.pageCover,
            });

            usePlayList.setState(state => {
              const listItem = state.list.find(item => item.id === state.playId);
              if (listItem) {
                listItem.audioUrl = mvPlayData?.audioUrl;
                listItem.videoUrl = mvPlayData?.videoUrl;
                listItem.isLossless = mvPlayData?.isLossless;
                listItem.isDolby = mvPlayData?.isDolby;
              }
            });
          } else {
            log.error("无法获取音频播放链接", {
              type: "mv",
              bvid: playItem.bvid,
              cid: playItem.cid,
              title: playItem.title,
              mvPlayData,
            });
            toastError("无法获取音频播放链接");
          }
        } else if (playItem?.bvid) {
          const mvData = await getMVData(playItem.bvid);
          if (isStale()) return;
          const [firstMV, ...restMV] = mvData;
          if (firstMV?.cid) {
            const mvPlayData = await getDashUrl(playItem.bvid, firstMV.cid);
            if (isStale()) return;
            if (mvPlayData?.audioUrl) {
              // Replacing playId below starts another switch, which plays the resolved audioUrl.
              // Playing here as well made that switch pause() a pending play() and raise AbortError.
              updateMediaSession({
                title: firstMV.pageTitle || firstMV.title,
                artist: firstMV.ownerName,
                cover: firstMV.pageCover,
              });

              continuingSwitch = true;
              try {
                usePlayList.setState(state => {
                  const listItemIndex = state.list.findIndex(item => item.id === state.playId);
                  state.list.splice(
                    listItemIndex,
                    1,
                    {
                      ...firstMV,
                      ...{
                        audioUrl: mvPlayData?.audioUrl,
                        videoUrl: mvPlayData?.videoUrl,
                        isLossless: mvPlayData?.isLossless,
                        isDolby: mvPlayData?.isDolby,
                      },
                    },
                    ...restMV,
                  );
                  state.playId = firstMV.id;
                });
              } finally {
                continuingSwitch = false;
              }
            } else {
              log.error("无法获取音频播放链接", {
                type: "mv",
                bvid: playItem.bvid,
                cid: firstMV.cid,
                title: firstMV.title,
                mvPlayData,
              });
              toastError("无法获取音频播放链接");
            }
          } else {
            log.error("无法获取音频播放链接", {
              type: "mv",
              bvid: playItem.bvid,
              title: playItem.title,
              mvData,
            });
            toastError("无法获取音频播放链接");
          }
        }
      }

      if (playItem?.type === "audio" && playItem?.sid) {
        const musicPlayData = await getAudioUrl(playItem.sid);
        if (isStale()) return;
        if (musicPlayData?.audioUrl) {
          resetAudioAndPlay(musicPlayData?.audioUrl);

          updateMediaSession({
            title: playItem.title,
            artist: playItem.ownerName,
            cover: playItem.pageCover,
          });

          usePlayList.setState(state => {
            const listItem = state.list.find(item => item.id === state.playId);
            if (listItem) {
              listItem.audioUrl = musicPlayData?.audioUrl;
            }
          });
        } else {
          log.error("无法获取音频播放链接", {
            type: "audio",
            sid: playItem.sid,
            title: playItem.title,
            musicPlayData,
          });
          toastError("无法获取音频播放链接");
        }
      }
    }
  }
});
