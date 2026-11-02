import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import type { PlayData } from "@/store/play-list";

import {
  computeDesktopLyricsPayload,
  getLineHighlightEnd,
  handleDesktopLyricsCommand,
  startDesktopLyricsSync,
} from "@/common/utils/desktop-lyrics";
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
      positionMs: 2000,
      index: 0,
    });
    expect(payload.style.lineMode).toBe("double");
    expect(payload.style.showTranslation).toBe(true);
  });

  test("does not highlight before the first line", () => {
    useLyricsStore.getState().applyLyrics("[00:05.00]まだ");
    usePlayProgress.setState({ currentTime: 1 });

    const payload = computeDesktopLyricsPayload();

    expect(payload.text).toBe("まだ");
    expect(payload.lineStartMs).toBeUndefined();
  });

  test("shows the song title and status when there are no lyrics", () => {
    useLyricsStore.setState({ lyrics: [], isLoaded: true, isLoading: false });

    const payload = computeDesktopLyricsPayload();

    expect(payload).toMatchObject({ active: true, index: -1, text: "星の歌 - 某UP", status: "暂无歌词" });
  });
});

describe("desktop lyrics sync", () => {
  test("does not resend while playback advances as the window extrapolates, resends after a seek", () => {
    useLyricsStore.getState().applyLyrics("[00:01.00]一行目\n[00:30.00]二行目");
    usePlayProgress.setState({ currentTime: 1 });
    const stop = startDesktopLyricsSync();
    try {
      const sent = syncMock().mock.calls.length;

      vi.advanceTimersByTime(250);
      usePlayProgress.setState({ currentTime: 1.25 });
      expect(syncMock().mock.calls.length).toBe(sent);

      usePlayProgress.setState({ currentTime: 10 });
      expect(syncMock().mock.calls.length).toBe(sent + 1);
      expect(syncMock().mock.lastCall?.[0].positionMs).toBe(10_000);
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
