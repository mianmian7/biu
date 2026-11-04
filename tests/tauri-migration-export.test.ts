import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  userData: "",
  cookiesGet: vi.fn(),
  storeBroken: false,
}));

vi.mock("electron", () => ({
  app: { getVersion: () => "1.16.1-autolyrics.7", getPath: () => mocks.userData },
  session: { defaultSession: { cookies: { get: mocks.cookiesGet } } },
}));
vi.mock("electron-log", () => ({ default: { info: vi.fn(), warn: vi.fn() } }));
vi.mock("../electron/store", () => {
  const store = (value: unknown) => ({
    get store() {
      if (mocks.storeBroken) throw new Error("corrupt store");
      return value;
    },
  });
  return {
    appSettingsStore: store({ appSettings: { primaryColor: "#000" } }),
    userStore: store({ isLogin: true }),
    mediaDownloadsStore: store({}),
    shortcutKeyStore: store({}),
    lyricsCacheStore: store({}),
  };
});

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(r => (resolve = r));
  return { promise, resolve };
}

const cookie = (value: string) => [{ name: "SESSDATA", value, domain: ".bilibili.com", path: "/" }];

describe("exportTauriMigration with a mocked Electron", () => {
  beforeEach(() => {
    vi.resetModules();
    mocks.userData = fs.mkdtempSync(path.join(os.tmpdir(), "biu-export-"));
    mocks.cookiesGet.mockReset();
    mocks.storeBroken = false;
  });
  afterEach(() => fs.rmSync(mocks.userData, { recursive: true, force: true }));

  const readExport = () =>
    JSON.parse(fs.readFileSync(path.join(mocks.userData, "tauri-migration.json"), "utf8")) as {
      cookies: { value: string }[];
    };

  it("runs exports one after another and leaves the newest snapshot", async () => {
    const first = deferred<unknown[]>();
    mocks.cookiesGet.mockReturnValueOnce(first.promise).mockResolvedValueOnce(cookie("second"));
    const { exportTauriMigration } = await import("../electron/migration");

    const a = exportTauriMigration();
    const b = exportTauriMigration();
    await vi.waitFor(() => expect(mocks.cookiesGet).toHaveBeenCalledTimes(1));
    // The second export must not start while the first is still reading cookies.
    await new Promise(r => setTimeout(r, 50));
    expect(mocks.cookiesGet).toHaveBeenCalledTimes(1);

    first.resolve(cookie("first"));
    expect(await a).toBe(path.join(mocks.userData, "tauri-migration.json"));
    expect(await b).toBe(path.join(mocks.userData, "tauri-migration.json"));
    expect(mocks.cookiesGet).toHaveBeenCalledTimes(2);
    expect(readExport().cookies[0].value).toBe("second");
  });

  it("fails without writing when cookies cannot be read or a store is broken", async () => {
    const { exportTauriMigration } = await import("../electron/migration");
    mocks.cookiesGet.mockRejectedValueOnce(new Error("cookie db locked"));
    expect(await exportTauriMigration()).toBeNull();
    expect(fs.existsSync(path.join(mocks.userData, "tauri-migration.json"))).toBe(false);

    mocks.storeBroken = true;
    mocks.cookiesGet.mockResolvedValueOnce(cookie("x"));
    expect(await exportTauriMigration()).toBeNull();
    expect(fs.existsSync(path.join(mocks.userData, "tauri-migration.json"))).toBe(false);
  });

  it("reports a timeout as a failed export", async () => {
    const { exportTauriMigrationWithin } = await import("../electron/migration");
    mocks.cookiesGet.mockReturnValueOnce(new Promise(() => {}));
    expect(await exportTauriMigrationWithin(30)).toBeNull();
  });
});
