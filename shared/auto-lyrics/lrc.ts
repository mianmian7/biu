import type { AlignLine, AlignResult } from "./types";

/** NetEase credit rows such as "作词 : xxx" are metadata, not sung lines. */
const CREDIT =
  /^(?:作词|作詞|作曲|编曲|編曲|制作人|製作人|监制|混音|母带|和声|吉他|贝斯|鼓|键盘|弦乐|词|曲|lyrics|lyricist|composer|arranger|producer)\s*[:：]/i;

/** Expand every time tag, discard empty lyrics and credits, and assign indices in time order. */
export function parseLrc(source: string): AlignLine[] {
  const entries: Omit<AlignLine, "idx">[] = [];
  for (const row of source.split(/\r?\n/)) {
    const tags = [...row.matchAll(/\[(\d+):(\d{2})(?:[.:](\d{1,3}))?\]/g)];
    const text = row.replace(/\[\d+:\d{2}(?:[.:]\d{1,3})?\]/g, "").trim();
    if (!text || CREDIT.test(text)) continue;
    for (const tag of tags) {
      const seconds = Number(tag[2]);
      if (seconds >= 60) continue;
      const fraction = tag[3] ? Number(`0.${tag[3]}`) : 0;
      entries.push({ studio_start: Number(tag[1]) * 60 + seconds + fraction, text });
    }
  }
  return entries.sort((a, b) => a.studio_start - b.studio_start).map((entry, idx) => ({ idx, ...entry }));
}

/** LRC timestamps have centisecond precision; round before splitting minutes. */
export function generateLrc(lines: readonly AlignResult[]): string {
  return [...lines]
    .filter(line => line.text.trim() && Number.isFinite(line.liveStartSec))
    .sort((a, b) => a.liveStartSec - b.liveStartSec)
    .map(line => {
      const centiseconds = Math.max(0, Math.round(line.liveStartSec * 100));
      const minutes = String(Math.floor(centiseconds / 6000)).padStart(2, "0");
      const seconds = String(Math.floor((centiseconds % 6000) / 100)).padStart(2, "0");
      const fraction = String(centiseconds % 100).padStart(2, "0");
      return `[${minutes}:${seconds}.${fraction}]${line.text.trim()}`;
    })
    .join("\n");
}

/** Map source timestamps, never text or row numbers; omitted lines stay omitted. */
export function mapCompanionLrc(source: string, aligned: readonly AlignResult[]): string {
  const companions = parseLrc(source);
  const mapped: AlignResult[] = [];
  for (const line of aligned) {
    // NetEase companions occasionally round milliseconds to centiseconds.
    const matches = companions.filter(c => Math.abs(c.studio_start - line.studioStart) <= 0.011);
    for (const companion of matches) mapped.push({ ...line, text: companion.text });
  }
  return generateLrc(mapped);
}
