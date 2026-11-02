import { createRoot } from "react-dom/client";
import { HashRouter } from "react-router";

import { App } from "./app";

// 桌面歌词是透明窗口：渲染前就去掉 index.html 上的不透明背景类，避免首帧露出底色
if (window.location.hash.includes("desktop-lyrics")) {
  for (const el of [document.documentElement, document.body]) {
    el.classList.remove("bg-background");
    el.style.background = "transparent";
  }
}

const root = createRoot(document.getElementById("root") as Element);
root.render(
  <HashRouter>
    <App />
  </HashRouter>,
);
