import { BrowserWindow } from "electron";
import isDev from "electron-is-dev";
import log from "electron-log";
import electronUpdater, { type UpdateDownloadedEvent } from "electron-updater";
import path from "node:path";

import { channel } from "../ipc/channel";
import { classifyUpdateError } from "./classify-error";
import { detectUpdateMode } from "./update-mode";

const { autoUpdater } = electronUpdater;
let phase: "checking" | "downloading" | "downloaded" | "installing" = "checking";
let checkForUpdatesInterval: NodeJS.Timeout | null = null;

export async function downloadUpdate() {
  phase = "downloading";
  return autoUpdater.downloadUpdate();
}

export async function quitAndInstall() {
  if ((await detectUpdateMode()) !== "auto") return false;
  phase = "installing";
  autoUpdater.quitAndInstall(true, true);
  return true;
}

async function setupAutoUpdater({ getMainWindow }: { getMainWindow: () => BrowserWindow | null }) {
  autoUpdater.logger = log;
  log.transports.file.level = "info";
  const mode = await detectUpdateMode();
  autoUpdater.autoDownload = mode === "auto";
  autoUpdater.autoInstallOnAppQuit = mode === "auto";
  autoUpdater.autoRunAppAfterInstall = true;
  autoUpdater.allowPrerelease = false;

  if (isDev) {
    autoUpdater.updateConfigPath = path.resolve(process.cwd(), "electron/updater/dev-app-update.yml");
    autoUpdater.forceDevUpdateConfig = true;
    autoUpdater.autoRunAppAfterInstall = false;
  }

  autoUpdater.on("update-available", info => {
    if (mode === "auto") phase = "downloading";
    getMainWindow()?.webContents.send(channel.app.onUpdateAvailable, {
      latestVersion: info.version,
      releaseNotes: info.releaseNotes,
    });
  });

  autoUpdater.on("download-progress", progressObj => {
    phase = "downloading";
    getMainWindow()?.webContents.send(channel.app.updateMessage, {
      status: "downloading",
      mode,
      processInfo: progressObj,
    });
  });

  autoUpdater.on("error", error => {
    log.warn(`[updater] ${classifyUpdateError(error)} (${phase})`, error);
    if (phase !== "downloading" && phase !== "installing") return;
    getMainWindow()?.webContents.send(channel.app.updateMessage, {
      status: "error",
      mode,
      error: error instanceof Error ? error.message : String(error),
    });
    phase = "checking";
  });

  autoUpdater.on("update-downloaded", (info: UpdateDownloadedEvent) => {
    phase = "downloaded";
    getMainWindow()?.webContents.send(channel.app.updateMessage, {
      status: "downloaded",
      mode,
      downloadInfo: { filePath: info.downloadedFile },
    });
  });

  const check = () => {
    autoUpdater.checkForUpdates().catch(error => {
      log.warn(`[updater] background check: ${classifyUpdateError(error)}`, error);
    });
  };
  check();
  checkForUpdatesInterval = setInterval(check, 60 * 60 * 1000);
}

const stopCheckForUpdates = () => {
  if (checkForUpdatesInterval) {
    clearInterval(checkForUpdatesInterval);
    checkForUpdatesInterval = null;
  }
};

export { autoUpdater, setupAutoUpdater, stopCheckForUpdates };
