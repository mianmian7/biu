import { EventEmitter } from "node:events";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ mode: "auto", warn: vi.fn(), updater: undefined as unknown }));
vi.mock("electron", () => ({}));
vi.mock("electron-is-dev", () => ({ default: false }));
vi.mock("electron-log", () => ({ default: { warn: mocks.warn, transports: { file: {} } } }));
vi.mock("electron-updater", () => ({
  default: {
    get autoUpdater() {
      return mocks.updater;
    },
  },
}));
vi.mock("../../electron/updater/update-mode", () => ({ detectUpdateMode: async () => mocks.mode }));

function makeUpdater() {
  return Object.assign(new EventEmitter(), {
    checkForUpdates: vi.fn().mockResolvedValue(null),
    downloadUpdate: vi.fn().mockResolvedValue([]),
    quitAndInstall: vi.fn(),
    autoDownload: false,
    autoInstallOnAppQuit: false,
    autoRunAppAfterInstall: false,
    allowPrerelease: true,
  });
}

let updater: ReturnType<typeof makeUpdater>;
beforeEach(() => {
  vi.resetModules();
  vi.useFakeTimers();
  updater = makeUpdater();
  mocks.updater = updater;
  mocks.mode = "auto";
});
afterEach(() => vi.useRealTimers());

describe("updater lifecycle", () => {
  it.each(["auto", "manual"])("configures %s and gates installation", async mode => {
    mocks.mode = mode;
    const { setupAutoUpdater, quitAndInstall, stopCheckForUpdates } = await import("../../electron/updater");
    await setupAutoUpdater({ getMainWindow: () => null });
    expect(updater.autoDownload).toBe(mode === "auto");
    expect(updater.autoInstallOnAppQuit).toBe(mode === "auto");
    expect(updater.autoRunAppAfterInstall).toBe(true);
    expect(updater.allowPrerelease).toBe(false);
    expect(await quitAndInstall()).toBe(mode === "auto");
    if (mode === "auto") expect(updater.quitAndInstall).toHaveBeenCalledWith(true, true);
    else expect(updater.quitAndInstall).not.toHaveBeenCalled();
    stopCheckForUpdates();
  });

  it("catches startup/hourly failures and suppresses check error messages", async () => {
    updater.checkForUpdates.mockRejectedValue(
      Object.assign(new Error("missing channel"), { code: "ERR_UPDATER_CHANNEL_FILE_NOT_FOUND" }),
    );
    const send = vi.fn();
    const { setupAutoUpdater, stopCheckForUpdates } = await import("../../electron/updater");
    await setupAutoUpdater({ getMainWindow: () => ({ webContents: { send } }) as never });
    updater.emit("error", new Error("check failed"));
    await vi.advanceTimersByTimeAsync(60 * 60 * 1000);
    expect(updater.checkForUpdates).toHaveBeenCalledTimes(2);
    expect(mocks.warn).toHaveBeenCalled();
    expect(send).not.toHaveBeenCalled();
    stopCheckForUpdates();
    await vi.advanceTimersByTimeAsync(60 * 60 * 1000);
    expect(updater.checkForUpdates).toHaveBeenCalledTimes(2);
  });

  it("forwards download failures and completed mode/file path", async () => {
    const send = vi.fn();
    const { setupAutoUpdater, downloadUpdate, stopCheckForUpdates } = await import("../../electron/updater");
    await setupAutoUpdater({ getMainWindow: () => ({ webContents: { send } }) as never });
    await downloadUpdate();
    updater.emit("error", new Error("download failed"));
    expect(send).toHaveBeenLastCalledWith(
      "app:update-message",
      expect.objectContaining({ status: "error", mode: "auto" }),
    );
    updater.emit("update-downloaded", { downloadedFile: "/tmp/Biu.zip" });
    expect(send).toHaveBeenLastCalledWith("app:update-message", {
      status: "downloaded",
      mode: "auto",
      downloadInfo: { filePath: "/tmp/Biu.zip" },
    });
    stopCheckForUpdates();
  });
});
