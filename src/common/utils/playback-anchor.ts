import { create } from "zustand";

/** 跨渲染进程可比较、进程内单调递增的时钟（毫秒）。 */
export const anchorNow = () => performance.timeOrigin + performance.now();

export const usePlaybackAnchor = create<DesktopLyricsAnchor>(() => ({
  mediaMs: 0,
  at: anchorNow(),
  rate: 1,
  running: false,
  seq: 0,
}));

/** 只在媒体事件发生时记录真实播放位置，不依赖 UI 的播放状态。 */
export function markPlaybackAnchor(
  media: Pick<HTMLMediaElement, "currentTime" | "playbackRate">,
  running: boolean,
  now = anchorNow(),
) {
  usePlaybackAnchor.setState(state => ({
    mediaMs: media.currentTime * 1000,
    at: now,
    rate: media.playbackRate || 1,
    running,
    seq: state.seq + 1,
  }));
}

export function resetPlaybackAnchor(now = anchorNow()) {
  usePlaybackAnchor.setState(state => ({ mediaMs: 0, at: now, running: false, seq: state.seq + 1 }));
}

export function getAnchorPositionMs(anchor: DesktopLyricsAnchor, offsetMs: number, now: number) {
  return anchor.mediaMs + (anchor.running ? Math.max(0, now - anchor.at) * anchor.rate : 0) + offsetMs;
}

export function getDesktopLyricsLineProgress(payload: DesktopLyricsPayload, now: number) {
  if (typeof payload.lineStartMs !== "number" || typeof payload.lineEndMs !== "number") return 0;
  const position = getAnchorPositionMs(payload.anchor, payload.offsetMs, now);
  return (position - payload.lineStartMs) / Math.max(1, payload.lineEndMs - payload.lineStartMs);
}

/**
 * 按桌面歌词窗口自己的时钟决定显示哪一句：取后续句中最后一个已经开始的句子
 * （与 getActiveLyricIndex 一致，同一时间的多句取最后一句），进度与换句用同一个锚点时钟，
 * 新的一句从行首开始。返回值保留其后的句子，窗口可以继续换句；没有句子开始时原样返回。
 */
export function resolveDisplayedLine(payload: DesktopLyricsPayload, now: number): DesktopLyricsPayload {
  const { upcoming } = payload;
  if (!upcoming?.length) return payload;
  const position = getAnchorPositionMs(payload.anchor, payload.offsetMs, now);
  let started = -1;
  while (started + 1 < upcoming.length && upcoming[started + 1].lineStartMs <= position) started += 1;
  if (started < 0) return payload;
  const line = upcoming[started];
  const rest = upcoming.slice(started + 1);
  return {
    ...payload,
    index: line.index,
    text: line.text,
    romaji: line.romaji,
    translation: line.translation,
    nextText: line.nextText,
    status: undefined,
    lineStartMs: line.lineStartMs,
    lineEndMs: line.lineEndMs,
    upcoming: rest.length ? rest : undefined,
  };
}
