import type { SongCandidate } from "./identify";

import { songTitles, strongMatch } from "./identify";

export interface ReferenceCandidate {
  bvid: string;
  title: string;
  duration: number;
  cid?: number;
  checked?: boolean;
}

/** Enforce the two-second duration gate before ranking title evidence. */
export function rankReferences(
  candidates: readonly ReferenceCandidate[],
  song: SongCandidate,
): (ReferenceCandidate & { score: number })[] {
  const songDuration = song.duration;
  if (!Number.isFinite(songDuration) || songDuration <= 0) return [];
  return candidates
    .filter(c => Number.isFinite(c.duration) && c.duration > 0 && Math.abs(c.duration - songDuration) <= 2)
    .filter(c => songTitles(song).some(title => strongMatch(title, c.title)))
    .map(c => {
      let score = Math.abs(c.duration - songDuration) <= 1 ? 60 : 40;
      if (/官方|原曲|\bcd\b|cd音源|hi-?res|完整版|无损/i.test(c.title)) score += 25;
      if (/罗马音|假名|歌词|扒谱|动态鼓谱|总谱/.test(c.title)) score += 15;
      if (/live|现场|現場|ライブ|演唱会|巡演|切片/i.test(c.title)) score -= 50;
      if (
        /(?<![a-z])(?:mv|pv)(?![a-z])|music ?video|project ?diva|游戏|遊戲|ゲーム|(?<![a-z])game(?![a-z])/i.test(
          c.title,
        )
      )
        score -= 30;
      if (/翻唱|cover|(?<![a-z])ai(?![a-z])|试听|伴奏|无贝斯|无吉他|反应|reaction|教[学學]/i.test(c.title)) score -= 80;
      return { ...c, score };
    })
    .sort(
      (a, b) =>
        b.score - a.score ||
        Math.abs(a.duration - songDuration) - Math.abs(b.duration - songDuration) ||
        a.bvid.localeCompare(b.bvid),
    );
}

/**
 * Energy-onset sanity check, not vocal separation. Within ±1.5 s of the first lyric,
 * accept either a sharp 100 ms jump or a sustained 500 ms entrance from a quieter bed.
 * A small flicker on an already-flat bed is not an onset. Accompaniment can still pass.
 * A cold open (声なき魚 sings at 0.42 s) has no bed to rise from; there the reference only has to be audible.
 */
export function checkVocalOnset(pcm: Float32Array, sampleRate: number, firstLineSec: number): boolean {
  if (!Number.isFinite(sampleRate) || sampleRate <= 0 || !Number.isFinite(firstLineSec) || firstLineSec < 0) {
    return false;
  }
  if (firstLineSec < 1.5) return audible(pcm, sampleRate, firstLineSec + 1.5);
  return (
    hasEnergyRise(pcm, sampleRate, firstLineSec, 0.1, 1.6, 0.01) ||
    hasEnergyRise(pcm, sampleRate, firstLineSec, 0.5, 1.6, 0.002)
  );
}

function hasEnergyRise(
  pcm: Float32Array,
  sampleRate: number,
  firstLineSec: number,
  windowSec: number,
  minRatio: number,
  minDelta: number,
): boolean {
  const window = Math.max(1, Math.round(sampleRate * windowSec));
  const start = Math.max(window, Math.floor((firstLineSec - 1.5) * sampleRate));
  const end = Math.min(pcm.length - window, Math.ceil((firstLineSec + 1.5) * sampleRate));
  const energy = (from: number) => {
    let sum = 0;
    for (let i = from; i < from + window; i++) sum += pcm[i] * pcm[i];
    return sum / window;
  };
  for (let center = start; center <= end; center += Math.max(1, Math.floor(window / 2))) {
    const before = energy(center - window);
    const after = energy(center);
    if (after >= before * minRatio && after - before >= minDelta) return true;
  }
  return false;
}

function audible(pcm: Float32Array, sampleRate: number, untilSec: number): boolean {
  const end = Math.min(pcm.length, Math.ceil(untilSec * sampleRate));
  let sum = 0;
  for (let i = 0; i < end; i++) sum += pcm[i] * pcm[i];
  return end > 0 && sum / end >= 1e-4;
}
