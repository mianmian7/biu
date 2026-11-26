import { useEffect, useState } from "react";

import { listenTauriEvent } from "@/platform";

/** 主进程在主窗口显示/隐藏（含隐藏到托盘）时发出：`{ visible: boolean }` */
export const WINDOW_VISIBILITY_EVENT = "biu:window-visibility";

/** 窗口隐藏后延迟多久卸载可视 UI；期间重新显示会取消卸载 */
export const HIDDEN_UNMOUNT_DELAY_MS = 3000;

export interface WindowVisibilityPayload {
  visible: boolean;
}

type Subscribe = (cb: (payload: WindowVisibilityPayload) => void) => VoidFunction;

const subscribeWindowVisibility: Subscribe = cb =>
  listenTauriEvent<WindowVisibilityPayload>(WINDOW_VISIBILITY_EVENT, cb);

/**
 * 主窗口可视 UI 是否应该挂载。
 *
 * - 收到 `visible: false` 后等待 `delayMs`，再返回 false（调用方据此卸载重型 UI）
 * - 收到 `visible: true` 立即返回 true，并取消尚未执行的卸载
 * 播放、快捷键、歌词同步和迷你播放器 BroadcastChannel 都在 App / 模块级运行，不受影响。
 */
export function useWindowUiVisible(
  subscribe: Subscribe = subscribeWindowVisibility,
  delayMs = HIDDEN_UNMOUNT_DELAY_MS,
) {
  const [visible, setVisible] = useState(true);

  useEffect(() => {
    let timer: ReturnType<typeof setTimeout> | null = null;
    const clear = () => {
      if (timer !== null) {
        clearTimeout(timer);
        timer = null;
      }
    };

    const off = subscribe(payload => {
      if (typeof payload?.visible !== "boolean") return;
      clear();
      if (payload.visible) {
        setVisible(true);
      } else {
        timer = setTimeout(() => {
          timer = null;
          setVisible(false);
        }, delayMs);
      }
    });

    return () => {
      clear();
      off();
    };
  }, [subscribe, delayMs]);

  return visible;
}
