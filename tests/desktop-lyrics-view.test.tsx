import { act } from "react";
import { createRoot } from "react-dom/client";

import { afterEach, beforeAll, describe, expect, test } from "vitest";

import DesktopLyricsView from "@/components/desktop-lyrics-view";
import { defaultDesktopLyricsStyle, resolveDesktopLyricsColors } from "@shared/settings/desktop-lyrics";

beforeAll(() => {
  (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
  globalThis.ResizeObserver ??= class {
    observe() {}
    disconnect() {}
    unobserve() {}
  } as any;
});

let container: HTMLDivElement | null = null;

afterEach(() => {
  container?.remove();
  container = null;
});

/** 渲染后统计某段文本出现的图层数：效果层 + 未唱层 + （高亮时）已唱层 */
const countLayers = (style: Partial<DesktopLyricsStyle>, text: string) => {
  const merged = { ...defaultDesktopLyricsStyle, effect: "stroke" as const, ...style };
  container = document.createElement("div");
  document.body.append(container);
  const root = createRoot(container);
  act(() => {
    root.render(
      <DesktopLyricsView
        fontSize={36}
        style={merged}
        colors={resolveDesktopLyricsColors(merged)}
        text="風に揺れる街の灯り"
        romaji="kaze ni yureru machi no akari"
        translation="随风摇曳的街灯"
        hasTranslation
        hasRomaji
        index={0}
        playId="p"
        getProgress={() => 0.5}
      />,
    );
  });
  const count = Array.from(container.querySelectorAll("span")).filter(el => el.textContent === text).length;
  act(() => root.unmount());
  return count;
};

describe("DesktopLyricsView sub-lines", () => {
  test("translation and romaji highlight together with the line by default", () => {
    expect(countLayers({}, "随风摇曳的街灯")).toBe(3);
    expect(countLayers({}, "kaze ni yureru machi no akari")).toBe(3);
  });

  test("sub-lines keep the unplayed color when sync highlight is off", () => {
    expect(countLayers({ karaokeSubLines: false }, "随风摇曳的街灯")).toBe(2);
    expect(countLayers({ karaokeSubLines: false }, "風に揺れる街の灯り")).toBe(3);
  });
});
