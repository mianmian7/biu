// @vitest-environment node
import { afterEach, expect, test, vi } from "vitest";

import type { AlignRequest } from "@/platform/auto-lyrics-worker";

import {
  alignAudioInWorker,
  createTauriAutoLyricsController,
  pcmS16leToFloat32,
  runAlignmentSync,
} from "@/platform/auto-lyrics-worker";
import { checkVocalOnset } from "@shared/auto-lyrics/reference";

afterEach(() => vi.unstubAllGlobals());

const rate = 22050;
const studioSamples = rate * 8;
const delay = rate;
const lines = [1, 3, 5].map((second, idx) => ({ idx, studio_start: second, text: `Line ${idx}` }));

function makePcm() {
  const studio = new Uint8Array(studioSamples * 2);
  const live = new Uint8Array((studioSamples + delay) * 2);
  const samples = new DataView(studio.buffer);
  for (let i = rate; i < studioSamples; i++) {
    const freq = 220 + 25 * Math.floor(i / rate);
    samples.setInt16(i * 2, 16000 * Math.sin((i * freq * 2 * Math.PI) / rate), true);
  }
  live.set(studio, delay * 2);
  return { studio, live };
}

test("s16le streaming and reused live features preserve exact alignment", () => {
  const { studio, live } = makePcm();
  const liveFloat = pcmS16leToFloat32(live);
  const baseline = runAlignmentSync({ studios: [pcmS16leToFloat32(studio)], live: liveFloat, lines });
  const { liveFeatures, ...streamed } = runAlignmentSync({ studios: [studio], live, lines });
  expect(streamed).toEqual(baseline);
  expect(liveFeatures).toBeDefined();
  expect(baseline.coarse.offsetSec).toBeCloseTo(1, 1);

  const replayed = runAlignmentSync({ studios: [studio], live: liveFeatures!, lines });
  expect(replayed).toEqual(baseline);

  const view = new DataView(live.buffer);
  const read = { length: live.length / 2, sampleAt: (i: number) => view.getInt16(2 * i, true) / 32768 };
  for (const at of [0.5, 1.6, 2, 6, 8]) {
    expect(checkVocalOnset(read, rate, at)).toBe(checkVocalOnset(liveFloat, rate, at));
  }
});

test("transfers shared studio and live buffers once and detaches them on the caller", async () => {
  const { studio, live } = makePcm();
  const bytes = new Uint8Array(studio.length + live.length);
  bytes.set(studio);
  bytes.set(live, studio.length);
  const post = vi.fn();
  const terminate = vi.fn();
  vi.stubGlobal(
    "Worker",
    class {
      onmessage!: (event: { data: unknown }) => void;
      terminate = terminate;
      postMessage(request: AlignRequest, transfer: ArrayBuffer[]) {
        const cloned = structuredClone(request, { transfer });
        post(transfer);
        queueMicrotask(() => {
          const output = runAlignmentSync(cloned);
          this.onmessage({ data: { kind: "fine", ...output } });
        });
      }
    },
  );

  const output = await alignAudioInWorker(
    {
      studios: [bytes.subarray(0, studio.length)],
      live: bytes.subarray(studio.length),
      lines,
    },
    new AbortController().signal,
    () => {},
  );
  expect(post).toHaveBeenCalledExactlyOnceWith([bytes.buffer]);
  expect(bytes.byteLength).toBe(0);
  expect(output.coarse.offsetSec).toBeCloseTo(1, 1);
  expect(terminate).toHaveBeenCalledOnce();
});

test("cancellation releases a fetched token before decode starts", async () => {
  let resolveFetch!: (token: string) => void;
  const fetchAudio = new Promise<string>(resolve => {
    resolveFetch = resolve;
  });
  const invoke = vi.fn(async (cmd: string, ...args: [Record<string, unknown>?]): Promise<unknown> => {
    if (cmd === "auto_lyrics_search_netease") return [{ id: 42, name: "DECORATOR", artists: ["Artist"], duration: 5 }];
    if (cmd === "auto_lyrics_get_netease_lyric") return { lrc: "[00:02.00]hello" };
    if (cmd === "auto_lyrics_search_bili_videos") return [];
    if (cmd === "auto_lyrics_fetch_audio" && args[0]?.bvid) return fetchAudio;
    return undefined;
  });
  const controller = createTauriAutoLyricsController(
    <T>(cmd: string, args?: Record<string, unknown>) => invoke(cmd, args) as Promise<T>,
  );
  const running = controller.start({
    bvid: "BV1xx411c7mD",
    cid: 1,
    title: "DECORATOR",
    duration: 5,
    tid: 3,
    ownerName: "Artist",
  });
  await vi.waitFor(() => expect(invoke).toHaveBeenCalledWith("auto_lyrics_fetch_audio", expect.anything()));
  await controller.cancel("BV1xx411c7mD-1");
  resolveFetch("test-token");

  expect((await running).status).toBe("cancelled");
  expect(invoke).toHaveBeenCalledWith("auto_lyrics_release_audio", { token: "test-token" });
  expect(invoke).not.toHaveBeenCalledWith("auto_lyrics_decode_audio", expect.anything());
});
