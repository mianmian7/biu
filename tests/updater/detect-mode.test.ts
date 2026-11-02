import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ packaged: true, dev: false, getPath: vi.fn(() => "/tmp/Biu"), info: vi.fn() }));
vi.mock("electron", () => ({
  app: {
    get isPackaged() {
      return mocks.packaged;
    },
    getPath: mocks.getPath,
  },
}));
vi.mock("electron-is-dev", () => ({
  get default() {
    return mocks.dev;
  },
}));
vi.mock("electron-log", () => ({ default: { info: mocks.info } }));

beforeEach(() => {
  vi.resetModules();
  mocks.packaged = true;
  mocks.dev = false;
  vi.stubEnv("APPIMAGE", "/tmp/Biu.AppImage");
});
afterEach(() => vi.unstubAllEnvs());

describe("development mode safety", () => {
  it("caches a manual downgrade for subsequent IPC queries and install guards", async () => {
    const { detectUpdateMode, disableAutoUpdate } = await import("../../electron/updater/update-mode");
    const previous = detectUpdateMode();
    await previous;
    disableAutoUpdate();
    const next = detectUpdateMode();
    expect(next).not.toBe(previous);
    expect(await next).toBe("manual");
    expect(detectUpdateMode()).toBe(next);
  });
  it.each([
    [true, true],
    [false, false],
    [true, false],
  ])("forces manual for isDev=%s / isPackaged=%s before inspecting the executable", async (dev, packaged) => {
    mocks.dev = dev;
    mocks.packaged = packaged;
    const { detectUpdateMode } = await import("../../electron/updater/update-mode");
    expect(await detectUpdateMode()).toBe("manual");
    expect(mocks.getPath).not.toHaveBeenCalled();
    expect(mocks.info).toHaveBeenCalledWith("[updater] mode=manual");
  });
});
