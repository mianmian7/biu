import { beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  handlers: new Map<string, (...args: unknown[]) => unknown>(),
  check: vi.fn(),
  mode: vi.fn(),
  download: vi.fn(),
  install: vi.fn(),
}));
vi.mock("electron", () => ({
  app: { getVersion: () => "1.16.1-autolyrics.5" },
  ipcMain: { handle: (name: string, handler: (...args: unknown[]) => unknown) => mocks.handlers.set(name, handler) },
  session: {},
}));
vi.mock("electron-is-dev", () => ({ default: false }));
vi.mock("electron-log", () => ({ default: { error: vi.fn() } }));
vi.mock("../../electron/updater", () => ({
  autoUpdater: { checkForUpdates: mocks.check },
  downloadUpdate: mocks.download,
  quitAndInstall: mocks.install,
}));
vi.mock("../../electron/updater/update-mode", () => ({ detectUpdateMode: mocks.mode }));

beforeEach(async () => {
  mocks.handlers.clear();
  const { registerAppHandlers } = await import("../../electron/ipc/app");
  registerAppHandlers();
});

describe("update IPC", () => {
  it("turns unpublished failures into a notice without an error", async () => {
    mocks.check.mockRejectedValue({ code: "ERR_UPDATER_CHANNEL_FILE_NOT_FOUND" });
    expect(await mocks.handlers.get("app:check-update")!()).toEqual({
      isUpdateAvailable: false,
      notice: "新版本正在发布，请几分钟后再试",
    });
  });
  it("keeps non-publication failures visible on manual checks", async () => {
    mocks.check.mockRejectedValue(new Error("network unavailable"));
    expect(await mocks.handlers.get("app:check-update")!()).toEqual({
      isUpdateAvailable: false,
      error: "network unavailable",
    });
  });
  it("returns available version information", async () => {
    mocks.check.mockResolvedValue({
      isUpdateAvailable: true,
      updateInfo: { version: "1.16.1-autolyrics.6", releaseNotes: "notes" },
    });
    expect(await mocks.handlers.get("app:check-update")!()).toEqual({
      isUpdateAvailable: true,
      latestVersion: "1.16.1-autolyrics.6",
      releaseNotes: "notes",
    });
  });
  it("uses cached mode detection and the phase-aware download/install entry points", async () => {
    mocks.mode.mockResolvedValue("manual");
    mocks.install.mockResolvedValue(false);
    expect(await mocks.handlers.get("app:get-update-mode")!()).toBe("manual");
    await mocks.handlers.get("app:download-update")!();
    expect(mocks.download).toHaveBeenCalledOnce();
    expect(await mocks.handlers.get("app:quit-and-install")!()).toBe(false);
    expect(mocks.install).toHaveBeenCalledOnce();
  });
});
