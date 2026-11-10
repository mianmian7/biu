import { createTauriAutoLyricsController } from "./auto-lyrics-worker";
import { romanizeLyricLines } from "./romaji";

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
    listen: <T = unknown>(event: string, handler: (event: { payload: T }) => void) => Promise<VoidFunction>;
  };
};

function getTauri(): TauriGlobal | undefined {
  return (window as unknown as { __TAURI__?: TauriGlobal }).__TAURI__;
}

export async function invokeTauri<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const tauri = getTauri();
  if (!tauri?.core?.invoke) {
    throw new Error(`[tauri] invoke unavailable for command: ${cmd}`);
  }
  return tauri.core.invoke<T>(cmd, args);
}

/**
 * Writes `localStorage` entries migrated from the Electron app (login refresh token,
 * play queue, …) that this webview does not have yet. Must run before any zustand
 * store module is imported, because those hydrate from `localStorage` on import.
 */
export async function applyMigratedLocalStorage(storage: Storage = window.localStorage): Promise<number> {
  let entries: Record<string, unknown>;
  try {
    entries = await invokeTauri<Record<string, unknown>>("migration_local_storage");
  } catch {
    return 0;
  }
  const keys = Object.keys(entries ?? {});
  if (keys.length === 0) return 0;
  let written = 0;
  for (const key of keys) {
    const value = entries[key];
    if (typeof value === "string" && storage.getItem(key) === null) {
      // One entry that does not fit (quota) must not keep the app from booting on every launch.
      try {
        storage.setItem(key, value);
        written += 1;
      } catch (error) {
        void invokeTauri("frontend_log", {
          level: "warn",
          message: `[migration] skipped localStorage entry ${key}: ${String(error)}`,
        }).catch(() => {});
      }
    }
  }
  await invokeTauri("migration_local_storage_done").catch(() => {});
  return written;
}

export function listenTauriEvent<T = unknown>(eventName: string, cb: (payload: T) => void): VoidFunction {
  let disposed = false;
  let unlisten: VoidFunction | null = null;
  const tauri = getTauri();
  if (tauri?.event?.listen) {
    void tauri.event
      .listen<T>(eventName, event => {
        if (!disposed) {
          try {
            cb(event.payload);
          } catch (err) {
            console.error(`[tauri] event listener error for ${eventName}:`, err);
          }
        }
      })
      .then(fn => {
        if (disposed) {
          fn();
        } else {
          unlisten = fn;
        }
      })
      .catch(() => {});
  }
  return () => {
    disposed = true;
    if (unlisten) {
      unlisten();
      unlisten = null;
    }
  };
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

function fileUrlToLocalPath(rawUrl: string): string {
  const withoutScheme = rawUrl.slice("file://".length);
  if (/^\/[a-zA-Z]:\//.test(withoutScheme)) {
    return withoutScheme.slice(1);
  }
  return withoutScheme;
}

export function toMediaUrlSync(wrappedOrRawUrl: string): string {
  if (!wrappedOrRawUrl || !cachedMediaBase) {
    return wrappedOrRawUrl;
  }
  if (wrappedOrRawUrl.startsWith(cachedMediaBase)) {
    return wrappedOrRawUrl;
  }
  // 播放列表里保存的可能是上次启动时的代理地址（端口和令牌已失效），先还原成原始地址
  const rawUrl = unwrapMediaUrl(wrappedOrRawUrl) ?? wrappedOrRawUrl;
  if (rawUrl.startsWith("http://") || rawUrl.startsWith("https://")) {
    return `${cachedMediaBase}remote?u=${encodeURIComponent(rawUrl)}`;
  }
  if (rawUrl.startsWith("file://")) {
    return `${cachedMediaBase}local?p=${encodeURIComponent(fileUrlToLocalPath(rawUrl))}`;
  }
  return rawUrl;
}

export async function toMediaUrl(wrappedOrRawUrl?: string): Promise<string> {
  if (!wrappedOrRawUrl) {
    return "";
  }
  try {
    const base = await ensureMediaBase();
    if (wrappedOrRawUrl.startsWith(base)) {
      return wrappedOrRawUrl;
    }
    // 播放列表里保存的可能是上次启动时的代理地址（端口和令牌已失效），先还原成原始地址
    const rawUrl = unwrapMediaUrl(wrappedOrRawUrl) ?? wrappedOrRawUrl;
    if (rawUrl.startsWith("http://") || rawUrl.startsWith("https://")) {
      return `${base}remote?u=${encodeURIComponent(rawUrl)}`;
    }
    if (rawUrl.startsWith("file://")) {
      return `${base}local?p=${encodeURIComponent(fileUrlToLocalPath(rawUrl))}`;
    }
    return rawUrl;
  } catch {
    return wrappedOrRawUrl;
  }
}

export function unwrapMediaUrl(maybeWrapped?: string): string | undefined {
  if (!maybeWrapped) {
    return maybeWrapped;
  }
  try {
    const parsed = new URL(maybeWrapped);
    if (parsed.hostname === "127.0.0.1" || parsed.hostname === "localhost") {
      if (parsed.pathname.endsWith("/remote")) {
        const inner = parsed.searchParams.get("u");
        if (inner) {
          return inner;
        }
      }
      if (parsed.pathname.endsWith("/local")) {
        const localPath = parsed.searchParams.get("p");
        if (localPath) {
          return `file://${localPath}`;
        }
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

export const UNIMPLEMENTED_ELECTRON_METHODS = [] as const;

export function createTauriElectronAPI(): ElectronAPI {
  const autoLyrics = createTauriAutoLyricsController(invokeTauri);

  const api: ElectronAPI = {
    getStore: <N extends StoreName>(name: N) => invokeTauri<StoreDataMap[N] | undefined>("store_get", { name }),
    setStore: <N extends StoreName>(name: N, value: StoreDataMap[N]) => invokeTauri<void>("store_set", { name, value }),
    clearStore: (name: StoreName) => invokeTauri<boolean>("store_clear", { name }).then(() => undefined),

    getCookie: (key: string) => invokeTauri<string | null>("cookie_get", { key }).then(v => v ?? undefined),
    setCookie: (name: string, value: string, expirationDate?: number) =>
      invokeTauri<void>("cookie_set", { payload: { name, value, expirationDate } }),

    setProxySettings: (proxySettings: ProxySettings) => invokeTauri<void>("proxy_set", { proxySettings }),

    getPlatform: () => detectPlatform(),
    getAppVersion: () => invokeTauri<string>("plugin:app|version").catch(() => "1.16.1-autolyrics.6"),
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
    isMaximized: () => invokeTauri<boolean>("plugin:window|is_maximized", { label: "main" }).catch(() => false),
    onWindowMaximizeChange: cb => {
      const handler = () => {
        void api.isMaximized().then(cb);
      };
      window.addEventListener("resize", handler);
      return () => window.removeEventListener("resize", handler);
    },
    isFullScreen: () => invokeTauri<boolean>("plugin:window|is_fullscreen", { label: "main" }).catch(() => false),
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

    // Step 2 methods
    searchNeteaseSongs: params => invokeTauri<SearchSongByNeteaseResponse>("lyrics_search_netease", { params }),
    getNeteaseLyrics: params => invokeTauri<GetLyricsByNeteaseResponse>("lyrics_get_netease", { params }),
    searchLrclibLyrics: params => invokeTauri<SearchSongByLrclibResponse[]>("lyrics_search_lrclib", { params }),
    romanizeLyrics: lines => romanizeLyricLines(lines),

    navigate: cb => listenTauriEvent<string>("router:navigate", cb),
    updatePlaybackState: isPlaying => {
      void invokeTauri("update_playback_state", { isPlaying }).catch(() => {});
    },
    onShortcutCommand: cb => listenTauriEvent<ShortcutCommand>("shortcut:triggered", cb),
    registerShortcut: ({ id, accelerator }) => invokeTauri<boolean>("shortcut_register", { id, accelerator }),
    unregisterShortcut: id => invokeTauri<void>("shortcut_unregister", { id }),
    registerAllShortcuts: () => invokeTauri<void>("shortcut_register_all"),
    unregisterAllShortcuts: () => invokeTauri<void>("shortcut_unregister_all"),
    onPlayerCommand: cb => {
      const offPrev = listenTauriEvent("player:prev", () => cb("prev"));
      const offNext = listenTauriEvent("player:next", () => cb("next"));
      const offToggle = listenTauriEvent("player:toggle", () => cb("toggle"));
      return () => {
        offPrev();
        offNext();
        offToggle();
      };
    },

    toggleMiniPlayer: () => invokeTauri<void>("toggle_mini_player"),
    syncDesktopLyrics: payload => {
      void invokeTauri("desktop_lyrics_sync", { payload }).catch(() => {});
    },
    onDesktopLyricsContent: cb => {
      const offContent = listenTauriEvent<DesktopLyricsPayload>("desktop-lyrics:content", cb);
      const offInside = listenTauriEvent("desktop-lyrics:cursor-inside", () => {
        const target = document.querySelector("#root > div");
        target?.dispatchEvent(new MouseEvent("mousemove", { bubbles: true }));
      });
      const offLeave = listenTauriEvent("desktop-lyrics:cursor-leave", () => {
        const target = document.querySelector("#root > div");
        target?.dispatchEvent(new MouseEvent("mouseleave", { bubbles: true }));
      });
      return () => {
        offContent();
        offInside();
        offLeave();
      };
    },
    getDesktopLyricsContent: () =>
      invokeTauri<DesktopLyricsPayload | null>("desktop_lyrics_get_content").then(v => v ?? undefined),
    onDesktopLyricsToggleRequested: cb => listenTauriEvent("desktop-lyrics:toggle-requested", () => cb()),
    onDesktopLyricsLockToggleRequested: cb => listenTauriEvent("desktop-lyrics:lock-toggle-requested", () => cb()),
    sendDesktopLyricsCommand: command => {
      void invokeTauri("desktop_lyrics_command", { command }).catch(() => {});
    },
    onDesktopLyricsCommand: cb => listenTauriEvent<DesktopLyricsCommand>("desktop-lyrics:command", cb),
    startDesktopLyricsDrag: () => {
      void invokeTauri("desktop_lyrics_start_drag").catch(() => {});
    },
    endDesktopLyricsDrag: () => {
      void invokeTauri("desktop_lyrics_end_drag").catch(() => {});
    },
    setDesktopLyricsIgnoreMouse: ignore => {
      void invokeTauri("desktop_lyrics_set_ignore_mouse", { ignore }).catch(() => {});
    },
    setDesktopLyricsContentHeight: height => {
      void invokeTauri("desktop_lyrics_set_content_height", { height }).catch(() => {});
    },
    resetDesktopLyricsPosition: () => {
      void invokeTauri("desktop_lyrics_reset_position").catch(() => {});
    },

    // Step 3: Dialogs, Fonts, Local Music, Download, AutoLyrics
    selectDirectory: (title?: string) =>
      invokeTauri<string | null>("select_directory", { title: title ?? null }).then(v => v ?? null),
    selectFile: () => invokeTauri<string | null>("select_file").then(v => v ?? null),
    openDirectory: (path?: string) => invokeTauri<boolean>("open_directory", { path: path ?? null }),
    showFileInFolder: (filePath: string) => invokeTauri<boolean>("show_file_in_folder", { filePath }),
    getFonts: () => invokeTauri<IFontInfo[]>("get_fonts"),
    scanLocalMusic: (dirs: string[]) => invokeTauri<LocalMusicItem[]>("scan_local_music", { dirs }),
    deleteLocalMusicFile: (filePath: string) => invokeTauri<boolean>("delete_local_music_file", { filePath }),

    getMediaDownloadTaskList: () => invokeTauri<MediaDownloadTask[]>("download_get_list"),
    syncMediaDownloadTaskList: (cb: (payload: MediaDownloadBroadcastPayload) => void) =>
      listenTauriEvent<MediaDownloadBroadcastPayload>("download:sync", cb),
    addMediaDownloadTask: (task: MediaDownloadInfo) => invokeTauri<void>("download_add", { task }),
    addMediaDownloadTaskList: (tasks: MediaDownloadInfo[]) => invokeTauri<void>("download_add_list", { tasks }),
    pauseMediaDownloadTask: (id: string) => invokeTauri<void>("download_pause", { id }),
    resumeMediaDownloadTask: (id: string) => invokeTauri<void>("download_resume", { id }),
    retryMediaDownloadTask: (id: string) => invokeTauri<void>("download_retry", { id }),
    cancelMediaDownloadTask: (id: string) => invokeTauri<void>("download_cancel", { id }),
    clearMediaDownloadTaskList: () => invokeTauri<void>("download_clear"),

    autoLyrics,

    // Step 4: App Updater
    checkAppUpdate: () => invokeTauri<CheckAppUpdateResult>("check_app_update"),
    onUpdateAvailable: cb =>
      listenTauriEvent<{ latestVersion: string; releaseNotes?: string }>("app:update-available", cb),
    getUpdateMode: () => invokeTauri<AppUpdateMode>("get_update_mode"),
    downloadAppUpdate: () => invokeTauri<void>("download_app_update"),
    onDownloadAppProgress: cb => listenTauriEvent<DownloadAppMessage>("app:update-message", cb),
    quitAndInstall: () => invokeTauri<boolean>("quit_and_install"),
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
