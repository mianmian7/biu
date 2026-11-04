import { create } from "zustand";

interface AppUpdateState extends CheckAppUpdateResult {
  mode?: AppUpdateMode;
  downloadStatus: "idle" | DownloadAppUpdateStatus;
  downloadProgress?: DownloadAppProgressInfo;
  downloadInfo?: DownloadInfo;
  downloadError?: string;
  setMode: (mode: AppUpdateMode) => void;
  setDownload: (message: DownloadAppMessage) => void;
  setUpdate: (updateInfo: CheckAppUpdateResult) => void;
}

export const useAppUpdateStore = create<AppUpdateState>(set => ({
  isUpdateAvailable: false,
  downloadStatus: "idle",
  setMode: mode => set({ mode }),
  setDownload: message =>
    set(state => ({
      mode: message.mode ?? state.mode,
      downloadStatus: message.status,
      downloadProgress: message.processInfo ?? state.downloadProgress,
      downloadInfo: message.downloadInfo ?? state.downloadInfo,
      downloadError: message.error,
    })),
  setUpdate: updateInfo => set(updateInfo),
}));
