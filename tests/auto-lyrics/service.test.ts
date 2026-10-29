// @vitest-environment node
import { describe, expect, it, vi } from "vitest";

import type { AutoLyricsDependencies } from "../../electron/auto-lyrics/service";
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
  const references = new Map<number, typeof reference>();
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
      audio: vi.fn(async () => new Uint8Array([1])),
    },
    decode: vi.fn(async () => pcm),
    align: vi.fn<AutoLyricsDependencies["align"]>(async (input, _signal, onCoarse) => {
      const coarse = { offsetSec: 1, confidence: 0.9 };
      onCoarse(coarse);
      return {
        coarse,
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
      set: vi.fn((id: number, value: typeof reference) => {
        references.set(id, value);
      }),
    },
  } satisfies AutoLyricsDependencies;
  return { dependencies, entries, pcm, service: new AutoLyricsService(dependencies) };
}

/** Two same-title songs by different artists; each one's reference decodes to a distinguishable length. */
function arbitration(scores: Record<number, number>) {
  const context = setup();
  const { dependencies, pcm } = context;
  const cover = { ...song, id: 43, artists: ["Other"] };
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
    const id = [...lengths].find(([, length]) => length === input.studio.length)?.[0] ?? 0;
    const coarse = { offsetSec: 1, confidence: 0.9 };
    onCoarse(coarse);
    return {
      coarse,
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

  it("collects every primary query before handing ambiguous artists to alignment", async () => {
    const { service, dependencies } = setup();
    dependencies.sources.search
      .mockResolvedValueOnce([song])
      .mockResolvedValueOnce([{ ...song, id: 43, artists: ["Other"] }]);
    const result = await service.identify({ ...params, title: "DECORATOR / stage", pageTitle: "DECORATOR" });
    expect(result).toMatchObject({ status: "ambiguous", contenders: [{ id: song.id }, { id: 43 }] });
    expect(dependencies.sources.search.mock.calls.map(args => args[0])).toEqual(["DECORATOR", "DECORATOR / stage"]);
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
