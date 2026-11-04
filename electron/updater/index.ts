import { app, autoUpdater as nativeAutoUpdater, BrowserWindow } from "electron";
import isDev from "electron-is-dev";
import log from "electron-log";
import electronUpdater, { type UpdateDownloadedEvent } from "electron-updater";
import path from "node:path";

import { channel } from "../ipc/channel";
import { exportTauriMigration, exportTauriMigrationWithin } from "../migration";
import { classifyUpdateError } from "./classify-error";
import { detectUpdateMode, disableAutoUpdate } from "./update-mode";

const { autoUpdater } = electronUpdater;
let phase: "checking" | "downloading" | "downloaded" | "installing" = "checking";
let checkForUpdatesInterval: NodeJS.Timeout | null = null;
let updateMode = "manual";

let installerHandedOff = false;

/** True once the installer has taken over the quit; main.ts must not hold it any more. */
export function isInstallerHandedOff() {
  return installerHandedOff;
}

function handOffToInstaller() {
  installerHandedOff = true;
  // Mac closes every window before installing; a hide-on-close window would block it.
  (app as unknown as { quitting?: boolean }).quitting = true;
}

/** The install failed and the app keeps running: later quits must export again. */
function takeBackFromInstaller() {
  if (!installerHandedOff) return;
  installerHandedOff = false;
  (app as unknown as { quitting?: boolean }).quitting = false;
}

/**
 * The bridge's next update is the Tauri app, listed in the release's tauri*.yml
 * (latest*.yml there send older Electron clients to this bridge). Windows ARM64 gets its
 * own feed because NsisUpdater installs the first exe it finds; an x64 build running
 * under emulation still picks the arm64 Tauri installer.
 */
export function tauriUpdateChannel(
  platform: NodeJS.Platform = process.platform,
  arch: string = process.arch,
  underArm64Translation = Boolean(app?.runningUnderARM64Translation),
) {
  return platform === "win32" && (arch === "arm64" || underArm64Translation) ? "tauri-arm64" : "tauri";
}

export async function downloadUpdate() {
  phase = "downloading";
  return autoUpdater.downloadUpdate();
}

/**
 * Installs the update only after a migration export has just succeeded, so the Tauri app
 * always finds the current login and settings. On failure the update stays downloaded.
 */
export async function quitAndInstall() {
  if ((await detectUpdateMode()) !== "auto") return false;
  const previousPhase = phase;
  phase = "installing";
  const exported = await exportTauriMigrationWithin(10_000);
  if (!exported) {
    // The error handler may have changed the phase meanwhile; only undo our own change.
    if (phase === "installing") phase = previousPhase;
    log.warn("[updater] migration export failed, update not installed this time");
    return false;
  }
  // An error (e.g. a bad signature) while exporting may have cancelled this install.
  if (phase !== "installing" || updateMode !== "auto") {
    log.warn("[updater] install cancelled while exporting");
    return false;
  }
  handOffToInstaller();
  autoUpdater.quitAndInstall(true, true);
  return true;
}

/**
 * Install on a normal quit. Mac (Squirrel installs natively when the process exits) and
 * Windows (installer starts on the "quit" event) do it after main.ts's before-quit hold has
 * exported. Linux stays off: AppImageUpdater runs the new AppImage synchronously on quit.
 */
export function installsOnQuit(platform: NodeJS.Platform = process.platform) {
  return updateMode === "auto" && platform !== "linux";
}

/**
 * Called by main.ts when the export before a normal quit failed. Windows checks the flag on
 * "quit", so the update waits for a later run; Squirrel.Mac has already staged it and installs
 * with the last successful export.
 */
export function skipInstallOnThisQuit() {
  autoUpdater.autoInstallOnAppQuit = false;
}

async function setupAutoUpdater({ getMainWindow }: { getMainWindow: () => BrowserWindow | null }) {
  autoUpdater.logger = log;
  log.transports.file.level = "info";
  let mode = await detectUpdateMode();
  updateMode = mode;
  autoUpdater.autoDownload = mode === "auto";
  autoUpdater.autoInstallOnAppQuit = installsOnQuit();
  autoUpdater.autoRunAppAfterInstall = true;
  autoUpdater.channel = tauriUpdateChannel();
  // The channel setter turns allowDowngrade on; reset both after it.
  autoUpdater.allowDowngrade = false;
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
    takeBackFromInstaller();
    const signatureFailure = /code[\s_-]*(?:requirement|sign(?:ature|ing))|ERR_UPDATER_INVALID_SIGNATURE/i.test(
      `${(error as Error & { code?: string }).code ?? ""} ${error.message}`,
    );
    // With Squirrel.Mac the signature is checked while installing, so cover that phase too.
    if ((phase === "downloaded" || phase === "installing") && signatureFailure) {
      mode = "manual";
      updateMode = mode;
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

  // Squirrel.Mac emits this on Electron's native autoUpdater, not on electron-updater's wrapper.
  for (const emitter of [autoUpdater as unknown as NodeJS.EventEmitter, nativeAutoUpdater]) {
    emitter?.on?.("before-quit-for-update", () => {
      phase = "installing";
      handOffToInstaller();
    });
  }

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
