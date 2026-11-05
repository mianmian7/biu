/**
 * 子窗口（迷你播放器、桌面歌词）加载同一套前端，但播放只能发生在主窗口。
 * Tauri 事件会广播到所有窗口，子窗口必须忽略播放相关的事件。
 */
export function isSubWindow() {
  const hash = window.location.hash;
  return hash.includes("mini-player") || hash.includes("desktop-lyrics");
}
