import { useCallback, useEffect, useLayoutEffect, useRef, useState } from "react";

import {
  RiCloseLine,
  RiLockLine,
  RiLockUnlockLine,
  RiPaletteLine,
  RiPauseFill,
  RiPlayFill,
  RiSettings3Line,
  RiSkipBackFill,
  RiSkipForwardFill,
  RiTranslate2,
} from "@remixicon/react";
import clsx from "classnames";

import { anchorNow, getDesktopLyricsLineProgress } from "@/common/utils/playback-anchor";
import DesktopLyricsView from "@/components/desktop-lyrics-view";

/** 顶部工具栏行高：始终预留，悬停显示工具栏时窗口高度不变 */
const TOOLBAR_HEIGHT = 34;
/** 内容区上下留白 */
const CONTENT_PADDING_Y = 6;
/** 锁定时悬停解锁按钮在指针静止多久后隐藏 */
const LOCKED_HOVER_HIDE_MS = 1500;

/** 把窗口背景透明化，只保留歌词内容 */
const useTransparentWindowStyle = () => {
  useLayoutEffect(() => {
    const root = document.documentElement;
    const body = document.body;
    const rootEl = document.querySelector<HTMLDivElement>("#root");

    root.classList.remove("bg-background");
    body.classList.remove("bg-background");
    root.style.background = "transparent";
    body.style.background = "transparent";
    body.style.margin = "0";
    body.style.overflow = "hidden";

    if (rootEl) {
      rootEl.style.background = "transparent";
      rootEl.style.overflow = "hidden";
    }
  }, []);
};

const ToolButton = ({
  title,
  active,
  onClick,
  children,
}: {
  title: string;
  active?: boolean;
  onClick: () => void;
  children: React.ReactNode;
}) => (
  <button
    type="button"
    title={title}
    aria-label={title}
    aria-pressed={active}
    className={clsx(
      "flex h-7 min-w-7 cursor-pointer items-center justify-center rounded-md px-1 text-[13px] font-semibold transition-colors",
      active ? "text-white" : "text-white/70",
      "hover:bg-white/15 hover:text-white",
    )}
    onClick={onClick}
  >
    {children}
  </button>
);

const Divider = () => <span className="mx-1 h-4 w-px bg-white/20" />;

const send = (command: DesktopLyricsCommand) => window.electron.sendDesktopLyricsCommand(command);

/**
 * 桌面悬浮歌词窗口页面（独立 BrowserWindow，透明置顶）
 *
 * 内容由主窗口通过主进程 IPC 同步过来，这里只负责展示、进度插值与窗口交互；
 * 播放控制与样式设置以主窗口渲染进程为数据源。
 */
const DesktopLyrics = () => {
  useTransparentWindowStyle();

  const [payload, setPayload] = useState<DesktopLyricsPayload | null>(null);
  const payloadRef = useRef<DesktopLyricsPayload | null>(null);
  const [hovered, setHovered] = useState(false);
  const [dragging, setDragging] = useState(false);
  const hideTimerRef = useRef<number | null>(null);
  const containerRef = useRef<HTMLDivElement | null>(null);

  useEffect(() => {
    const offContent = window.electron.onDesktopLyricsContent(nextPayload => {
      if (!nextPayload) return;
      payloadRef.current = nextPayload;
      setPayload(nextPayload);
    });

    // 窗口重载可能错过主进程的首次推送，这里主动拉取一次
    void window.electron.getDesktopLyricsContent().then(nextPayload => {
      if (!nextPayload || payloadRef.current) return;
      payloadRef.current = nextPayload;
      setPayload(nextPayload);
    });

    return () => {
      offContent?.();
    };
  }, []);

  /** 按最近一次同步的位置与时间戳插值出当前行进度 */
  const getProgress = useCallback(() => {
    const current = payloadRef.current;
    return current ? getDesktopLyricsLineProgress(current, anchorNow()) : 0;
  }, []);

  // 内容高度变化时通知主进程调整窗口高度
  useEffect(() => {
    const el = containerRef.current;
    if (!el) return;

    let lastHeight = 0;
    const report = () => {
      const height = Math.ceil(el.getBoundingClientRect().height) + TOOLBAR_HEIGHT + CONTENT_PADDING_Y * 2;
      if (height === lastHeight) return;
      lastHeight = height;
      window.electron.setDesktopLyricsContentHeight(height);
    };

    report();
    const observer = new ResizeObserver(report);
    observer.observe(el);
    return () => observer.disconnect();
  }, []);

  const locked = payload?.locked ?? true;

  const clearHideTimer = () => {
    if (hideTimerRef.current !== null) {
      window.clearTimeout(hideTimerRef.current);
      hideTimerRef.current = null;
    }
  };

  // 锁定时鼠标事件是转发来的，离开窗口不一定收到 mouseleave，静止一段时间后自动隐藏
  const handleMouseMove = () => {
    setHovered(true);
    clearHideTimer();
    if (locked) {
      hideTimerRef.current = window.setTimeout(() => setHovered(false), LOCKED_HOVER_HIDE_MS);
    }
  };

  const handleMouseLeave = () => {
    clearHideTimer();
    setHovered(false);
  };

  useEffect(() => clearHideTimer, []);

  // 拖动：左键按下开始，任意位置松开结束
  useEffect(() => {
    if (!dragging) return;
    const stop = () => {
      setDragging(false);
      window.electron.endDesktopLyricsDrag();
    };
    window.addEventListener("mouseup", stop);
    window.addEventListener("blur", stop);
    return () => {
      window.removeEventListener("mouseup", stop);
      window.removeEventListener("blur", stop);
    };
  }, [dragging]);

  // 锁定后结束可能进行中的拖动
  useEffect(() => {
    if (locked) setDragging(false);
  }, [locked]);

  const handleMouseDown = (event: React.MouseEvent) => {
    if (locked || event.button !== 0) return;
    if ((event.target as HTMLElement).closest("[data-no-drag]")) return;
    event.preventDefault();
    setDragging(true);
    window.electron.startDesktopLyricsDrag();
  };

  const style = payload?.style;
  const showPanel = !locked && (hovered || dragging);

  return (
    <div
      className={clsx(
        "relative flex h-screen w-screen flex-col overflow-hidden rounded-xl select-none",
        !locked && (dragging ? "cursor-grabbing" : "cursor-grab"),
      )}
      style={{
        background: showPanel ? "rgba(12, 14, 18, 0.42)" : "transparent",
        transition: "background-color 160ms ease",
      }}
      onMouseMove={handleMouseMove}
      onMouseEnter={handleMouseMove}
      onMouseLeave={handleMouseLeave}
      onMouseDown={handleMouseDown}
    >
      <div className="flex flex-none items-center justify-center" style={{ height: TOOLBAR_HEIGHT }}>
        {showPanel && payload && style ? (
          <div data-no-drag className="flex cursor-default items-center rounded-lg bg-black/45 px-1.5">
            <ToolButton title="上一首" onClick={() => send("prev")}>
              <RiSkipBackFill size={16} />
            </ToolButton>
            <ToolButton title={payload.isPlaying ? "暂停" : "播放"} onClick={() => send("togglePlay")}>
              {payload.isPlaying ? <RiPauseFill size={18} /> : <RiPlayFill size={18} />}
            </ToolButton>
            <ToolButton title="下一首" onClick={() => send("next")}>
              <RiSkipForwardFill size={16} />
            </ToolButton>
            <Divider />
            <ToolButton title="减小字号" onClick={() => send("fontSmaller")}>
              A-
            </ToolButton>
            <ToolButton title="增大字号" onClick={() => send("fontLarger")}>
              A+
            </ToolButton>
            <ToolButton title="切换配色" onClick={() => send("nextColor")}>
              <RiPaletteLine size={16} />
            </ToolButton>
            <ToolButton
              title={style.lineMode === "double" ? "切换为单行" : "切换为双行"}
              onClick={() => send("toggleLineMode")}
            >
              {style.lineMode === "double" ? "双" : "单"}
            </ToolButton>
            <ToolButton title="显示翻译" active={style.showTranslation} onClick={() => send("toggleTranslation")}>
              <RiTranslate2 size={16} />
            </ToolButton>
            <ToolButton title="显示罗马音" active={style.showRomaji} onClick={() => send("toggleRomaji")}>
              音
            </ToolButton>
            <Divider />
            <ToolButton title="锁定桌面歌词（鼠标穿透）" onClick={() => send("toggleLock")}>
              <RiLockLine size={16} />
            </ToolButton>
            <ToolButton title="桌面歌词设置" onClick={() => send("openSettings")}>
              <RiSettings3Line size={16} />
            </ToolButton>
            <ToolButton title="关闭桌面歌词" onClick={() => send("close")}>
              <RiCloseLine size={17} />
            </ToolButton>
          </div>
        ) : null}

        {locked && hovered ? (
          <button
            type="button"
            title="解锁桌面歌词"
            aria-label="解锁桌面歌词"
            className="flex h-7 w-7 cursor-pointer items-center justify-center rounded-full bg-black/45 text-white/85 transition-colors hover:bg-black/65 hover:text-white"
            onMouseEnter={() => window.electron.setDesktopLyricsIgnoreMouse(false)}
            onMouseLeave={() => window.electron.setDesktopLyricsIgnoreMouse(true)}
            onClick={() => {
              window.electron.setDesktopLyricsIgnoreMouse(true);
              send("toggleLock");
            }}
          >
            <RiLockUnlockLine size={15} />
          </button>
        ) : null}
      </div>

      <div
        className="flex flex-1 items-center px-4"
        style={{ paddingTop: CONTENT_PADDING_Y, paddingBottom: CONTENT_PADDING_Y }}
      >
        <div ref={containerRef} className="w-full">
          {payload && style ? (
            <DesktopLyricsView
              fontSize={payload.fontSize}
              style={style}
              colors={payload.colors}
              text={payload.text}
              romaji={payload.romaji}
              translation={payload.translation}
              nextText={payload.nextText}
              status={payload.status}
              hasTranslation={payload.hasTranslation}
              hasRomaji={payload.hasRomaji}
              index={payload.index}
              playId={payload.playId}
              getProgress={getProgress}
            />
          ) : null}
        </div>
      </div>
    </div>
  );
};

export default DesktopLyrics;
