import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";

import { StoreNameMap } from "@shared/store";

import {
  buildTauriMigrationPayload,
  exportTauriMigration,
  filterAndFormatBilibiliCookies,
  isBilibiliDomain,
  parseLocalStorageDump,
  TAURI_MIGRATION_FILE,
  writeTauriMigrationFileAtomic,
} from "../electron/migration";

describe("tauri migration export", () => {
  const tempDirs: string[] = [];

  afterEach(() => {
    for (const dir of tempDirs.splice(0)) {
      fs.rmSync(dir, { recursive: true, force: true });
    }
    vi.restoreAllMocks();
  });

  it("matches bilibili.com and subdomains only", () => {
    expect(isBilibiliDomain(".bilibili.com")).toBe(true);
    expect(isBilibiliDomain("bilibili.com")).toBe(true);
    expect(isBilibiliDomain("api.bilibili.com")).toBe(true);
    expect(isBilibiliDomain(".passport.bilibili.com")).toBe(true);
    expect(isBilibiliDomain("evilbilibili.com")).toBe(false);
    expect(isBilibiliDomain("bilibili.com.evil.org")).toBe(false);
    expect(isBilibiliDomain("")).toBe(false);
    expect(isBilibiliDomain(undefined)).toBe(false);
  });

  it("filters cookies by bilibili.com domain and normalizes fields", () => {
    const cookies = filterAndFormatBilibiliCookies([
      {
        name: "SESSDATA",
        value: "fake_sessdata_value",
        domain: ".bilibili.com",
        path: "/",
        expirationDate: 1900000000,
        secure: true,
        httpOnly: true,
        sameSite: "no_restriction",
      },
      {
        name: "bili_jct",
        value: "fake_csrf",
        domain: "bilibili.com",
        path: "",
        secure: false,
        httpOnly: false,
      },
      {
        name: "other_cookie",
        value: "ignored",
        domain: ".example.com",
        path: "/",
        secure: true,
        httpOnly: false,
      },
      {
        name: "fake_prefix",
        value: "ignored",
        domain: "notbilibili.com",
        path: "/",
        secure: true,
        httpOnly: false,
      },
    ]);

    expect(cookies).toEqual([
      {
        name: "SESSDATA",
        value: "fake_sessdata_value",
        domain: ".bilibili.com",
        path: "/",
        expirationDate: 1900000000,
        secure: true,
        httpOnly: true,
        sameSite: "no_restriction",
      },
      {
        name: "bili_jct",
        value: "fake_csrf",
        domain: "bilibili.com",
        path: "/",
        secure: false,
        httpOnly: false,
      },
    ]);
  });

  it("builds payload with all StoreNameMap stores and writes atomically with 0600 permissions", () => {
    const tmpDir = fs.mkdtempSync(path.join(os.tmpdir(), "biu-migration-test-"));
    tempDirs.push(tmpDir);

    const payload = buildTauriMigrationPayload({
      appVersion: "1.16.1-autolyrics.7",
      exportedAt: 1730000000000,
      stores: {
        [StoreNameMap.AppSettings]: {
          appSettings: { primaryColor: "#ff6699", downloadPath: "/tmp/downloads" },
        },
        [StoreNameMap.UserLoginInfo]: {
          isLogin: true,
          mid: 999999,
          uname: "fake_user",
        },
        [StoreNameMap.ShortcutSettings]: {
          enableGlobalShortcuts: false,
        },
      },
      cookies: [
        {
          name: "SESSDATA",
          value: "fake_sess",
          domain: ".bilibili.com",
          path: "/",
          expirationDate: 1900000000,
          secure: true,
          httpOnly: true,
          sameSite: "lax",
        },
      ],
    });

    expect(payload.version).toBe(1);
    expect(payload.exportedAt).toBe(1730000000000);
    expect(payload.appVersion).toBe("1.16.1-autolyrics.7");
    expect(Object.keys(payload.stores).sort()).toEqual(Object.values(StoreNameMap).sort());
    expect(payload.stores[StoreNameMap.MediaDownloads]).toEqual({});
    expect(payload.stores[StoreNameMap.LyricsCache]).toEqual({});
    expect(payload.stores[StoreNameMap.UserLoginInfo]).toEqual({
      isLogin: true,
      mid: 999999,
      uname: "fake_user",
    });

    const outPath = writeTauriMigrationFileAtomic(tmpDir, payload);
    expect(outPath).toBe(path.join(tmpDir, TAURI_MIGRATION_FILE));
    expect(fs.existsSync(outPath)).toBe(true);

    const parsed = JSON.parse(fs.readFileSync(outPath, "utf8"));
    expect(parsed).toEqual(payload);

    if (process.platform !== "win32") {
      const mode = fs.statSync(outPath).mode & 0o777;
      expect(mode).toBe(0o600);
    }
  });

  it("catches errors and returns null without throwing when export fails", async () => {
    const result = await exportTauriMigration();
    expect(result).toBeNull();
  });

  it("keeps only string localStorage entries and includes them in the payload", () => {
    expect(parseLocalStorageDump('{"user-token":"{}","n":null,"x":"1"}')).toEqual({ "user-token": "{}", x: "1" });
    expect(parseLocalStorageDump("[1]")).toBeNull();
    expect(parseLocalStorageDump(undefined)).toBeNull();
    const payload = buildTauriMigrationPayload({
      appVersion: "1.16.1-autolyrics.7",
      stores: {},
      cookies: [],
      localStorage: { "user-token": "{}" },
    });
    expect(payload.localStorage).toEqual({ "user-token": "{}" });
  });

  it("runs exports one after another", async () => {
    const results = await Promise.all([exportTauriMigration(), exportTauriMigration()]);
    expect(results).toEqual([null, null]);
  });
});
