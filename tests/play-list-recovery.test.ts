import { addToast } from "@heroui/react";
import { beforeEach, describe, expect, test, vi } from "vitest";

import { PlayMode } from "@/common/constants/audio";
import { getAudioUrl, getDashUrl, isUrlValid } from "@/common/utils/audio";
import { resetPlaybackAnchor, usePlaybackAnchor } from "@/common/utils/playback-anchor";
import * as platform from "@/platform";
import { audio, usePlayList, type PlayData } from "@/store/play-list";
import { usePlayProgress } from "@/store/play-progress";

vi.mock("@heroui/react", () => ({ addToast: vi.fn() }));
vi.mock("@/common/utils/play-report", () => ({
  beginPlayReport: vi.fn(),
  endPlayReport: vi.fn(),
  reportHeartbeat: vi.fn(),
}));
vi.mock("@/common/utils/audio", async () => {
  const actual = await vi.importActual<typeof import("@/common/utils/audio")>("@/common/utils/audio");
  return { ...actual, getAudioUrl: vi.fn(), getDashUrl: vi.fn() };
});
vi.mock("@/service/audio-song-info", () => ({ getAudioSongInfo: vi.fn() }));
vi.mock("@/service/web-interface-view", () => ({ getWebInterfaceView: vi.fn() }));

const url = (id: string) => `https://cdn.test/${id}.m4s?deadline=4102444800`;
const item = (id: string, sid = 1): PlayData => ({ id, type: "audio", sid, title: id, audioUrl: url(id) });
const flush = async () => {
  for (let i = 0; i < 30; i++) await Promise.resolve();
};
function deferred<T>() {
  let resolve!: (value: T) => void;
  let reject!: (reason: unknown) => void;
  const promise = new Promise<T>((res, rej) => {
    resolve = res;
    reject = rej;
  });
  return { promise, resolve, reject };
}
async function seed(items = [item("a"), item("b", 2), item("c", 3)]) {
  usePlayList.setState({ list: items, playId: items[0].id });
  await flush();
  audio.pause();
}
const mediaError = (code = 2) => {
  Object.defineProperty(audio, "error", { configurable: true, value: { code, message: "late error" } });
  audio.onerror?.call(audio, new Event("error"));
};
const metadataTitle = () => (navigator.mediaSession.metadata as unknown as { data: { title: string } }).data.title;

beforeEach(async () => {
  window.location.hash = "";
  Object.assign(window, {
    __TAURI__: {
      core: {
        invoke: vi.fn(async (cmd: string) => (cmd === "media_base" ? "http://127.0.0.1:9999/test-token/" : false)),
      },
    },
  });
  usePlayList.getState().clear();
  usePlayList.setState({ playMode: PlayMode.Loop, duration: undefined });
  resetPlaybackAnchor();
  audio.currentTime = 0;
  Reflect.deleteProperty(audio, "error");
  vi.mocked(getAudioUrl).mockImplementation(async sid => ({
    audioUrl: url(String(sid)),
    isLossless: false,
    audioCodecs: "",
  }));
  vi.mocked(getDashUrl).mockImplementation(async () => ({
    audioUrl: url("mv"),
    isLossless: false,
    audioCodecs: "",
    audioBandwidth: undefined,
    videoUrl: undefined,
    videoResolution: "",
  }));
  await usePlayList.getState().init();
  await flush();
  vi.mocked(addToast).mockClear();
});

describe("playback recovery guards", () => {
  test("normal switch subscription discards an old URL response", async () => {
    const pending = deferred<Awaited<ReturnType<typeof getAudioUrl>>>();
    vi.mocked(getAudioUrl).mockReturnValueOnce(pending.promise);
    usePlayList.setState({ list: [{ ...item("a"), audioUrl: undefined }, item("b", 2)], playId: "a" });
    await flush();
    await usePlayList.getState().playListItem("b");
    await flush();
    pending.resolve({ audioUrl: url("old-a"), isLossless: false, audioCodecs: "" });
    await flush();
    expect(audio.src).toContain("b.m4s");
    expect(usePlayList.getState().getPlayItem()?.audioUrl).toBe(url("b"));
  });

  test("proxy URLs from a previous launch are rewrapped and use the inner deadline", async () => {
    const old = `http://127.0.0.1:1234/old-token/remote?u=${encodeURIComponent(url("old"))}`;
    expect(await platform.toMediaUrl(old)).toBe(
      `http://127.0.0.1:9999/test-token/remote?u=${encodeURIComponent(url("old"))}`,
    );
    expect(platform.unwrapMediaUrl(old)).toBe(url("old"));
    expect(isUrlValid(old)).toBe(true);
    expect(
      isUrlValid(
        `http://127.0.0.1:1234/old-token/remote?u=${encodeURIComponent("https://cdn.test/old.m4s?deadline=1")}`,
      ),
    ).toBe(false);
  });

  test.each(["#/mini-player", "#/desktop-lyrics"])("subwindow %s never plays or recovers", async hash => {
    await seed();
    const play = vi.spyOn(audio, "play");
    window.location.hash = hash;
    try {
      await usePlayList.getState().togglePlay();
      mediaError();
      await flush();
      expect(play).not.toHaveBeenCalled();
      expect(addToast).not.toHaveBeenCalled();
    } finally {
      window.location.hash = "";
    }
  });
});

describe("B01 shared playback generation", () => {
  test.each(["resolve", "reject"])("resume %s must not overwrite or stop the newer track", async outcome => {
    await seed();
    usePlayList.setState(s => {
      s.list[0].audioUrl = undefined;
    });
    const pending = deferred<Awaited<ReturnType<typeof getAudioUrl>>>();
    vi.mocked(getAudioUrl).mockReturnValueOnce(pending.promise);
    const resume = usePlayList.getState().togglePlay();
    await flush();
    await usePlayList.getState().playListItem("b");
    await flush();
    if (outcome === "resolve") pending.resolve({ audioUrl: url("a-refreshed"), isLossless: false, audioCodecs: "" });
    else pending.reject(new DOMException("old load cancelled", "AbortError"));
    await resume;
    await flush();
    expect(audio.src).toContain("b.m4s");
    expect(usePlayList.getState().getPlayItem()?.audioUrl).toBe(url("b"));
    expect(metadataTitle()).toBe("b");
    expect(usePlayList.getState().isPlaying).toBe(true);
    expect(addToast).not.toHaveBeenCalled();
  });

  test("NotSupported refresh must not overwrite or replay after a newer switch", async () => {
    await seed();
    const pending = deferred<Awaited<ReturnType<typeof getAudioUrl>>>();
    vi.mocked(getAudioUrl).mockReturnValueOnce(pending.promise);
    const play = vi.spyOn(audio, "play").mockRejectedValueOnce(new DOMException("unsupported", "NotSupportedError"));
    const resume = usePlayList.getState().togglePlay();
    await flush();
    await usePlayList.getState().playListItem("b");
    await flush();
    const calls = play.mock.calls.length;
    pending.resolve({ audioUrl: url("a-retry"), isLossless: false, audioCodecs: "" });
    await resume;
    expect(audio.src).toContain("b.m4s");
    expect(play).toHaveBeenCalledTimes(calls);
    expect(usePlayList.getState().getPlayItem()?.audioUrl).toBe(url("b"));
  });

  test("startup restoration discards stale URL, position and metadata", async () => {
    await seed();
    usePlayList.setState(s => {
      s.list[0].audioUrl = undefined;
    });
    usePlayProgress.getState().setCurrentTime(80);
    usePlayProgress.getState().saveCurrentTime("a");
    const pending = deferred<Awaited<ReturnType<typeof getAudioUrl>>>();
    vi.mocked(getAudioUrl).mockReturnValueOnce(pending.promise);
    const init = usePlayList.getState().init();
    await flush();
    await usePlayList.getState().playListItem("b");
    await flush();
    pending.resolve({ audioUrl: url("a-startup"), isLossless: false, audioCodecs: "" });
    await init;
    expect(audio.src).toContain("b.m4s");
    expect(audio.currentTime).toBe(0);
    expect(metadataTitle()).toBe("b");
  });

  test("local URL mapping cannot write after switching tracks", async () => {
    await seed([{ ...item("a"), source: "local", audioUrl: "/music/a.mp3" }, item("b", 2)]);
    const pending = deferred<string>();
    vi.spyOn(platform, "toMediaUrl").mockReturnValueOnce(pending.promise);
    const resume = usePlayList.getState().togglePlay();
    await flush();
    await usePlayList.getState().playListItem("b");
    await flush();
    pending.resolve("http://127.0.0.1:9999/local-a");
    await resume;
    expect(audio.src).toContain("b.m4s");
    expect(metadataTitle()).toBe("b");
  });
});

describe("B02 bounded media error recovery", () => {
  test("old-source error during a new track load cannot start another refresh", async () => {
    await seed();
    const pending = deferred<Awaited<ReturnType<typeof getAudioUrl>>>();
    vi.mocked(getAudioUrl).mockReturnValue(pending.promise);
    usePlayList.setState(s => {
      s.list[1].audioUrl = undefined;
    });
    await usePlayList.getState().playListItem("b");
    await flush();
    mediaError(3);
    await flush();
    const refreshCalls = vi.mocked(getAudioUrl).mock.calls.length;
    pending.resolve({ audioUrl: url("b-new"), isLossless: false, audioCodecs: "" });
    await flush();
    expect(refreshCalls).toBe(1);
    expect(audio.src).toContain("b-new.m4s");
    expect(addToast).not.toHaveBeenCalled();
  });

  test.each([2, 3, 4])("late media error code %i freezes the anchor and refreshes once", async code => {
    await seed();
    await usePlayList.getState().togglePlay();
    audio.onplaying!.call(audio, new Event("playing"));
    vi.mocked(getAudioUrl).mockClear();
    mediaError(code);
    await flush();
    expect(typeof audio.onerror).toBe("function");
    expect(usePlaybackAnchor.getState().running).toBe(false);
    expect(getAudioUrl).toHaveBeenCalledTimes(1);
    expect(usePlayList.getState().isPlaying).toBe(true);
    expect(usePlayList.getState().playId).toBe("a");
  });

  test("error event and play rejection share one refresh and one replay", async () => {
    await seed();
    const first = deferred<void>();
    const refresh = deferred<Awaited<ReturnType<typeof getAudioUrl>>>();
    const play = vi.spyOn(audio, "play").mockReturnValueOnce(first.promise);
    vi.mocked(getAudioUrl).mockReturnValueOnce(refresh.promise);
    const resume = usePlayList.getState().togglePlay();
    await flush();
    mediaError(4);
    first.reject(new DOMException("unsupported", "NotSupportedError"));
    await flush();
    refresh.resolve({ audioUrl: url("recovered"), isLossless: false, audioCodecs: "" });
    await resume;
    await flush();
    expect(getAudioUrl).toHaveBeenCalledTimes(1);
    expect(play).toHaveBeenCalledTimes(2);
    expect(addToast).not.toHaveBeenCalled();
  });

  test("retry failure stops, freezes and toasts once without skipping the track", async () => {
    await seed();
    vi.spyOn(audio, "play").mockRejectedValue(new DOMException("unsupported", "NotSupportedError"));
    await usePlayList.getState().togglePlay();
    mediaError(4);
    mediaError(4);
    await flush();
    expect(getAudioUrl).toHaveBeenCalledTimes(1);
    expect(addToast).toHaveBeenCalledTimes(1);
    expect(audio.paused).toBe(true);
    expect(usePlayList.getState()).toMatchObject({ playId: "a", isPlaying: false });
    expect(usePlaybackAnchor.getState().running).toBe(false);
  });

  test("second late error cannot refresh again in the same generation", async () => {
    await seed();
    await usePlayList.getState().togglePlay();
    mediaError();
    await flush();
    audio.onplaying!.call(audio, new Event("playing"));
    mediaError(3);
    await flush();
    expect(getAudioUrl).toHaveBeenCalledTimes(1);
    expect(addToast).toHaveBeenCalledTimes(1);
    expect(usePlayList.getState()).toMatchObject({ playId: "a", isPlaying: false });
    expect(usePlaybackAnchor.getState().running).toBe(false);
  });

  test("missing refreshed URL stops with a toast", async () => {
    await seed();
    await usePlayList.getState().togglePlay();
    vi.mocked(getAudioUrl).mockResolvedValueOnce({ audioUrl: undefined, isLossless: false, audioCodecs: "" });
    mediaError();
    await flush();
    expect(addToast).toHaveBeenCalledTimes(1);
    expect(usePlayList.getState().isPlaying).toBe(false);
  });
});

describe("B03 playback intent and rejection state", () => {
  test.each(["NotAllowedError", "NotSupportedError", "AbortError"])(
    "rejected play (%s) leaves isPlaying=false",
    async name => {
      await seed();
      vi.spyOn(audio, "play").mockRejectedValue(new DOMException("The operation failed.", name));
      await usePlayList.getState().togglePlay();
      expect(usePlayList.getState().isPlaying).toBe(false);
      expect(usePlaybackAnchor.getState().running).toBe(false);
      if (name === "AbortError") expect(addToast).not.toHaveBeenCalled();
      if (name === "NotAllowedError")
        expect(addToast).toHaveBeenCalledWith(expect.objectContaining({ title: "播放需要用户交互，请再次点击播放" }));
    },
  );

  test("pressing pause during source resolution cancels playback and source writes", async () => {
    await seed();
    usePlayList.setState(s => {
      s.list[0].audioUrl = undefined;
    });
    const oldSource = audio.src;
    const pending = deferred<Awaited<ReturnType<typeof getAudioUrl>>>();
    vi.mocked(getAudioUrl).mockReturnValue(pending.promise);
    const firstClick = usePlayList.getState().togglePlay();
    await flush();
    await usePlayList.getState().togglePlay();
    pending.resolve({ audioUrl: url("a-new"), isLossless: false, audioCodecs: "" });
    await firstClick;
    expect(audio.paused).toBe(true);
    expect(audio.src).toBe(oldSource);
    expect(usePlayList.getState().isPlaying).toBe(false);
    expect(getAudioUrl).toHaveBeenCalledTimes(1);
  });

  test("switch-generated AbortError neither toasts nor stops the new track", async () => {
    await seed();
    const pending = deferred<void>();
    vi.spyOn(audio, "play").mockReturnValueOnce(pending.promise);
    const resume = usePlayList.getState().togglePlay();
    await flush();
    await usePlayList.getState().playListItem("b");
    await flush();
    pending.reject(new DOMException("aborted", "AbortError"));
    await resume;
    expect(addToast).not.toHaveBeenCalled();
    expect(usePlayList.getState()).toMatchObject({ playId: "b", isPlaying: true });
  });
});

describe("B04 queue references", () => {
  test.each(["del", "delPage"] as const)("%s clears deleted nextId", async action => {
    await seed();
    await usePlayList.getState().addToNext({ type: "audio", sid: 2, title: "b" });
    await usePlayList.getState()[action]("b");
    expect(usePlayList.getState().nextId).toBeUndefined();
    await usePlayList.getState().next();
    await flush();
    expect(usePlayList.getState().playId).toBe("c");
  });

  test("replacing the queue discards old nextId", async () => {
    await seed();
    await usePlayList.getState().addToNext({ type: "audio", sid: 2, title: "b" });
    await usePlayList.getState().playList([item("x", 10), item("y", 11)]);
    expect(usePlayList.getState().nextId).toBeUndefined();
    await usePlayList.getState().next();
    expect(usePlayList.getState().getPlayItem()?.title).toBe("y");
    await flush();
  });

  test("next rejects stale nextId and defends random mode against a missing current item", async () => {
    await seed();
    usePlayList.setState({ nextId: "missing" });
    await usePlayList.getState().next();
    expect(usePlayList.getState().playId).toBe("b");
    usePlayList.setState({ playId: "missing", playMode: PlayMode.Random });
    await usePlayList.getState().next();
    expect(usePlayList.getState().playId).toBe("a");
    await flush();
  });

  test("rehydration validates both persisted IDs", async () => {
    localStorage.setItem(
      "play-list-store",
      JSON.stringify({ state: { list: [item("a")], playId: "missing", nextId: "missing" }, version: 0 }),
    );
    await usePlayList.persist.rehydrate();
    expect(usePlayList.getState().playId).toBeUndefined();
    expect(usePlayList.getState().nextId).toBeUndefined();
    await flush();
  });
});

describe("B05 track-bound saved position", () => {
  test("saving zero overwrites the old position and clear deletes it", async () => {
    await seed();
    usePlayProgress.getState().setCurrentTime(120);
    usePlayProgress.getState().saveCurrentTime("a");
    expect(JSON.parse(localStorage.getItem("play-current-time")!)).toEqual({ playId: "a", currentTime: 120 });
    usePlayProgress.getState().setCurrentTime(0);
    usePlayProgress.getState().saveCurrentTime("a");
    expect(JSON.parse(localStorage.getItem("play-current-time")!)).toEqual({ playId: "a", currentTime: 0 });
    usePlayList.getState().clear();
    expect(localStorage.getItem("play-current-time")).toBeNull();
    usePlayProgress.getState().saveCurrentTime();
    await seed([item("new")]);
    await usePlayList.getState().init();
    expect(audio.currentTime).toBe(0);
  });

  test.each([
    "120",
    "invalid json",
    '{"playId":"b","currentTime":80}',
    '{"playId":"a","currentTime":-1}',
    '{"playId":"a","currentTime":"12"}',
    '{"playId":"a","currentTime":1e999}',
  ])("rejects old or invalid position %s", async saved => {
    await seed();
    localStorage.setItem("play-current-time", saved);
    await usePlayList.getState().init();
    expect(audio.currentTime).toBe(0);
    expect(usePlayProgress.getState().currentTime).toBe(0);
    expect(localStorage.getItem("play-current-time")).toBeNull();
  });

  test("startup restores a valid position for the same track", async () => {
    await seed();
    localStorage.setItem("play-current-time", JSON.stringify({ playId: "a", currentTime: 42 }));
    await usePlayList.getState().init();
    expect(audio.currentTime).toBe(42);
    expect(audio.paused).toBe(true);
  });
});

describe("B06 insert existing item after current", () => {
  test.each([
    { sid: 4, expected: ["a", "b", "d", "c"] },
    { sid: 1, expected: ["b", "a", "c", "d"] },
    { sid: 3, expected: ["a", "b", "c", "d"] },
  ])("moves sid=$sid using the remaining current index", async ({ sid, expected }) => {
    await seed([item("a"), item("b", 2), item("c", 3), item("d", 4)]);
    await usePlayList.getState().playListItem("b");
    await flush();
    await usePlayList.getState().addToNext({ type: "audio", sid, title: "next" });
    expect(usePlayList.getState().list.map(x => x.id)).toEqual(expected);
  });
});

describe("B07 Now Playing metadata", () => {
  test.each(["online", "local"] as const)("switching to %s cached URL updates OS metadata", async source => {
    await seed([item("a"), { ...item("b", 2), source }]);
    navigator.mediaSession.metadata = new MediaMetadata({ title: "previous track" });
    await usePlayList.getState().playListItem("b");
    await flush();
    expect(metadataTitle()).toBe("b");
  });
});

describe("B08 source recovery position", () => {
  test("NotSupported refresh preserves seek position across src reset", async () => {
    await seed();
    audio.currentTime = 90;
    usePlayProgress.getState().setCurrentTime(90);
    vi.spyOn(audio, "play").mockRejectedValueOnce(new DOMException("unsupported", "NotSupportedError"));
    let src = audio.src;
    const descriptor = Object.getOwnPropertyDescriptor(audio, "src");
    Object.defineProperty(audio, "src", {
      configurable: true,
      get: () => src,
      set: value => {
        src = value;
        audio.currentTime = 0;
      },
    });
    try {
      await usePlayList.getState().togglePlay();
      expect(audio.currentTime).toBe(90);
      expect(usePlayProgress.getState().currentTime).toBe(90);
    } finally {
      if (descriptor) Object.defineProperty(audio, "src", descriptor);
      else Reflect.deleteProperty(audio, "src");
    }
  });

  test("metadata-ready restoration clamps the captured position to duration", async () => {
    await seed();
    audio.currentTime = 90;
    usePlayProgress.getState().setCurrentTime(90);
    const readyState = Object.getOwnPropertyDescriptor(audio, "readyState");
    const load = vi.spyOn(audio, "load").mockImplementation(() => {
      audio.currentTime = 0;
    });
    Object.defineProperty(audio, "readyState", { configurable: true, value: 0 });
    vi.spyOn(audio, "play").mockRejectedValueOnce(new DOMException("unsupported", "NotSupportedError"));
    try {
      await usePlayList.getState().togglePlay();
      audio.ontimeupdate!.call(audio, new Event("timeupdate"));
      expect(usePlayProgress.getState().currentTime).toBe(90);
      Object.defineProperty(audio, "readyState", { configurable: true, value: 1 });
      Object.defineProperty(audio, "duration", { configurable: true, writable: true, value: 60 });
      audio.onloadedmetadata!.call(audio, new Event("loadedmetadata"));
      expect(audio.currentTime).toBe(60);
      expect(usePlayProgress.getState().currentTime).toBe(60);
    } finally {
      if (readyState) Object.defineProperty(audio, "readyState", readyState);
      else Reflect.deleteProperty(audio, "readyState");
      load.mockRestore();
    }
  });
});

describe("forceSilentMode compatibility", () => {
  test("initial playback and recovery remain muted in silent mode", async () => {
    await seed();
    Object.assign(window, {
      __TAURI__: {
        core: {
          invoke: vi.fn(async (cmd: string) => cmd === "is_silent_mode"),
        },
      },
    });
    await usePlayList.getState().init();
    await flush();
    expect(audio.muted).toBe(true);
    const originalPlay = audio.play.bind(audio);
    const mutedAtPlay: boolean[] = [];
    vi.spyOn(audio, "play").mockImplementation(async () => {
      mutedAtPlay.push(audio.muted);
      if (mutedAtPlay.length === 1) throw new DOMException("unsupported", "NotSupportedError");
      await originalPlay();
    });
    await usePlayList.getState().togglePlay();
    expect(mutedAtPlay).toEqual([true, true]);
    usePlayList.getState().toggleMute();
    expect(audio.muted).toBe(true);
    audio.muted = false;
    audio.onvolumechange!.call(audio, new Event("volumechange"));
    expect(audio.muted).toBe(true);
  });
});
