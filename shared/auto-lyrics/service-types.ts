import type { IdentifyInput, SongCandidate } from "./identify";
import type { ReferenceCandidate } from "./reference";
import type { AlignResult, GlobalOffset } from "./types";

export interface AutoLyricsParams extends IdentifyInput {
  bvid: string;
  cid: number;
  bypassCache?: boolean;
}

/** Milliseconds since this task started, including network and decode work. */
export type AutoLyricsTimings = Record<string, number>;

export interface AutoLyricsResult {
  key: string;
  status:
    | "matched"
    | "cached"
    | "unidentified"
    | "not-music"
    | "collection"
    | "no-reference"
    | "decode-unavailable"
    | "cancelled"
    | "error";
  timings: AutoLyricsTimings;
  lyrics?: MusicLyrics;
  song?: SongCandidate;
  reference?: ReferenceCandidate;
  coarse?: GlobalOffset;
  lines?: AlignResult[];
  confidence?: number;
  utilityMemoryPeakKB?: number;
  error?: string;
}

export type AutoLyricsProgress =
  | { key: string; phase: "stage"; stage: string; timings: AutoLyricsTimings }
  | { key: string; phase: "coarse"; coarse: GlobalOffset; lyrics: MusicLyrics; timings: AutoLyricsTimings }
  | { key: string; phase: "fine"; result: AutoLyricsResult; timings: AutoLyricsTimings };
