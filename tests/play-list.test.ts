import { describe, expect, test, beforeEach, vi } from "vitest";

import { PlayMode } from "@/common/constants/audio";
import { anchorNow, markPlaybackAnchor, resetPlaybackAnchor, usePlaybackAnchor } from "@/common/utils/playback-anchor";
import { usePlayList } from "@/store/play-list";

vi.mock("@/common/utils/audio", () => ({
  getAudioUrl: vi.fn(async () => ({ audioUrl: "https://audio.test/a.mp3", isLossless: false })),
  getDashUrl: vi.fn(async () => ({
    audioUrl: "https://video.test/a.mp3",
    videoUrl: "https://video.test/v.mp4",
    isLossless: false,
  })),
  getMVUrl: vi.fn(async () => ({
    audioUrl: "https://video.test/a.mp3",
    videoUrl: "https://video.test/v.mp4",
    isLossless: false,
  })),
  isUrlValid: vi.fn(url => typeof url === "string" && url.length > 0),
}));

vi.mock("@/service/audio-song-info", () => ({
  getAudioSongInfo: vi.fn(async ({ sid }) => ({
    data: {
      id: sid,
      uid: 1,
      uname: "owner",
      author: "owner",
      title: "audio-title",
      cover: "https://cover.test/c.png",
      intro: "",
      crtype: 1,
      duration: 123,
      passtime: Date.now(),
      curtime: Date.now(),
      aid: 0,
    },
  })),
}));

vi.mock("@/service/web-interface-view", () => ({
  getWebInterfaceView: vi.fn(async () => ({
    data: {
      aid: 100,
      title: "mv-title",
      pic: "https://cover.test/m.png",
      owner: { name: "owner", mid: 1 },
      pages: [
        { cid: 11, page: 1, part: "p1", duration: 60, first_frame: "https://ff.test/1.png" },
        { cid: 12, page: 2, part: "p2", duration: 60, first_frame: "https://ff.test/2.png" },
      ],
    },
  })),
}));

vi.mock("@heroui/react", async () => {
  const actual: any = await vi.importActual("@heroui/react");
  return { ...actual, addToast: vi.fn() };
});

beforeEach(() => {
  vi.clearAllMocks();
  usePlayList.getState().clear();
  resetPlaybackAnchor();
});

describe("play-list store", () => {
  test("initial state", () => {
    const s = usePlayList.getState();
    expect(s.isPlaying).toBe(false);
    expect(s.isMuted).toBe(false);
    expect(s.volume).toBe(0.5);
    expect(s.playMode).toBe(PlayMode.Loop);
    expect(s.rate).toBe(1);
    expect(s.list.length).toBe(0);
  });

  test("init sets audio props and handlers", async () => {
    const s = usePlayList.getState();
    await s.init();
    const audio = s.getAudio();
    expect(audio.volume).toBe(0.5);
    expect(audio.muted).toBe(false);
    expect(audio.playbackRate).toBe(1);
    expect(typeof audio.onplay).toBe("function");
    expect(typeof audio.onplaying).toBe("function");
    expect(typeof audio.onwaiting).toBe("function");
    expect(typeof audio.onseeking).toBe("function");
    expect(typeof audio.onloadstart).toBe("function");
    const seq = usePlaybackAnchor.getState().seq;
    audio.onseeking!.call(audio, new Event("seeking"));
    expect(usePlaybackAnchor.getState()).toMatchObject({ seq: seq + 1, running: false });
  });

  test("sub windows never start playback", async () => {
    const s = usePlayList.getState();
    await s.init();
    const audio = s.getAudio();
    const play = vi.spyOn(audio, "play").mockResolvedValue(undefined);
    usePlayList.setState({
      list: [{ id: "a", type: "audio", sid: 1, title: "t", audioUrl: "https://audio.test/a.mp3" }],
      playId: "a",
    } as any);
    const originalHash = window.location.hash;
    try {
      window.location.hash = "#/desktop-lyrics";
      await usePlayList.getState().togglePlay();
      expect(play).not.toHaveBeenCalled();

      window.location.hash = "";
      await usePlayList.getState().togglePlay();
      expect(play).toHaveBeenCalledTimes(1);
    } finally {
      window.location.hash = originalHash;
      play.mockRestore();
    }
  });

  test("switching tracks clears the previous track's duration until the new one loads", async () => {
    const s = usePlayList.getState();
    await s.init();
    const play = vi.spyOn(s.getAudio(), "play").mockResolvedValue(undefined);
    usePlayList.setState({
      list: [
        { id: "a", type: "audio", sid: 1, title: "a", audioUrl: "https://audio.test/a.mp3" },
        { id: "b", type: "audio", sid: 2, title: "b", audioUrl: "https://audio.test/b.mp3" },
      ],
      playId: "a",
      duration: 220,
    } as any);
    try {
      usePlayList.setState({ playId: "b" });
      expect(usePlayList.getState().duration).toBeUndefined();
    } finally {
      play.mockRestore();
    }
  });

  test("anchors media events but leaves timeupdate and onplay unanchored", async () => {
    const s = usePlayList.getState();
    await s.init();
    const audio = s.getAudio();
    audio.currentTime = 12.345;
    const seq = usePlaybackAnchor.getState().seq;
    audio.onplay!.call(audio, new Event("play"));
    audio.ontimeupdate!.call(audio, new Event("timeupdate"));
    expect(usePlaybackAnchor.getState().seq).toBe(seq);

    audio.onplaying!.call(audio, new Event("playing"));
    expect(usePlaybackAnchor.getState()).toMatchObject({ mediaMs: 12345, running: true, seq: seq + 1 });
    audio.onwaiting!.call(audio, new Event("waiting"));
    expect(usePlaybackAnchor.getState()).toMatchObject({ running: false, seq: seq + 2 });
    audio.playbackRate = 1.5;
    audio.onratechange!.call(audio, new Event("ratechange"));
    expect(usePlaybackAnchor.getState()).toMatchObject({ rate: 1.5, running: false, seq: seq + 3 });
    audio.onseeked!.call(audio, new Event("seeked"));
    expect(usePlaybackAnchor.getState()).toMatchObject({
      running: !audio.paused && audio.readyState >= 3,
      seq: seq + 4,
    });
    audio.onpause!.call(audio, new Event("pause"));
    expect(usePlaybackAnchor.getState()).toMatchObject({ running: false, seq: seq + 5 });
    audio.onloadstart!.call(audio, new Event("loadstart"));
    expect(usePlaybackAnchor.getState()).toMatchObject({ mediaMs: 0, running: false, seq: seq + 6 });
    audio.playbackRate = 1;
  });

  test.each([
    { driftMs: 100, reanchored: false },
    { driftMs: -100, reanchored: false },
    { driftMs: 300, reanchored: true },
    { driftMs: -300, reanchored: true },
  ])("timeupdate re-anchors only when drift exceeds the limit ($driftMs ms)", async ({ driftMs, reanchored }) => {
    const s = usePlayList.getState();
    await s.init();
    const audio = s.getAudio();
    const paused = Object.getOwnPropertyDescriptor(audio, "paused");
    try {
      Object.defineProperty(audio, "paused", { configurable: true, value: false });
      audio.currentTime = 10;
      markPlaybackAnchor(audio, true, anchorNow() - 5000);
      // the extrapolated position is about 15000ms; the media clock reports 15000 - drift
      audio.currentTime = (15000 - driftMs) / 1000;
      const seq = usePlaybackAnchor.getState().seq;
      audio.ontimeupdate!.call(audio, new Event("timeupdate"));
      const anchor = usePlaybackAnchor.getState();
      expect(anchor.seq).toBe(reanchored ? seq + 1 : seq);
      if (reanchored) expect(anchor).toMatchObject({ mediaMs: 15000 - driftMs, running: true });
    } finally {
      if (paused) Object.defineProperty(audio, "paused", paused);
      else Reflect.deleteProperty(audio, "paused");
    }
  });

  test.each([
    { paused: false, readyState: 0, running: false },
    { paused: false, readyState: 2, running: false },
    { paused: false, readyState: 3, running: true },
    { paused: false, readyState: 4, running: true },
    { paused: true, readyState: 3, running: false },
  ])("seeked anchors paused=$paused readyState=$readyState as running=$running", async state => {
    const s = usePlayList.getState();
    await s.init();
    const audio = s.getAudio();
    const paused = Object.getOwnPropertyDescriptor(audio, "paused");
    const readyState = Object.getOwnPropertyDescriptor(audio, "readyState");
    try {
      Object.defineProperties(audio, {
        paused: { configurable: true, value: state.paused },
        readyState: { configurable: true, value: state.readyState },
      });
      const seq = usePlaybackAnchor.getState().seq;
      audio.onseeked!.call(audio, new Event("seeked"));
      expect(usePlaybackAnchor.getState()).toMatchObject({ running: state.running, seq: seq + 1 });
    } finally {
      if (paused) Object.defineProperty(audio, "paused", paused);
      else Reflect.deleteProperty(audio, "paused");
      if (readyState) Object.defineProperty(audio, "readyState", readyState);
      else Reflect.deleteProperty(audio, "readyState");
    }
  });

  test("setVolume, setRate, setPlayMode", async () => {
    const s = usePlayList.getState();
    await s.init();
    s.setVolume(0.8);
    s.setRate(1.25);
    s.togglePlayMode();
    s.togglePlayMode();
    const audio = s.getAudio();
    expect(usePlayList.getState().volume).toBe(0.8);
    expect(audio.volume).toBe(0.8);
    expect(usePlayList.getState().rate).toBe(1.25);
    expect(audio.playbackRate).toBe(1.25);
    expect(usePlayList.getState().playMode).toBe(PlayMode.Single);
    expect(audio.loop).toBe(true);
  });

  test("play audio adds item and toggles playing", async () => {
    const s = usePlayList.getState();
    await s.init();
    await s.play({ type: "audio", sid: 101, title: "a", cover: "", ownerName: "", ownerMid: 0 });
    expect(usePlayList.getState().list.length).toBe(1);
    const id = usePlayList.getState().playId as string;
    expect(typeof id).toBe("string");
    const audio = s.getAudio();
    expect(audio.src).toContain("audio.test");
    expect(navigator.mediaSession.playbackState).toBe("playing");
  });

  test("playList sets list and next/prev in sequence", async () => {
    const s = usePlayList.getState();
    await s.init();
    await s.playList([
      { type: "audio", sid: 1, title: "a1" },
      { type: "audio", sid: 2, title: "a2" },
    ]);
    const firstId = usePlayList.getState().playId as string;
    await s.next();
    const secondId = usePlayList.getState().playId as string;
    expect(secondId).not.toBe(firstId);
    await s.prev();
    expect(usePlayList.getState().playId).toBe(firstId);
  });

  test("random mode keeps pages order", async () => {
    const s = usePlayList.getState();
    await s.init();
    await s.playList([{ type: "mv", bvid: "BVx", title: "m1" }]);
    const mv = usePlayList.getState().list[0];
    const { getWebInterfaceView } = await import("@/service/web-interface-view");
    const pages = await getWebInterfaceView({ bvid: mv.bvid as string });
    usePlayList.setState(() => ({
      list: pages.data.pages.map(p => ({
        id: `${p.page}-id`,
        type: "mv",
        bvid: mv.bvid,
        aid: "100",
        cid: String(p.cid),
        title: "mv-title",
        cover: "",
        ownerName: "owner",
        ownerMid: 1,
        hasMultiPart: true,
        pageIndex: p.page,
        pageTitle: p.part,
        pageCover: p.first_frame,
        totalPage: pages.data.pages.length,
        duration: p.duration,
      })),
      playId: "1-id",
    }));
    s.togglePlayMode();
    s.setShouldKeepPagesOrderInRandomPlayMode(true);
    await s.next();
    expect(usePlayList.getState().playId).toBe("2-id");
  });

  test("addToNext inserts after current", async () => {
    const s = usePlayList.getState();
    await s.init();
    await s.playList([{ type: "audio", sid: 10, title: "a10" }]);
    const currentId = usePlayList.getState().playId as string;
    await s.addToNext({ type: "audio", sid: 20, title: "a20" });
    const idx = usePlayList.getState().list.findIndex(i => i.id === currentId);
    const nextItem = usePlayList.getState().list[idx + 1];
    expect(usePlayList.getState().nextId).toBe(nextItem.id);
    expect(nextItem.sid).toBe(20);
  });

  test("addList deduplicates and preserves playing item", async () => {
    const s = usePlayList.getState();
    await s.init();
    await s.playList([{ type: "audio", sid: 1, title: "a1" }]);
    await s.addList([
      { type: "audio", sid: 1, title: "a1" },
      { type: "audio", sid: 3, title: "a3" },
    ]);
    expect(usePlayList.getState().list.some(i => i.sid === 1)).toBe(true);
    expect(usePlayList.getState().list.some(i => i.sid === 3)).toBe(true);
    const newId = usePlayList.getState().playId as string;
    const newItem = usePlayList.getState().list.find(i => i.id === newId);
    expect(newItem?.sid).toBe(1);
  });

  test("del removes by id and clear works", async () => {
    const s = usePlayList.getState();
    await s.init();
    await s.playList([
      { type: "audio", sid: 1, title: "a1" },
      { type: "audio", sid: 2, title: "a2" },
    ]);
    const otherId = usePlayList.getState().list.find(i => i.sid === 2)?.id as string;
    await s.del(otherId);
    expect(usePlayList.getState().list.some(i => i.id === otherId)).toBe(false);
    s.clear();
    expect(usePlayList.getState().list.length).toBe(0);
    expect(usePlayList.getState().playId).toBeUndefined();
  });

  test("play handles data fetch failure gracefully", async () => {
    const s = usePlayList.getState();
    await s.init();
    // Mock getWebInterfaceView to return empty/error structure
    const { getWebInterfaceView } = await import("@/service/web-interface-view");
    vi.mocked(getWebInterfaceView).mockResolvedValueOnce({ code: -1 } as any);

    // This should not crash
    await s.play({ type: "mv", bvid: "BV_fail", title: "fail" });
    expect(usePlayList.getState().list.length).toBe(0);
  });

  test("addToNext handles data fetch failure gracefully", async () => {
    const s = usePlayList.getState();
    await s.init();
    await s.playList([{ type: "audio", sid: 1, title: "a1" }]);

    const { getWebInterfaceView } = await import("@/service/web-interface-view");
    vi.mocked(getWebInterfaceView).mockResolvedValueOnce({ code: -1 } as any);

    await s.addToNext({ type: "mv", bvid: "BV_fail", title: "fail" });
    expect(usePlayList.getState().list.length).toBe(1);
  });
});
