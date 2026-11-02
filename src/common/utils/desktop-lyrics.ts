import type { PlayData } from "@/store/play-list";

import { getActiveLyricIndex, useLyricsStore } from "@/store/lyrics";
import { usePlayList } from "@/store/play-list";
import { usePlayProgress } from "@/store/play-progress";
import { useSettings } from "@/store/settings";
import {
  DESKTOP_LYRICS_FONT_SIZE_MAX,
  DESKTOP_LYRICS_FONT_SIZE_MIN,
  desktopLyricsColorCycle,
  resolveDesktopLyricsColors,
  resolveDesktopLyricsStyle,
} from "@shared/settings/desktop-lyrics";

/** 判断当前渲染进程是否是子窗口（迷你播放器/桌面歌词） */
function isSubWindow() {
  const hash = window.location.hash;
  return hash.includes("mini-player") || hash.includes("desktop-lyrics");
}

/** 按歌词时间查找翻译或罗马音文本 */
const getCompanionByTime = (companions: LyricLine[], time?: number) => {
  if (typeof time !== "number") return undefined;
  return companions.find(item => item.time === time)?.text;
};

/** 从播放列表中查找当前播放项 */
const findPlayItem = (playId?: string): PlayData | undefined => {
  const { list } = usePlayList.getState();
  return list.find(item => item.id === playId);
};

/** 单个字符的大致演唱时长权重：中日韩字符按一个音节，其他字符（字母/空格）按 0.35 */
const CJK_PATTERN = /[぀-ヿ㐀-鿿가-힯豈-﫿]/;
const MS_PER_SYLLABLE = 320;
const MIN_LINE_MS = 1200;

/**
 * 计算一行歌词进度高亮的结束时间。
 *
 * 逐行 LRC 只有每行的开始时间：用下一行开始时间作为上限，
 * 再按文本长度估算演唱时长，避免间奏很长时高亮拖到间奏结束。
 */
export function getLineHighlightEnd(text: string, startMs: number, nextStartMs?: number) {
  let syllables = 0;
  for (const ch of text) {
    if (CJK_PATTERN.test(ch)) syllables += 1;
    else if (ch.trim()) syllables += 0.35;
  }

  const estimated = Math.max(MIN_LINE_MS, Math.round(syllables * MS_PER_SYLLABLE));
  if (typeof nextStartMs !== "number" || nextStartMs <= startMs) return startMs + estimated;
  return Math.min(nextStartMs, startMs + estimated);
}

/** 没有歌词行时的提示文字 */
function getStatusText() {
  const { isLoading, isLoaded, autoStatus } = useLyricsStore.getState();
  if (autoStatus) return autoStatus;
  if (isLoading || !isLoaded) return "歌词加载中...";
  return "暂无歌词";
}

/**
 * 计算桌面歌词窗口需要展示的内容快照
 *
 * 以主窗口的播放/歌词状态为准，通过 IPC 交给主进程再转发给桌面歌词窗口，
 * 这样主窗口缩到托盘/最小化时桌面歌词仍然可以正常更新。
 */
export function computeDesktopLyricsPayload(now = Date.now()): DesktopLyricsPayload {
  const settings = useSettings.getState();
  const { playId, isPlaying, rate } = usePlayList.getState();
  const playItem = findPlayItem(playId);
  const { lyrics, translatedLyrics, romajiLyrics, offset } = useLyricsStore.getState();
  const positionMs = Math.round(usePlayProgress.getState().currentTime * 1000 + offset);
  const style = resolveDesktopLyricsStyle(settings.desktopLyricsStyle);

  const activeIndex = getActiveLyricIndex(lyrics, positionMs);
  // 还没唱到第一句时展示第一句（与顶部歌词栏保持一致）
  const index = activeIndex >= 0 ? activeIndex : lyrics.length ? 0 : -1;
  const line = index >= 0 ? lyrics[index] : undefined;
  const nextLine = index >= 0 ? lyrics[index + 1] : undefined;
  // 未唱到第一句时不高亮
  const isBeforeFirstLine = activeIndex < 0;

  const base = {
    enabled: Boolean(settings.showDesktopLyrics),
    locked: Boolean(settings.desktopLyricsLocked),
    active: Boolean(playItem),
    hasTranslation: translatedLyrics.length > 0,
    hasRomaji: romajiLyrics.length > 0,
    positionMs,
    syncedAt: now,
    rate: Number(rate) || 1,
    isPlaying: Boolean(isPlaying),
    playId,
    index,
    fontSize: Number(settings.desktopLyricsFontSize) || 36,
    style,
    colors: resolveDesktopLyricsColors(style, settings.primaryColor),
  };

  if (!line) {
    const title = playItem
      ? [playItem.pageTitle || playItem.title, playItem.ownerName].filter(Boolean).join(" - ")
      : "";
    return { ...base, text: title || undefined, status: playItem ? getStatusText() : undefined };
  }

  return {
    ...base,
    text: line.text,
    romaji: getCompanionByTime(romajiLyrics, line.time),
    translation: getCompanionByTime(translatedLyrics, line.time),
    nextText: nextLine?.text,
    lineStartMs: isBeforeFirstLine ? undefined : line.time,
    lineEndMs: isBeforeFirstLine ? undefined : getLineHighlightEnd(line.text, line.time, nextLine?.time),
  };
}

/** 只比较展示相关字段：播放中位置每次 timeupdate 都会变，由窗口端插值，不必每次同步 */
function serializeForCompare(payload: DesktopLyricsPayload) {
  return JSON.stringify({ ...payload, positionMs: 0, syncedAt: 0 });
}

/** 播放中时窗口端插值误差超过该值才重新同步位置 */
const POSITION_DRIFT_MS = 120;

let isSyncStarted = false;
let lastSerialized = "";
let lastSent: DesktopLyricsPayload | null = null;

/** 计算内容快照，并在内容或播放位置明显变化时同步给主进程 */
function syncPayload() {
  if (!window.electron?.syncDesktopLyrics) return;

  const payload = computeDesktopLyricsPayload();
  const serialized = serializeForCompare(payload);

  if (serialized === lastSerialized && lastSent) {
    const elapsed = lastSent.isPlaying ? (payload.syncedAt - lastSent.syncedAt) * lastSent.rate : 0;
    const expected = lastSent.positionMs + elapsed;
    if (Math.abs(expected - payload.positionMs) < POSITION_DRIFT_MS) return;
  }

  lastSerialized = serialized;
  lastSent = payload;
  window.electron.syncDesktopLyrics(payload);
}

const clampFontSize = (value: number) =>
  Math.min(DESKTOP_LYRICS_FONT_SIZE_MAX, Math.max(DESKTOP_LYRICS_FONT_SIZE_MIN, value));

/** 修改桌面歌词样式的部分字段 */
function patchStyle(patch: Partial<DesktopLyricsStyle>) {
  const { desktopLyricsStyle, update } = useSettings.getState();
  update({ desktopLyricsStyle: { ...resolveDesktopLyricsStyle(desktopLyricsStyle), ...patch } });
}

/** 处理托盘/桌面歌词窗口发来的操作（主窗口是播放与设置的数据源） */
export function handleDesktopLyricsCommand(command: DesktopLyricsCommand) {
  const settings = useSettings.getState();
  const style = resolveDesktopLyricsStyle(settings.desktopLyricsStyle);
  const player = usePlayList.getState();

  switch (command) {
    case "prev":
      player.prev();
      break;
    case "next":
      player.next();
      break;
    case "togglePlay":
      player.togglePlay();
      break;
    case "toggleLock":
      settings.update({ desktopLyricsLocked: !settings.desktopLyricsLocked });
      break;
    case "close":
      settings.update({ showDesktopLyrics: false });
      break;
    case "fontLarger":
      settings.update({ desktopLyricsFontSize: clampFontSize((Number(settings.desktopLyricsFontSize) || 36) + 2) });
      break;
    case "fontSmaller":
      settings.update({ desktopLyricsFontSize: clampFontSize((Number(settings.desktopLyricsFontSize) || 36) - 2) });
      break;
    case "nextColor": {
      const current = desktopLyricsColorCycle.indexOf(style.colorPreset);
      patchStyle({ colorPreset: desktopLyricsColorCycle[(current + 1) % desktopLyricsColorCycle.length] });
      break;
    }
    case "toggleLineMode":
      patchStyle({ lineMode: style.lineMode === "double" ? "single" : "double" });
      break;
    case "toggleTranslation":
      patchStyle({ showTranslation: !style.showTranslation });
      break;
    case "toggleRomaji":
      patchStyle({ showRomaji: !style.showRomaji });
      break;
    default:
      break;
  }
}

/**
 * 启动主窗口 -> 桌面歌词窗口的状态同步。
 *
 * - 只在主窗口调用，子窗口调用会直接返回
 * - 订阅播放状态、歌词、进度与设置变化，变化时向主进程同步一次
 * - 同时接收托盘/桌面歌词窗口发来的操作（主窗口是设置的数据源）
 */
export function startDesktopLyricsSync() {
  if (isSyncStarted || isSubWindow()) {
    return () => {};
  }

  isSyncStarted = true;
  syncPayload();

  // 播放时就加载共享歌词，不依赖歌词视图是否开启
  void useLyricsStore.getState().load();

  const unsubscribers = [
    usePlayList.subscribe((state, prevState) => {
      if (state.playId !== prevState.playId) {
        void useLyricsStore.getState().load();
      }
      if (
        state.playId !== prevState.playId ||
        state.isPlaying !== prevState.isPlaying ||
        state.rate !== prevState.rate ||
        state.list !== prevState.list
      ) {
        syncPayload();
      }
    }),
    usePlayProgress.subscribe((state, prevState) => {
      if (state.currentTime !== prevState.currentTime) {
        syncPayload();
      }
    }),
    useLyricsStore.subscribe((state, prevState) => {
      if (
        state.lyrics !== prevState.lyrics ||
        state.translatedLyrics !== prevState.translatedLyrics ||
        state.romajiLyrics !== prevState.romajiLyrics ||
        state.offset !== prevState.offset ||
        state.isLoaded !== prevState.isLoaded ||
        state.isLoading !== prevState.isLoading ||
        state.autoStatus !== prevState.autoStatus
      ) {
        syncPayload();
      }
    }),
    useSettings.subscribe((state, prevState) => {
      if (state.showDesktopLyrics !== prevState.showDesktopLyrics) {
        if (state.showDesktopLyrics) {
          void useLyricsStore.getState().load();
        }
        syncPayload();
        return;
      }
      if (
        state.desktopLyricsLocked !== prevState.desktopLyricsLocked ||
        state.desktopLyricsFontSize !== prevState.desktopLyricsFontSize ||
        state.desktopLyricsStyle !== prevState.desktopLyricsStyle ||
        state.primaryColor !== prevState.primaryColor
      ) {
        syncPayload();
      }
    }),
  ];

  // 托盘菜单：显示/隐藏桌面歌词
  const offToggleRequested = window.electron.onDesktopLyricsToggleRequested(() => {
    const { showDesktopLyrics, update } = useSettings.getState();
    update({ showDesktopLyrics: !showDesktopLyrics });
  });
  const offLockToggleRequested = window.electron.onDesktopLyricsLockToggleRequested(() =>
    handleDesktopLyricsCommand("toggleLock"),
  );
  const offCommand = window.electron.onDesktopLyricsCommand?.(handleDesktopLyricsCommand);

  return () => {
    isSyncStarted = false;
    lastSerialized = "";
    lastSent = null;
    unsubscribers.forEach(unsubscribe => unsubscribe());
    offToggleRequested?.();
    offLockToggleRequested?.();
    offCommand?.();
  };
}
