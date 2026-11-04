import { BrowserWindow } from "electron";
import isDev from "electron-is-dev";
import log from "electron-log";
import electronUpdater, { type UpdateDownloadedEvent } from "electron-updater";
import path from "node:path";

import { channel } from "../ipc/channel";
import { exportTauriMigration } from "../migration";
import { classifyUpdateError } from "./classify-error";
import { detectUpdateMode, disableAutoUpdate } from "./update-mode";

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
  await exportTauriMigration();
  autoUpdater.quitAndInstall(true, true);
  return true;
}

async function setupAutoUpdater({ getMainWindow }: { getMainWindow: () => BrowserWindow | null }) {
  autoUpdater.logger = log;
  log.transports.file.level = "info";
  let mode = await detectUpdateMode();
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
    const signatureFailure = /code[\s_-]*(?:requirement|sign(?:ature|ing))|ERR_UPDATER_INVALID_SIGNATURE/i.test(
      `${(error as Error & { code?: string }).code ?? ""} ${error.message}`,
    );
    if (phase === "downloaded" && signatureFailure) {
      mode = "manual";
      disableAutoUpdate();
      autoUpdater.autoDownload = false;
      autoUpdater.autoInstallOnAppQuit = false;
      log.warn("[updater] mode=manual (native signature validation failed)");
    } else if (phase !== "downloading" && phase !== "installing") {
      return;
    }
    getMainWindow()?.webContents.send(channel.app.updateMessage, {
      status: "error",
      mode,
      error: error instanceof Error ? error.message : String(error),
    });
    phase = "checking";
  });

  autoUpdater.on("update-downloaded", (info: UpdateDownloadedEvent) => {
    phase = "downloaded";
    void exportTauriMigration();
    getMainWindow()?.webContents.send(channel.app.updateMessage, {
      status: "downloaded",
      mode,
      downloadInfo: { filePath: info.downloadedFile },
    });
  });

  (autoUpdater as unknown as NodeJS.EventEmitter).on("before-quit-for-update", () => {
    void exportTauriMigration();
  });

  const check = () => {
    if (phase === "downloaded") return;
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
