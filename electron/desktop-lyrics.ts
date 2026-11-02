import { BrowserWindow, screen } from "electron";
import isDev from "electron-is-dev";
import log from "electron-log";
import path from "node:path";
import { fileURLToPath } from "node:url";

import { channel } from "./ipc/channel";
import { desktopLyricsStore } from "./store";

const __filename = fileURLToPath(import.meta.url);
const __dirname = path.dirname(__filename);

const DEFAULT_WIDTH = 1000;
const DEFAULT_HEIGHT = 150;
const MIN_WIDTH = 360;
const MIN_HEIGHT = 60;
const MAX_HEIGHT = 520;
/** 宽度不设上限：给一个足够大的值（DIP） */
const UNBOUNDED_WIDTH = 20000;
/** 拖动时跟随光标的刷新间隔 */
const DRAG_INTERVAL_MS = 12;
/** 内容变化后强制整窗重绘的延迟（等待淡入动画结束） */
const REPAINT_DELAY_MS = 260;
/** 默认位置距离屏幕底部（任务栏上方）的间距 */
const DEFAULT_BOTTOM_OFFSET = 72;
/** 判断已保存位置是否仍可见的最小重叠区域 */
const MIN_VISIBLE_SIZE = 40;

let desktopLyricsWindow: BrowserWindow | null = null;
/** 最近一次主窗口同步过来的内容，用于窗口创建/重载后补发 */
let lastPayload: DesktopLyricsPayload | null = null;
let isContentReady = false;
let saveBoundsTimer: NodeJS.Timeout | null = null;
let dragTimer: NodeJS.Timeout | null = null;
let repaintTimer: NodeJS.Timeout | null = null;
/** 渲染进程上报的内容高度，窗口高度跟随该值 */
let contentHeight: number | null = null;
/** 锁定状态下，指针悬停在解锁按钮上时临时取消穿透 */
let hoverUnlockButton = false;

/** 计算默认窗口位置：主屏工作区底部居中 */
const getDefaultBounds = (): DesktopLyricsBounds => {
  const { workArea } = screen.getPrimaryDisplay();
  const width = Math.min(DEFAULT_WIDTH, Math.max(MIN_WIDTH, workArea.width - 80));
  const preferredHeight = contentHeight === null ? DEFAULT_HEIGHT : Math.min(MAX_HEIGHT, Math.round(contentHeight));
  const height = Math.min(preferredHeight, Math.max(MIN_HEIGHT, workArea.height - 80));

  return {
    x: Math.round(workArea.x + (workArea.width - width) / 2),
    y: Math.round(workArea.y + workArea.height - height - DEFAULT_BOTTOM_OFFSET),
    width,
    height,
  };
};

/** 判断保存的位置是否还落在某个显示器的工作区内 */
const isBoundsVisible = (bounds: DesktopLyricsBounds) => {
  return screen.getAllDisplays().some(({ workArea }) => {
    const overlapWidth =
      Math.min(bounds.x + bounds.width, workArea.x + workArea.width) - Math.max(bounds.x, workArea.x);
    const overlapHeight =
      Math.min(bounds.y + bounds.height, workArea.y + workArea.height) - Math.max(bounds.y, workArea.y);
    return overlapWidth >= MIN_VISIBLE_SIZE && overlapHeight >= MIN_VISIBLE_SIZE;
  });
};

/** 读取上次保存的位置；位置已不可见（如拔掉副屏）时回退到默认位置 */
const getInitialBounds = (): DesktopLyricsBounds => {
  const saved = desktopLyricsStore.get("bounds");

  if (saved && isBoundsVisible(saved)) {
    return {
      x: saved.x,
      y: saved.y,
      width: Math.max(MIN_WIDTH, saved.width ?? DEFAULT_WIDTH),
      height: Math.max(MIN_HEIGHT, saved.height ?? DEFAULT_HEIGHT),
    };
  }

  return getDefaultBounds();
};

/** 把当前窗口位置写入本地存储 */
const persistBounds = () => {
  if (!desktopLyricsWindow || desktopLyricsWindow.isDestroyed()) return;

  try {
    desktopLyricsStore.set("bounds", desktopLyricsWindow.getBounds());
  } catch (err) {
    log.warn("[desktop-lyrics] save bounds failed:", err);
  }
};

/** 拖动/缩放时防抖保存位置，避免频繁写盘 */
const persistBoundsDebounced = () => {
  if (saveBoundsTimer) {
    clearTimeout(saveBoundsTimer);
  }
  saveBoundsTimer = setTimeout(() => {
    saveBoundsTimer = null;
    persistBounds();
  }, 400);
};

/** 窗口被拖动到屏幕外后重新拉回默认位置 */
const ensureBoundsVisible = () => {
  if (!desktopLyricsWindow || desktopLyricsWindow.isDestroyed()) return;

  const bounds = desktopLyricsWindow.getBounds();
  if (!isBoundsVisible(bounds)) {
    desktopLyricsWindow.setBounds(getDefaultBounds());
  }
};

/** 应用锁定状态：锁定后鼠标穿透窗口，不影响桌面其他操作 */
const applyLockedState = (locked: boolean) => {
  if (!desktopLyricsWindow || desktopLyricsWindow.isDestroyed()) return;

  if (!locked) hoverUnlockButton = false;
  const ignore = locked && !hoverUnlockButton;

  try {
    // forward：穿透时仍把鼠标移动事件转发给页面，用于悬停显示解锁按钮（Windows/macOS）
    desktopLyricsWindow.setIgnoreMouseEvents(ignore, { forward: true });
  } catch (err) {
    log.warn("[desktop-lyrics] set ignore mouse events failed:", err);
  }
};

/** 把最近一次内容快照发送给桌面歌词窗口 */
const sendContentToWindow = () => {
  if (!desktopLyricsWindow || desktopLyricsWindow.isDestroyed() || !isContentReady) return;

  try {
    desktopLyricsWindow.webContents.send(channel.desktopLyrics.content, lastPayload);
  } catch (err) {
    log.warn("[desktop-lyrics] send content failed:", err);
  }
};

/**
 * 内容变化后强制整窗重绘。
 *
 * 透明窗口只重绘变化区域，文字阴影/描边/淡入动画留下的半透明像素可能残留；
 * 等动画结束后整窗重绘一次即可清掉。
 */
const scheduleRepaint = () => {
  if (repaintTimer) clearTimeout(repaintTimer);
  repaintTimer = setTimeout(() => {
    repaintTimer = null;
    if (!desktopLyricsWindow || desktopLyricsWindow.isDestroyed()) return;
    try {
      desktopLyricsWindow.webContents.invalidate();
    } catch (err) {
      log.warn("[desktop-lyrics] invalidate failed:", err);
    }
  }, REPAINT_DELAY_MS);
};

/** 判断两次快照是否有可见内容变化（不含播放位置） */
const hasVisualChange = (prev: DesktopLyricsPayload | null, next: DesktopLyricsPayload) => {
  if (!prev) return true;
  return (
    prev.playId !== next.playId ||
    prev.index !== next.index ||
    prev.text !== next.text ||
    prev.locked !== next.locked ||
    prev.fontSize !== next.fontSize ||
    prev.isPlaying !== next.isPlaying ||
    JSON.stringify(prev.style) !== JSON.stringify(next.style) ||
    JSON.stringify(prev.colors) !== JSON.stringify(next.colors)
  );
};

/** 按内容高度锁定窗口高度（宽度仍可拖动调整） */
const applyContentHeight = () => {
  if (!desktopLyricsWindow || desktopLyricsWindow.isDestroyed() || contentHeight === null) return;

  const height = Math.round(Math.min(MAX_HEIGHT, Math.max(MIN_HEIGHT, contentHeight)));
  const bounds = desktopLyricsWindow.getBounds();

  try {
    desktopLyricsWindow.setMinimumSize(MIN_WIDTH, height);
    desktopLyricsWindow.setMaximumSize(UNBOUNDED_WIDTH, height);
  } catch (err) {
    log.warn("[desktop-lyrics] set size constraints failed:", err);
  }

  if (bounds.height === height) return;

  // 窗口在屏幕下半部分时保持底边不动（向上长高），否则保持顶边不动
  const { workArea } = screen.getDisplayMatching(bounds);
  const anchorBottom = bounds.y + bounds.height / 2 > workArea.y + workArea.height / 2;
  const y = anchorBottom ? bounds.y + bounds.height - height : bounds.y;

  desktopLyricsWindow.setBounds({ x: bounds.x, y: Math.round(y), width: bounds.width, height });
  persistBoundsDebounced();
};

/**
 * 根据内容快照控制窗口显示：
 * - 有歌词且页面已加载时显示窗口（暂停时保持显示）
 * - 无歌词/未启用时隐藏窗口
 */
const applyVisibility = () => {
  if (!desktopLyricsWindow || desktopLyricsWindow.isDestroyed()) return;

  const payload = lastPayload;
  if (!payload) return;

  applyLockedState(payload.locked);

  if (payload.active && isContentReady) {
    ensureBoundsVisible();
    if (!desktopLyricsWindow.isVisible()) {
      desktopLyricsWindow.showInactive();
    }
  } else if (desktopLyricsWindow.isVisible()) {
    desktopLyricsWindow.hide();
  }
};

/** 创建桌面歌词窗口（透明、无边框、置顶、不占任务栏） */
const createDesktopLyricsWindow = () => {
  if (desktopLyricsWindow && !desktopLyricsWindow.isDestroyed()) {
    return desktopLyricsWindow;
  }

  const bounds = getInitialBounds();
  isContentReady = false;

  desktopLyricsWindow = new BrowserWindow({
    ...bounds,
    title: "Biu Desktop Lyrics",
    show: false,
    hasShadow: false,
    frame: false,
    transparent: true,
    backgroundColor: "#00000000",
    resizable: true,
    movable: true,
    minimizable: false,
    maximizable: false,
    fullscreenable: false,
    closable: false,
    alwaysOnTop: true,
    skipTaskbar: true,
    minWidth: MIN_WIDTH,
    minHeight: MIN_HEIGHT,
    webPreferences: {
      preload: path.join(__dirname, "preload.cjs"),
      webSecurity: true,
      contextIsolation: true,
      nodeIntegration: false,
      devTools: isDev,
      // 桌面歌词窗口常驻后台，避免隐藏时定时器被节流
      backgroundThrottling: false,
    },
  });

  desktopLyricsWindow.setAlwaysOnTop(true, "floating");

  desktopLyricsWindow.webContents.setWindowOpenHandler(() => {
    return { action: "deny" };
  });

  desktopLyricsWindow.webContents.on("before-input-event", (event, input) => {
    if ((input.control || input.meta) && input.key.toLowerCase() === "r") {
      event.preventDefault();
    }
  });

  desktopLyricsWindow.webContents.on("context-menu", e => {
    e.preventDefault();
  });

  if (process.platform === "win32" && desktopLyricsWindow) {
    // 拦截 WM_INITMENU (0x0116) 消息，阻止系统菜单
    desktopLyricsWindow.hookWindowMessage(0x0116, () => {
      desktopLyricsWindow?.setEnabled(false);
      setTimeout(() => {
        desktopLyricsWindow?.setEnabled(true);
      }, 100);
      return true;
    });
  }

  desktopLyricsWindow.on("moved", persistBoundsDebounced);
  desktopLyricsWindow.on("resized", persistBoundsDebounced);

  desktopLyricsWindow.on("closed", () => {
    stopDrag();
    desktopLyricsWindow = null;
    isContentReady = false;
  });

  // 失焦时结束拖动，避免漏掉 mouseup 后窗口一直跟随光标
  desktopLyricsWindow.on("blur", () => {
    if (dragTimer) endDesktopLyricsDrag();
  });

  desktopLyricsWindow.webContents.on("did-finish-load", () => {
    isContentReady = true;
    applyContentHeight();
    sendContentToWindow();
    applyVisibility();
  });

  const indexPath = path.resolve(__dirname, "../dist/web/index.html");
  void desktopLyricsWindow.loadFile(indexPath, { hash: "desktop-lyrics" });

  return desktopLyricsWindow;
};

/** 销毁桌面歌词窗口并清理保存位置的定时器 */
const destroyDesktopLyricsWindow = () => {
  if (saveBoundsTimer) {
    clearTimeout(saveBoundsTimer);
    saveBoundsTimer = null;
  }
  if (repaintTimer) {
    clearTimeout(repaintTimer);
    repaintTimer = null;
  }
  stopDrag();
  hoverUnlockButton = false;

  if (desktopLyricsWindow && !desktopLyricsWindow.isDestroyed()) {
    desktopLyricsWindow.destroy();
  }

  desktopLyricsWindow = null;
  isContentReady = false;
};

/**
 * 应用主窗口同步过来的桌面歌词状态：
 * - 关闭时销毁窗口；开启时按需创建窗口
 * - 锁定状态对应鼠标穿透
 * - 仅在有歌词内容时显示窗口（暂停时保持显示）
 */
export const applyDesktopLyricsPayload = (payload: DesktopLyricsPayload) => {
  const visualChanged = hasVisualChange(lastPayload, payload);
  lastPayload = payload;

  if (!payload.enabled) {
    destroyDesktopLyricsWindow();
    return;
  }

  if (!desktopLyricsWindow || desktopLyricsWindow.isDestroyed()) {
    createDesktopLyricsWindow();
    return;
  }

  sendContentToWindow();
  applyVisibility();
  if (visualChanged) scheduleRepaint();
};

/** 获取最近一次同步的内容快照（页面重载后用于补发） */
export const getLastDesktopLyricsPayload = () => lastPayload;

/** 把窗口重置回默认位置（主屏底部居中） */
export const resetDesktopLyricsPosition = () => {
  if (!desktopLyricsWindow || desktopLyricsWindow.isDestroyed()) return;

  desktopLyricsWindow.setBounds(getDefaultBounds());
  persistBounds();
};

/** 停止跟随光标 */
function stopDrag() {
  if (dragTimer) {
    clearInterval(dragTimer);
    dragTimer = null;
  }
}

/**
 * 开始拖动：主进程按固定间隔读取光标位置移动窗口。
 *
 * 不用 `app-region: drag`，因为拖拽区域收不到鼠标悬停事件，无法实现悬停显示工具栏。
 */
export const startDesktopLyricsDrag = () => {
  if (!desktopLyricsWindow || desktopLyricsWindow.isDestroyed()) return;
  if (lastPayload?.locked) return;

  stopDrag();
  const startCursor = screen.getCursorScreenPoint();
  const startBounds = desktopLyricsWindow.getBounds();

  dragTimer = setInterval(() => {
    if (!desktopLyricsWindow || desktopLyricsWindow.isDestroyed()) {
      stopDrag();
      return;
    }
    const cursor = screen.getCursorScreenPoint();
    // 固定宽高调用 setBounds，避免跨不同缩放比例的屏幕时窗口尺寸漂移
    desktopLyricsWindow.setBounds({
      x: startBounds.x + cursor.x - startCursor.x,
      y: startBounds.y + cursor.y - startCursor.y,
      width: startBounds.width,
      height: startBounds.height,
    });
  }, DRAG_INTERVAL_MS);
};

/** 结束拖动并保存位置 */
export const endDesktopLyricsDrag = () => {
  if (!dragTimer) return;
  stopDrag();
  ensureBoundsVisible();
  persistBoundsDebounced();
};

/** 锁定状态下，指针进入/离开解锁按钮时切换穿透 */
export const setDesktopLyricsIgnoreMouse = (ignore: boolean) => {
  hoverUnlockButton = !ignore;
  applyLockedState(Boolean(lastPayload?.locked));
};

/** 渲染进程上报内容高度 */
export const setDesktopLyricsContentHeight = (height: number) => {
  contentHeight = height;
  applyContentHeight();
};

export { createDesktopLyricsWindow, destroyDesktopLyricsWindow };
