import { app } from "electron";
import isDev from "electron-is-dev";
import log from "electron-log";
import ffmpeg from "fluent-ffmpeg";
import { spawnSync } from "node:child_process";
import fs from "node:fs";
import path from "node:path";

import { ELECTRON_ICON_BASE_PATH } from "@shared/path";

import { bundledFfmpegFileName, selectRunnableFfmpeg, SYSTEM_FFMPEG_PATHS } from "./auto-lyrics/ffmpeg-resolve";
import { appSettingsStore } from "./store";

export const IconBase = isDev ? process.cwd() : process.resourcesPath;

export function getUserDataPath() {
  return isDev ? path.join(app.getPath("appData"), `biu-dev`) : app.getPath("userData");
}

export const getWindowIcon = () =>
  process.platform === "darwin"
    ? undefined
    : path.resolve(
        IconBase,
        ELECTRON_ICON_BASE_PATH,
        process.platform === "win32" ? (isDev ? "dev.ico" : "logo.ico") : "logo.png",
      );

let resolvedFfmpegPath: string | undefined;

/** Drop a cached binary after ENOEXEC so the next decode tries another candidate. */
export function clearResolvedFfmpegPath() {
  resolvedFfmpegPath = undefined;
}

function readExecutableHeader(candidate: string): Uint8Array | undefined {
  let fd: number | undefined;
  try {
    fd = fs.openSync(candidate, "r");
    const header = Buffer.alloc(4096);
    const bytes = fs.readSync(fd, header, 0, header.length, 0);
    return header.subarray(0, bytes);
  } catch {
    return undefined;
  } finally {
    if (fd !== undefined) fs.closeSync(fd);
  }
}

function probeFfmpeg(candidate: string, bundled: boolean): "ok" | "enoexec" | "failed" {
  if (bundled && process.platform !== "win32") {
    try {
      fs.chmodSync(candidate, 0o755);
    } catch (err) {
      log.error(`Failed to chmod ffmpeg at ${candidate}`, err);
    }
  }
  const result = spawnSync(candidate, ["-hide_banner", "-version"], {
    timeout: 8000,
    windowsHide: true,
    stdio: "ignore",
  });
  const code = result.error && "code" in result.error ? result.error.code : undefined;
  if (code === "ENOEXEC") return "enoexec";
  return result.status === 0 ? "ok" : "failed";
}

/**
 * User settings, then BIU_FFMPEG_PATH, then common system paths, then the bundled binary.
 * A candidate whose CPU does not match, or that fails with ENOEXEC, is skipped.
 */
export const resolveFfmpegPath = (): string => {
  if (resolvedFfmpegPath && fs.existsSync(resolvedFfmpegPath)) return resolvedFfmpegPath;
  const userPaths: string[] = [];
  try {
    const settings = appSettingsStore.get("appSettings");
    if (settings?.ffmpegPath) userPaths.push(settings.ffmpegPath);
  } catch (err) {
    log.error("Error reading ffmpeg path from settings:", err);
  }
  if (process.env.BIU_FFMPEG_PATH) userPaths.push(process.env.BIU_FFMPEG_PATH);
  const systemPaths = process.platform === "darwin" || process.platform === "linux" ? [...SYSTEM_FFMPEG_PATHS] : [];
  const bundledPath = path.join(
    isDev ? process.cwd() : process.resourcesPath,
    "electron",
    "ffmpeg",
    bundledFfmpegFileName(process.platform, process.arch),
  );
  const chosen = selectRunnableFfmpeg({
    userPaths,
    systemPaths,
    bundledPath,
    processArch: process.arch,
    exists: candidate => fs.existsSync(candidate),
    readHeader: readExecutableHeader,
    probe: candidate => probeFfmpeg(candidate, candidate === bundledPath),
  });
  resolvedFfmpegPath = chosen;
  log.info(`Found ffmpeg at ${chosen}`);
  return chosen;
};

export const fixFfmpegPath = () => {
  try {
    ffmpeg.setFfmpegPath(resolveFfmpegPath());
  } catch (error) {
    log.error("Failed to resolve ffmpeg:", error);
  }
};

const entities = {
  amp: "&",
  apos: "'",
  gt: ">",
  lt: "<",
  quot: '"',
  nbsp: " ",
};

/**
 * 将包含 HTML 和特殊字符的字符串转换为合法的文件名
 */
export function sanitizeFilename(input?: string, replacement = "_") {
  if (!input) return "";

  const decoded = input.replace(/&([a-z0-9]+|#[0-9]{1,6}|#x[0-9a-f]{1,6});/gi, (match, entity) => {
    return entities[entity.toLowerCase()] || match;
  });

  const noHtml = decoded.replace(/<[^>]+>/g, "");

  // 替换 Windows/Linux 文件名非法字符
  // 非法集：\ / : * ? " < > | 以及控制字符
  // eslint-disable-next-line no-control-regex
  const illegalRe = /[\\/:*?"<>|\x00-\x1f\x80-\x9f]/g;
  const sanitized = noHtml.replace(illegalRe, replacement);

  return sanitized.replace(/\s+/g, " ").trim().replace(/\.$/, "").slice(0, 255); // 截断长度，防止文件名过长
}
