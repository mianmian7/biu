import { act } from "react";
import { createRoot, type Root } from "react-dom/client";

import { afterEach, beforeAll, beforeEach, describe, expect, test, vi } from "vitest";

import { PlayMode } from "@/common/constants/audio";
import MiniPlayer from "@/pages/mini-player";
import { getMiniCoverSrc } from "@/pages/mini-player/cover";
import { usePlayState } from "@/pages/mini-player/play-state";
import { usePlayProgress } from "@/store/play-progress";

class FakeChannel {
  static instances: FakeChannel[] = [];
  sent: any[] = [];
  closed = false;
  onmessage: ((ev: { data: any }) => void) | null = null;
  constructor(public name: string) {
    FakeChannel.instances.push(this);
  }
  postMessage(data: any) {
    this.sent.push(data);
  }
  close() {
    this.closed = true;
  }
}

let container: HTMLDivElement;
let root: Root;
const invoke = vi.fn<(cmd: string, args?: unknown) => Promise<undefined>>(async () => undefined);

beforeAll(() => {
  (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
});

beforeEach(() => {
  FakeChannel.instances = [];
  (globalThis as any).BroadcastChannel = FakeChannel;
  (window as any).__TAURI__ = { core: { invoke }, event: { listen: vi.fn(async () => () => {}) } };
  usePlayState.setState({ isPlaying: false, isSingle: false, duration: 0, title: undefined, cover: undefined });
  usePlayProgress.setState({ currentTime: 0 });
  container = document.createElement("div");
  container.id = "root";
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  delete (window as any).__TAURI__;
});

const channel = () => FakeChannel.instances[0];
const commands = () => channel().sent.map(m => m.data.type);
const button = (label: string) => container.querySelector<HTMLButtonElement>(`button[aria-label='${label}']`)!;
const fromMain = (state: Record<string, unknown>) =>
  act(() => channel().onmessage?.({ data: { from: "main", state, ts: Date.now() } }));

describe("mini player (no HeroUI)", () => {
  test("sends init on mount and renders the empty state with controls disabled", () => {
    act(() => root.render(<MiniPlayer />));
    expect(channel().name).toBe("play-list-store-sync-channel");
    expect(channel().sent[0]).toMatchObject({ from: "mini", data: { type: "init" } });
    expect(container.textContent).toContain("暂无播放内容");
    expect(button("上一首").disabled).toBe(true);
    expect(button("播放").disabled).toBe(true);
    expect(button("下一首").disabled).toBe(true);
    expect(container.querySelector<HTMLInputElement>("input[type='range']")!.disabled).toBe(true);
    // 播放模式与展开按钮始终可用
    expect(button("播放模式").disabled).toBe(false);
    expect(button("展开主窗口").disabled).toBe(false);
  });

  test("applies state from the main window and posts commands back", () => {
    act(() => root.render(<MiniPlayer />));
    fromMain({
      isSingle: false,
      isPlaying: true,
      title: "Song",
      cover: "http://i0.hdslb.com/bfs/archive/abc.jpg",
      currentTime: 42,
      duration: 200,
      playMode: PlayMode.Random,
    });

    expect(container.textContent).toContain("Song");
    const range = container.querySelector<HTMLInputElement>("input[type='range']")!;
    expect(range.disabled).toBe(false);
    expect(range.value).toBe("42");
    expect(range.max).toBe("200");
    expect(button("暂停").disabled).toBe(false);
    expect(container.querySelector("img")!.getAttribute("src")).toBe(
      "https://i0.hdslb.com/bfs/archive/abc.jpg@240w_240h_1c.webp",
    );

    act(() => button("上一首").click());
    act(() => button("暂停").click());
    act(() => button("下一首").click());
    act(() => button("播放模式").click());
    expect(commands()).toEqual(["init", "prev", "togglePlay", "next", "togglePlayMode"]);

    // 原生 range 的 input 事件触发 seek
    const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, "value")!.set!;
    act(() => {
      setter.call(range, "120");
      range.dispatchEvent(new Event("input", { bubbles: true }));
    });
    expect(channel().sent.at(-1)).toMatchObject({ from: "mini", data: { type: "seek", state: { currentTime: 120 } } });
  });

  test("single-item list disables prev/next; expand toggles the mini window", async () => {
    act(() => root.render(<MiniPlayer />));
    fromMain({ isSingle: true, isPlaying: false, title: "Only", currentTime: 0, duration: 10 });
    expect(button("上一首").disabled).toBe(true);
    expect(button("下一首").disabled).toBe(true);
    expect(button("播放").disabled).toBe(false);

    await act(async () => button("展开主窗口").click());
    expect(invoke).toHaveBeenCalledWith("toggle_mini_player", undefined);
  });

  test("cover falls back to the original image, then to a placeholder", () => {
    act(() => root.render(<MiniPlayer />));
    fromMain({ isSingle: false, isPlaying: false, title: "S", cover: "https://i0.hdslb.com/bfs/c.jpg", duration: 1 });
    const img = () => container.querySelector("img");
    expect(img()!.getAttribute("src")).toBe("https://i0.hdslb.com/bfs/c.jpg@240w_240h_1c.webp");
    act(() => img()!.dispatchEvent(new Event("error")));
    expect(img()!.getAttribute("src")).toBe("https://i0.hdslb.com/bfs/c.jpg");
    act(() => img()!.dispatchEvent(new Event("error")));
    expect(img()).toBeNull();
  });

  test("drag region attributes match the Tauri drag observer", () => {
    act(() => root.render(<MiniPlayer />));
    const shell = container.firstElementChild as HTMLElement;
    expect(shell.getAttribute("data-tauri-drag-region")).toBe("deep");
    expect(shell.classList.contains("window-drag")).toBe(true);
    for (const el of Array.from(container.querySelectorAll("button"))) {
      expect(el.getAttribute("data-tauri-drag-region")).toBe("false");
    }
  });

  test("ignores messages not from main and closes the channel on unmount", () => {
    act(() => root.render(<MiniPlayer />));
    act(() => channel().onmessage?.({ data: { from: "mini", state: { title: "x" } } }));
    expect(container.textContent).toContain("暂无播放内容");
    act(() => root.unmount());
    expect(channel().closed).toBe(true);
    root = createRoot(container);
  });
});

describe("getMiniCoverSrc", () => {
  test("resizes bilibili /bfs/ images and upgrades protocol", () => {
    expect(getMiniCoverSrc("//i0.hdslb.com/bfs/a.png")).toBe("https://i0.hdslb.com/bfs/a.png@240w_240h_1c.webp");
    expect(getMiniCoverSrc("https://i0.hdslb.com/bfs/a.png@100w.webp")).toBe(
      "https://i0.hdslb.com/bfs/a.png@100w.webp",
    );
    expect(getMiniCoverSrc("https://example.com/a.png")).toBe("https://example.com/a.png");
    expect(getMiniCoverSrc(undefined)).toBeUndefined();
  });
});
