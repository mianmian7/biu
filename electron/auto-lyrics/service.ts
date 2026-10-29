import type { IdentifyInput, SongCandidate } from "../../shared/auto-lyrics/identify";
import type { ReferenceCandidate } from "../../shared/auto-lyrics/reference";
import type { AutoLyricsParams, AutoLyricsProgress, AutoLyricsResult } from "../../shared/auto-lyrics/service-types";
import type { AlignLine, AlignResult, GlobalOffset } from "../../shared/auto-lyrics/types";

import {
  identifyPrimaryQueries,
  identifyQueries,
  identifyRules,
  ruleMatches,
  songText,
} from "../../shared/auto-lyrics/identify";
import { generateLrc, mapCompanionLrc, parseLrc } from "../../shared/auto-lyrics/lrc";
import { checkVocalOnset, rankReferences } from "../../shared/auto-lyrics/reference";
import { isDecodeUnavailable } from "./ffmpeg-resolve";

export const AUTO_LYRICS_ALGO_VERSION = "p1-gap-dtw-1";

export interface AutoLyricsDependencies {
  sources: {
    search(query: string, signal: AbortSignal): Promise<SongCandidate[]>;
    lyric(songId: number, signal: AbortSignal): Promise<{ lrc: string; tlyric?: string; romalrc?: string }>;
    references(song: SongCandidate, signal: AbortSignal): Promise<ReferenceCandidate[]>;
    audio(bvid: string, cid: number | undefined, signal: AbortSignal): Promise<Uint8Array>;
  };
  decode(data: Uint8Array, signal: AbortSignal): Promise<Float32Array>;
  align(
    input: { studio: Float32Array; live: Float32Array; lines: AlignLine[] },
    signal: AbortSignal,
    onCoarse: (coarse: GlobalOffset) => void,
  ): Promise<{ coarse: GlobalOffset; lines: AlignResult[]; memoryPeakKB?: number }>;
  cache: { get(key: string): MusicLyrics | undefined; set(key: string, value: MusicLyrics): void };
  references: {
    get(songId: number): ReferenceCandidate | undefined;
    set(songId: number, value: ReferenceCandidate): void;
  };
  now?: () => number;
}

interface Task {
  controller: AbortController;
  promise: Promise<AutoLyricsResult>;
  prefetch: boolean;
}

function isManual(entry: MusicLyrics | undefined): boolean {
  return !!entry && (entry.source !== "auto" || !!entry.offset);
}

function waitUnlessAborted<T>(promise: Promise<T>, signal: AbortSignal): Promise<T> {
  signal.throwIfAborted();
  return new Promise<T>((resolve, reject) => {
    const abort = () => reject(signal.reason);
    signal.addEventListener("abort", abort, { once: true });
    promise.then(resolve, reject).finally(() => signal.removeEventListener("abort", abort));
  });
}

export class AutoLyricsService {
  private tasks = new Map<string, Task>();
  private foreground?: Task;
  private referenceCache = new Map<number, ReferenceCandidate>();
  private listeners = new Set<(event: AutoLyricsProgress) => void>();

  constructor(private dependencies: AutoLyricsDependencies) {}

  onProgress(listener: (event: AutoLyricsProgress) => void): () => void {
    this.listeners.add(listener);
    return () => this.listeners.delete(listener);
  }

  private emit(event: AutoLyricsProgress) {
    for (const listener of this.listeners) listener(event);
  }

  start(params: AutoLyricsParams): Promise<AutoLyricsResult> {
    return this.schedule(params, false);
  }

  prefetch(params: AutoLyricsParams): Promise<AutoLyricsResult> {
    return this.schedule(params, true);
  }

  cancel(key?: string): void {
    for (const [taskKey, task] of this.tasks) {
      if (!key || taskKey === key) task.controller.abort();
    }
  }

  private schedule(params: AutoLyricsParams, prefetch: boolean): Promise<AutoLyricsResult> {
    const key = `${params.bvid}-${params.cid}`;
    const existing = this.tasks.get(key);
    if (existing && !existing.controller.signal.aborted) {
      if (!prefetch) {
        if (this.foreground !== existing) this.foreground?.controller.abort();
        existing.prefetch = false;
        this.foreground = existing;
      }
      return existing.promise;
    }
    if (!prefetch) this.foreground?.controller.abort();
    const controller = new AbortController();
    const task: Task = {
      controller,
      prefetch,
      promise: Promise.resolve()
        .then(async () => {
          while (task.prefetch && this.foreground && this.foreground !== task) {
            await waitUnlessAborted(this.foreground.promise, controller.signal);
            controller.signal.throwIfAborted();
          }
          return this.run(params, controller.signal);
        })
        .catch(
          error =>
            ({
              key,
              status: controller.signal.aborted ? "cancelled" : "error",
              timings: {},
              error: error instanceof Error ? error.message : String(error),
            }) satisfies AutoLyricsResult,
        )
        .finally(() => {
          if (this.tasks.get(key) === task) this.tasks.delete(key);
          if (this.foreground === task) this.foreground = undefined;
        }),
    };
    this.tasks.set(key, task);
    if (!prefetch) this.foreground = task;
    return task.promise;
  }

  async identify(params: IdentifyInput, signal = new AbortController().signal) {
    const initial = identifyRules(params, []);
    if (initial.status === "collection" || initial.status === "not-music") return initial;
    const candidates = new Map<number, SongCandidate>();
    const text = songText(params.title, params.pageTitle);
    const context = `${params.title}\n${params.pageTitle ?? ""}`;
    const primary = identifyPrimaryQueries(params);
    const search = async (query: string, extra: boolean) => {
      signal.throwIfAborted();
      const songs = await this.dependencies.sources.search(query, signal);
      signal.throwIfAborted();
      for (const match of ruleMatches(songs, text, context, extra)) candidates.set(match.song.id, match.song);
    };
    for (const query of primary) await search(query, false);
    if (!candidates.size) {
      for (const query of identifyQueries(params)) {
        if (primary.includes(query)) continue;
        await search(query, true);
        if (candidates.size) break;
      }
    }
    return identifyRules(params, [...candidates.values()]);
  }

  private async run(params: AutoLyricsParams, parentSignal: AbortSignal): Promise<AutoLyricsResult> {
    const controller = new AbortController();
    const signal = AbortSignal.any([parentSignal, controller.signal]);
    const key = `${params.bvid}-${params.cid}`;
    const now = this.dependencies.now ?? (() => performance.now());
    const start = now();
    const timings: Record<string, number> = { start: 0 };
    const stamp = (stage: string) => {
      signal.throwIfAborted();
      timings[stage] = now() - start;
      this.emit({ key, phase: "stage", stage, timings: { ...timings } });
    };
    let song: SongCandidate | undefined;
    let reference: ReferenceCandidate | undefined;
    const result = (status: AutoLyricsResult["status"], extra: Partial<AutoLyricsResult> = {}): AutoLyricsResult => ({
      key,
      status,
      timings: { ...timings, done: now() - start },
      song,
      reference,
      ...extra,
    });
    try {
      signal.throwIfAborted();
      const cached = this.dependencies.cache.get(key);
      stamp("cache");
      if (
        cached?.lyrics &&
        (isManual(cached) || (!params.bypassCache && cached.algoVersion === AUTO_LYRICS_ALGO_VERSION))
      ) {
        return result("cached", { lyrics: cached, confidence: cached.confidence });
      }
      const identified = await this.identify(params, signal);
      stamp("identify");
      if (identified.status !== "matched" || !identified.song) return result(identified.status);
      song = identified.song;
      const selectedSong = song;
      const savedReference = this.referenceCache.get(song.id) ?? this.dependencies.references.get(song.id);
      const lyricPromise = this.dependencies.sources.lyric(song.id, signal).then(lyrics => {
        stamp("lyric");
        return lyrics;
      });
      const livePromise = this.dependencies.sources.audio(params.bvid, params.cid, signal).then(async bytes => {
        stamp("liveDownload");
        const pcm = await this.dependencies.decode(bytes, signal);
        stamp("liveDecode");
        return pcm;
      });
      const referenceErrors: string[] = [];
      const referencePromise = (async () => {
        const candidates = savedReference
          ? [savedReference]
          : rankReferences(
              await this.dependencies.sources.references(selectedSong, signal),
              selectedSong.duration,
            ).slice(0, 3);
        stamp("referenceSearch");
        for (let index = 0; index < candidates.length && index < 3; index++) {
          const candidate = candidates[index];
          try {
            const bytes = await this.dependencies.sources.audio(candidate.bvid, candidate.cid, signal);
            stamp(`reference${index + 1}Download`);
            const pcm = await this.dependencies.decode(bytes, signal);
            stamp(`reference${index + 1}Decode`);
            const original = parseLrc((await lyricPromise).lrc);
            if (original.length && checkVocalOnset(pcm, 22050, original[0].studio_start)) {
              return { pcm, candidate };
            }
            referenceErrors.push(`${candidate.bvid}: vocal onset mismatch`);
          } catch (error) {
            signal.throwIfAborted();
            if (isDecodeUnavailable(error)) throw error;
            referenceErrors.push(`${candidate.bvid}: ${error instanceof Error ? error.message : String(error)}`);
          }
          if (savedReference && index === 0) {
            const fallback = rankReferences(
              await this.dependencies.sources.references(selectedSong, signal),
              selectedSong.duration,
            );
            candidates.push(...fallback.filter(item => item.bvid !== savedReference.bvid).slice(0, 2));
          }
        }
        return undefined;
      })();
      // Observe every branch immediately so cancellation cannot leave unhandled rejections.
      const [lyrics, live, chosen] = await Promise.all([lyricPromise, livePromise, referencePromise]);
      const lines = parseLrc(lyrics.lrc);
      stamp("versionCheck");
      if (!chosen)
        return result("no-reference", {
          error: referenceErrors.join("; ") || "No duration-matched reference or timed lyrics",
        });
      reference = chosen.candidate;
      const studio = chosen.pcm;
      this.referenceCache.set(song.id, reference);
      this.dependencies.references.set(song.id, reference);
      const makeLyrics = (aligned: AlignResult[], confidence: number): MusicLyrics => ({
        source: "auto",
        algoVersion: AUTO_LYRICS_ALGO_VERSION,
        songId: selectedSong.id,
        refBvid: reference?.bvid,
        confidence,
        lyrics: generateLrc(aligned),
        tLyrics: lyrics.tlyric ? mapCompanionLrc(lyrics.tlyric, aligned) : undefined,
        rLyrics: lyrics.romalrc ? mapCompanionLrc(lyrics.romalrc, aligned) : undefined,
      });
      const aligned = await this.dependencies.align({ studio, live, lines }, signal, coarse => {
        stamp("coarse");
        if (coarse.confidence < 0.1) return;
        const coarseLines = lines.map(line => ({
          idx: line.idx,
          text: line.text,
          studioStart: line.studio_start,
          liveStartSec: line.studio_start + coarse.offsetSec,
          confidence: coarse.confidence,
          uncertain: false,
        }));
        this.emit({
          key,
          phase: "coarse",
          coarse,
          lyrics: makeLyrics(coarseLines, coarse.confidence),
          timings: { ...timings },
        });
      });
      stamp("fine");
      if (aligned.coarse.confidence < 0.1)
        return result("no-reference", {
          coarse: aligned.coarse,
          utilityMemoryPeakKB: aligned.memoryPeakKB,
          error: "Low alignment confidence",
        });
      const musicLyrics = makeLyrics(aligned.lines, aligned.coarse.confidence);
      // Re-read at write time: the user may have edited lyrics while this task ran.
      if (!isManual(this.dependencies.cache.get(key))) this.dependencies.cache.set(key, musicLyrics);
      stamp("cacheWrite");
      const completed = result("matched", {
        lyrics: musicLyrics,
        coarse: aligned.coarse,
        lines: aligned.lines,
        confidence: aligned.coarse.confidence,
        utilityMemoryPeakKB: aligned.memoryPeakKB,
      });
      this.emit({ key, phase: "fine", result: completed, timings: { ...timings } });
      return completed;
    } catch (error) {
      const message = error instanceof Error ? error.message : String(error);
      if (signal.aborted) return result("cancelled", { error: message });
      if (isDecodeUnavailable(error)) return result("decode-unavailable", { error: message });
      return result("error", { error: message });
    } finally {
      controller.abort();
    }
  }
}
