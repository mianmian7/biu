// @vitest-environment node
import { describe, expect, it, vi } from "vitest";

import type { AutoLyricsDependencies } from "../../electron/auto-lyrics/service";
import type { ReferenceCandidate } from "../../shared/auto-lyrics/reference";
import type { AutoLyricsParams, AutoLyricsProgress } from "../../shared/auto-lyrics/service-types";

import { DecodeUnavailableError } from "../../electron/auto-lyrics/ffmpeg-resolve";
import { AUTO_LYRICS_ALGO_VERSION, AutoLyricsService } from "../../electron/auto-lyrics/service";

const params: AutoLyricsParams = {
  bvid: "BVcurrent",
  cid: 1,
  title: "DECORATOR",
  duration: 5,
  tid: 3,
  ownerName: "Artist",
};
const song = { id: 42, name: "DECORATOR", artists: ["Artist"], duration: 5 };
const reference = { bvid: "BVreference", cid: 2, title: "DECORATOR 原曲 CD", duration: 5 };

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(done => {
    resolve = done;
  });
  return { promise, resolve };
}

function setup() {
  const entries = new Map<string, MusicLyrics>();
  const references = new Map<number, ReferenceCandidate>();
  const pcm = Float32Array.from({ length: 22050 * 5 }, (_, index) =>
    index < 22050 * 2 ? 0 : 0.4 * Math.sin(index * 0.13),
  );
  const dependencies = {
    sources: {
      search: vi.fn<AutoLyricsDependencies["sources"]["search"]>(async () => [song]),
      lyric: vi.fn(async () => ({
        lrc: "[00:02.00]hello\n[00:03.00]world",
        tlyric: "[00:02.00]你好",
        romalrc: "[00:02.00]hello",
      })),
      references: vi.fn(async () => [reference]),
      audio: vi.fn<AutoLyricsDependencies["sources"]["audio"]>(async () => new Uint8Array([1])),
    },
    decode: vi.fn(async () => pcm),
    align: vi.fn<AutoLyricsDependencies["align"]>(async (input, _signal, onCoarse) => {
      const coarse = { offsetSec: 1, confidence: 0.9 };
      onCoarse(coarse);
      return {
        coarse,
        referenceIndex: 0,
        referenceConsensus: true,
        referenceLags: [],
        lines: input.lines.map(line => ({
          idx: line.idx,
          text: line.text,
          studioStart: line.studio_start,
          liveStartSec: line.studio_start + 1,
          confidence: 0.9,
          uncertain: false,
        })),
      };
    }),
    cache: {
      get: (key: string) => entries.get(key),
      set: vi.fn((key: string, value: MusicLyrics) => {
        entries.set(key, value);
      }),
    },
    references: {
      get: (id: number) => references.get(id),
      set: vi.fn((id: number, value: ReferenceCandidate) => {
        references.set(id, value);
      }),
    },
  } satisfies AutoLyricsDependencies;
  return { dependencies, entries, pcm, service: new AutoLyricsService(dependencies) };
}

/** Two same-title songs by different artists; each one's reference decodes to a distinguishable length. */
/** Two contenders sharing a name; they are different songs (different lyrics) unless `sameLyrics`. */
function arbitration(scores: Record<number, number>, sameLyrics = false) {
  const context = setup();
  const { dependencies, pcm } = context;
  const cover = { ...song, id: 43, artists: ["Other"] };
  if (!sameLyrics)
    dependencies.sources.lyric.mockImplementation((async (id: number) => ({
      lrc: id === cover.id ? "[00:02.00]other\n[00:03.00]song" : "[00:02.00]hello\n[00:03.00]world",
    })) as never);
  const lengths = new Map([
    [song.id, pcm.length],
    [cover.id, pcm.length + 1],
  ]);
  dependencies.sources.search.mockResolvedValue([song, cover]);
  dependencies.sources.references.mockImplementation((async (candidate: typeof song) => [
    { ...reference, bvid: `BVref${candidate.id}` },
  ]) as never);
  dependencies.sources.audio.mockImplementation(
    (async (bvid: string) => new Uint8Array([bvid === `BVref${cover.id}` ? 2 : 1])) as never,
  );
  dependencies.decode.mockImplementation((async (bytes: Uint8Array) =>
    bytes[0] === 2 ? Float32Array.from([...pcm, 0]) : pcm) as never);
  dependencies.align.mockImplementation(async (input, _signal, onCoarse) => {
    const id = [...lengths].find(([, length]) => length === input.studios[0].length)?.[0] ?? 0;
    const coarse = { offsetSec: 1, confidence: 0.9 };
    onCoarse(coarse);
    return {
      coarse,
      referenceIndex: 0,
      referenceConsensus: true,
      referenceLags: [],
      lines: input.lines.map(line => ({
        idx: line.idx,
        text: line.text,
        studioStart: line.studio_start,
        liveStartSec: line.studio_start + 1,
        confidence: scores[id] ?? 0,
        uncertain: (scores[id] ?? 0) < 0.65,
      })),
    };
  });
  const events: AutoLyricsProgress[] = [];
  context.service.onProgress(event => events.push(event));
  return { ...context, cover, events };
}

describe("AutoLyricsService", () => {
  it.each([
    [0.44, "no-reference"],
    [0.5, "matched"],
  ] as const)("gates single-song alignment score %s as %s", async (score, status) => {
    const { service, dependencies } = setup();
    const align = dependencies.align.getMockImplementation()!;
    dependencies.align.mockImplementation(async (...args) => {
      const aligned = await align(...args);
      return { ...aligned, lines: aligned.lines.map(line => ({ ...line, confidence: score, uncertain: false })) };
    });
    const result = await service.start(params);
    expect(result.status).toBe(status);
    expect(result.verifyScore).toBeCloseTo(score);
    if (status === "no-reference") {
      expect(result).toMatchObject({
        error: "Line alignment score too low",
        coarse: { offsetSec: 1 },
        referenceCheck: { chosen: 0 },
      });
      expect(result.lyrics).toBeUndefined();
      expect(dependencies.cache.set).not.toHaveBeenCalled();
      expect(dependencies.references.set).not.toHaveBeenCalled();
      await service.start({ ...params, cid: 3 });
      expect(dependencies.sources.references).toHaveBeenCalledTimes(2);
    } else {
      expect(dependencies.cache.set).toHaveBeenCalledTimes(1);
      expect(dependencies.references.set).toHaveBeenCalledTimes(1);
    }
  });

  it("aligns three ranked references and saves the selected reference as checked", async () => {
    const { service, dependencies, pcm } = setup();
    const candidates = ["BVref0", "BVref1", "BVref2"].map(bvid => ({ ...reference, bvid }));
    dependencies.sources.references.mockResolvedValue(candidates);
    const originalAlign = dependencies.align.getMockImplementation()!;
    dependencies.align.mockImplementation(async (...args) => ({
      ...(await originalAlign(...args)),
      referenceIndex: 1,
    }));
    const result = await service.start(params);
    expect(dependencies.align.mock.calls[0][0].studios).toHaveLength(3);
    expect(dependencies.align.mock.calls[0][0].studios.every(studio => studio === pcm)).toBe(true);
    expect(result).toMatchObject({
      status: "matched",
      reference: { bvid: "BVref1" },
      referenceCheck: { candidates: candidates.map(candidate => candidate.bvid), chosen: 1, consensus: true, lags: [] },
    });
    expect(result.lyrics?.refBvid).toBe("BVref1");
    expect(dependencies.references.set).toHaveBeenCalledWith(song.id, { ...candidates[1], score: 85, checked: true });
  });

  it("ignores saved references without a check", async () => {
    const { service, dependencies } = setup();
    dependencies.references.set(song.id, { ...reference, bvid: "BVold" });
    expect((await service.start(params)).status).toBe("matched");
    expect(dependencies.sources.references).toHaveBeenCalledTimes(1);
    expect(dependencies.sources.audio.mock.calls.flat()).not.toContain("BVold");
  });

  it("downloads candidates concurrently and retains rank order after one fails", async () => {
    const { service, dependencies, pcm } = setup();
    const downloads = [deferred<Uint8Array>(), deferred<Uint8Array>(), deferred<Uint8Array>()];
    dependencies.sources.references.mockResolvedValue(
      downloads.map((_, index) => ({ ...reference, bvid: `BVref${index}` })),
    );
    dependencies.sources.audio.mockImplementation(async (...args: unknown[]) => {
      const index = downloads.findIndex((_, index) => args[0] === `BVref${index}`);
      return index < 0 ? new Uint8Array([1]) : downloads[index].promise;
    });
    const second = Float32Array.from([...pcm, 0]);
    dependencies.decode.mockImplementation(async (...args: unknown[]) => {
      const bytes = args[0];
      if (bytes instanceof Uint8Array && bytes[0] === 0) throw new Error("download corrupt");
      return bytes instanceof Uint8Array && bytes[0] === 2 ? second : pcm;
    });
    const task = service.start(params);
    await vi.waitFor(() => expect(dependencies.sources.audio).toHaveBeenCalledTimes(4));
    downloads[2].resolve(new Uint8Array([2]));
    downloads[0].resolve(new Uint8Array([0]));
    downloads[1].resolve(new Uint8Array([1]));
    const result = await task;
    expect(result).toMatchObject({ status: "matched", referenceCheck: { candidates: ["BVref1", "BVref2"] } });
    const studios = dependencies.align.mock.calls[0][0].studios;
    expect(studios).toHaveLength(2);
    expect(studios[0]).toBe(pcm);
    expect(studios[1]).toBe(second);
  });

  it("falls back to search when the saved checked reference fails", async () => {
    const { service, dependencies, pcm } = setup();
    dependencies.references.set(song.id, { ...reference, bvid: "BVsaved", checked: true });
    dependencies.sources.audio.mockImplementation(
      async (...args: unknown[]) => new Uint8Array([args[0] === "BVsaved" ? 0 : 1]),
    );
    dependencies.decode.mockImplementation(async (...args: unknown[]) => {
      const bytes = args[0];
      return bytes instanceof Uint8Array && bytes[0] === 0 ? new Float32Array(pcm.length) : pcm;
    });
    expect(await service.start(params)).toMatchObject({
      status: "matched",
      reference: { bvid: reference.bvid, checked: true },
    });
    expect(dependencies.sources.references).toHaveBeenCalledTimes(1);
  });

  it("downloads only the saved checked reference", async () => {
    const { service, dependencies } = setup();
    dependencies.references.set(song.id, { ...reference, bvid: "BVsaved", checked: true });
    expect((await service.start(params)).status).toBe("matched");
    expect(dependencies.sources.references).not.toHaveBeenCalled();
    expect(dependencies.sources.audio).toHaveBeenCalledTimes(2);
    expect(dependencies.sources.audio).toHaveBeenCalledWith("BVsaved", reference.cid, expect.any(AbortSignal));
    expect(dependencies.align.mock.calls[0][0].studios).toHaveLength(1);
  });

  it("ignores a saved checked reference whose title does not name the song", async () => {
    const { service, dependencies } = setup();
    dependencies.references.set(song.id, { ...reference, bvid: "BVsaved", title: "声なき魚 CD", checked: true });
    expect((await service.start(params)).status).toBe("matched");
    expect(dependencies.sources.references).toHaveBeenCalledTimes(1);
    expect(dependencies.sources.audio).not.toHaveBeenCalledWith("BVsaved", reference.cid, expect.any(AbortSignal));
  });

  it("returns a valid automatic cache without fetching", async () => {
    const { service, entries, dependencies } = setup();
    entries.set("BVcurrent-1", { source: "auto", lyrics: "cached", algoVersion: AUTO_LYRICS_ALGO_VERSION });
    expect(await service.start(params)).toMatchObject({ status: "cached", lyrics: { lyrics: "cached" } });
    expect(dependencies.sources.search).not.toHaveBeenCalled();
  });

  it.each([
    { lyrics: "legacy" },
    { source: "manual", lyrics: "manual" },
    { source: "auto", offset: 500, lyrics: "offset" },
  ] satisfies MusicLyrics[])("preserves user lyrics even when bypassing cache: %j", async entry => {
    const { service, entries, dependencies } = setup();
    entries.set("BVcurrent-1", entry);
    expect((await service.start({ ...params, bypassCache: true })).status).toBe("cached");
    expect(entries.get("BVcurrent-1")).toBe(entry);
    expect(dependencies.cache.set).not.toHaveBeenCalled();
  });

  it("reuses one promise for the same running key", async () => {
    const { service, dependencies } = setup();
    const search = deferred<(typeof song)[]>();
    dependencies.sources.search.mockReturnValue(search.promise);
    const first = service.start(params);
    const second = service.start(params);
    expect(first).toBe(second);
    search.resolve([song]);
    expect((await first).status).toBe("matched");
    expect(dependencies.sources.audio).toHaveBeenCalledTimes(2);
  });

  it("emits coarse before fine and caches aligned translations and romanization", async () => {
    const { service, entries } = setup();
    const phases: string[] = [];
    service.onProgress(event => phases.push(event.phase));
    const result = await service.start(params);
    expect(result.status).toBe("matched");
    expect(phases.indexOf("coarse")).toBeLessThan(phases.indexOf("fine"));
    expect(entries.get("BVcurrent-1")).toMatchObject({
      source: "auto",
      songId: 42,
      refBvid: "BVreference",
      tLyrics: "[00:03.00]你好",
      rLyrics: "[00:03.00]hello",
    });
    expect(result.timings.coarse).toBeLessThanOrEqual(result.timings.fine);
  });

  it("does not overwrite a user edit made during alignment", async () => {
    const { service, entries } = setup();
    service.onProgress(event => {
      if (event.phase === "coarse") entries.set(event.key, { source: "manual", lyrics: "edited" });
    });
    expect((await service.start(params)).status).toBe("matched");
    expect(entries.get("BVcurrent-1")).toEqual({ source: "manual", lyrics: "edited" });
  });

  it("cancels the previous foreground on a new song", async () => {
    const { service, dependencies } = setup();
    const search = deferred<(typeof song)[]>();
    dependencies.sources.search.mockReturnValue(search.promise);
    const first = service.start(params);
    await Promise.resolve();
    const second = service.start({ ...params, cid: 3 });
    search.resolve([song]);
    expect((await first).status).toBe("cancelled");
    expect((await second).status).toBe("matched");
  });

  it("supports explicit cancellation and does not write a result", async () => {
    const { service, dependencies } = setup();
    const search = deferred<(typeof song)[]>();
    dependencies.sources.search.mockReturnValue(search.promise);
    const task = service.start(params);
    await Promise.resolve();
    service.cancel("BVcurrent-1");
    search.resolve([song]);
    expect((await task).status).toBe("cancelled");
    expect(dependencies.cache.set).not.toHaveBeenCalled();
  });

  it("waits for the foreground before starting prefetch", async () => {
    const { service, dependencies } = setup();
    const search = deferred<(typeof song)[]>();
    dependencies.sources.search.mockReturnValueOnce(search.promise);
    const first = service.start(params);
    const background = service.prefetch({ ...params, cid: 3 });
    await Promise.resolve();
    expect(dependencies.sources.search).toHaveBeenCalledTimes(1);
    search.resolve([song]);
    expect((await first).status).toBe("matched");
    expect((await background).status).toBe("matched");
  });

  it("returns no-reference after three failed onset checks without emitting lyrics", async () => {
    const { service, dependencies } = setup();
    dependencies.sources.references.mockResolvedValue(
      Array.from({ length: 5 }, (_, i) => ({ ...reference, bvid: `BVref${i}` })),
    );
    dependencies.decode.mockResolvedValue(new Float32Array(22050 * 5));
    const events: AutoLyricsProgress[] = [];
    service.onProgress(event => events.push(event));
    expect((await service.start(params)).status).toBe("no-reference");
    expect(dependencies.sources.audio).toHaveBeenCalledTimes(4);
    expect(dependencies.align).not.toHaveBeenCalled();
    expect(events.some(event => event.phase === "coarse" || event.phase === "fine")).toBe(false);
  });

  it("cancels a queued prefetch without waiting for the foreground", async () => {
    const { service, dependencies } = setup();
    const search = deferred<(typeof song)[]>();
    dependencies.sources.search.mockReturnValueOnce(search.promise);
    const first = service.start(params);
    const background = service.prefetch({ ...params, cid: 3 });
    await Promise.resolve();
    service.cancel("BVcurrent-3");
    expect((await background).status).toBe("cancelled");
    expect(dependencies.sources.search).toHaveBeenCalledTimes(1);
    search.resolve([song]);
    await first;
  });

  it("promotes a queued prefetch and reuses its promise", async () => {
    const { service, dependencies } = setup();
    const search = deferred<(typeof song)[]>();
    dependencies.sources.search.mockReturnValueOnce(search.promise);
    const first = service.start(params);
    const background = service.prefetch({ ...params, cid: 3 });
    await Promise.resolve();
    const promoted = service.start({ ...params, cid: 3 });
    expect(promoted).toBe(background);
    search.resolve([song]);
    expect((await first).status).toBe("cancelled");
    expect((await promoted).status).toBe("matched");
  });

  it("records decode-unavailable and does not write lyrics", async () => {
    const { service, dependencies, entries } = setup();
    dependencies.decode.mockRejectedValue(new DecodeUnavailableError("ENOEXEC"));
    await expect(service.start(params)).resolves.toMatchObject({
      status: "decode-unavailable",
      error: expect.stringContaining("decode-unavailable"),
    });
    expect(entries.size).toBe(0);
    expect(dependencies.cache.set).not.toHaveBeenCalled();
    expect(dependencies.align).not.toHaveBeenCalled();
  });

  it("aborts sibling IO after a source error", async () => {
    const { service, dependencies } = setup();
    let downloadSignal: AbortSignal | undefined;
    dependencies.sources.audio.mockImplementation(async (_bvid?: unknown, _cid?: unknown, signal?: AbortSignal) => {
      downloadSignal = signal;
      return new Uint8Array([1]);
    });
    dependencies.sources.lyric.mockRejectedValue(new Error("lyric unavailable"));
    expect(await service.start(params)).toMatchObject({ status: "error", error: "lyric unavailable" });
    expect(downloadSignal?.aborted).toBe(true);
  });

  it("reuses the song reference cache across video keys", async () => {
    const { service, dependencies } = setup();
    expect((await service.start(params)).status).toBe("matched");
    expect((await service.start({ ...params, cid: 3 })).status).toBe("matched");
    expect(dependencies.sources.references).toHaveBeenCalledTimes(1);
  });

  it("lets line alignment choose between ambiguous artists without showing coarse lyrics first", async () => {
    const { service, dependencies, entries, cover, events } = arbitration({ 42: 0.7, 43: 0.9 });
    const result = await service.start(params);
    expect(result).toMatchObject({ status: "matched", song: { id: cover.id }, reference: { bvid: "BVref43" } });
    expect(result.verifyScore).toBeCloseTo(0.9);
    expect(result.contenders?.map(c => [c.song.id, c.verifyScore])).toEqual([
      [42, expect.closeTo(0.7)],
      [43, expect.closeTo(0.9)],
    ]);
    expect(dependencies.sources.audio.mock.calls.filter(args => (args as unknown[])[0] === params.bvid)).toHaveLength(
      1,
    );
    expect(events.some(event => event.phase === "coarse")).toBe(false);
    expect(entries.get(result.key)?.songId).toBe(cover.id);
  });

  it("stays unidentified when alignment cannot separate the contenders", async () => {
    for (const scores of [
      { 42: 0.9, 43: 0.88 },
      { 42: 0.69, 43: 0.4 },
    ]) {
      const { service, entries } = arbitration(scores);
      const result = await service.start(params);
      expect(result.status).toBe("unidentified");
      expect(result.contenders).toHaveLength(2);
      expect(entries.size).toBe(0);
    }
  });

  it("treats tied contenders with the same lyrics as one song and keeps the top search hit", async () => {
    for (const scores of [
      { 42: 0.9, 43: 0.92 },
      { 42: 0.66, 43: 0.67 },
    ]) {
      const { service } = arbitration(scores, true);
      const result = await service.start(params);
      expect(result.status).toBe("matched");
      expect(result.song?.id).toBe(42);
    }
  });

  it("prefers the release without an unmentioned note when alignment ties (影色舞 over 影色舞(中文版))", async () => {
    const { service, dependencies, cover } = arbitration({ 42: 0.88, 43: 0.9 });
    dependencies.sources.search.mockResolvedValue([song, { ...cover, name: `${song.name} (中文版)` }]);
    const result = await service.start(params);
    expect(result.status).toBe("matched");
    expect(result.song?.id).toBe(42);
    const mentioned = arbitration({ 42: 0.88, 43: 0.9 });
    mentioned.dependencies.sources.search.mockResolvedValue([song, { ...cover, name: `${song.name} (中文版)` }]);
    const named = await mentioned.service.start({ ...params, title: `${params.title} 中文版` });
    expect([named.status, named.song?.id]).toEqual(["matched", 43]);
  });

  it("tries another release by the same artist when the chosen one has no reference", async () => {
    const { service, dependencies } = setup();
    const anime = { ...song, id: 50, name: "DECORATOR (Anime ver.)", duration: params.duration };
    const studio = { ...song, id: 51, duration: params.duration + 1 };
    dependencies.sources.search.mockResolvedValue([anime, studio]);
    dependencies.sources.references.mockImplementation((async (candidate: typeof song) =>
      candidate.id === studio.id ? [reference] : []) as never);
    const result = await service.start(params);
    expect(result.status).toBe("matched");
    expect(result.song?.id).toBe(studio.id);
  });

  it("uses a studio upload as its own reference when no reference exists", async () => {
    const { service, dependencies, entries } = setup();
    dependencies.sources.references.mockResolvedValue([]);
    const result = await service.start(params);
    expect(result.status).toBe("matched");
    expect(result.lines?.[0]).toMatchObject({ liveStartSec: 2 });
    expect(entries.get(result.key)).toMatchObject({ refBvid: params.bvid, lyrics: expect.stringContaining("hello") });

    const live = setup();
    live.dependencies.sources.references.mockResolvedValue([]);
    expect((await live.service.start({ ...params, title: "DECORATOR LIVE" })).status).toBe("no-reference");

    const silent = setup();
    silent.dependencies.sources.references.mockResolvedValue([]);
    silent.dependencies.decode.mockResolvedValue(new Float32Array(22050 * 5));
    expect((await silent.service.start(params)).status).toBe("no-reference");
  });

  it("accepts the top search hit when it is the only contender with a reference", async () => {
    for (const [scores, status] of [
      [{ 42: 0.52 }, "matched"],
      [{ 43: 0.52 }, "unidentified"],
      [{ 42: 0.45 }, "unidentified"],
    ] as const) {
      const { service, dependencies } = arbitration(scores);
      dependencies.sources.references.mockImplementation((async (candidate: typeof song) =>
        candidate.id in scores ? [{ ...reference, bvid: `BVref${candidate.id}` }] : []) as never);
      const align = dependencies.align.getMockImplementation()!;
      dependencies.align.mockImplementation(async (...args) => {
        const aligned = await align(...args);
        return { ...aligned, lines: aligned.lines.map(line => ({ ...line, uncertain: false })) };
      });
      const result = await service.start(params);
      expect(result.status).toBe(status);
      if (status === "matched") expect(result.song?.id).toBe(42);
    }
  });

  it("orders ambiguous contenders by NetEase search rank", async () => {
    const { service, dependencies } = setup();
    const near = { ...song, id: 44, artists: ["Near"], duration: params.duration };
    dependencies.sources.search.mockResolvedValue([{ ...song, duration: params.duration + 1 }, near]);
    const result = await service.identify(params);
    expect(result).toMatchObject({ status: "ambiguous", contenders: [{ id: song.id }, { id: 44 }] });
  });

  it("collects every primary query before handing ambiguous artists to alignment", async () => {
    const { service, dependencies } = setup();
    dependencies.sources.search
      .mockResolvedValueOnce([song])
      .mockResolvedValueOnce([{ ...song, id: 43, artists: ["Other"] }]);
    const result = await service.identify({ ...params, title: "DECORATOR / stage", pageTitle: "DECORATOR" });
    expect(result).toMatchObject({ status: "ambiguous", contenders: [{ id: song.id }, { id: 43 }] });
    // Still ambiguous after the primary queries, so the fallback fragments are searched too.
    expect(dependencies.sources.search.mock.calls.map(args => args[0]).slice(0, 2)).toEqual([
      "DECORATOR",
      "DECORATOR / stage",
    ]);
  });

  it("applies the stricter fallback rule to avoid matching a title prefix", async () => {
    const { service, dependencies } = setup();
    const candidate = { ...song, name: "名もなき何もかも" };
    dependencies.sources.search.mockImplementation(async (...args: unknown[]) =>
      args[0] === candidate.name ? [candidate] : [],
    );
    const result = await service.identify({
      ...params,
      title: "番組",
      pageTitle: "名もなき何もかも-X123 / Concert M-10",
    });
    expect(result.status).toBe("unidentified");
    expect(dependencies.sources.search).toHaveBeenCalledWith(candidate.name, expect.any(AbortSignal));
  });
});
