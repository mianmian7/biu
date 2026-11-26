import type { SongCandidate } from "../../shared/auto-lyrics/identify";
import type { ReferenceCandidate } from "../../shared/auto-lyrics/reference";
import type { AutoLyricsParams, AutoLyricsProgress, AutoLyricsResult } from "../../shared/auto-lyrics/service-types";
import type { AlignLine, AlignResult, AudioFeatureSet, GlobalOffset } from "../../shared/auto-lyrics/types";

import { alignLinesGapDTW, estimateGlobalOffset, lyricAnchors, selectReference } from "../../shared/auto-lyrics/align";
import { AudioFeatures } from "../../shared/auto-lyrics/features";
import { DecodeUnavailableError } from "../../shared/auto-lyrics/ffmpeg-resolve";
import { withoutTrailingNote } from "../../shared/auto-lyrics/identify";
import { rankReferences } from "../../shared/auto-lyrics/reference";
import { AutoLyricsService } from "../../shared/auto-lyrics/service";

type Pcm = Float32Array | Uint8Array;

export type AlignRequest = {
  studios: Pcm[];
  live: Pcm | AudioFeatureSet;
  lines: AlignLine[];
};

export type AlignWorkerOutput = {
  coarse: GlobalOffset;
  lines: AlignResult[];
  memoryPeakKB: number;
  liveFeatures?: AudioFeatureSet;
} & ReturnType<typeof selectReference>;

type AlignResponse =
  | ({
      kind: "coarse";
      coarse: GlobalOffset;
      elapsedMs: number;
      memoryPeakKB: number;
    } & ReturnType<typeof selectReference>)
  | ({
      kind: "fine";
      coarse: GlobalOffset;
      lines: AlignResult[];
      liveFeatures?: AudioFeatureSet;
      elapsedMs: number;
      memoryPeakKB: number;
    } & ReturnType<typeof selectReference>)
  | {
      kind: "error";
      error: string;
    };

function isPcm(value: unknown): value is Pcm {
  return (
    (value instanceof Float32Array && value.length > 0) ||
    (value instanceof Uint8Array && value.length > 0 && value.length % 2 === 0)
  );
}

function parseAlignRequest(input: unknown): AlignRequest {
  if (!input || typeof input !== "object") {
    throw new Error("Invalid alignment request");
  }
  const req = input as AlignRequest;
  if (!Array.isArray(req.studios) || req.studios.length === 0 || req.studios.length > 3 || !req.studios.every(isPcm)) {
    throw new Error("Invalid studio PCM in alignment request");
  }
  if (!isPcm(req.live) && (!req.live || !Array.isArray(req.live.cens))) {
    throw new Error("Invalid live audio in alignment request");
  }
  if (!Array.isArray(req.lines)) {
    throw new Error("Invalid lines array in alignment request");
  }
  return req;
}

function parseAlignResponse(input: unknown): AlignResponse {
  if (!input || typeof input !== "object") {
    throw new Error("Invalid alignment response");
  }
  const res = input as Record<string, unknown>;
  if (res.kind !== "coarse" && res.kind !== "fine" && res.kind !== "error") {
    throw new Error("Invalid alignment response kind");
  }
  return input as AlignResponse;
}

function runAlignment(
  request: AlignRequest,
  studios: AudioFeatureSet[],
  live: AudioFeatureSet,
  onCoarse: (coarse: GlobalOffset, selection: ReturnType<typeof selectReference>) => void,
): AlignWorkerOutput {
  const selection = selectReference(studios);
  const studio = studios[selection.referenceIndex];
  const coarse = estimateGlobalOffset(studio, live, null, lyricAnchors(request.lines));
  onCoarse(coarse, selection);
  return {
    ...selection,
    coarse,
    lines: alignLinesGapDTW(request.lines, studio, live),
    memoryPeakKB: 0,
  };
}

function extractPcm(extractor: AudioFeatures, pcm: Pcm) {
  return extractor.extract(pcm instanceof Uint8Array ? pcmS16leToFloat32(pcm) : pcm);
}

export function runAlignmentSync(
  input: AlignRequest,
  onCoarse: (coarse: GlobalOffset) => void = () => {},
): AlignWorkerOutput {
  const request = parseAlignRequest(input);
  const extractor = new AudioFeatures({ sampleRate: 22050 });
  const studios = request.studios.map(pcm => extractPcm(extractor, pcm));
  const live = isPcm(request.live) ? extractPcm(extractor, request.live) : request.live;
  return {
    ...runAlignment(request, studios, live, onCoarse),
    liveFeatures: request.live instanceof Uint8Array ? live : undefined,
  };
}

// Web Worker entry point when loaded via `new Worker(new URL("./auto-lyrics-worker.ts", import.meta.url))`
const isDedicatedWorker =
  typeof self !== "undefined" && typeof window === "undefined" && typeof self.postMessage === "function";

if (isDedicatedWorker) {
  self.addEventListener("message", (event: MessageEvent<unknown>) => {
    const started = performance.now();
    try {
      const request = parseAlignRequest(event.data);
      const extractor = new AudioFeatures({ sampleRate: 22050 });
      const studios: AudioFeatureSet[] = [];
      // Drop each PCM view as it is converted into features.
      while (request.studios.length) {
        studios.push(extractPcm(extractor, request.studios.shift()!));
      }
      const returnLiveFeatures = request.live instanceof Uint8Array;
      const live = isPcm(request.live) ? extractPcm(extractor, request.live) : request.live;
      request.live = live;
      const output = runAlignment(request, studios, live, (coarse, selection) => {
        self.postMessage({
          kind: "coarse",
          ...selection,
          coarse,
          elapsedMs: performance.now() - started,
          memoryPeakKB: 0,
        } satisfies AlignResponse);
      });
      self.postMessage({
        kind: "fine",
        ...output,
        liveFeatures: returnLiveFeatures ? live : undefined,
        elapsedMs: performance.now() - started,
      } satisfies AlignResponse);
    } catch (error) {
      self.postMessage({
        kind: "error",
        error: error instanceof Error ? error.message : String(error),
      } satisfies AlignResponse);
    }
  });
}

/** The worker takes ownership of the PCM buffers in this request. */
export function alignAudioInWorker(
  request: AlignRequest,
  signal: AbortSignal,
  onCoarse: (coarse: GlobalOffset) => void,
): Promise<AlignWorkerOutput> {
  signal.throwIfAborted();
  if (typeof Worker === "undefined") {
    return Promise.resolve().then(() => {
      signal.throwIfAborted();
      return runAlignmentSync(request, onCoarse);
    });
  }

  return new Promise<AlignWorkerOutput>((resolve, reject) => {
    const worker = new Worker(new URL("./auto-lyrics-worker.ts", import.meta.url), {
      type: "module",
    });
    let settled = false;
    const cleanup = () => {
      signal.removeEventListener("abort", cancel);
      worker.terminate();
    };
    const fail = (error: unknown) => {
      if (settled) return;
      settled = true;
      cleanup();
      reject(error);
    };
    const cancel = () => fail(signal.reason ?? new DOMException("Cancelled", "AbortError"));
    signal.addEventListener("abort", cancel, { once: true });

    worker.onerror = event => {
      fail(new Error(event.message || "Alignment Web Worker error"));
    };

    worker.onmessageerror = () => fail(new Error("Invalid alignment response"));
    worker.onmessage = (event: MessageEvent<unknown>) => {
      if (settled) return;
      let data: AlignResponse;
      try {
        data = parseAlignResponse(event.data);
      } catch (err) {
        fail(err);
        return;
      }
      if (data.kind === "error") {
        fail(new Error(data.error));
        return;
      }
      if (data.kind === "coarse") {
        try {
          onCoarse(data.coarse);
        } catch (error) {
          fail(error);
        }
      } else {
        settled = true;
        cleanup();
        resolve({
          coarse: data.coarse,
          lines: data.lines,
          memoryPeakKB: data.memoryPeakKB,
          liveFeatures: data.liveFeatures,
          referenceIndex: data.referenceIndex,
          referenceConsensus: data.referenceConsensus,
          referenceLags: data.referenceLags,
        });
      }
    };

    if (signal.aborted) {
      cancel();
      return;
    }
    const transfer = new Set<ArrayBuffer>();
    const addBuffer = (pcm: Pcm) => {
      if (pcm.buffer instanceof ArrayBuffer) transfer.add(pcm.buffer);
    };
    request.studios.forEach(addBuffer);
    if (isPcm(request.live)) addBuffer(request.live);
    try {
      worker.postMessage(request, [...transfer]);
    } catch (error) {
      fail(error);
    }
  });
}

function parseInputParams(input: unknown): AutoLyricsParams | { bvid: string; cid: number; bypassCache?: boolean } {
  if (!input || typeof input !== "object") {
    throw new Error("Invalid autoLyrics params");
  }
  const obj = input as Record<string, unknown>;
  if (typeof obj.bvid !== "string" || !/^BV[\da-zA-Z]{10}$/.test(obj.bvid)) {
    throw new Error("Invalid bvid");
  }
  if (typeof obj.cid !== "number" || !Number.isInteger(obj.cid) || obj.cid <= 0) {
    throw new Error("Invalid cid");
  }
  if (obj.bypassCache !== undefined && typeof obj.bypassCache !== "boolean") {
    throw new Error("Invalid bypassCache");
  }
  if ("tid" in obj && obj.tid !== undefined) {
    if (
      typeof obj.title !== "string" ||
      typeof obj.duration !== "number" ||
      !Number.isFinite(obj.duration) ||
      obj.duration <= 0 ||
      typeof obj.tid !== "number" ||
      !Number.isInteger(obj.tid) ||
      typeof obj.ownerName !== "string"
    ) {
      throw new Error("Invalid full autoLyrics params");
    }
    return {
      bvid: obj.bvid,
      cid: obj.cid,
      title: obj.title,
      pageTitle: typeof obj.pageTitle === "string" ? obj.pageTitle : undefined,
      duration: obj.duration,
      tid: obj.tid,
      ownerName: obj.ownerName,
      bypassCache: obj.bypassCache,
    };
  }
  return {
    bvid: obj.bvid,
    cid: obj.cid,
    bypassCache: obj.bypassCache,
  };
}

interface VideoViewResponse {
  bvid: string;
  cid: number;
  title: string;
  duration: number;
  tid: number;
  ownerName: string;
  pages: Array<{ cid: number; part: string; duration: number }>;
}

class S16leAudio {
  readonly length: number;
  private readonly view: DataView;
  features?: AudioFeatureSet;

  constructor(readonly bytes: Uint8Array) {
    if (bytes.byteLength < 2 || bytes.byteLength % 2 !== 0) {
      throw new Error("ffmpeg returned invalid PCM");
    }
    this.view = new DataView(bytes.buffer, bytes.byteOffset, bytes.byteLength);
    this.length = bytes.byteLength / 2;
  }

  sampleAt(index: number) {
    return this.view.getInt16(index * 2, true) / 32768;
  }
}

export function pcmS16leToFloat32(raw: ArrayBuffer | Uint8Array): Float32Array {
  const pcm = new S16leAudio(raw instanceof Uint8Array ? raw : new Uint8Array(raw));
  const samples = new Float32Array(pcm.length);
  for (let i = 0; i < samples.length; i++) samples[i] = pcm.sampleAt(i);
  return samples;
}

export function createTauriAutoLyricsController(
  invokeTauri: <T>(cmd: string, args?: Record<string, unknown>) => Promise<T>,
) {
  const lyricsCacheMem = new Map<string, MusicLyrics>();
  const referencesMem = new Map<string, ReferenceCandidate>();
  const audioTokens = new WeakMap<Uint8Array, () => void>();

  const syncStateFromBackend = async () => {
    try {
      const snapshot = await invokeTauri<{
        lyricsCache?: Record<string, MusicLyrics>;
        references?: Record<string, ReferenceCandidate>;
      }>("auto_lyrics_state_snapshot");
      lyricsCacheMem.clear();
      for (const [k, v] of Object.entries(snapshot?.lyricsCache ?? {})) {
        if (v && typeof v === "object") {
          lyricsCacheMem.set(k, v);
        }
      }
      referencesMem.clear();
      for (const [k, v] of Object.entries(snapshot?.references ?? {})) {
        if (v && typeof v === "object") {
          referencesMem.set(k, v);
        }
      }
    } catch {
      // Ignore snapshot errors in isolated unit tests
    }
  };

  const resolveInputParams = async (input: unknown): Promise<AutoLyricsParams> => {
    const parsed = parseInputParams(input);
    if ("tid" in parsed) {
      return parsed;
    }
    const video = await invokeTauri<VideoViewResponse>("auto_lyrics_view", {
      bvid: parsed.bvid,
    });
    const page = video.pages.find(item => item.cid === parsed.cid);
    return {
      bvid: video.bvid,
      cid: parsed.cid,
      title: video.title,
      pageTitle: video.pages.length > 1 ? page?.part : undefined,
      duration: page?.duration ?? video.duration,
      tid: video.tid,
      ownerName: video.ownerName,
      bypassCache: parsed.bypassCache,
    };
  };

  const service = new AutoLyricsService<S16leAudio>({
    sources: {
      search: async (query: string, signal: AbortSignal): Promise<SongCandidate[]> => {
        signal.throwIfAborted();
        return invokeTauri<SongCandidate[]>("auto_lyrics_search_netease", { query });
      },
      lyric: async (songId: number, signal: AbortSignal) => {
        signal.throwIfAborted();
        return invokeTauri<{ lrc: string; tlyric?: string; romalrc?: string }>("auto_lyrics_get_netease_lyric", {
          songId,
        });
      },
      references: async (song: SongCandidate, signal: AbortSignal): Promise<ReferenceCandidate[]> => {
        signal.throwIfAborted();
        const keyword = `${withoutTrailingNote(song.name)} ${song.artists[0] ?? ""}`;
        const items = await invokeTauri<Array<{ bvid: string; title: string; duration: number }>>(
          "auto_lyrics_search_bili_videos",
          { keyword },
        );
        return rankReferences(items, song);
      },
      audio: async (bvid: string, cid: number | undefined, signal: AbortSignal): Promise<Uint8Array> => {
        signal.throwIfAborted();
        const token = await invokeTauri<string>("auto_lyrics_fetch_audio", {
          bvid,
          cid: cid ?? null,
        });
        const release = () => {
          void invokeTauri("auto_lyrics_release_audio", { token }).catch(() => {});
        };
        if (signal.aborted) {
          release();
          signal.throwIfAborted();
        }
        const bytes = new TextEncoder().encode(token);
        signal.addEventListener("abort", release, { once: true });
        audioTokens.set(bytes, () => signal.removeEventListener("abort", release));
        return bytes;
      },
    },
    decode: async (data: Uint8Array, signal: AbortSignal): Promise<S16leAudio> => {
      signal.throwIfAborted();
      audioTokens.get(data)?.();
      audioTokens.delete(data);
      const token = new TextDecoder().decode(data);
      try {
        const raw = await invokeTauri<ArrayBuffer | Uint8Array | number[]>("auto_lyrics_decode_audio", { token });
        signal.throwIfAborted();
        const bytes = raw instanceof Uint8Array ? raw : Array.isArray(raw) ? Uint8Array.from(raw) : new Uint8Array(raw);
        return new S16leAudio(bytes);
      } catch (error) {
        const msg = error instanceof Error ? error.message : String(error);
        if (msg.startsWith("decode-unavailable:")) {
          throw new DecodeUnavailableError(msg.slice("decode-unavailable:".length).trim());
        }
        throw error;
      }
    },
    align: async ({ studios, live, lines }, signal, onCoarse) => {
      const output = await alignAudioInWorker(
        {
          studios: studios.map(studio => studio.bytes),
          // Keep the original s16le samples for the self-reference onset check.
          live: live.features ?? new Uint8Array(live.bytes),
          lines,
        },
        signal,
        onCoarse,
      );
      live.features ??= output.liveFeatures;
      return output;
    },
    cache: {
      get: (key: string) => lyricsCacheMem.get(key),
      set: (key: string, value: MusicLyrics) => {
        lyricsCacheMem.set(key, value);
        void invokeTauri("auto_lyrics_cache_set", { key, value }).catch(() => {});
      },
    },
    references: {
      get: (songId: number) => referencesMem.get(String(songId)),
      set: (songId: number, value: ReferenceCandidate) => {
        referencesMem.set(String(songId), value);
        void invokeTauri("auto_lyrics_ref_set", { id: String(songId), value }).catch(() => {});
      },
    },
  });

  return {
    start: async (
      params: AutoLyricsParams | { bvid: string; cid: number; bypassCache?: boolean },
    ): Promise<AutoLyricsResult> => {
      await syncStateFromBackend();
      const resolved = await resolveInputParams(params);
      return service.start(resolved);
    },
    prefetch: async (
      params: AutoLyricsParams | { bvid: string; cid: number; bypassCache?: boolean },
    ): Promise<AutoLyricsResult> => {
      await syncStateFromBackend();
      const resolved = await resolveInputParams(params);
      return service.prefetch(resolved);
    },
    cancel: (key?: string): Promise<void> => {
      if (key !== undefined && typeof key !== "string") {
        return Promise.reject(new Error("Invalid cancel key"));
      }
      service.cancel(key);
      return Promise.resolve();
    },
    onProgress: (cb: (progress: AutoLyricsProgress) => void) => service.onProgress(cb),
  };
}
