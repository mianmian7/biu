import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import type { PlayData } from "@/store/play-list";

import {
  computeDesktopLyricsPayload,
  getLineHighlightEnd,
  handleDesktopLyricsCommand,
  startDesktopLyricsSync,
} from "@/common/utils/desktop-lyrics";
import {
  anchorNow,
  getDesktopLyricsLineProgress,
  markPlaybackAnchor,
  resetPlaybackAnchor,
  resolveDisplayedLine,
  usePlaybackAnchor,
} from "@/common/utils/playback-anchor";
import { useLyricsStore } from "@/store/lyrics";
import { usePlayList } from "@/store/play-list";
import { usePlayProgress } from "@/store/play-progress";
import { useSettings } from "@/store/settings";
import { defaultAppSettings } from "@shared/settings/app-settings";
import {
  defaultDesktopLyricsStyle,
  desktopLyricsColorCycle,
  resolveDesktopLyricsColors,
  resolveDesktopLyricsStyle,
} from "@shared/settings/desktop-lyrics";

vi.mock("@/components/lyrics/get-lyrics", () => ({
  getLyricsByBili: vi.fn().mockResolvedValue({}),
}));

const item: PlayData = {
  id: "play-1",
  title: "星の歌",
  ownerName: "某UP",
  type: "mv",
  bvid: "BV1xx",
  cid: "100",
};

const syncMock = () => (window as any).electron.syncDesktopLyrics as ReturnType<typeof vi.fn>;

beforeEach(() => {
  vi.useFakeTimers();
  vi.setSystemTime(1_000_000);
  resetPlaybackAnchor();
  (window as any).electron = {
    getStore: vi.fn().mockResolvedValue(undefined),
    setStore: vi.fn().mockResolvedValue(undefined),
    autoLyrics: { start: vi.fn().mockResolvedValue({ status: "unidentified", timings: {} }), onProgress: vi.fn() },
    syncDesktopLyrics: vi.fn(),
    onDesktopLyricsToggleRequested: vi.fn().mockReturnValue(() => {}),
    onDesktopLyricsLockToggleRequested: vi.fn().mockReturnValue(() => {}),
    onDesktopLyricsCommand: vi.fn().mockReturnValue(() => {}),
  };
  useLyricsStore.getState().reset();
  // 同步启动时会加载歌词，这里只测试同步本身，直接用 applyLyrics 准备歌词
  useLyricsStore.setState({ load: vi.fn().mockResolvedValue(undefined) });
  usePlayProgress.setState({ currentTime: 0 });
  usePlayList.setState({
    playId: item.id,
    list: [item],
    isPlaying: true,
    rate: 1,
    getPlayItem: () => item,
  });
  useSettings.setState({
    ...defaultAppSettings,
    showDesktopLyrics: true,
    desktopLyricsStyle: defaultDesktopLyricsStyle,
  });
});

afterEach(() => {
  vi.useRealTimers();
});

describe("getLineHighlightEnd", () => {
  test("estimates singing time from text length and caps it at the next line", () => {
    // 8 个假名约 2.56 秒，下一行在 10 秒后：取估算值
    expect(getLineHighlightEnd("かぜにゆれるまち", 1000, 11_000)).toBe(1000 + 8 * 320);
    // 下一行更早开始：以下一行为准
    expect(getLineHighlightEnd("かぜにゆれるまち", 1000, 2000)).toBe(2000);
  });

  test("uses a minimum duration for short lines and handles the last line", () => {
    expect(getLineHighlightEnd("ah", 5000)).toBe(5000 + 1200);
    expect(getLineHighlightEnd("ah", 5000, 4000)).toBe(5000 + 1200);
  });
});

describe("resolveDesktopLyricsStyle", () => {
  test("fills missing fields from defaults and clamps invalid values", () => {
    const style = resolveDesktopLyricsStyle({
      lineMode: "double",
      subScale: 5,
      opacity: 0,
      align: "diagonal" as DesktopLyricsAlign,
      customPlayedColor: "red",
    });

    expect(style.lineMode).toBe("double");
    expect(style.subScale).toBe(1);
    expect(style.opacity).toBe(1);
    expect(style.align).toBe("center");
    expect(style.customPlayedColor).toBe(defaultDesktopLyricsStyle.customPlayedColor);
    expect(style.karaoke).toBe(defaultDesktopLyricsStyle.karaoke);
  });

  test("resolves custom and theme colors", () => {
    const custom = resolveDesktopLyricsColors({
      ...defaultDesktopLyricsStyle,
      colorPreset: "custom",
      customPlayedColor: "#ff0000",
      customUnplayedColor: "#ffffff",
    });
    expect(custom.played[1]).toBe("#ff0000");
    expect(custom.unplayed[0]).toBe("#ffffff");

    const theme = resolveDesktopLyricsColors({ ...defaultDesktopLyricsStyle, colorPreset: "theme" }, "#00ff00");
    expect(theme.played[1]).toBe("#00ff00");
  });
});

describe("computeDesktopLyricsPayload", () => {
  test("carries the next line, line timing and the merged style", () => {
    useSettings.setState({ desktopLyricsStyle: { lineMode: "double" } as DesktopLyricsStyle });
    useLyricsStore.getState().applyLyrics("[00:01.00]一行目\n[00:04.00]二行目", "[00:01.00]第一行");
    usePlayProgress.setState({ currentTime: 2 });

    const payload = computeDesktopLyricsPayload();

    expect(payload).toMatchObject({
      text: "一行目",
      translation: "第一行",
      nextText: "二行目",
      hasTranslation: true,
      hasRomaji: false,
      lineStartMs: 1000,
      lineEndMs: 1000 + 1200,
      anchor: usePlaybackAnchor.getState(),
      offsetMs: 0,
      index: 0,
    });
    expect(payload.style.lineMode).toBe("double");
    expect(payload.style.showTranslation).toBe(true);
  });

  test("shows the song title and prelude status before the first line", () => {
    useLyricsStore.getState().applyLyrics("[00:05.00]まだ");
    usePlayProgress.setState({ currentTime: 1 });

    const payload = computeDesktopLyricsPayload();

    expect(payload).toMatchObject({ index: -1, text: "星の歌 - 某UP", status: "前奏" });
    expect(payload.lineStartMs).toBeUndefined();
    expect(payload.lineEndMs).toBeUndefined();

    usePlayProgress.setState({ currentTime: 6 });
    expect(computeDesktopLyricsPayload()).toMatchObject({ index: 0, text: "まだ" });
  });

  test("sends the next line so the lyrics window can switch on its own clock", () => {
    useLyricsStore.getState().applyLyrics("[00:01.00]一行目\n[00:04.00]二行目\n[00:09.00]三行目", "[00:04.00]第二行");
    usePlayProgress.setState({ currentTime: 0.5 });
    expect(computeDesktopLyricsPayload().upcoming?.map(line => line.index)).toEqual([0, 1, 2]);
    expect(computeDesktopLyricsPayload().upcoming?.[0]).toMatchObject({ text: "一行目", lineStartMs: 1000 });

    usePlayProgress.setState({ currentTime: 2 });
    expect(computeDesktopLyricsPayload().upcoming).toEqual([
      {
        index: 1,
        text: "二行目",
        romaji: undefined,
        translation: "第二行",
        nextText: "三行目",
        lineStartMs: 4000,
        lineEndMs: getLineHighlightEnd("二行目", 4000, 9000),
      },
      {
        index: 2,
        text: "三行目",
        romaji: undefined,
        translation: undefined,
        nextText: undefined,
        lineStartMs: 9000,
        lineEndMs: getLineHighlightEnd("三行目", 9000, undefined),
      },
    ]);

    usePlayProgress.setState({ currentTime: 10 });
    expect(computeDesktopLyricsPayload().upcoming).toBeUndefined();
  });

  test("shows the song title and status when there are no lyrics", () => {
    useLyricsStore.setState({ lyrics: [], isLoaded: true, isLoading: false });

    const payload = computeDesktopLyricsPayload();

    expect(payload).toMatchObject({ active: true, index: -1, text: "星の歌 - 某UP", status: "暂无歌词" });
  });
});

describe("desktop lyrics sync", () => {
  test("does not resend during four seconds within a long line, but its cached anchor reaches 100%", () => {
    useLyricsStore.getState().applyLyrics("[04:15.77]打ち砕いて欲しいから この空虚\n[04:30.00]次の行");
    usePlayProgress.setState({ currentTime: 255.77 });
    markPlaybackAnchor({ currentTime: 255.77, playbackRate: 1 }, true);
    const stop = startDesktopLyricsSync();
    try {
      const sent = syncMock().mock.calls.length;
      const payload = syncMock().mock.lastCall![0] as DesktopLyricsPayload;
      expect(payload.lineEndMs! - payload.lineStartMs!).toBe(4480);
      for (let elapsed = 250; elapsed <= 4000; elapsed += 250) {
        vi.advanceTimersByTime(250);
        usePlayProgress.setState({ currentTime: 255.77 + elapsed / 1000 });
        expect(syncMock().mock.calls.length).toBe(sent);
      }
      expect(getDesktopLyricsLineProgress(payload, payload.anchor.at + 4480)).toBeGreaterThanOrEqual(1);
    } finally {
      stop();
    }
  });

  test("sends exactly one anchor for seeking and one for seeked", () => {
    useLyricsStore.getState().applyLyrics("[00:01.00]一行目");
    const stop = startDesktopLyricsSync();
    try {
      const sent = syncMock().mock.calls.length;
      markPlaybackAnchor({ currentTime: 100, playbackRate: 1 }, false);
      expect(syncMock().mock.calls.length).toBe(sent + 1);
      expect(syncMock().mock.lastCall?.[0].anchor).toMatchObject({ mediaMs: 100000, running: false });
      markPlaybackAnchor({ currentTime: 100, playbackRate: 1 }, true);
      expect(syncMock().mock.calls.length).toBe(sent + 2);
      expect(syncMock().mock.lastCall?.[0].anchor).toMatchObject({ mediaMs: 100000, running: true });
    } finally {
      stop();
    }
  });

  test.each([true, false])("a backward seek never sends the target line with the old anchor (playing=%s)", running => {
    useLyricsStore.getState().applyLyrics("[00:00.00]zero\n[00:04.00]four\n[00:08.00]eight\n[00:12.00]twelve");
    usePlayProgress.setState({ currentTime: 12 });
    markPlaybackAnchor({ currentTime: 12, playbackRate: 1 }, running);
    const stop = startDesktopLyricsSync();
    try {
      const sent = syncMock().mock.calls.length;
      usePlayList.getState().seek(0);
      const payloads = syncMock()
        .mock.calls.slice(sent)
        .map(call => call[0] as DesktopLyricsPayload);
      expect(payloads.at(-1)).toMatchObject({ index: 0, anchor: { mediaMs: 0 } });
      for (const sentPayload of payloads) {
        const shown = resolveDisplayedLine(sentPayload, anchorNow() + 50);
        expect(shown.index).toBe(sentPayload.index);
      }
    } finally {
      stop();
    }
  });

  test("loadstart resets the track anchor and sends exactly once", async () => {
    // 只初始化事件处理器，不让 init 为测试歌曲请求音频地址。
    usePlayList.setState({ playId: undefined });
    await usePlayList.getState().init();
    usePlayList.setState({ playId: item.id });
    const audio = usePlayList.getState().getAudio();
    const previousTime = audio.currentTime;
    audio.currentTime = 100;
    audio.onplaying!.call(audio, new Event("playing"));
    const stop = startDesktopLyricsSync();
    try {
      const sent = syncMock().mock.calls.length;
      const seq = usePlaybackAnchor.getState().seq;
      audio.onloadstart!.call(audio, new Event("loadstart"));
      expect(usePlaybackAnchor.getState()).toMatchObject({ mediaMs: 0, running: false, seq: seq + 1 });
      expect(syncMock().mock.calls.length).toBe(sent + 1);
      expect(syncMock().mock.lastCall?.[0].anchor).toMatchObject({ mediaMs: 0, running: false, seq: seq + 1 });
      vi.advanceTimersByTime(4000);
      expect(syncMock().mock.calls.length).toBe(sent + 1);
    } finally {
      stop();
      audio.currentTime = previousTime;
    }
  });

  test("sends a changed rate once and extrapolates at that rate", () => {
    useLyricsStore.getState().applyLyrics("[04:15.77]打ち砕いて欲しいから この空虚");
    usePlayProgress.setState({ currentTime: 255.77 });
    const stop = startDesktopLyricsSync();
    try {
      const sent = syncMock().mock.calls.length;
      markPlaybackAnchor({ currentTime: 255.77, playbackRate: 1.5 }, true);
      expect(syncMock().mock.calls.length).toBe(sent + 1);
      const payload = syncMock().mock.lastCall![0] as DesktopLyricsPayload;
      expect(payload.anchor.rate).toBe(1.5);
      expect(getDesktopLyricsLineProgress(payload, payload.anchor.at + 1000)).toBeCloseTo(1500 / 4480);
    } finally {
      stop();
    }
  });

  test("sends a changed offset without changing the anchor", () => {
    const stop = startDesktopLyricsSync();
    try {
      const sent = syncMock().mock.calls.length;
      const seq = usePlaybackAnchor.getState().seq;
      useLyricsStore.setState({ offset: 500 });
      expect(syncMock().mock.calls.length).toBe(sent + 1);
      expect(syncMock().mock.lastCall?.[0]).toMatchObject({ offsetMs: 500, anchor: { seq } });
      expect(usePlaybackAnchor.getState().seq).toBe(seq);
    } finally {
      stop();
    }
  });

  test("resends when the style changes", () => {
    useLyricsStore.getState().applyLyrics("[00:01.00]一行目");
    const stop = startDesktopLyricsSync();
    try {
      handleDesktopLyricsCommand("toggleLineMode");
      expect(syncMock().mock.lastCall?.[0].style.lineMode).toBe("double");
    } finally {
      stop();
    }
  });
});

describe("handleDesktopLyricsCommand", () => {
  test("adjusts font size within bounds", () => {
    useSettings.setState({ desktopLyricsFontSize: 71 });
    handleDesktopLyricsCommand("fontLarger");
    expect(useSettings.getState().desktopLyricsFontSize).toBe(72);

    useSettings.setState({ desktopLyricsFontSize: 17 });
    handleDesktopLyricsCommand("fontSmaller");
    expect(useSettings.getState().desktopLyricsFontSize).toBe(16);
  });

  test("cycles color presets and toggles companions", () => {
    const last = desktopLyricsColorCycle.at(-1)!;
    useSettings.setState({ desktopLyricsStyle: { ...defaultDesktopLyricsStyle, colorPreset: last } });
    handleDesktopLyricsCommand("nextColor");
    expect(useSettings.getState().desktopLyricsStyle.colorPreset).toBe(desktopLyricsColorCycle[0]);

    handleDesktopLyricsCommand("toggleTranslation");
    handleDesktopLyricsCommand("toggleRomaji");
    expect(useSettings.getState().desktopLyricsStyle).toMatchObject({ showTranslation: false, showRomaji: false });
  });

  test("forwards playback commands and lock/close to settings", () => {
    const prev = vi.fn();
    const next = vi.fn();
    const togglePlay = vi.fn();
    usePlayList.setState({ prev, next, togglePlay });

    handleDesktopLyricsCommand("prev");
    handleDesktopLyricsCommand("next");
    handleDesktopLyricsCommand("togglePlay");
    expect([prev, next, togglePlay].map(fn => fn.mock.calls.length)).toEqual([1, 1, 1]);

    useSettings.setState({ desktopLyricsLocked: true });
    handleDesktopLyricsCommand("toggleLock");
    expect(useSettings.getState().desktopLyricsLocked).toBe(false);

    handleDesktopLyricsCommand("close");
    expect(useSettings.getState().showDesktopLyrics).toBe(false);
  });
});
