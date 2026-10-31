import type {
  AlignLine,
  AlignResult,
  AudioFeatureSet,
  GapDTWOptions,
  GlobalOffset,
  MedleyMatch,
  MedleySong,
} from "./types";

/**
 * DTW and Audio Alignment Algorithms in pure JS.
 * Zero external dependencies.
 */

function dot12(a: Float32Array, b: Float32Array): number {
  return (
    a[0] * b[0] +
    a[1] * b[1] +
    a[2] * b[2] +
    a[3] * b[3] +
    a[4] * b[4] +
    a[5] * b[5] +
    a[6] * b[6] +
    a[7] * b[7] +
    a[8] * b[8] +
    a[9] * b[9] +
    a[10] * b[10] +
    a[11] * b[11]
  );
}

/** Locate five windows of a in b; positive lag means b plays the same music later. */
export function referenceLag(
  a: AudioFeatureSet,
  b: AudioFeatureSet,
): { lagSec: number; spreadSec: number; sim: number } {
  const winFrames = Math.floor(Math.min(12, a.duration * 0.25) * a.fps);
  if (winFrames < 1 || a.numFrames < winFrames || b.numFrames < winFrames) {
    return { lagSec: 0, spreadSec: 0, sim: 0 };
  }
  const votes = [0.1, 0.3, 0.5, 0.7, 0.9].map(fraction => {
    const start = Math.min(Math.floor(a.numFrames * fraction), a.numFrames - winFrames);
    let bestSim = -1;
    let bestFrame = 0;
    for (let frame = 0; frame <= b.numFrames - winFrames; frame++) {
      let sum = 0;
      for (let k = 0; k < winFrames; k += 2) sum += dot12(a.cens[start + k], b.cens[frame + k]);
      const sim = sum / Math.ceil(winFrames / 2);
      if (sim > bestSim) {
        bestSim = sim;
        bestFrame = frame;
      }
    }
    return { lag: (bestFrame - start) / a.fps, sim: bestSim };
  });
  const lags = votes.map(vote => vote.lag).sort((a, b) => a - b);
  return {
    lagSec: lags[2],
    spreadSec: lags[4] - lags[0],
    sim: votes.reduce((sum, vote) => sum + vote.sim, 0) / votes.length,
  };
}

/** First ranked reference agreeing within 0.3 s lag/spread and similarity >= 0.8; fall back to rank zero. */
export function chooseConsensus(
  lags: { i: number; j: number; lagSec: number; spreadSec: number; sim: number }[],
  count: number,
): { index: number; consensus: boolean } {
  if (count === 1) return { index: 0, consensus: true };
  for (let index = 0; index < count; index++) {
    if (
      lags.some(
        lag =>
          (lag.i === index || lag.j === index) && Math.abs(lag.lagSec) < 0.3 && lag.spreadSec < 0.3 && lag.sim >= 0.8,
      )
    ) {
      return { index, consensus: true };
    }
  }
  return { index: 0, consensus: false };
}

export function selectReference(studios: AudioFeatureSet[]) {
  const referenceLags = studios.flatMap((a, i) =>
    studios.slice(i + 1).map((b, offset) => ({ i, j: i + offset + 1, ...referenceLag(a, b) })),
  );
  const { index: referenceIndex, consensus: referenceConsensus } = chooseConsensus(referenceLags, studios.length);
  return { referenceIndex, referenceConsensus, referenceLags };
}

/**
 * 1. Coarse Alignment: Global Offset Estimation
 * Estimates the start time of the reference song in live audio. Without anchors one window just after the studio
 * start decides. Lives often rework the intro, so with lyric anchors (studio seconds, see lyricAnchors) the intro
 * window and one window per anchor vote, and the largest group agreeing within 0.3 s wins.
 */
export function estimateGlobalOffset(
  studioFeat: AudioFeatureSet,
  liveFeat: AudioFeatureSet,
  maxSearchSec: number | null = null,
  anchorsSec: readonly number[] = [],
): GlobalOffset {
  const fps = studioFeat.fps;
  const winSec = Math.min(12.0, studioFeat.duration * 0.25);
  const winFrames = Math.floor(winSec * fps);
  const liveMaxFrames = maxSearchSec
    ? Math.min(liveFeat.numFrames, Math.floor(maxSearchSec * fps))
    : liveFeat.numFrames;

  if (liveMaxFrames < winFrames || studioFeat.numFrames < winFrames) {
    return { offsetSec: 0, confidence: 0 };
  }

  const scan = (refStart: number, earliestPeak: boolean) =>
    scanWindow(studioFeat, liveFeat, refStart, winFrames, liveMaxFrames, earliestPeak);
  const intro = scan(Math.min(Math.floor(1.0 * fps), studioFeat.numFrames - winFrames), true);
  if (!anchorsSec.length) return intro;

  // A chorus window can land on another chorus; agreement between windows outvotes it.
  const votes = [
    intro,
    ...anchorsSec.map(sec =>
      scan(Math.max(0, Math.min(Math.floor((sec - 0.5) * fps), studioFeat.numFrames - winFrames)), false),
    ),
  ];
  const sum = (group: GlobalOffset[], key: "offsetSec" | "confidence") =>
    group.reduce((total, vote) => total + vote[key], 0);
  const best = votes
    .map(vote => votes.filter(other => Math.abs(other.offsetSec - vote.offsetSec) < 0.3))
    .reduce((a, b) =>
      b.length > a.length || (b.length === a.length && sum(b, "confidence") > sum(a, "confidence")) ? b : a,
    );
  // No two windows agree: a Live that repeats choruses scatters them, so keep the intro estimate.
  if (best.length < 2) return intro;
  const confidence = sum(best, "confidence") / best.length;
  return { offsetSec: sum(best, "offsetSec") / best.length, confidence, bestSim: confidence };
}

/** Studio times of the first lyric line and the lines a third and two thirds in, for estimateGlobalOffset. */
export function lyricAnchors(lines: readonly AlignLine[]): number[] {
  const picks = new Set([0, Math.floor(lines.length / 3), Math.floor((lines.length * 2) / 3)]);
  return lines.length ? [...picks].map(index => lines[index].studio_start) : [];
}

/** Best Live position of one studio window; earliestPeak prefers the first of near-equal peaks (intro window). */
function scanWindow(
  studioFeat: AudioFeatureSet,
  liveFeat: AudioFeatureSet,
  refStart: number,
  winFrames: number,
  liveMaxFrames: number,
  earliestPeak: boolean,
): GlobalOffset {
  const fps = studioFeat.fps;
  const refCens = studioFeat.cens.slice(refStart, refStart + winFrames);

  const scores: { frame: number; sim: number }[] = [];
  const step = 2;
  let bestSim = -1;
  let bestOffset = 0;

  for (let l = 0; l <= liveMaxFrames - winFrames; l += step) {
    let simSum = 0;
    for (let k = 0; k < winFrames; k += 2) {
      simSum += dot12(refCens[k], liveFeat.cens[l + k]);
    }
    const avgSim = simSum / (winFrames / 2);
    scores.push({ frame: l, sim: avgSim });
    if (avgSim > bestSim) {
      bestSim = avgSim;
      bestOffset = l;
    }
  }

  // Detect earliest significant peak
  const peaks: typeof scores = [];
  for (let i = 1; earliestPeak && i < scores.length - 1; i++) {
    if (scores[i].sim > scores[i - 1].sim && scores[i].sim > scores[i + 1].sim && scores[i].sim > 0.65) {
      peaks.push(scores[i]);
    }
  }

  let chosen = { frame: bestOffset, sim: bestSim };
  if (peaks.length > 0) {
    peaks.sort((a, b) => b.sim - a.sim);
    const topScore = peaks[0].sim;
    const candidates = peaks.filter(p => p.sim >= topScore * 0.88);
    candidates.sort((a, b) => a.frame - b.frame);
    chosen = candidates[0];
  }

  const offsetFrames = chosen.frame - refStart;
  const offsetSec = offsetFrames / fps;

  return {
    offsetSec,
    confidence: Math.max(0, Math.min(1, chosen.sim)),
    bestSim: chosen.sim,
  };
}

/**
 * Medley Finder: Locates start and end times of each song in a long medley audio.
 */
export function findMedleySongs(songs: MedleySong[], liveFeat: AudioFeatureSet): MedleyMatch[] {
  const fps = liveFeat.fps;
  const results: MedleyMatch[] = [];
  const winSec = 10.0;
  const winFrames = Math.floor(winSec * fps);

  for (const song of songs) {
    const sFeat = song.studioFeat;
    const ref = sFeat.cens.slice(0, winFrames);

    let bestSim = -1;
    let bestStartFrame = 0;

    for (let l = 0; l <= liveFeat.numFrames - winFrames; l += 4) {
      let sim = 0;
      for (let k = 0; k < winFrames; k += 2) {
        sim += dot12(ref[k], liveFeat.cens[l + k]);
      }
      sim /= winFrames / 2;
      if (sim > bestSim) {
        bestSim = sim;
        bestStartFrame = l;
      }
    }

    const startSec = bestStartFrame / fps;

    // Determine end time by tracking when match degrades
    let endSec = startSec + winSec;
    const checkWin = Math.floor(2.0 * fps);
    const maxDur = Math.min(liveFeat.duration - startSec, sFeat.duration);

    for (let t = startSec + winSec; t < startSec + maxDur; t += 1.0) {
      const sIdx = Math.floor((t - startSec) * fps);
      const lIdx = Math.floor(t * fps);
      if (sIdx + checkWin >= sFeat.numFrames || lIdx + checkWin >= liveFeat.numFrames) break;

      let sim = 0;
      for (let k = 0; k < checkWin; k += 2) {
        sim += dot12(sFeat.cens[sIdx + k], liveFeat.cens[lIdx + k]);
      }
      sim /= checkWin / 2;

      if (sim >= 0.85) {
        endSec = t + 1.0;
      } else {
        // Drop detected
        break;
      }
    }

    results.push({
      id: song.id,
      startSec,
      endSec: Math.min(liveFeat.duration, endSec),
      duration: endSec - startSec,
      confidence: bestSim,
    });
  }

  // Sort by startSec
  results.sort((a, b) => a.startSec - b.startSec);
  return results;
}

/**
 * Variant 2: Gap-Penalized Elastic DTW (GapDTW)
 */
export function alignLinesGapDTW(
  lines: AlignLine[],
  studioFeat: AudioFeatureSet,
  liveFeat: AudioFeatureSet,
  options: GapDTWOptions = {},
): AlignResult[] {
  const fps = studioFeat.fps;
  const S = studioFeat.numFrames;
  const L = liveFeat.numFrames;

  // Optimized gap penalties for MC accommodation
  const gapLiveOpen = options.gapLiveOpen !== undefined ? options.gapLiveOpen : 0.25;
  const gapLiveExt = options.gapLiveExt !== undefined ? options.gapLiveExt : 0.015;
  const gapStudioSkip = options.gapStudioSkip !== undefined ? options.gapStudioSkip : 0.4;
  const onsetRefineSec = options.onsetRefineSec ?? 0;
  const outlierWindow = options.outlierWindow ?? 3;

  const step = 2; // ~21.5 fps for speed & memory
  const sSteps = Math.floor(S / step);
  const lSteps = Math.floor(L / step);

  const traceback = new Uint8Array(sSteps * lSteps);

  let prevM = new Float32Array(lSteps);
  let prevG_live = new Float32Array(lSteps);
  let prevG_studio = new Float32Array(lSteps);

  let currM = new Float32Array(lSteps);
  let currG_live = new Float32Array(lSteps);
  let currG_studio = new Float32Array(lSteps);

  // Free start on live axis for leading MC/intro
  for (let l = 0; l < lSteps; l++) {
    prevM[l] = 0;
    prevG_live[l] = 0;
    prevG_studio[l] = 1e9;
  }

  for (let s = 1; s < sSteps; s++) {
    const sFeatVec = studioFeat.cens[s * step];
    const tbRowOffset = s * lSteps;

    currM[0] = 1e9;
    currG_live[0] = 1e9;
    currG_studio[0] = prevG_studio[0] + gapStudioSkip;

    for (let l = 1; l < lSteps; l++) {
      const lFeatVec = liveFeat.cens[l * step];
      const dist = 1.0 - dot12(sFeatVec, lFeatVec);

      // Studio skip transition (vertical)
      const costSkipM = prevM[l] + gapStudioSkip;
      const costSkipExt = prevG_studio[l] + gapStudioSkip * 0.7;
      const minSkip = costSkipM < costSkipExt ? costSkipM : costSkipExt;
      currG_studio[l] = minSkip;

      // Live gap transition (horizontal - MC/applause)
      const costLiveOpen = currM[l - 1] + gapLiveOpen;
      const costLiveExt = currG_live[l - 1] + gapLiveExt;
      const minLive = costLiveOpen < costLiveExt ? costLiveOpen : costLiveExt;
      currG_live[l] = minLive;

      // Match transition (diagonal)
      const prevMin = Math.min(prevM[l - 1], Math.min(prevG_live[l - 1], prevG_studio[l - 1]));
      currM[l] = prevMin + dist;

      let bestState = 0;
      let minOverall = currM[l];
      if (currG_live[l] < minOverall) {
        minOverall = currG_live[l];
        bestState = 1;
      }
      if (currG_studio[l] < minOverall) {
        minOverall = currG_studio[l];
        bestState = 2;
      }
      traceback[tbRowOffset + l] = bestState;
    }

    const tM = prevM;
    prevM = currM;
    currM = tM;
    const tL = prevG_live;
    prevG_live = currG_live;
    currG_live = tL;
    const tS = prevG_studio;
    prevG_studio = currG_studio;
    currG_studio = tS;
  }

  // Traceback
  const studioToLive = new Int32Array(sSteps);
  const studioState = new Uint8Array(sSteps);
  for (let i = 0; i < sSteps; i++) studioToLive[i] = -1;

  let curS = sSteps - 1;
  let curL = lSteps - 1;
  while (curS > 0 && curL > 0) {
    const st = traceback[curS * lSteps + curL];
    studioToLive[curS] = curL * step;
    studioState[curS] = st;

    if (st === 0) {
      curS--;
      curL--;
    } else if (st === 1) {
      curL--;
    } else {
      curS--;
    }
  }

  let lastSeen = 0;
  for (let s = 0; s < sSteps; s++) {
    if (studioToLive[s] >= 0) {
      lastSeen = studioToLive[s];
    } else {
      studioToLive[s] = lastSeen;
    }
  }

  const results: AlignResult[] = [];
  for (const line of lines) {
    const sSec = line.studio_start;
    const sStepIdx = Math.min(sSteps - 1, Math.max(0, Math.floor((sSec * fps) / step)));
    const liveFrame = studioToLive[sStepIdx];
    let liveSec = liveFrame / fps;

    const sFeatVec = studioFeat.cens[Math.min(studioFeat.numFrames - 1, sStepIdx * step)];
    const lFeatVec = liveFeat.cens[Math.min(liveFeat.numFrames - 1, liveFrame)];
    const sim = dot12(sFeatVec, lFeatVec);
    const inGap = studioState[sStepIdx] === 1 || studioState[sStepIdx] === 2;

    if (onsetRefineSec > 0) liveSec = refineWithOnset(liveFeat, liveSec, onsetRefineSec);

    const confidence = inGap ? Math.min(0.3, sim * 0.5) : Math.max(0, Math.min(1, sim));
    const uncertain = inGap || confidence < 0.65;

    results.push({
      idx: line.idx,
      text: line.text,
      studioStart: sSec,
      liveStartSec: liveSec,
      confidence,
      uncertain,
    });
  }

  if (outlierWindow > 0) smoothOutliers(results, outlierWindow);
  return results;
}

/**
 * Within a song the Live-minus-studio offset is piecewise constant, but the path can wander 0.4–1 s at section
 * changes (chorus entries after a break). A line whose offset is far from both the preceding and the following
 * neighbours' median takes the nearer median; a line beside a real step (cut or held pause) matches one side and
 * stays. The first and last lines have one side only and are left alone.
 */
function smoothOutliers(results: AlignResult[], window: number) {
  const offsets = results.map(result => result.liveStartSec - result.studioStart);
  const median = (values: number[]) => {
    const sorted = [...values].sort((a, b) => a - b);
    const mid = sorted.length >> 1;
    return sorted.length % 2 ? sorted[mid] : (sorted[mid - 1] + sorted[mid]) / 2;
  };
  results.forEach((result, index) => {
    if (index === 0 || index === results.length - 1) return;
    const before = median(offsets.slice(Math.max(0, index - window), index));
    const after = median(offsets.slice(index + 1, index + 1 + window));
    const own = offsets[index];
    if (Math.abs(own - before) <= 0.25 || Math.abs(own - after) <= 0.25) return;
    result.liveStartSec = result.studioStart + (Math.abs(own - before) < Math.abs(own - after) ? before : after);
  });
}

function refineWithOnset(feat: AudioFeatureSet, timestampSec: number, radiusSec = 0.25): number {
  const fps = feat.fps;
  const center = Math.floor(timestampSec * fps);
  const radius = Math.floor(radiusSec * fps);
  const start = Math.max(0, center - radius);
  const end = Math.min(feat.numFrames - 1, center + radius);

  let maxVal = -1;
  let bestFrame = center;
  for (let f = start; f <= end; f++) {
    const v = feat.onset[f];
    if (v > maxVal) {
      maxVal = v;
      bestFrame = f;
    }
  }

  if (maxVal > 0.05) {
    return bestFrame / fps;
  }
  return timestampSec;
}

/**
 * How well a reference explains the Live, for choosing between candidate songs. Coarse confidence cannot do this:
 * a 12 s chroma window of a different song by the same band often scores higher than the right song. The per-line
 * path can: mean line confidence times the share of confident lines (lyrics-proto cross bench: same song ≥ 0.79,
 * different song ≤ 0.67). A TV-size Live scores low for every candidate because GapDTW cannot skip the cut
 * sections, so arbitration leaves it unidentified rather than guessing.
 */
export function verifyScore(lines: readonly AlignResult[]): number {
  if (!lines.length) return 0;
  const mean = lines.reduce((sum, line) => sum + line.confidence, 0) / lines.length;
  return mean * (lines.filter(line => !line.uncertain).length / lines.length);
}
