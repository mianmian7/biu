export const NOT_IMPLEMENTED_MESSAGE = "未实现";

export function notImplementedError(method: string): Error {
  return new Error(`[tauri] ${method} ${NOT_IMPLEMENTED_MESSAGE}`);
}

export function rejectNotImplemented<T = never>(method: string): Promise<T> {
  return Promise.reject<T>(notImplementedError(method));
}

export function warnNotImplemented(method: string): void {
  console.warn(`[tauri] ${method} ${NOT_IMPLEMENTED_MESSAGE}`);
}

export function noopUnsubscribe(method: string): VoidFunction {
  warnNotImplemented(method);
  return () => {};
}

type TauriGlobal = {
  core: {
    invoke: <T = unknown>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
  };
  event?: {
    listen: <T = unknown>(
      event: string,
      handler: (event: { payload: T }) => void,
    ) => Promise<VoidFunction>;
  };
};

function getTauri(): TauriGlobal | undefined {
  return (window as unknown as { __TAURI__?: TauriGlobal }).__TAURI__;
}

export async function invokeTauri<T = unknown>(
  cmd: string,
  args?: Record<string, unknown>,
): Promise<T> {
  const tauri = getTauri();
  if (!tauri?.core?.invoke) {
    throw new Error(`[tauri] invoke unavailable for command: ${cmd}`);
  }
  return tauri.core.invoke<T>(cmd, args);
}

let cachedMediaBase: string | null = null;
let mediaBasePromise: Promise<string> | null = null;

export async function ensureMediaBase(): Promise<string> {
  if (cachedMediaBase) {
    return cachedMediaBase;
  }
  if (!mediaBasePromise) {
    mediaBasePromise = invokeTauri<string>("media_base")
      .then(base => {
        cachedMediaBase = base;
        return base;
      })
      .finally(() => {
        mediaBasePromise = null;
      });
  }
  return mediaBasePromise;
}

export function toMediaUrlSync(rawUrl: string): string {
  if (!rawUrl || !cachedMediaBase) {
    return rawUrl;
  }
  if (rawUrl.startsWith(cachedMediaBase)) {
    return rawUrl;
  }
  if (rawUrl.startsWith("http://") || rawUrl.startsWith("https://")) {
    return `${cachedMediaBase}remote?u=${encodeURIComponent(rawUrl)}`;
  }
  return rawUrl;
}

export async function toMediaUrl(rawUrl?: string): Promise<string> {
  if (!rawUrl) {
    return "";
  }
  try {
    const base = await ensureMediaBase();
    if (rawUrl.startsWith(base)) {
      return rawUrl;
    }
    if (rawUrl.startsWith("http://") || rawUrl.startsWith("https://")) {
      return `${base}remote?u=${encodeURIComponent(rawUrl)}`;
    }
    return rawUrl;
  } catch {
    return rawUrl;
  }
}

export function unwrapMediaUrl(maybeWrapped?: string): string | undefined {
  if (!maybeWrapped) {
    return maybeWrapped;
  }
  try {
    const parsed = new URL(maybeWrapped);
    if (
      (parsed.hostname === "127.0.0.1" || parsed.hostname === "localhost") &&
      parsed.pathname.endsWith("/remote")
    ) {
      const inner = parsed.searchParams.get("u");
      if (inner) {
        return inner;
      }
    }
  } catch {
    // ignore malformed URL
  }
  return maybeWrapped;
}

export function detectPlatform(): AppPlatForm {
  const ua = typeof navigator !== "undefined" ? navigator.userAgent : "";
  if (/Macintosh|Mac OS X/i.test(ua)) {
    return "macos";
  }
  if (/Windows/i.test(ua)) {
    return "windows";
  }
  return "linux";
}

function installDragRegionObserver(): void {
  if (typeof document === "undefined") {
    return;
  }
  const syncDragRegions = (root: ParentNode = document) => {
    const dragNodes = root.querySelectorAll?.(".window-drag") ?? [];
    dragNodes.forEach(node => {
      if (node instanceof HTMLElement && node.getAttribute("data-tauri-drag-region") !== "deep") {
        node.setAttribute("data-tauri-drag-region", "deep");
      }
    });
    const noDragNodes = root.querySelectorAll?.(".window-no-drag") ?? [];
    noDragNodes.forEach(node => {
      if (node instanceof HTMLElement && node.getAttribute("data-tauri-drag-region") !== "false") {
        node.setAttribute("data-tauri-drag-region", "false");
      }
    });
  };

  syncDragRegions();
  const observer = new MutationObserver(() => {
    syncDragRegions();
  });
  observer.observe(document.documentElement, {
    childList: true,
    subtree: true,
    attributes: true,
    attributeFilter: ["class"],
  });
}

export const UNIMPLEMENTED_ELECTRON_METHODS = [
  "selectDirectory",
  "selectFile",
  "openDirectory",
  "showFileInFolder",
  "getFonts",
  "searchNeteaseSongs",
  "getNeteaseLyrics",
  "searchLrclibLyrics",
  "romanizeLyrics",
  "autoLyrics.start",
  "autoLyrics.prefetch",
  "autoLyrics.cancel",
  "autoLyrics.onProgress",
  "scanLocalMusic",
  "deleteLocalMusicFile",
  "navigate",
  "updatePlaybackState",
  "onShortcutCommand",
  "registerShortcut",
  "unregisterShortcut",
  "registerAllShortcuts",
  "unregisterAllShortcuts",
  "onPlayerCommand",
  "checkAppUpdate",
  "onUpdateAvailable",
  "getUpdateMode",
  "downloadAppUpdate",
  "onDownloadAppProgress",
  "quitAndInstall",
  "toggleMiniPlayer",
  "syncDesktopLyrics",
  "onDesktopLyricsContent",
  "getDesktopLyricsContent",
  "onDesktopLyricsToggleRequested",
  "onDesktopLyricsLockToggleRequested",
  "sendDesktopLyricsCommand",
  "onDesktopLyricsCommand",
  "startDesktopLyricsDrag",
  "endDesktopLyricsDrag",
  "setDesktopLyricsIgnoreMouse",
  "setDesktopLyricsContentHeight",
  "resetDesktopLyricsPosition",
  "getMediaDownloadTaskList",
  "syncMediaDownloadTaskList",
  "addMediaDownloadTask",
  "addMediaDownloadTaskList",
  "pauseMediaDownloadTask",
  "resumeMediaDownloadTask",
  "retryMediaDownloadTask",
  "cancelMediaDownloadTask",
  "clearMediaDownloadTaskList",
] as const;

export function createTauriElectronAPI(): ElectronAPI {
  const api: ElectronAPI = {
    getStore: <N extends StoreName>(name: N) =>
      invokeTauri<StoreDataMap[N] | undefined>("store_get", { name }),
    setStore: <N extends StoreName>(name: N, value: StoreDataMap[N]) =>
      invokeTauri<void>("store_set", { name, value }),
    clearStore: (name: StoreName) =>
      invokeTauri<boolean>("store_clear", { name }).then(() => undefined),

    getCookie: (key: string) =>
      invokeTauri<string | null>("cookie_get", { key }).then(v => v ?? undefined),
    setCookie: (name: string, value: string, expirationDate?: number) =>
      invokeTauri<void>("cookie_set", { payload: { name, value, expirationDate } }),

    setProxySettings: (proxySettings: ProxySettings) =>
      invokeTauri<void>("proxy_set", { proxySettings }),

    getPlatform: () => detectPlatform(),
    getAppVersion: () =>
      invokeTauri<string>("plugin:app|version").catch(() => "1.16.1-autolyrics.6"),
    isDev: () => Promise.resolve(Boolean(import.meta.env?.DEV)),

    openExternal: async (url: string) => {
      const parsed = new URL(url);
      if (parsed.protocol !== "http:" && parsed.protocol !== "https:") {
        throw new Error("Only http/https external URLs are allowed");
      }
      await invokeTauri<void>("plugin:opener|open_url", { url });
      return true;
    },

    minimizeWindow: () => {
      void invokeTauri("plugin:window|minimize", { label: "main" });
    },
    toggleMaximizeWindow: () => {
      void invokeTauri("plugin:window|toggle_maximize", { label: "main" });
    },
    closeWindow: () => {
      void invokeTauri("window_close");
    },
    isMaximized: () =>
      invokeTauri<boolean>("plugin:window|is_maximized", { label: "main" }).catch(() => false),
    onWindowMaximizeChange: cb => {
      const handler = () => {
        void api.isMaximized().then(cb);
      };
      window.addEventListener("resize", handler);
      return () => window.removeEventListener("resize", handler);
    },
    isFullScreen: () =>
      invokeTauri<boolean>("plugin:window|is_fullscreen", { label: "main" }).catch(() => false),
    onWindowFullScreenChange: cb => {
      const handler = () => {
        void api.isFullScreen().then(cb);
      };
      window.addEventListener("resize", handler);
      return () => window.removeEventListener("resize", handler);
    },
    toggleDevTools: () => {
      void invokeTauri("devtools_toggle");
    },

    // Step 2+ unimplemented methods: return explicit "未实现" errors without crashing startup
    selectDirectory: () => rejectNotImplemented("selectDirectory"),
    selectFile: () => rejectNotImplemented("selectFile"),
    openDirectory: () => rejectNotImplemented("openDirectory"),
    showFileInFolder: () => rejectNotImplemented("showFileInFolder"),
    getFonts: () => rejectNotImplemented("getFonts"),
    searchNeteaseSongs: () => rejectNotImplemented("searchNeteaseSongs"),
    getNeteaseLyrics: () => rejectNotImplemented("getNeteaseLyrics"),
    searchLrclibLyrics: () => rejectNotImplemented("searchLrclibLyrics"),
    romanizeLyrics: () => rejectNotImplemented("romanizeLyrics"),
    autoLyrics: {
      start: () => rejectNotImplemented("autoLyrics.start"),
      prefetch: () => rejectNotImplemented("autoLyrics.prefetch"),
      cancel: () => rejectNotImplemented("autoLyrics.cancel"),
      onProgress: () => noopUnsubscribe("autoLyrics.onProgress"),
    },
    scanLocalMusic: () => rejectNotImplemented("scanLocalMusic"),
    deleteLocalMusicFile: () => rejectNotImplemented("deleteLocalMusicFile"),
    navigate: () => noopUnsubscribe("navigate"),
    updatePlaybackState: () => warnNotImplemented("updatePlaybackState"),
    onShortcutCommand: () => noopUnsubscribe("onShortcutCommand"),
    registerShortcut: () => rejectNotImplemented("registerShortcut"),
    unregisterShortcut: () => rejectNotImplemented("unregisterShortcut"),
    registerAllShortcuts: () => rejectNotImplemented("registerAllShortcuts"),
    unregisterAllShortcuts: () => rejectNotImplemented("unregisterAllShortcuts"),
    onPlayerCommand: () => noopUnsubscribe("onPlayerCommand"),
    checkAppUpdate: () => rejectNotImplemented("checkAppUpdate"),
    onUpdateAvailable: () => noopUnsubscribe("onUpdateAvailable"),
    getUpdateMode: () => rejectNotImplemented("getUpdateMode"),
    downloadAppUpdate: () => rejectNotImplemented("downloadAppUpdate"),
    onDownloadAppProgress: () => noopUnsubscribe("onDownloadAppProgress"),
    quitAndInstall: () => rejectNotImplemented("quitAndInstall"),
    toggleMiniPlayer: () => rejectNotImplemented("toggleMiniPlayer"),
    syncDesktopLyrics: () => warnNotImplemented("syncDesktopLyrics"),
    onDesktopLyricsContent: () => noopUnsubscribe("onDesktopLyricsContent"),
    getDesktopLyricsContent: () => rejectNotImplemented("getDesktopLyricsContent"),
    onDesktopLyricsToggleRequested: () => noopUnsubscribe("onDesktopLyricsToggleRequested"),
    onDesktopLyricsLockToggleRequested: () =>
      noopUnsubscribe("onDesktopLyricsLockToggleRequested"),
    sendDesktopLyricsCommand: () => warnNotImplemented("sendDesktopLyricsCommand"),
    onDesktopLyricsCommand: () => noopUnsubscribe("onDesktopLyricsCommand"),
    startDesktopLyricsDrag: () => warnNotImplemented("startDesktopLyricsDrag"),
    endDesktopLyricsDrag: () => warnNotImplemented("endDesktopLyricsDrag"),
    setDesktopLyricsIgnoreMouse: () => warnNotImplemented("setDesktopLyricsIgnoreMouse"),
    setDesktopLyricsContentHeight: () => warnNotImplemented("setDesktopLyricsContentHeight"),
    resetDesktopLyricsPosition: () => warnNotImplemented("resetDesktopLyricsPosition"),
    getMediaDownloadTaskList: () => rejectNotImplemented("getMediaDownloadTaskList"),
    syncMediaDownloadTaskList: () => noopUnsubscribe("syncMediaDownloadTaskList"),
    addMediaDownloadTask: () => rejectNotImplemented("addMediaDownloadTask"),
    addMediaDownloadTaskList: () => rejectNotImplemented("addMediaDownloadTaskList"),
    pauseMediaDownloadTask: () => rejectNotImplemented("pauseMediaDownloadTask"),
    resumeMediaDownloadTask: () => rejectNotImplemented("resumeMediaDownloadTask"),
    retryMediaDownloadTask: () => rejectNotImplemented("retryMediaDownloadTask"),
    cancelMediaDownloadTask: () => rejectNotImplemented("cancelMediaDownloadTask"),
    clearMediaDownloadTaskList: () => rejectNotImplemented("clearMediaDownloadTaskList"),
  };

  return api;
}

export function initPlatform(): void {
  if (typeof window === "undefined") {
    return;
  }
  if (!window.electron) {
    window.electron = createTauriElectronAPI();
  }
  installDragRegionObserver();
  void ensureMediaBase().catch(() => {});
}

initPlatform();
