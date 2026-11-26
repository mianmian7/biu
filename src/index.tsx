import { createRoot } from "react-dom/client";
import { HashRouter } from "react-router";

import { applyMigratedLocalStorage } from "./platform";

// 桌面歌词是透明窗口：渲染前就去掉 index.html 上的不透明背景类，避免首帧露出底色
if (window.location.hash.includes("desktop-lyrics")) {
  for (const el of [document.documentElement, document.body]) {
    el.classList.remove("bg-background");
    el.style.background = "transparent";
  }
}

// WebViews show a native Back/Reload menu on right click. Electron had none, so suppress it
// everywhere except editable fields; in-app menus call preventDefault themselves and still open.
document.addEventListener("contextmenu", event => {
  const target = event.target as HTMLElement | null;
  if (target?.closest("input, textarea, [contenteditable=''], [contenteditable='true']")) {
    return;
  }
  event.preventDefault();
});

async function boot() {
  // index.html 已经跳转到子窗口的轻量入口，不再加载主应用
  if ((window as { __BIU_SUB_WINDOW_REDIRECT__?: boolean }).__BIU_SUB_WINDOW_REDIRECT__) return;
  // Stores hydrate from localStorage when ./app is imported, so migrated entries go first.
  await applyMigratedLocalStorage();
  const { App } = await import("./app");
  const root = createRoot(document.getElementById("root") as Element);
  root.render(
    <HashRouter>
      <App />
    </HashRouter>,
  );
}

void boot();
