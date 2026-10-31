import { beforeEach, describe, expect, test, vi } from "vitest";

import type { PlayData } from "@/store/play-list";
import type { AutoLyricsProgress, AutoLyricsResult } from "@shared/auto-lyrics/service-types";

import { startDesktopLyricsSync } from "@/common/utils/desktop-lyrics";
import { getLyricsByBili } from "@/components/lyrics/get-lyrics";
import { getActiveLyricIndex, parseLrc, useLyricsStore } from "@/store/lyrics";
import { usePlayList } from "@/store/play-list";
import { useSettings } from "@/store/settings";

vi.mock("@/components/lyrics/get-lyrics", () => ({
  getLyricsByBili: vi.fn(),
}));

const flushPromises = () => new Promise(resolve => setTimeout(resolve, 0));

const createPlayItem = (overrides: Partial<PlayData> = {}): PlayData => ({
  id: "play-1",
  title: "test",
  type: "mv",
  bvid: "BV1xx",
  cid: "100",
  ...overrides,
});

const setPlayItem = (item: PlayData | undefined) => {
  usePlayList.setState({ getPlayItem: () => item });
};

const getElectronStoreMock = () => (window as any).electron.getStore as ReturnType<typeof vi.fn>;

const autoStart = vi.fn<typeof window.electron.autoLyrics.start>();
const autoProgress = vi.fn<typeof window.electron.autoLyrics.onProgress>();
const unsubscribe = vi.fn();
const deferred = <T>() => {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => {
    resolve = done;
  });
  return { promise, resolve };
};

beforeEach(() => {
  vi.clearAllMocks();
  useLyricsStore.getState().reset();
  setPlayItem(undefined);
  autoStart.mockResolvedValue({ key: "BV1xx-100", status: "unidentified", timings: {} });
  autoProgress.mockReturnValue(unsubscribe);
  (window as any).electron = {
    getStore: vi.fn().mockResolvedValue(undefined),
    setStore: vi.fn().mockResolvedValue(undefined),
    autoLyrics: { start: autoStart, onProgress: autoProgress },
    syncDesktopLyrics: vi.fn(),
    onDesktopLyricsToggleRequested: vi.fn().mockReturnValue(() => {}),
    onDesktopLyricsLockToggleRequested: vi.fn().mockReturnValue(() => {}),
  };
});

describe("parseLrc", () => {
  test("parses timestamped lines", () => {
    const result = parseLrc("[00:01.00]first\n[00:02.500]second\n");

    expect(result).toEqual([
      { time: 1000, text: "first" },
      { time: 2500, text: "second" },
    ]);
  });

  test("supports one line with multiple timestamps", () => {
    const result = parseLrc("[00:05.00][00:01.00]chorus\n");

    expect(result).toEqual([
      { time: 1000, text: "chorus" },
      { time: 5000, text: "chorus" },
    ]);
  });

  test("drops metadata, empty lines and empty text", () => {
    const result = parseLrc("[ti:title]\n[00:01.00]\n[00:02.00] kept \n");

    expect(result).toEqual([{ time: 2000, text: "kept" }]);
  });

  test("returns empty array for missing input", () => {
    expect(parseLrc()).toEqual([]);
    expect(parseLrc(undefined)).toEqual([]);
  });
});

describe("getActiveLyricIndex", () => {
  const lyrics = [
    { time: 1000, text: "a" },
    { time: 3000, text: "b" },
  ];

  test("returns -1 for empty lyrics or before first line", () => {
    expect(getActiveLyricIndex([], 0)).toBe(-1);
    expect(getActiveLyricIndex(lyrics, 500)).toBe(-1);
  });

  test("matches line boundaries", () => {
    expect(getActiveLyricIndex(lyrics, 1000)).toBe(0);
    expect(getActiveLyricIndex(lyrics, 2999)).toBe(0);
    expect(getActiveLyricIndex(lyrics, 3000)).toBe(1);
    expect(getActiveLyricIndex(lyrics, 99999)).toBe(1);
  });
});

describe("useLyricsStore", () => {
  test("loads lyrics on playback changes with both lyrics bars disabled", async () => {
    useSettings.setState({ showTopLyrics: false, showDesktopLyrics: false });
    usePlayList.setState({ playId: undefined });
    const stopSync = startDesktopLyricsSync();
    try {
      const item = createPlayItem();
      setPlayItem(item);
      usePlayList.setState({ playId: item.id, list: [item] });
      await flushPromises();
      expect(autoStart).toHaveBeenCalledWith({ bvid: item.bvid, cid: 100 });
    } finally {
      stopSync();
    }
  });

  const coarse = (key = "BV1xx-100"): AutoLyricsProgress => ({
    key,
    phase: "coarse",
    coarse: { offsetSec: 0, confidence: 1 },
    lyrics: { lyrics: "[00:01.00]coarse", tLyrics: "[00:01.00]粗对齐", rLyrics: "[00:01.00]so a ra i" },
    timings: {},
  });

  test("applies coarse auto lyrics before replacing them with the final result", async () => {
    const result = deferred<AutoLyricsResult>();
    const subtitles = deferred<LyricLine[]>();
    autoStart.mockReturnValue(result.promise);
    vi.mocked(getLyricsByBili).mockReturnValue(subtitles.promise);
    setPlayItem(createPlayItem());
    const loading = useLyricsStore.getState().load();
    await flushPromises();
    expect(autoStart).toHaveBeenCalledWith({ bvid: "BV1xx", cid: 100 });
    expect(useLyricsStore.getState().autoStatus).toBe("自动歌词识别中...");

    const progress = autoProgress.mock.calls[0][0];
    progress(coarse("another-key"));
    expect(useLyricsStore.getState().lyrics).toEqual([]);
    progress(coarse());
    expect(useLyricsStore.getState()).toMatchObject({
      lyrics: [{ time: 1000, text: "coarse" }],
      translatedLyrics: [{ time: 1000, text: "粗对齐" }],
      romajiLyrics: [{ time: 1000, text: "so a ra i" }],
      isLoading: false,
      isLoaded: true,
    });
    subtitles.resolve([{ time: 0, text: "late subtitles" }]);
    await loading;
    expect(useLyricsStore.getState().lyrics[0].text).toBe("coarse");

    result.resolve({
      key: "BV1xx-100",
      status: "matched",
      timings: {},
      lyrics: { lyrics: "[00:02.00]final", tLyrics: "[00:02.00]最终", rLyrics: "[00:02.00]sa i shu u" },
    });
    await flushPromises();
    expect(useLyricsStore.getState().lyrics).toEqual([{ time: 2000, text: "final" }]);
    expect(useLyricsStore.getState().translatedLyrics).toEqual([{ time: 2000, text: "最终" }]);
    expect(useLyricsStore.getState().romajiLyrics).toEqual([{ time: 2000, text: "sa i shu u" }]);
    expect(useLyricsStore.getState().autoStatus).toBeUndefined();
    expect(unsubscribe).toHaveBeenCalledTimes(1);
  });

  test("manual adoption invalidates both coarse and final auto lyrics", async () => {
    const result = deferred<AutoLyricsResult>();
    autoStart.mockReturnValue(result.promise);
    setPlayItem(createPlayItem());
    await useLyricsStore.getState().load();
    const progress = autoProgress.mock.calls[0][0];

    useLyricsStore.getState().applyLyrics("[00:03.00]manual", "[00:03.00]手动");
    expect(unsubscribe).toHaveBeenCalledTimes(1);
    progress(coarse());
    result.resolve({ key: "BV1xx-100", status: "matched", timings: {}, lyrics: { lyrics: "[00:02.00]auto" } });
    await flushPromises();

    expect(useLyricsStore.getState().lyrics).toEqual([{ time: 3000, text: "manual" }]);
    expect(useLyricsStore.getState().translatedLyrics).toEqual([{ time: 3000, text: "手动" }]);
    expect(useLyricsStore.getState().autoStatus).toBeUndefined();
  });

  test("clears coarse lyrics when the final result has no reference", async () => {
    const result = deferred<AutoLyricsResult>();
    const subtitles = deferred<LyricLine[]>();
    autoStart.mockReturnValue(result.promise);
    vi.mocked(getLyricsByBili).mockReturnValue(subtitles.promise);
    setPlayItem(createPlayItem());
    const loading = useLyricsStore.getState().load();
    await flushPromises();
    autoProgress.mock.calls[0][0](coarse());

    result.resolve({ key: "BV1xx-100", status: "no-reference", timings: {} });
    await flushPromises();
    subtitles.resolve([{ time: 0, text: "late subtitles" }]);
    await loading;

    expect(useLyricsStore.getState()).toMatchObject({
      lyrics: [],
      translatedLyrics: [],
      romajiLyrics: [],
      autoStatus: "自动歌词：没有找到可对齐的原曲",
      isLoading: false,
      isLoaded: true,
    });
  });

  test("shows cached auto lyrics immediately and requests algorithm revalidation", async () => {
    const result = deferred<AutoLyricsResult>();
    autoStart.mockReturnValue(result.promise);
    getElectronStoreMock().mockResolvedValue({
      "BV1xx-100": { source: "auto", lyrics: "[00:01.00]cached", algoVersion: "old" },
    });
    setPlayItem(createPlayItem());
    await useLyricsStore.getState().load();

    expect(useLyricsStore.getState().lyrics).toEqual([{ time: 1000, text: "cached" }]);
    expect(autoStart).toHaveBeenCalledTimes(1);
    expect(getLyricsByBili).not.toHaveBeenCalled();

    result.resolve({ key: "BV1xx-100", status: "matched", timings: {}, lyrics: { lyrics: "[00:02.00]refreshed" } });
    await flushPromises();
    expect(useLyricsStore.getState().lyrics).toEqual([{ time: 2000, text: "refreshed" }]);
  });

  test("revalidates cached auto lyrics saved with a zero offset (an adjusted offset is marked manual)", async () => {
    autoStart.mockReturnValue(new Promise(() => {}));
    getElectronStoreMock().mockResolvedValue({ "BV1xx-100": { source: "auto", lyrics: "[00:01.00]zero", offset: 0 } });
    setPlayItem(createPlayItem());
    await useLyricsStore.getState().load();

    expect(useLyricsStore.getState().lyrics).toEqual([{ time: 1000, text: "zero" }]);
    expect(autoStart).toHaveBeenCalledTimes(1);
  });

  test.each<MusicLyrics>([
    { source: "manual", lyrics: "[00:01.00]manual" },
    { lyrics: "[00:01.00]legacy manual" },
    { source: "auto", lyrics: "[00:01.00]adjusted", offset: 500 },
  ])("preserves manual or offset-adjusted cached lyrics: %j", async cached => {
    getElectronStoreMock().mockResolvedValue({ "BV1xx-100": cached });
    setPlayItem(createPlayItem());
    await useLyricsStore.getState().load();

    expect(useLyricsStore.getState().lyrics).toEqual(parseLrc(cached.lyrics));
    expect(useLyricsStore.getState().offset).toBe(cached.offset ?? 0);
    expect(autoStart).not.toHaveBeenCalled();
  });

  test("switching songs invalidates the previous auto request without unsubscribing the new one", async () => {
    const first = deferred<AutoLyricsResult>();
    const second = deferred<AutoLyricsResult>();
    const stopFirst = vi.fn();
    const stopSecond = vi.fn();
    autoStart.mockReturnValueOnce(first.promise).mockReturnValueOnce(second.promise);
    autoProgress.mockReturnValueOnce(stopFirst).mockReturnValueOnce(stopSecond);
    setPlayItem(createPlayItem());
    await useLyricsStore.getState().load();
    const firstProgress = autoProgress.mock.calls[0][0];

    setPlayItem(createPlayItem({ bvid: "BVsecond", cid: "200" }));
    await useLyricsStore.getState().load();
    expect(stopFirst).toHaveBeenCalledTimes(1);
    autoProgress.mock.calls[1][0](coarse("BVsecond-200"));
    firstProgress(coarse());
    first.resolve({ key: "BV1xx-100", status: "matched", timings: {}, lyrics: { lyrics: "[00:02.00]stale" } });
    await flushPromises();

    expect(useLyricsStore.getState().cacheKey).toBe("BVsecond-200");
    expect(useLyricsStore.getState().lyrics).toEqual([{ time: 1000, text: "coarse" }]);
    expect(stopSecond).not.toHaveBeenCalled();
    second.resolve({ key: "BVsecond-200", status: "matched", timings: {}, lyrics: { lyrics: "[00:04.00]second" } });
    await flushPromises();
    expect(useLyricsStore.getState().lyrics).toEqual([{ time: 4000, text: "second" }]);
    expect(stopSecond).toHaveBeenCalledTimes(1);
  });

  test("reset unsubscribes and invalidates pending auto lyrics", async () => {
    const result = deferred<AutoLyricsResult>();
    autoStart.mockReturnValue(result.promise);
    setPlayItem(createPlayItem());
    await useLyricsStore.getState().load();
    const progress = autoProgress.mock.calls[0][0];
    useLyricsStore.getState().reset();
    expect(unsubscribe).toHaveBeenCalledTimes(1);
    progress(coarse());
    result.resolve({ key: "BV1xx-100", status: "matched", timings: {}, lyrics: { lyrics: "[00:02.00]stale" } });
    await flushPromises();
    expect(useLyricsStore.getState()).toMatchObject({ cacheKey: null, lyrics: [], isLoaded: false });
    expect(useLyricsStore.getState().autoStatus).toBeUndefined();
  });

  test("uses cached lyrics without requesting the network", async () => {
    getElectronStoreMock().mockResolvedValue({
      "BV1xx-100": { lyrics: "[00:01.00]cached line", rLyrics: "[00:01.00]ka n" },
    });
    setPlayItem(createPlayItem());

    await useLyricsStore.getState().load();

    expect(getLyricsByBili).not.toHaveBeenCalled();
    expect(useLyricsStore.getState().lyrics).toEqual([{ time: 1000, text: "cached line" }]);
    expect(useLyricsStore.getState().romajiLyrics).toEqual([{ time: 1000, text: "ka n" }]);
    expect(useLyricsStore.getState().isLoaded).toBe(true);
    expect(useLyricsStore.getState().isLoading).toBe(false);
  });

  test("falls back to bilibili lyrics when cache is empty", async () => {
    vi.mocked(getLyricsByBili).mockResolvedValue([{ time: 1000, text: "online line" }]);
    setPlayItem(createPlayItem());

    await useLyricsStore.getState().load();

    expect(getLyricsByBili).toHaveBeenCalledTimes(1);
    expect(getLyricsByBili).toHaveBeenCalledWith({ bvid: "BV1xx", cid: 100 });
    expect(useLyricsStore.getState().lyrics).toEqual([{ time: 1000, text: "online line" }]);
  });

  test("does not request lyrics for items without bvid/cid", async () => {
    setPlayItem(createPlayItem({ bvid: undefined, cid: undefined }));

    await useLyricsStore.getState().load();

    expect(getLyricsByBili).not.toHaveBeenCalled();
    expect(useLyricsStore.getState().lyrics).toEqual([]);
    expect(useLyricsStore.getState().isLoaded).toBe(true);
  });

  test("deduplicates concurrent loads for the same song", async () => {
    vi.mocked(getLyricsByBili).mockResolvedValue([{ time: 1000, text: "line" }]);
    setPlayItem(createPlayItem());

    await Promise.all([useLyricsStore.getState().load(), useLyricsStore.getState().load()]);

    expect(getLyricsByBili).toHaveBeenCalledTimes(1);
  });

  test("does not load the same song twice", async () => {
    vi.mocked(getLyricsByBili).mockResolvedValue([{ time: 1000, text: "line" }]);
    setPlayItem(createPlayItem());

    await useLyricsStore.getState().load();
    await useLyricsStore.getState().load();

    expect(getLyricsByBili).toHaveBeenCalledTimes(1);
  });

  test("discards stale response when switching songs quickly", async () => {
    let resolveFirst: (value: LyricLine[]) => void = () => {};
    vi.mocked(getLyricsByBili).mockImplementationOnce(
      () =>
        new Promise<LyricLine[]>(resolve => {
          resolveFirst = resolve;
        }),
    );
    setPlayItem(createPlayItem({ bvid: "BVfirst", cid: "1" }));
    const first = useLyricsStore.getState().load();
    await flushPromises();

    vi.mocked(getLyricsByBili).mockResolvedValueOnce([{ time: 0, text: "second song" }]);
    setPlayItem(createPlayItem({ bvid: "BVsecond", cid: "2" }));
    await useLyricsStore.getState().load();

    resolveFirst([{ time: 0, text: "first song" }]);
    await first;

    const state = useLyricsStore.getState();
    expect(state.cacheKey).toBe("BVsecond-2");
    expect(state.lyrics).toEqual([{ time: 0, text: "second song" }]);
  });

  test("applyLyrics updates original, translated and romaji lyrics", () => {
    useLyricsStore.getState().applyLyrics("[00:01.00]hello", "[00:01.00]你好", "[00:01.00]ha ro o");

    const state = useLyricsStore.getState();
    expect(state.lyrics).toEqual([{ time: 1000, text: "hello" }]);
    expect(state.translatedLyrics).toEqual([{ time: 1000, text: "你好" }]);
    expect(state.romajiLyrics).toEqual([{ time: 1000, text: "ha ro o" }]);
    expect(state.isLoaded).toBe(true);
  });

  test("desktop lyrics payload carries romaji and translation of the active line", () => {
    useSettings.setState({ showDesktopLyrics: true });
    const item = createPlayItem();
    setPlayItem(item);
    usePlayList.setState({ playId: item.id, list: [item] });
    useLyricsStore.getState().applyLyrics("[00:00.00]空の箱", "[00:00.00]空箱子", "[00:00.00]so ra no ha ko");
    const stopSync = startDesktopLyricsSync();
    try {
      const sync = (window as any).electron.syncDesktopLyrics as ReturnType<typeof vi.fn>;
      expect(sync.mock.lastCall?.[0]).toMatchObject({
        text: "空の箱",
        romaji: "so ra no ha ko",
        translation: "空箱子",
      });
    } finally {
      stopSync();
    }
  });

  test("applies cached offset when loading lyrics", async () => {
    getElectronStoreMock().mockResolvedValue({
      "BV1xx-100": { lyrics: "[00:01.00]cached line", offset: 1200 },
    });
    setPlayItem(createPlayItem());

    await useLyricsStore.getState().load();

    expect(useLyricsStore.getState().offset).toBe(1200);
  });

  test("resets offset when loading another song without cached offset", async () => {
    getElectronStoreMock().mockResolvedValue({
      "BV1xx-100": { lyrics: "[00:01.00]first", offset: 800 },
    });
    setPlayItem(createPlayItem());
    await useLyricsStore.getState().load();
    expect(useLyricsStore.getState().offset).toBe(800);

    getElectronStoreMock().mockResolvedValue({ "BV2xx-200": { lyrics: "[00:01.00]second" } });
    setPlayItem(createPlayItem({ bvid: "BV2xx", cid: "200" }));
    await useLyricsStore.getState().load();

    expect(useLyricsStore.getState().offset).toBe(0);
  });

  test("updates offset through setOffset", () => {
    useLyricsStore.getState().setOffset(1500);

    expect(useLyricsStore.getState().offset).toBe(1500);
  });

  test("applyLyrics invalidates an in-flight load", async () => {
    let resolveLoad: (value: LyricLine[]) => void = () => {};
    vi.mocked(getLyricsByBili).mockImplementationOnce(
      () =>
        new Promise<LyricLine[]>(resolve => {
          resolveLoad = resolve;
        }),
    );
    setPlayItem(createPlayItem());
    const pending = useLyricsStore.getState().load();
    await flushPromises();

    useLyricsStore.getState().applyLyrics("[00:02.00]adopted", "[00:02.00]采用");
    resolveLoad([{ time: 0, text: "late network" }]);
    await pending;

    expect(useLyricsStore.getState().lyrics).toEqual([{ time: 2000, text: "adopted" }]);
  });

  test("switching to an item without lyrics invalidates an in-flight load", async () => {
    let resolveLoad: (value: LyricLine[]) => void = () => {};
    vi.mocked(getLyricsByBili).mockImplementationOnce(
      () =>
        new Promise<LyricLine[]>(resolve => {
          resolveLoad = resolve;
        }),
    );
    setPlayItem(createPlayItem());
    const pending = useLyricsStore.getState().load();
    await flushPromises();

    setPlayItem(createPlayItem({ bvid: undefined, cid: undefined }));
    await useLyricsStore.getState().load();

    resolveLoad([{ time: 0, text: "late network" }]);
    await pending;

    expect(useLyricsStore.getState().lyrics).toEqual([]);
  });

  test("restores cached offset even when lyrics come from the network", async () => {
    getElectronStoreMock().mockResolvedValue({
      "BV1xx-100": { offset: 900 },
    });
    vi.mocked(getLyricsByBili).mockResolvedValue([{ time: 1000, text: "online line" }]);
    setPlayItem(createPlayItem());

    await useLyricsStore.getState().load();

    expect(useLyricsStore.getState().offset).toBe(900);
    expect(useLyricsStore.getState().lyrics).toEqual([{ time: 1000, text: "online line" }]);
  });

  test("does not overwrite offset adjusted while loading", async () => {
    let resolveStore: (value: unknown) => void = () => {};
    (window as any).electron.getStore = vi.fn(
      () =>
        new Promise(resolve => {
          resolveStore = resolve;
        }),
    );
    setPlayItem(createPlayItem());
    const pending = useLyricsStore.getState().load();
    await flushPromises();

    useLyricsStore.getState().setOffset(2500);
    resolveStore({ "BV1xx-100": { lyrics: "[00:01.00]cached", offset: 800 } });
    await pending;

    expect(useLyricsStore.getState().offset).toBe(2500);
    expect(useLyricsStore.getState().lyrics).toEqual([{ time: 1000, text: "cached" }]);
  });
});
