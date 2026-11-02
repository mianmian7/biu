import { beforeEach, describe, expect, test } from "vitest";

import {
  anchorNow,
  getAnchorPositionMs,
  getDesktopLyricsLineProgress,
  markPlaybackAnchor,
  resetPlaybackAnchor,
  usePlaybackAnchor,
} from "@/common/utils/playback-anchor";
import { defaultDesktopLyricsStyle, resolveDesktopLyricsColors } from "@shared/settings/desktop-lyrics";

const anchor: DesktopLyricsAnchor = { mediaMs: 255770, at: 10000, rate: 1, running: true, seq: 1 };
const payload: DesktopLyricsPayload = {
  enabled: true,
  locked: true,
  active: true,
  hasTranslation: false,
  hasRomaji: false,
  anchor,
  offsetMs: 0,
  isPlaying: true,
  index: 0,
  lineStartMs: 255770,
  lineEndMs: 260250,
  fontSize: 36,
  style: defaultDesktopLyricsStyle,
  colors: resolveDesktopLyricsColors(defaultDesktopLyricsStyle),
};

beforeEach(() => resetPlaybackAnchor());

describe("playback anchor clock", () => {
  test("uses a cross-window timestamp", () => {
    const before = performance.timeOrigin + performance.now();
    expect(anchorNow()).toBeGreaterThanOrEqual(before);
    expect(anchorNow()).toBeLessThanOrEqual(performance.timeOrigin + performance.now());
  });
});

describe("getAnchorPositionMs", () => {
  test("extrapolates without a 1.5 second limit", () => {
    expect(getAnchorPositionMs(anchor, 0, anchor.at + 4480)).toBe(anchor.mediaMs + 4480);
  });

  test("applies playback rate and lyric offset", () => {
    expect(getAnchorPositionMs({ ...anchor, rate: 1.5 }, 0, anchor.at + 1000)).toBe(anchor.mediaMs + 1500);
    expect(getAnchorPositionMs(anchor, 500, anchor.at + 1000)).toBe(anchor.mediaMs + 1500);
  });

  test("freezes when paused or buffering and does not extrapolate backwards", () => {
    expect(getAnchorPositionMs({ ...anchor, running: false }, 0, anchor.at + 60000)).toBe(anchor.mediaMs);
    expect(getAnchorPositionMs(anchor, 0, anchor.at - 1000)).toBe(anchor.mediaMs);
  });
});

describe("getDesktopLyricsLineProgress", () => {
  test("sweeps a 4480ms line to completion using the cached anchor", () => {
    expect(getDesktopLyricsLineProgress(payload, anchor.at + 2240)).toBe(0.5);
    expect(getDesktopLyricsLineProgress(payload, anchor.at + 4480)).toBeGreaterThanOrEqual(1);
  });

  test("freezes a long line at the pause position", () => {
    const paused = {
      ...payload,
      anchor: { ...anchor, mediaMs: anchor.mediaMs + 3000, at: anchor.at + 3000, running: false },
    };
    expect(getDesktopLyricsLineProgress(paused, anchor.at + 3000)).toBeCloseTo(3000 / 4480);
    expect(getDesktopLyricsLineProgress(paused, anchor.at + 30000)).toBeCloseTo(3000 / 4480);
  });

  test("does not highlight without line timing and handles a zero duration", () => {
    expect(getDesktopLyricsLineProgress({ ...payload, lineStartMs: undefined }, anchor.at)).toBe(0);
    expect(getDesktopLyricsLineProgress({ ...payload, lineEndMs: undefined }, anchor.at)).toBe(0);
    expect(getDesktopLyricsLineProgress({ ...payload, lineEndMs: payload.lineStartMs }, anchor.at + 1)).toBe(1);
  });
});

describe("playback anchor store", () => {
  test("marks exact media position, rate and a new sequence, then resets on track load", () => {
    const seq = usePlaybackAnchor.getState().seq;
    markPlaybackAnchor({ currentTime: 12.345, playbackRate: 2 }, true, 1000);
    expect(usePlaybackAnchor.getState()).toEqual({ mediaMs: 12345, at: 1000, rate: 2, running: true, seq: seq + 1 });
    resetPlaybackAnchor(2000);
    expect(usePlaybackAnchor.getState()).toMatchObject({ mediaMs: 0, at: 2000, running: false, seq: seq + 2 });
  });

  test("preserves fractional milliseconds and defaults a missing rate to one", () => {
    markPlaybackAnchor({ currentTime: 1.2345, playbackRate: 0 }, false, 1000);
    expect(usePlaybackAnchor.getState()).toMatchObject({ mediaMs: 1234.5, rate: 1, running: false });
  });
});
