/**
 * 子窗口（桌面歌词、迷你播放器）专用的极简 Tauri 桥。
 *
 * 子窗口入口不加载 `@/platform`（它会拉入自动歌词 worker、罗马音等主窗口依赖），
 * 只直接调用 `window.__TAURI__` / `window.__TAURI_INTERNALS__`。
 * 命令名与事件名必须与 `src/platform/index.ts` 完全一致（tests/sub-window-bridge.test.ts 会逐项比对）。
 * 不在 Tauri 中运行（浏览器预览、测试）时全部安全降级为 no-op。
 */

type Invoke = <T = unknown>(cmd: string, args?: Record<string, unknown>) => Promise<T>;
type Listen = <T = unknown>(event: string, handler: (event: { payload: T }) => void) => Promise<VoidFunction>;

interface TauriWindow {
  __TAURI__?: { core?: { invoke?: Invoke }; event?: { listen?: Listen } };
  __TAURI_INTERNALS__?: { invoke?: Invoke };
}

const tauriWindow = () => (typeof window === "undefined" ? undefined : (window as unknown as TauriWindow));

function getInvoke(): Invoke | undefined {
  const w = tauriWindow();
  return w?.__TAURI__?.core?.invoke ?? w?.__TAURI_INTERNALS__?.invoke;
}

export function invoke<T = unknown>(cmd: string, args?: Record<string, unknown>): Promise<T> {
  const fn = getInvoke();
  if (!fn) return Promise.reject(new Error(`[sub-window] invoke unavailable for command: ${cmd}`));
  return fn<T>(cmd, args);
}

/** 只发不等结果的命令：失败静默（与 platform 中 `.catch(() => {})` 的行为一致） */
const fire = (cmd: string, args?: Record<string, unknown>) => {
  void invoke(cmd, args).catch(() => {});
};

export function listen<T = unknown>(eventName: string, cb: (payload: T) => void): VoidFunction {
  let disposed = false;
  let unlisten: VoidFunction | null = null;
  const listenFn = tauriWindow()?.__TAURI__?.event?.listen;
  if (listenFn) {
    void listenFn<T>(eventName, event => {
      if (disposed) return;
      try {
        cb(event.payload);
      } catch (err) {
        console.error(`[sub-window] event listener error for ${eventName}:`, err);
      }
    })
      .then(fn => {
        if (disposed) fn();
        else unlisten = fn;
      })
      .catch(() => {});
  }
  return () => {
    disposed = true;
    unlisten?.();
    unlisten = null;
  };
}

/** 与 platform 的 detectPlatform 相同 */
export function getPlatform(): AppPlatForm {
  const ua = typeof navigator !== "undefined" ? navigator.userAgent : "";
  if (/Macintosh|Mac OS X/i.test(ua)) return "macos";
  if (/Windows/i.test(ua)) return "windows";
  return "linux";
}

export const getStore = <N extends StoreName>(name: N) =>
  invoke<StoreDataMap[N] | undefined>("store_get", { name }).catch(() => undefined);

// ---------- 迷你播放器 ----------

/** 迷你播放器 ⇄ 主窗口的 BroadcastChannel 名称，必须与 `src/common/utils/mini-player.ts` 一致 */
export const MINI_PLAYER_CHANNEL = "play-list-store-sync-channel";

export const createMiniPlayerChannel = () => new BroadcastChannel(MINI_PLAYER_CHANNEL);

/** 迷你播放器窗口里点击「展开」：只需让主进程切回主窗口 */
export const toggleMiniMode = () => invoke<void>("toggle_mini_player").catch(() => {});

// ---------- 桌面歌词 ----------

/** 锁定（鼠标穿透）时主进程轮询光标，转发为 mousemove / mouseleave，与 platform 的实现一致 */
const dispatchToLyricsRoot = (type: "mousemove" | "mouseleave") => {
  const target = document.querySelector("#root > div");
  target?.dispatchEvent(new MouseEvent(type, { bubbles: true }));
};

export const onDesktopLyricsContent = (cb: (payload: DesktopLyricsPayload) => void): VoidFunction => {
  const offContent = listen<DesktopLyricsPayload>("desktop-lyrics:content", cb);
  const offInside = listen("desktop-lyrics:cursor-inside", () => dispatchToLyricsRoot("mousemove"));
  const offLeave = listen("desktop-lyrics:cursor-leave", () => dispatchToLyricsRoot("mouseleave"));
  return () => {
    offContent();
    offInside();
    offLeave();
  };
};

export const getDesktopLyricsContent = () =>
  invoke<DesktopLyricsPayload | null>("desktop_lyrics_get_content")
    .then(v => v ?? undefined)
    .catch(() => undefined);

export const sendDesktopLyricsCommand = (command: DesktopLyricsCommand) => fire("desktop_lyrics_command", { command });

export const startDesktopLyricsDrag = () => fire("desktop_lyrics_start_drag");

export const endDesktopLyricsDrag = () => fire("desktop_lyrics_end_drag");

export const setDesktopLyricsIgnoreMouse = (ignore: boolean) => fire("desktop_lyrics_set_ignore_mouse", { ignore });

export const setDesktopLyricsContentHeight = (height: number) => fire("desktop_lyrics_set_content_height", { height });
