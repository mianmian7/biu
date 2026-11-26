import type { ReactNode } from "react";
import { createRoot } from "react-dom/client";

/** 子窗口公共启动：屏蔽 WebView 原生右键菜单（与主入口一致），然后渲染页面 */
export function bootSubWindow(node: ReactNode) {
  document.addEventListener("contextmenu", event => {
    const target = event.target as HTMLElement | null;
    if (target?.closest("input, textarea, [contenteditable=''], [contenteditable='true']")) {
      return;
    }
    event.preventDefault();
  });

  createRoot(document.getElementById("root") as Element).render(node);
}
