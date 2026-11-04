import { EventEmitter } from "node:events";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  mode: "auto",
  warn: vi.fn(),
  updater: undefined as unknown,
  exportResult: "/tmp/tauri-migration.json" as string | null,
  exportPromise: null as Promise<string | null> | null,
  app: { runningUnderARM64Translation: false } as { runningUnderARM64Translation: boolean; quitting?: boolean },
}));
vi.mock("electron", async () => {
  const { EventEmitter } = await import("node:events");
  const nativeAutoUpdater = new EventEmitter();
  nativeAutoUpdater.setMaxListeners(0); // every test re-imports the updater module
  return { app: mocks.app, autoUpdater: nativeAutoUpdater };
});
vi.mock("../../electron/migration", () => ({
  exportTauriMigration: async () => mocks.exportResult,
  exportTauriMigrationWithin: () => mocks.exportPromise ?? Promise.resolve(mocks.exportResult),
}));
vi.mock("electron-is-dev", () => ({ default: false }));
vi.mock("electron-log", () => ({ default: { warn: mocks.warn, transports: { file: {} } } }));
vi.mock("electron-updater", () => ({
  default: {
    get autoUpdater() {
      return mocks.updater;
    },
  },
}));
vi.mock("../../electron/updater/update-mode", () => ({
  detectUpdateMode: async () => mocks.mode,
  disableAutoUpdate: () => {
    mocks.mode = "manual";
  },
}));

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
  mocks.exportResult = "/tmp/tauri-migration.json";
  mocks.exportPromise = null;
  delete mocks.app.quitting;
});
afterEach(() => vi.useRealTimers());

describe("updater lifecycle", () => {
  it("takes the quit back when the install fails and the app keeps running", async () => {
    const { setupAutoUpdater, quitAndInstall, isInstallerHandedOff, stopCheckForUpdates } =
      await import("../../electron/updater");
    await setupAutoUpdater({ getMainWindow: () => ({ webContents: { send: vi.fn() } }) as never });
    updater.emit("update-downloaded", { downloadedFile: "/tmp/Biu.zip" });

    expect(await quitAndInstall()).toBe(true);
    expect(isInstallerHandedOff()).toBe(true);
    // Squirrel.Mac rejects the signature; the app stays open.
    updater.emit("error", new Error("update does not satisfy code requirement"));
    expect(isInstallerHandedOff()).toBe(false);
    expect(mocks.app.quitting).toBe(false);
    stopCheckForUpdates();
  });

  it("does not install when a signature error arrives while exporting", async () => {
    const { setupAutoUpdater, quitAndInstall, isInstallerHandedOff, stopCheckForUpdates } =
      await import("../../electron/updater");
    await setupAutoUpdater({ getMainWindow: () => ({ webContents: { send: vi.fn() } }) as never });
    updater.emit("update-downloaded", { downloadedFile: "/tmp/Biu.zip" });

    let finishExport!: (value: string | null) => void;
    mocks.exportPromise = new Promise(resolve => (finishExport = resolve));
    const pending = quitAndInstall();
    await vi.advanceTimersByTimeAsync(0);
    updater.emit("error", new Error("update does not satisfy code requirement"));
    finishExport("/tmp/tauri-migration.json");

    expect(await pending).toBe(false);
    expect(updater.quitAndInstall).not.toHaveBeenCalled();
    expect(isInstallerHandedOff()).toBe(false);
    expect(mocks.app.quitting).toBeUndefined();
    expect(mocks.mode).toBe("manual");
    stopCheckForUpdates();
  });

  it("installs only after a successful export and handles signature errors while installing", async () => {
    const { setupAutoUpdater, quitAndInstall, installsOnQuit, isInstallerHandedOff, stopCheckForUpdates } =
      await import("../../electron/updater");
    const send = vi.fn();
    await setupAutoUpdater({ getMainWindow: () => ({ webContents: { send } }) as never });
    updater.emit("update-downloaded", { downloadedFile: "/tmp/Biu.zip" });

    mocks.exportResult = null;
    expect(await quitAndInstall()).toBe(false);
    expect(updater.quitAndInstall).not.toHaveBeenCalled();
    expect(isInstallerHandedOff()).toBe(false);
    expect(mocks.app.quitting).toBeUndefined();

    // Normal-quit install stays off on Linux, where AppImageUpdater would block the quit.
    expect(installsOnQuit("darwin")).toBe(true);
    expect(installsOnQuit("win32")).toBe(true);
    expect(installsOnQuit("linux")).toBe(false);

    mocks.exportResult = "/tmp/tauri-migration.json";
    expect(await quitAndInstall()).toBe(true);
    expect(updater.quitAndInstall).toHaveBeenCalledWith(true, true);
    expect(mocks.app.quitting).toBe(true);

    // Squirrel.Mac reports a bad signature during the install itself.
    updater.emit("error", new Error("update does not satisfy code requirement"));
    expect(mocks.mode).toBe("manual");
    expect(updater.autoInstallOnAppQuit).toBe(false);
    expect(installsOnQuit("darwin")).toBe(false);
    stopCheckForUpdates();
  });

  it("reads the Tauri feed (tauri*.yml) without allowing downgrade or prereleases", async () => {
    const { setupAutoUpdater, stopCheckForUpdates, tauriUpdateChannel } = await import("../../electron/updater");
    await setupAutoUpdater({ getMainWindow: () => null });
    expect((updater as unknown as { channel: string }).channel).toBe(tauriUpdateChannel());
    expect((updater as unknown as { allowDowngrade: boolean }).allowDowngrade).toBe(false);
    expect(updater.allowPrerelease).toBe(false);
    expect(tauriUpdateChannel("darwin", "arm64", false)).toBe("tauri");
    expect(tauriUpdateChannel("linux", "arm64", false)).toBe("tauri");
    expect(tauriUpdateChannel("win32", "x64", false)).toBe("tauri");
    expect(tauriUpdateChannel("win32", "arm64", false)).toBe("tauri-arm64");
    expect(tauriUpdateChannel("win32", "x64", true)).toBe("tauri-arm64");
    stopCheckForUpdates();
  });

  it("skips hourly checks after downloading", async () => {
    const { setupAutoUpdater, stopCheckForUpdates } = await import("../../electron/updater");
    await setupAutoUpdater({ getMainWindow: () => null });
    updater.emit("update-downloaded", { downloadedFile: "/tmp/Biu.zip" });
    await vi.advanceTimersByTimeAsync(2 * 60 * 60 * 1000);
    expect(updater.checkForUpdates).toHaveBeenCalledTimes(1);
    stopCheckForUpdates();
  });

  it.each([
    new Error("Code signature at URL file:///Applications/Biu.app did not pass validation"),
    new Error("update does not satisfy code requirement"),
    Object.assign(new Error("invalid update"), { code: "ERR_UPDATER_INVALID_SIGNATURE" }),
  ])("downgrades late signature failures to manual and blocks installation: %s", async error => {
    const send = vi.fn();
    const { setupAutoUpdater, quitAndInstall, stopCheckForUpdates } = await import("../../electron/updater");
    await setupAutoUpdater({ getMainWindow: () => ({ webContents: { send } }) as never });
    updater.emit("update-downloaded", { downloadedFile: "/tmp/Biu.zip" });
    updater.emit("error", error);
    expect(send).toHaveBeenLastCalledWith(
      "app:update-message",
      expect.objectContaining({ status: "error", mode: "manual" }),
    );
    expect(updater.autoDownload).toBe(false);
    expect(updater.autoInstallOnAppQuit).toBe(false);
    expect(await quitAndInstall()).toBe(false);
    expect(updater.quitAndInstall).not.toHaveBeenCalled();
    expect(mocks.mode).toBe("manual");
    stopCheckForUpdates();
  });

  it.each(["auto", "manual"])("configures %s and gates installation", async mode => {
    mocks.mode = mode;
    const { setupAutoUpdater, quitAndInstall, stopCheckForUpdates } = await import("../../electron/updater");
    await setupAutoUpdater({ getMainWindow: () => null });
    expect(updater.autoDownload).toBe(mode === "auto");
    // Off on Linux (AppImage install on quit is synchronous); main.ts holds the quit for the export.
    expect(updater.autoInstallOnAppQuit).toBe(mode === "auto" && process.platform !== "linux");
    expect(updater.autoRunAppAfterInstall).toBe(true);
    expect(updater.allowPrerelease).toBe(false);
    expect(await quitAndInstall()).toBe(mode === "auto");
    if (mode === "auto") {
      expect(updater.quitAndInstall).toHaveBeenCalledWith(true, true);
      // Set before installing, so a hide-on-close window does not block Squirrel.Mac.
      expect(mocks.app.quitting).toBe(true);
    } else expect(updater.quitAndInstall).not.toHaveBeenCalled();
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
