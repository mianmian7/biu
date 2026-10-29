import type { IdentifyInput, SongCandidate } from "../../shared/auto-lyrics/identify";
import type { ReferenceCandidate } from "../../shared/auto-lyrics/reference";
import type { AutoLyricsParams, AutoLyricsProgress, AutoLyricsResult } from "../../shared/auto-lyrics/service-types";
import type { AlignLine, AlignResult, GlobalOffset } from "../../shared/auto-lyrics/types";

import { selectReference, verifyScore } from "../../shared/auto-lyrics/align";
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

// Correct device matches scored 0.71–0.88; the wrong 空の箱 reference scored 0.44.
const VERIFY_MIN_SINGLE = 0.6;

export const AUTO_LYRICS_ALGO_VERSION = "p1-gap-dtw-4";

export interface AutoLyricsDependencies {
  sources: {
    search(query: string, signal: AbortSignal): Promise<SongCandidate[]>;
    lyric(songId: number, signal: AbortSignal): Promise<{ lrc: string; tlyric?: string; romalrc?: string }>;
    references(song: SongCandidate, signal: AbortSignal): Promise<ReferenceCandidate[]>;
    audio(bvid: string, cid: number | undefined, signal: AbortSignal): Promise<Uint8Array>;
  };
  decode(data: Uint8Array, signal: AbortSignal): Promise<Float32Array>;
  align(
    input: { studios: Float32Array[]; live: Float32Array; lines: AlignLine[] },
    signal: AbortSignal,
    onCoarse: (coarse: GlobalOffset) => void,
  ): Promise<
    { coarse: GlobalOffset; lines: AlignResult[]; memoryPeakKB?: number } & ReturnType<typeof selectReference>
  >;
  cache: { get(key: string): MusicLyrics | undefined; set(key: string, value: MusicLyrics): void };
  references: {
    get(songId: number): ReferenceCandidate | undefined;
    set(songId: number, value: ReferenceCandidate): void;
  };
  now?: () => number;
}

/** verifyScore a contender needs, and its lead over the runner-up (cross bench: same song ≥ 0.79, other ≤ 0.67). */
const VERIFY_MIN_SCORE = 0.72;
const VERIFY_MIN_MARGIN = 0.05;

interface Attempt {
  song: SongCandidate;
  status: "matched" | "no-reference";
  reference?: ReferenceCandidate;
  referenceCheck?: AutoLyricsResult["referenceCheck"];
  coarse?: GlobalOffset;
  lines?: AlignResult[];
  verifyScore?: number;
  lyrics?: MusicLyrics;
  memoryPeakKB?: number;
  error?: string;
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

  /** Lyrics, a version-checked reference and alignment for one candidate song against the shared Live audio. */
  private async attempt(
    song: SongCandidate,
    livePromise: Promise<Float32Array>,
    signal: AbortSignal,
    stamp: (stage: string) => void,
    onCoarse?: (event: { coarse: GlobalOffset; lyrics: MusicLyrics }) => void,
  ): Promise<Attempt> {
    const storedReference = this.referenceCache.get(song.id) ?? this.dependencies.references.get(song.id);
    // A saved reference must still pass today's gates (title naming the song, duration).
    const savedReference =
      storedReference?.checked && rankReferences([storedReference], song).length ? storedReference : undefined;
    const lyricPromise = this.dependencies.sources.lyric(song.id, signal).then(lyrics => {
      stamp("lyric");
      return lyrics;
    });
    const referenceErrors: string[] = [];
    const referencePromise = (async () => {
      const load = async (candidate: ReferenceCandidate, index: number) => {
        const bytes = await this.dependencies.sources.audio(candidate.bvid, candidate.cid, signal);
        stamp(`reference${index + 1}Download`);
        const pcm = await this.dependencies.decode(bytes, signal);
        stamp(`reference${index + 1}Decode`);
        const original = parseLrc((await lyricPromise).lrc);
        if (!original.length || !checkVocalOnset(pcm, 22050, original[0].studio_start)) {
          throw new Error("vocal onset mismatch");
        }
        return { pcm, candidate };
      };
      const loadCandidates = async (candidates: ReferenceCandidate[], offset = 0) => {
        const results = await Promise.allSettled(candidates.map((candidate, index) => load(candidate, index + offset)));
        signal.throwIfAborted();
        return results.flatMap((result, index) => {
          if (result.status === "fulfilled") return [result.value];
          if (isDecodeUnavailable(result.reason)) throw result.reason;
          referenceErrors.push(
            `${candidates[index].bvid}: ${result.reason instanceof Error ? result.reason.message : String(result.reason)}`,
          );
          return [];
        });
      };
      if (savedReference) {
        stamp("referenceSearch");
        const saved = await loadCandidates([savedReference]);
        if (saved.length) return saved;
      }
      const candidates = rankReferences(await this.dependencies.sources.references(song, signal), song)
        .filter(candidate => candidate.bvid !== savedReference?.bvid)
        .slice(0, savedReference ? 2 : 3);
      stamp("referenceSearch");
      return loadCandidates(candidates, savedReference ? 1 : 0);
    })();
    // Observe every branch immediately so cancellation cannot leave unhandled rejections.
    const [lyrics, live, chosen] = await Promise.all([lyricPromise, livePromise, referencePromise]);
    const lines = parseLrc(lyrics.lrc);
    stamp("versionCheck");
    if (!chosen.length)
      return {
        song,
        status: "no-reference",
        error: referenceErrors.join("; ") || "No duration-matched reference or timed lyrics",
      };
    let reference: ReferenceCandidate | undefined = chosen.length === 1 ? chosen[0].candidate : undefined;
    const makeLyrics = (aligned: AlignResult[], confidence: number): MusicLyrics => ({
      source: "auto",
      algoVersion: AUTO_LYRICS_ALGO_VERSION,
      songId: song.id,
      refBvid: reference?.bvid,
      confidence,
      lyrics: generateLrc(aligned),
      tLyrics: lyrics.tlyric ? mapCompanionLrc(lyrics.tlyric, aligned) : undefined,
      rLyrics: lyrics.romalrc ? mapCompanionLrc(lyrics.romalrc, aligned) : undefined,
    });
    const aligned = await this.dependencies.align(
      { studios: chosen.map(item => item.pcm), live, lines },
      signal,
      coarse => {
        stamp("coarse");
        if (!onCoarse || coarse.confidence < 0.1) return;
        const coarseLines = lines.map(line => ({
          idx: line.idx,
          text: line.text,
          studioStart: line.studio_start,
          liveStartSec: line.studio_start + coarse.offsetSec,
          confidence: coarse.confidence,
          uncertain: false,
        }));
        onCoarse({ coarse, lyrics: makeLyrics(coarseLines, coarse.confidence) });
      },
    );
    reference = { ...chosen[aligned.referenceIndex].candidate, checked: true };
    stamp("fine");
    const base = { song, reference, coarse: aligned.coarse, memoryPeakKB: aligned.memoryPeakKB };
    if (aligned.coarse.confidence < 0.1) return { ...base, status: "no-reference", error: "Low alignment confidence" };
    return {
      ...base,
      status: "matched",
      referenceCheck: {
        candidates: chosen.map(item => item.candidate.bvid),
        lags: aligned.referenceLags,
        chosen: aligned.referenceIndex,
        consensus: aligned.referenceConsensus,
      },
      lines: aligned.lines,
      verifyScore: verifyScore(aligned.lines),
      lyrics: makeLyrics(aligned.lines, aligned.coarse.confidence),
    };
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
      if (identified.status !== "matched" && identified.status !== "ambiguous") return result(identified.status);
      const arbitrate = identified.status === "ambiguous";
      const songs = arbitrate ? identified.contenders : [identified.song];
      const livePromise = this.dependencies.sources.audio(params.bvid, params.cid, signal).then(async bytes => {
        stamp("liveDownload");
        const pcm = await this.dependencies.decode(bytes, signal);
        stamp("liveDecode");
        return pcm;
      });
      // Observe the shared Live branch now so a failed first contender cannot leave an unhandled rejection.
      livePromise.catch(() => {});
      const attempts: Attempt[] = [];
      for (const [index, candidate] of songs.entries()) {
        if (!arbitrate) {
          attempts.push(
            await this.attempt(candidate, livePromise, signal, stamp, event =>
              this.emit({ key, phase: "coarse", ...event, timings: { ...timings } }),
            ),
          );
          continue;
        }
        try {
          attempts.push(await this.attempt(candidate, livePromise, signal, stage => stamp(`${stage}#${index + 1}`)));
        } catch (error) {
          signal.throwIfAborted();
          await livePromise;
          if (isDecodeUnavailable(error)) throw error;
          attempts.push({
            song: candidate,
            status: "no-reference",
            error: error instanceof Error ? error.message : String(error),
          });
        }
      }
      const contenders = arbitrate
        ? attempts.map(a => ({ song: a.song, status: a.status, verifyScore: a.verifyScore, error: a.error }))
        : undefined;
      const best = attempts
        .filter(a => a.status === "matched")
        .sort((a, b) => (b.verifyScore ?? 0) - (a.verifyScore ?? 0));
      const [first, second] = best;
      const chosen =
        arbitrate &&
        first &&
        (first.verifyScore ?? 0) >= VERIFY_MIN_SCORE &&
        (!second || (first.verifyScore ?? 0) - (second.verifyScore ?? 0) >= VERIFY_MIN_MARGIN)
          ? first
          : arbitrate
            ? undefined
            : attempts[0];
      if (!chosen) return result("unidentified", { contenders, error: "Alignment could not confirm a contender" });
      song = chosen.song;
      reference = chosen.reference;
      if (!arbitrate && chosen.status === "matched" && (chosen.verifyScore ?? 0) < VERIFY_MIN_SINGLE)
        return result("no-reference", {
          error: "Line alignment score too low",
          verifyScore: chosen.verifyScore,
          referenceCheck: chosen.referenceCheck,
          coarse: chosen.coarse,
        });
      if (chosen.status !== "matched" || !chosen.lyrics || !chosen.reference)
        return result(chosen.status, {
          coarse: chosen.coarse,
          utilityMemoryPeakKB: chosen.memoryPeakKB,
          error: chosen.error,
        });
      this.referenceCache.set(chosen.song.id, chosen.reference);
      this.dependencies.references.set(chosen.song.id, chosen.reference);
      // Re-read at write time: the user may have edited lyrics while this task ran.
      if (!isManual(this.dependencies.cache.get(key))) this.dependencies.cache.set(key, chosen.lyrics);
      stamp("cacheWrite");
      const completed = result("matched", {
        lyrics: chosen.lyrics,
        coarse: chosen.coarse,
        lines: chosen.lines,
        confidence: chosen.coarse?.confidence,
        referenceCheck: chosen.referenceCheck,
        verifyScore: chosen.verifyScore,
        contenders,
        utilityMemoryPeakKB: chosen.memoryPeakKB,
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
