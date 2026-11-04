import type { BrowserWindow, Cookie } from "electron";

import { randomUUID } from "node:crypto";
import fs from "node:fs";
import path from "node:path";

import { StoreNameMap } from "@shared/store";

export const TAURI_MIGRATION_FILE = "tauri-migration.json";

export interface ExportedCookie {
  name: string;
  value: string;
  domain: string;
  path: string;
  expirationDate?: number;
  secure: boolean;
  httpOnly: boolean;
  sameSite?: string;
}

export interface TauriMigrationPayload {
  version: 1;
  exportedAt: number;
  appVersion: string;
  stores: Record<string, unknown>;
  cookies: ExportedCookie[];
  /** Renderer localStorage (zustand stores such as the login refresh token and play queue). */
  localStorage: Record<string, string>;
}

export function isBilibiliDomain(domain?: string): boolean {
  if (!domain) return false;
  const normalized = domain.trim().toLowerCase().replace(/^\.+/, "");
  return normalized === "bilibili.com" || normalized.endsWith(".bilibili.com");
}

export type RawCookieInput = ExportedCookie | Partial<Cookie>;

export function filterAndFormatBilibiliCookies(cookies: readonly RawCookieInput[]): ExportedCookie[] {
  const result: ExportedCookie[] = [];
  for (const c of cookies) {
    if (!c || typeof c.name !== "string" || typeof c.value !== "string" || !isBilibiliDomain(c.domain)) {
      continue;
    }
    const item: ExportedCookie = {
      name: c.name,
      value: c.value,
      domain: c.domain!,
      path: typeof c.path === "string" && c.path.length > 0 ? c.path : "/",
      secure: Boolean(c.secure),
      httpOnly: Boolean(c.httpOnly),
    };
    if (typeof c.expirationDate === "number" && Number.isFinite(c.expirationDate)) {
      item.expirationDate = c.expirationDate;
    }
    if (typeof c.sameSite === "string" && c.sameSite.length > 0) {
      item.sameSite = c.sameSite;
    }
    result.push(item);
  }
  return result;
}

export function buildTauriMigrationPayload(options: {
  appVersion: string;
  exportedAt?: number;
  stores: Record<string, unknown>;
  cookies: readonly RawCookieInput[];
  localStorage?: Record<string, string>;
}): TauriMigrationPayload {
  const storeEntries: Record<string, unknown> = {};
  for (const storeName of Object.values(StoreNameMap)) {
    storeEntries[storeName] = options.stores[storeName] ?? {};
  }
  return {
    version: 1,
    exportedAt: options.exportedAt ?? Date.now(),
    appVersion: options.appVersion,
    stores: storeEntries,
    cookies: filterAndFormatBilibiliCookies(options.cookies),
    localStorage: options.localStorage ?? {},
  };
}

export function writeTauriMigrationFileAtomic(userDataDir: string, payload: TauriMigrationPayload): string {
  fs.mkdirSync(userDataDir, { recursive: true });
  const targetPath = path.join(userDataDir, TAURI_MIGRATION_FILE);
  const tmpPath = path.join(userDataDir, `.${TAURI_MIGRATION_FILE}.${process.pid}.${randomUUID()}.tmp`);
  const content = JSON.stringify(payload, null, 2);
  try {
    fs.writeFileSync(tmpPath, content, { encoding: "utf8", mode: 0o600 });
    if (process.platform !== "win32") {
      fs.chmodSync(tmpPath, 0o600);
    }
    fs.renameSync(tmpPath, targetPath);
    if (process.platform !== "win32") {
      fs.chmodSync(targetPath, 0o600);
    }
    return targetPath;
  } catch (error) {
    try {
      if (fs.existsSync(tmpPath)) {
        fs.unlinkSync(tmpPath);
      }
    } catch {
      // ignore cleanup error
    }
    throw error;
  }
}

function readAllStoresSafe(storesModule: typeof import("./store")): Record<string, unknown> {
  const mapping: Record<StoreNameMap, { store: unknown }> = {
    [StoreNameMap.AppSettings]: storesModule.appSettingsStore,
    [StoreNameMap.UserLoginInfo]: storesModule.userStore,
    [StoreNameMap.MediaDownloads]: storesModule.mediaDownloadsStore,
    [StoreNameMap.ShortcutSettings]: storesModule.shortcutKeyStore,
    [StoreNameMap.LyricsCache]: storesModule.lyricsCacheStore,
  };
  const result: Record<string, unknown> = {};
  for (const storeName of Object.values(StoreNameMap)) {
    try {
      result[storeName] = mapping[storeName]?.store ?? {};
    } catch {
      result[storeName] = {};
    }
  }
  return result;
}

let cachedCookies: ExportedCookie[] = [];
let cachedLocalStorage: Record<string, string> = {};
let getMainWindow: () => BrowserWindow | null = () => null;
let exportChain: Promise<unknown> = Promise.resolve();

const READ_LOCAL_STORAGE_JS =
  "JSON.stringify(Object.fromEntries(Object.keys(localStorage).map(k => [k, localStorage.getItem(k)])))";

/** Lets the export read renderer localStorage from the main window. */
export function setMigrationWindowGetter(getter: () => BrowserWindow | null) {
  getMainWindow = getter;
}

function withTimeout<T>(promise: Promise<T>, ms: number): Promise<T> {
  return new Promise<T>((resolve, reject) => {
    const timer = setTimeout(() => reject(new Error(`timed out after ${ms} ms`)), ms);
    promise.then(
      value => {
        clearTimeout(timer);
        resolve(value);
      },
      error => {
        clearTimeout(timer);
        reject(error);
      },
    );
  });
}

export function parseLocalStorageDump(raw: unknown): Record<string, string> | null {
  if (typeof raw !== "string") return null;
  const parsed: unknown = JSON.parse(raw);
  if (!parsed || typeof parsed !== "object" || Array.isArray(parsed)) return null;
  const result: Record<string, string> = {};
  for (const [key, value] of Object.entries(parsed as Record<string, unknown>)) {
    if (typeof value === "string") result[key] = value;
  }
  return result;
}

async function readLocalStorage(): Promise<Record<string, string> | null> {
  const win = getMainWindow();
  if (!win || win.isDestroyed() || win.webContents.isDestroyed()) return null;
  return parseLocalStorageDump(await withTimeout(win.webContents.executeJavaScript(READ_LOCAL_STORAGE_JS), 2000));
}

async function exportOnce(): Promise<string | null> {
  try {
    const [{ app, session }, { default: log }, storesModule] = await Promise.all([
      import("electron"),
      import("electron-log"),
      import("./store"),
    ]);
    try {
      const rawCookies = await session.defaultSession.cookies.get({});
      cachedCookies = filterAndFormatBilibiliCookies(rawCookies);
    } catch (cookieErr) {
      log.warn("[migration] Failed to read session cookies, using last cached cookies:", cookieErr);
    }
    try {
      const entries = await readLocalStorage();
      if (entries) cachedLocalStorage = entries;
    } catch (lsErr) {
      log.warn("[migration] Failed to read localStorage, using last cached copy:", lsErr);
    }
    const payload = buildTauriMigrationPayload({
      appVersion: app.getVersion(),
      stores: readAllStoresSafe(storesModule),
      cookies: cachedCookies,
      localStorage: cachedLocalStorage,
    });
    const outPath = writeTauriMigrationFileAtomic(app.getPath("userData"), payload);
    log.info(
      `[migration] Exported tauri-migration.json (stores=${Object.keys(payload.stores).length}, cookies=${payload.cookies.length}, localStorage=${Object.keys(payload.localStorage).length})`,
    );
    return outPath;
  } catch (error) {
    try {
      const { default: log } = await import("electron-log");
      log.warn("[migration] Failed to export tauri-migration.json:", error);
    } catch {
      // ignore logger error
    }
    return null;
  }
}

/** Exports one snapshot. Calls run one after another, so the last call writes the newest data. */
export function exportTauriMigration(): Promise<string | null> {
  const run = exportChain.then(exportOnce, exportOnce);
  exportChain = run;
  return run;
}
