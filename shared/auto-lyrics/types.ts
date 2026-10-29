/** All timestamps and durations are in seconds. */
export interface AlignLine {
  idx: number;
  studio_start: number;
  text: string;
}

export interface AlignResult {
  idx: number;
  text: string;
  studioStart: number;
  liveStartSec: number;
  confidence: number;
  uncertain: boolean;
}

export interface AudioFeatureOptions {
  sampleRate?: number;
  nFft?: number;
  hopSize?: number;
  minFreq?: number;
  maxFreq?: number;
}

export interface AudioFeatureSet {
  chroma: Float32Array[];
  cens: Float32Array[];
  onset: Float32Array;
  fps: number;
  numFrames: number;
  duration: number;
}

export interface GlobalOffset {
  offsetSec: number;
  confidence: number;
  bestSim?: number;
}

export interface GapDTWOptions {
  gapLiveOpen?: number;
  gapLiveExt?: number;
  gapStudioSkip?: number;
  /** Snap each line to the strongest onset within this radius; 0 disables (full-band onsets are mostly drums). */
  onsetRefineSec?: number;
  /** Replace a line offset > 0.25 s from the medians of both its N preceding and N following lines; 0 disables. */
  outlierWindow?: number;
}

export interface MedleySong {
  id: string;
  studioFeat: AudioFeatureSet;
}

export interface MedleyMatch {
  id: string;
  startSec: number;
  endSec: number;
  duration: number;
  confidence: number;
}
