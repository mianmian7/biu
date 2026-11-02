import type { BrowserWindow } from "electron";

import { ipcMain } from "electron";

import {
  applyDesktopLyricsPayload,
  endDesktopLyricsDrag,
  getLastDesktopLyricsPayload,
  resetDesktopLyricsPosition,
  setDesktopLyricsContentHeight,
  setDesktopLyricsIgnoreMouse,
  startDesktopLyricsDrag,
} from "../desktop-lyrics";
import { updateDesktopLyricsState } from "../windows/tray";
import { channel } from "./channel";

const COMMANDS = new Set<DesktopLyricsCommand>([
  "prev",
  "next",
  "togglePlay",
  "toggleLock",
  "close",
  "fontLarger",
  "fontSmaller",
  "nextColor",
  "toggleLineMode",
  "toggleTranslation",
  "toggleRomaji",
  "openSettings",
]);

/**
 * 注册桌面歌词相关 IPC：
 * - 主窗口 -> 主进程：同步内容快照/开关状态
 * - 桌面歌词窗口 -> 主进程：初始化拉取内容、拖动、鼠标穿透、内容高度
 * - 桌面歌词窗口工具栏 -> 主窗口渲染进程：播放控制与样式切换（设置数据源在主窗口）
 */
export function registerDesktopLyricsHandlers({ getMainWindow }: { getMainWindow: () => BrowserWindow | null }) {
  // 主窗口同步桌面歌词状态与内容
  ipcMain.on(channel.desktopLyrics.sync, (_, payload: DesktopLyricsPayload) => {
    if (!payload || typeof payload !== "object") return;

    applyDesktopLyricsPayload(payload);
    updateDesktopLyricsState({
      enabled: Boolean(payload.enabled),
      locked: Boolean(payload.locked),
    });
  });

  // 桌面歌词窗口初始化时主动拉取当前内容
  ipcMain.handle(channel.desktopLyrics.getContent, () => getLastDesktopLyricsPayload());

  // 工具栏操作：打开设置由主进程直接处理，其余转发给主窗口渲染进程
  ipcMain.on(channel.desktopLyrics.command, (_, command: DesktopLyricsCommand) => {
    if (!COMMANDS.has(command)) return;

    const win = getMainWindow?.();
    if (!win || win.isDestroyed()) return;

    if (command === "openSettings") {
      win.show();
      win.focus();
      win.webContents.send(channel.router.navigate, "/settings?tab=desktop-lyrics");
      return;
    }

    win.webContents.send(channel.desktopLyrics.command, command);
  });

  ipcMain.on(channel.desktopLyrics.dragStart, () => startDesktopLyricsDrag());
  ipcMain.on(channel.desktopLyrics.dragEnd, () => endDesktopLyricsDrag());

  ipcMain.on(channel.desktopLyrics.setIgnoreMouse, (_, ignore: boolean) => {
    setDesktopLyricsIgnoreMouse(Boolean(ignore));
  });

  ipcMain.on(channel.desktopLyrics.setContentHeight, (_, height: number) => {
    if (typeof height !== "number" || !Number.isFinite(height)) return;
    setDesktopLyricsContentHeight(height);
  });

  ipcMain.on(channel.desktopLyrics.resetPosition, () => resetDesktopLyricsPosition());
}
