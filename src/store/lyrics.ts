import { create } from "zustand";

import type { WebPlayerParams } from "@/service/web-player";
import type { AutoLyricsResult } from "@shared/auto-lyrics/service-types";

import { getLyricsByBili } from "@/components/lyrics/get-lyrics";
import { StoreNameMap } from "@shared/store";

import { usePlayList } from "./play-list";

const timeTagPattern = /\[(\d{1,2}):(\d{1,2})(?:\.(\d{1,3}))?\]/g;

const AUTO_FAILURE: Partial<Record<AutoLyricsResult["status"], string>> = {
  unidentified: "自动歌词：未识别到歌曲",
  "no-reference": "自动歌词：没有找到可对齐的原曲",
  "decode-unavailable": "自动歌词：本机无法解码音频",
  error: "自动歌词：出错了",
};

/** 解析 LRC 格式的歌词文本 */
export function parseLrc(raw?: null | string): LyricLine[] {
  if (!raw) return [];

  const result: LyricLine[] = [];
  const lines = raw.split(/\r?\n/);

  lines.forEach(line => {
    const text = line.replace(timeTagPattern, "").trim();
    if (!text) return;

    let match: null | RegExpExecArray;
    while ((match = timeTagPattern.exec(line)) !== null) {
      const minutes = Number(match[1]);
      const seconds = Number(match[2]);
      const millis = match[3] ? Number(match[3].padEnd(3, "0")) : 0;

      if (Number.isNaN(minutes) || Number.isNaN(seconds) || Number.isNaN(millis)) continue;

      const time = Math.max(0, minutes * 60 * 1000 + seconds * 1000 + millis);
      result.push({ text, time });
    }

    timeTagPattern.lastIndex = 0;
  });

  return result.toSorted((a, b) => a.time - b.time);
}

/** 当前播放歌曲对应的歌词缓存 key（bvid-cid），本地音乐/普通音频没有歌词来源 */
export function getLyricsCacheKey(bvid?: string, cid?: string) {
  return bvid && cid ? `${bvid}-${cid}` : null;
}

/** 计算当前毫秒进度对应的歌词行下标，未到第一句时返回 -1 */
export function getActiveLyricIndex(lyrics: LyricLine[], currentMs: number) {
  if (!lyrics.length) return -1;
  if (currentMs < lyrics[0].time) return -1;

  let left = 0;
  let right = lyrics.length - 1;
  let index = 0;

  while (left <= right) {
    const mid = (left + right) >> 1;
    if (lyrics[mid].time <= currentMs) {
      index = mid;
      left = mid + 1;
    } else {
      right = mid - 1;
    }
  }

  return index;
}

interface LyricsState {
  /** 当前歌词对应的缓存 key */
  cacheKey: null | string;
  /** 当前歌曲的歌词偏移量（毫秒），与缓存保持一致，供所有歌词视图共用 */
  offset: number;
  /** 原文歌词 */
  lyrics: LyricLine[];
  /** 翻译歌词 */
  translatedLyrics: LyricLine[];
  /** 罗马音歌词，与原文共用时间戳 */
  romajiLyrics: LyricLine[];
  /** 是否正在加载 */
  isLoading: boolean;
  /** 是否已完成一次加载（用于区分「加载中」和「确认没有歌词」） */
  isLoaded: boolean;
  autoStatus?: string;
  /** 加载当前播放歌曲的歌词（内部做去重，可被多处同时调用） */
  load: () => Promise<void>;
  /** 使用用户手动选择/搜索到的歌词 */
  applyLyrics: (lyricsText?: string, tLyricsText?: string, rLyricsText?: string) => void;
  /** 更新当前歌曲的歌词偏移量 */
  setOffset: (offset: number) => void;
  reset: () => void;
}

let requestSeq = 0;
let stopAutoProgress: VoidFunction | undefined;
/** 用户手动调整偏移量的版本号：异步加载回来时若用户已调整，不要用旧缓存覆盖 */
let offsetRevision = 0;

/** 读取指定歌曲的歌词缓存（包含手动调整的偏移量、字号等） */
const readCachedLyrics = async (cacheKey: string): Promise<MusicLyrics | null> => {
  try {
    const cache = await window.electron?.getStore(StoreNameMap.LyricsCache);
    return cache?.[cacheKey] ?? null;
  } catch {
    return null;
  }
};

export const useLyricsStore = create<LyricsState>((set, get) => ({
  cacheKey: null,
  offset: 0,
  lyrics: [],
  translatedLyrics: [],
  romajiLyrics: [],
  isLoading: false,
  isLoaded: false,
  reset: () => {
    requestSeq += 1;
    stopAutoProgress?.();
    stopAutoProgress = undefined;
    set({
      cacheKey: null,
      offset: 0,
      lyrics: [],
      translatedLyrics: [],
      romajiLyrics: [],
      isLoading: false,
      isLoaded: false,
      autoStatus: undefined,
    });
  },
  applyLyrics: (lyricsText, tLyricsText, rLyricsText) => {
    // 手动采用歌词后，之前仍在途的网络请求必须失效，避免旧歌词覆盖用户的选择
    requestSeq += 1;
    stopAutoProgress?.();
    stopAutoProgress = undefined;
    set({
      lyrics: parseLrc(lyricsText),
      translatedLyrics: parseLrc(tLyricsText),
      romajiLyrics: parseLrc(rLyricsText),
      isLoading: false,
      isLoaded: true,
      autoStatus: undefined,
    });
  },
  setOffset: offset => {
    offsetRevision += 1;
    set({ offset });
  },
  load: async () => {
    const playItem = usePlayList.getState().getPlayItem();
    const cacheKey = getLyricsCacheKey(playItem?.bvid, playItem?.cid);

    if (!cacheKey) {
      // 切到没有歌词来源的歌曲时，让之前仍在途的请求失效
      requestSeq += 1;
      stopAutoProgress?.();
      stopAutoProgress = undefined;
      set({
        cacheKey: null,
        offset: 0,
        lyrics: [],
        translatedLyrics: [],
        romajiLyrics: [],
        isLoading: false,
        isLoaded: true,
        autoStatus: undefined,
      });
      return;
    }

    // 同一首歌已经加载过或正在加载，避免重复请求
    if (get().cacheKey === cacheKey && (get().isLoaded || get().isLoading)) {
      return;
    }

    const seq = ++requestSeq;
    stopAutoProgress?.();
    stopAutoProgress = undefined;
    const offsetRevisionAtRead = offsetRevision;
    set({
      cacheKey,
      offset: 0,
      lyrics: [],
      translatedLyrics: [],
      romajiLyrics: [],
      isLoading: true,
      isLoaded: false,
      autoStatus: undefined,
    });

    let autoApplied = false;
    const applyAuto = (auto?: MusicLyrics) => {
      if (seq !== requestSeq || !auto?.lyrics) return;
      autoApplied = true;
      set({
        lyrics: parseLrc(auto.lyrics),
        translatedLyrics: parseLrc(auto.tLyrics),
        romajiLyrics: parseLrc(auto.rLyrics),
        isLoading: false,
        isLoaded: true,
      });
    };
    const startAuto = (bvid: string, cid: number) => {
      const key = `${bvid}-${cid}`;
      set({ autoStatus: "自动歌词识别中..." });
      stopAutoProgress = window.electron.autoLyrics.onProgress(progress => {
        if (seq !== requestSeq || progress.key !== key) return;
        if (progress.phase === "coarse") applyAuto(progress.lyrics);
      });
      window.electron.autoLyrics
        .start({ bvid, cid })
        .then(result => {
          if (seq !== requestSeq) return;
          applyAuto(result.lyrics);
          if (!result.lyrics && autoApplied) {
            set({ lyrics: [], translatedLyrics: [], romajiLyrics: [] });
          }
          set({ autoStatus: result.lyrics ? undefined : AUTO_FAILURE[result.status] });
        })
        .catch(() => {
          if (seq === requestSeq) set({ autoStatus: undefined });
        })
        .finally(() => {
          if (seq !== requestSeq) return;
          stopAutoProgress?.();
          stopAutoProgress = undefined;
        });
    };

    try {
      // 优先使用手动搜索/调整过偏移的缓存歌词
      const cached = await readCachedLyrics(cacheKey);
      if (seq !== requestSeq) return;

      const cachedOffset = typeof cached?.offset === "number" ? cached.offset : 0;
      // 用户可能只调整过偏移量（缓存里还没有歌词正文），偏移量需要先恢复，
      // 这样无论歌词来自缓存还是网络，都能沿用用户之前的调整；
      // 若读取缓存期间用户又调整了偏移量，则以用户最新调整为准
      if (offsetRevision === offsetRevisionAtRead) {
        set({ offset: cachedOffset });
      }

      if (cached?.lyrics || cached?.tLyrics) {
        set({
          lyrics: parseLrc(cached.lyrics),
          translatedLyrics: parseLrc(cached.tLyrics),
          romajiLyrics: parseLrc(cached.rLyrics),
          isLoading: false,
          isLoaded: true,
        });
        const cid = Number(playItem?.cid);
        if (cached.source === "auto" && !cached.offset && playItem?.bvid && Number.isFinite(cid)) {
          startAuto(playItem.bvid, cid);
        }
        return;
      }

      const cid = Number(playItem?.cid);
      if (!Number.isFinite(cid)) {
        set({ lyrics: [], translatedLyrics: [], romajiLyrics: [], isLoading: false, isLoaded: true });
        return;
      }

      const params: WebPlayerParams = { cid };

      if (playItem?.bvid) {
        startAuto(playItem.bvid, cid);
        params.bvid = playItem.bvid;
      }

      const aid = playItem?.aid ? Number(playItem.aid) : undefined;
      if (aid && !Number.isNaN(aid)) {
        params.aid = aid;
      }

      const body = await getLyricsByBili(params);
      if (seq !== requestSeq || autoApplied) return;

      set({ lyrics: body ?? [], translatedLyrics: [], romajiLyrics: [], isLoading: false, isLoaded: true });
    } catch {
      if (seq !== requestSeq || autoApplied) return;
      set({ lyrics: [], translatedLyrics: [], romajiLyrics: [], isLoading: false, isLoaded: true });
    }
  },
}));
