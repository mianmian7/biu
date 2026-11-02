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
