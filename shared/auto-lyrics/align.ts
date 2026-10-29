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

/**
 * 1. Coarse Alignment: Global Offset Estimation
 * Estimates the start time of the reference song in live audio.
 */
export function estimateGlobalOffset(
  studioFeat: AudioFeatureSet,
  liveFeat: AudioFeatureSet,
  maxSearchSec: number | null = null,
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

  const refStart = Math.min(Math.floor(1.0 * fps), studioFeat.numFrames - winFrames);
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
  for (let i = 1; i < scores.length - 1; i++) {
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

    liveSec = refineWithOnset(liveFeat, liveSec, 0.25);

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

  return results;
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
