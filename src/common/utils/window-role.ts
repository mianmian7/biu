const SUB_WINDOW_HASH_RE = /^#\/?(mini-player|desktop-lyrics)(?:[/?#]|$)/;
const SUB_WINDOW_PATH_RE = /(?:^|\/)(mini-player|desktop-lyrics)\.html$/;

/**
 * 子窗口（迷你播放器、桌面歌词）的判断必须按精确入口或路由段匹配，
 * 不能用 `hash.includes("desktop-lyrics")`，否则主窗口打开 `#/settings?tab=desktop-lyrics` 会被误判为子窗口。
 */
export function isSubWindow() {
  const { pathname, hash } = window.location;
  return SUB_WINDOW_PATH_RE.test(pathname) || SUB_WINDOW_HASH_RE.test(hash);
}
