import { act, useEffect } from "react";
import { createRoot, type Root } from "react-dom/client";

import { afterEach, beforeAll, beforeEach, describe, expect, test, vi } from "vitest";

import { useLingeringOpen } from "@/layout/use-lingering-open";
import {
  HIDDEN_UNMOUNT_DELAY_MS,
  useWindowUiVisible,
  WINDOW_VISIBILITY_EVENT,
  type WindowVisibilityPayload,
} from "@/layout/window-visibility";

beforeAll(() => {
  (globalThis as any).IS_REACT_ACT_ENVIRONMENT = true;
});

let container: HTMLDivElement;
let root: Root;

beforeEach(() => {
  vi.useFakeTimers();
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});

afterEach(() => {
  act(() => root.unmount());
  container.remove();
  vi.useRealTimers();
  delete (window as any).__TAURI__;
});

/** 模拟 Tauri 事件总线：记录监听的事件名并可手动派发 */
const createBus = () => {
  let handler: ((payload: WindowVisibilityPayload) => void) | null = null;
  const subscribe = vi.fn((cb: (payload: WindowVisibilityPayload) => void) => {
    handler = cb;
    return () => {
      handler = null;
    };
  });
  return { subscribe, emit: (payload: WindowVisibilityPayload) => act(() => handler?.(payload)) };
};

const mounts = { count: 0, unmounts: 0 };

const HeavyUi = () => {
  useEffect(() => {
    mounts.count += 1;
    return () => {
      mounts.unmounts += 1;
    };
  }, []);
  return <div data-testid="heavy-ui">ui</div>;
};

const Shell = ({ subscribe }: { subscribe: (cb: (p: WindowVisibilityPayload) => void) => VoidFunction }) => {
  const visible = useWindowUiVisible(subscribe);
  return visible ? <HeavyUi /> : null;
};

const hasUi = () => container.querySelector("[data-testid='heavy-ui']") !== null;

describe("biu:window-visibility unmount / remount", () => {
  beforeEach(() => {
    mounts.count = 0;
    mounts.unmounts = 0;
  });

  test("hidden for 3s unmounts the visual UI, visible remounts it immediately", () => {
    const bus = createBus();
    act(() => root.render(<Shell subscribe={bus.subscribe} />));
    expect(hasUi()).toBe(true);
    expect(mounts.count).toBe(1);

    bus.emit({ visible: false });
    act(() => vi.advanceTimersByTime(HIDDEN_UNMOUNT_DELAY_MS - 1));
    expect(hasUi()).toBe(true);

    act(() => vi.advanceTimersByTime(1));
    expect(hasUi()).toBe(false);
    expect(mounts.unmounts).toBe(1);

    bus.emit({ visible: true });
    expect(hasUi()).toBe(true);
    expect(mounts.count).toBe(2);
  });

  test("showing again within 3s cancels the pending unmount", () => {
    const bus = createBus();
    act(() => root.render(<Shell subscribe={bus.subscribe} />));

    bus.emit({ visible: false });
    act(() => vi.advanceTimersByTime(2000));
    bus.emit({ visible: true });
    act(() => vi.advanceTimersByTime(10_000));

    expect(hasUi()).toBe(true);
    expect(mounts.count).toBe(1);
    expect(mounts.unmounts).toBe(0);
  });

  test("repeated hidden events restart the delay; malformed payloads are ignored", () => {
    const bus = createBus();
    act(() => root.render(<Shell subscribe={bus.subscribe} />));

    bus.emit({ visible: false });
    act(() => vi.advanceTimersByTime(2000));
    bus.emit({ visible: false });
    act(() => vi.advanceTimersByTime(2000));
    expect(hasUi()).toBe(true);
    bus.emit({} as WindowVisibilityPayload);
    act(() => vi.advanceTimersByTime(1000));
    expect(hasUi()).toBe(false);
  });

  test("default subscription listens to the biu:window-visibility Tauri event", async () => {
    let tauriHandler: ((event: { payload: WindowVisibilityPayload }) => void) | null = null;
    const listen = vi.fn(async (_event: string, cb: (event: { payload: WindowVisibilityPayload }) => void) => {
      tauriHandler = cb;
      return () => {};
    });
    (window as any).__TAURI__ = { core: { invoke: vi.fn() }, event: { listen } };

    const DefaultShell = () => (useWindowUiVisible() ? <HeavyUi /> : null);
    act(() => root.render(<DefaultShell />));

    expect(listen).toHaveBeenCalledWith(WINDOW_VISIBILITY_EVENT, expect.any(Function));
    expect(WINDOW_VISIBILITY_EVENT).toBe("biu:window-visibility");
    act(() => tauriHandler?.({ payload: { visible: false } }));
    act(() => vi.advanceTimersByTime(HIDDEN_UNMOUNT_DELAY_MS));
    expect(hasUi()).toBe(false);
    act(() => tauriHandler?.({ payload: { visible: true } }));
    expect(hasUi()).toBe(true);
  });
});

describe("overlay lazy mounting", () => {
  const Probe = ({ open }: { open: boolean }) => (useLingeringOpen(open, 500) ? <span data-testid="overlay" /> : null);
  const hasOverlay = () => container.querySelector("[data-testid='overlay']") !== null;

  test("not mounted while closed, mounted on open, unmounted after the exit animation", () => {
    act(() => root.render(<Probe open={false} />));
    expect(hasOverlay()).toBe(false);

    act(() => root.render(<Probe open />));
    expect(hasOverlay()).toBe(true);

    act(() => root.render(<Probe open={false} />));
    act(() => vi.advanceTimersByTime(499));
    expect(hasOverlay()).toBe(true);
    act(() => vi.advanceTimersByTime(1));
    expect(hasOverlay()).toBe(false);
  });

  test("reopening during the exit animation keeps it mounted", () => {
    act(() => root.render(<Probe open />));
    act(() => root.render(<Probe open={false} />));
    act(() => vi.advanceTimersByTime(300));
    act(() => root.render(<Probe open />));
    act(() => vi.advanceTimersByTime(1000));
    expect(hasOverlay()).toBe(true);
  });
});
