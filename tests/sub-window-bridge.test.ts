import { readFileSync } from "node:fs";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, test, vi } from "vitest";

import * as bridge from "@/entries/sub-window-bridge";
import { createTauriElectronAPI } from "@/platform";

type Call = { cmd: string; args?: Record<string, unknown> };

let calls: Call[];
let listened: string[];
let handlers: Map<string, (event: { payload: unknown }) => void>;

beforeEach(() => {
  calls = [];
  listened = [];
  handlers = new Map();
  (window as any).__TAURI__ = {
    core: {
      invoke: vi.fn(async (cmd: string, args?: Record<string, unknown>) => {
        calls.push({ cmd, args });
        return null;
      }),
    },
    event: {
      listen: vi.fn(async (event: string, cb: (event: { payload: unknown }) => void) => {
        listened.push(event);
        handlers.set(event, cb);
        return () => {};
      }),
    },
  };
});

afterEach(() => {
  delete (window as any).__TAURI__;
  delete (window as any).__TAURI_INTERNALS__;
});

/** 分别通过 platform 与 bridge 执行同一动作，比较发出的 Tauri 命令 / 监听的事件 */
const record = async (run: () => unknown) => {
  calls = [];
  listened = [];
  await run();
  await Promise.resolve();
  return { calls: [...calls], listened: [...listened] };
};

describe("sub-window bridge matches src/platform/index.ts exactly", () => {
  test("desktop lyrics commands and events", async () => {
    const api = createTauriElectronAPI();
    const pairs: Array<[string, () => unknown, () => unknown]> = [
      ["getDesktopLyricsContent", () => api.getDesktopLyricsContent(), () => bridge.getDesktopLyricsContent()],
      [
        "sendDesktopLyricsCommand",
        () => api.sendDesktopLyricsCommand("toggleLock"),
        () => bridge.sendDesktopLyricsCommand("toggleLock"),
      ],
      ["startDesktopLyricsDrag", () => api.startDesktopLyricsDrag(), () => bridge.startDesktopLyricsDrag()],
      ["endDesktopLyricsDrag", () => api.endDesktopLyricsDrag(), () => bridge.endDesktopLyricsDrag()],
      [
        "setDesktopLyricsIgnoreMouse",
        () => api.setDesktopLyricsIgnoreMouse(false),
        () => bridge.setDesktopLyricsIgnoreMouse(false),
      ],
      [
        "setDesktopLyricsContentHeight",
        () => api.setDesktopLyricsContentHeight(88),
        () => bridge.setDesktopLyricsContentHeight(88),
      ],
      [
        "onDesktopLyricsContent",
        () => api.onDesktopLyricsContent(() => {}),
        () => bridge.onDesktopLyricsContent(() => {}),
      ],
      ["toggleMiniPlayer", () => api.toggleMiniPlayer(), () => bridge.toggleMiniMode()],
      ["getStore", () => api.getStore("app-settings" as StoreName), () => bridge.getStore("app-settings" as StoreName)],
    ];
    for (const [name, viaPlatform, viaBridge] of pairs) {
      const expected = await record(viaPlatform);
      const actual = await record(viaBridge);
      expect(actual, name).toEqual(expected);
      expect(expected.calls.length + expected.listened.length, name).toBeGreaterThan(0);
    }
    expect(bridge.getPlatform()).toBe(api.getPlatform());
  });

  test("cursor-inside / cursor-leave are forwarded to #root > div like platform", async () => {
    const root = document.createElement("div");
    root.id = "root";
    const page = document.createElement("div");
    root.append(page);
    document.body.append(root);
    const seen: string[] = [];
    page.addEventListener("mousemove", () => seen.push("mousemove"));
    page.addEventListener("mouseleave", () => seen.push("mouseleave"));

    const payloads: unknown[] = [];
    const off = bridge.onDesktopLyricsContent(p => payloads.push(p));
    await Promise.resolve();
    handlers.get("desktop-lyrics:cursor-inside")?.({ payload: null });
    handlers.get("desktop-lyrics:cursor-leave")?.({ payload: null });
    handlers.get("desktop-lyrics:content")?.({ payload: { text: "hi" } });
    expect(seen).toEqual(["mousemove", "mouseleave"]);
    expect(payloads).toEqual([{ text: "hi" }]);
    off();
    root.remove();
  });

  test("falls back to __TAURI_INTERNALS__.invoke and is a safe no-op outside Tauri", async () => {
    delete (window as any).__TAURI__;
    const invoke = vi.fn(async () => undefined);
    (window as any).__TAURI_INTERNALS__ = { invoke };
    bridge.startDesktopLyricsDrag();
    expect(invoke).toHaveBeenCalledWith("desktop_lyrics_start_drag", undefined);

    delete (window as any).__TAURI_INTERNALS__;
    expect(() => bridge.sendDesktopLyricsCommand("next")).not.toThrow();
    await expect(bridge.getDesktopLyricsContent()).resolves.toBeUndefined();
    await expect(bridge.getStore("app-settings" as StoreName)).resolves.toBeUndefined();
    await expect(bridge.toggleMiniMode()).resolves.toBeUndefined();
    const off = bridge.onDesktopLyricsContent(() => {});
    expect(off).not.toThrow();
  });

  test("mini player BroadcastChannel name matches the main-window side", () => {
    const source = readFileSync(path.resolve(__dirname, "../src/common/utils/mini-player.ts"), "utf8");
    expect(source).toContain(`new BroadcastChannel("${bridge.MINI_PLAYER_CHANNEL}")`);
  });
});
