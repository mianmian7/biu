import { memo, useEffect, useMemo, useRef, useState } from "react";

import {
  RiExpandDiagonalLine,
  RiFileImageLine,
  RiPauseCircleFill,
  RiPlayCircleFill,
  RiSkipBackFill,
  RiSkipForwardFill,
} from "@remixicon/react";
import clx from "classnames";
import { useShallow } from "zustand/react/shallow";

import { getPlayModeList } from "@/common/constants/audio";
import { formatUrlProtocol } from "@/common/utils/url";
import { createMiniPlayerChannel, toggleMiniMode } from "@/entries/sub-window-bridge";
import { usePlayProgress } from "@/store/play-progress";

import { getMiniCoverSrc } from "./cover";
import { usePlayState } from "./play-state";
import { useStyle } from "./use-style";

const PlayModeList = getPlayModeList(16);

/** 与 HeroUI `Button isIconOnly size="sm" variant="light"` 视觉一致的原生按钮 */
const IconButton = ({
  className,
  children,
  ...rest
}: React.ButtonHTMLAttributes<HTMLButtonElement> & { children: React.ReactNode }) => (
  <button
    type="button"
    data-tauri-drag-region="false"
    className={clx(
      "window-no-drag rounded-small text-foreground hover:bg-default/40 hover:text-primary inline-flex h-8 w-8 min-w-8 shrink-0 cursor-pointer items-center justify-center outline-none select-none",
      "focus-visible:outline-focus focus-visible:outline-2 focus-visible:outline-offset-2",
      "disabled:pointer-events-none disabled:opacity-50",
      className,
    )}
    {...rest}
  >
    {children}
  </button>
);

const CoverView = memo(() => {
  const cover = usePlayState(s => s.cover);
  const resized = getMiniCoverSrc(cover);
  const original = formatUrlProtocol(cover);
  const [failed, setFailed] = useState<{ src?: string; level: 0 | 1 | 2 }>({ level: 0 });
  // 封面变化时重置失败状态
  const level = failed.src === cover ? failed.level : 0;

  if (!cover) return null;

  // 先用缩略图；失败回退原图；再失败显示占位
  const src = level === 0 ? resized : original;

  return (
    <div className="relative h-full w-[100px] flex-shrink-0">
      {level < 2 && src ? (
        <img
          src={src}
          alt=""
          width={100}
          loading="eager"
          decoding="async"
          draggable={false}
          className="h-full w-[100px] object-cover"
          style={{
            transform: "translateZ(0)",
            backfaceVisibility: "hidden",
            willChange: "transform",
            contain: "paint",
          }}
          onError={() => setFailed({ src: cover, level: level === 0 && resized !== original ? 1 : 2 })}
        />
      ) : (
        <div className="border-content2 flex h-full items-center justify-center rounded-md border">
          <RiFileImageLine size="40%" className="text-default-500" />
        </div>
      )}
      <div className="from-background pointer-events-none absolute inset-y-0 right-0 z-10 w-12 bg-gradient-to-l to-transparent" />
    </div>
  );
});

const MiniPlayer = () => {
  const { isSingle, isPlaying, title, duration, playMode } = usePlayState(
    useShallow(state => ({
      isSingle: state.isSingle,
      isPlaying: state.isPlaying,
      title: state.title,
      duration: state.duration,
      playMode: state.playMode,
    })),
  );
  const currentTime = usePlayProgress(s => s.currentTime);
  const setCurrentTime = usePlayProgress(s => s.setCurrentTime);
  const updatePlayState = usePlayState(state => state.update);
  const bcRef = useRef<BroadcastChannel>(null);

  const postMessage = (type: string, state?: any) => {
    if (!bcRef.current) return;
    bcRef.current.postMessage({
      from: "mini",
      data: {
        type,
        state,
      },
      ts: Date.now(),
    });
  };

  useStyle();

  const playModeIcon = useMemo(() => {
    return PlayModeList.find(item => item.value === playMode)?.icon;
  }, [playMode]);

  useEffect(() => {
    bcRef.current = createMiniPlayerChannel();
    postMessage("init");

    bcRef.current.onmessage = ev => {
      const { from, state } = ev.data || {};
      if (from !== "main" || !state) return;

      updatePlayState(state);
      if (typeof state.currentTime === "number") {
        setCurrentTime(state.currentTime);
      }
    };

    return () => {
      if (!bcRef.current) return;
      bcRef.current.close();
    };
    // eslint-disable-next-line react-hooks/exhaustive-deps
  }, []);

  const handleSeek = (v: number) => {
    postMessage("seek", { currentTime: v });
  };

  const max = Math.max(0, duration || 0);
  const value = Math.min(Math.max(0, currentTime || 0), max);
  const percent = max > 0 ? (value / max) * 100 : 0;
  const disabled = !title;

  return (
    <div
      data-tauri-drag-region="deep"
      className="window-drag rounded-medium flex h-screen w-screen flex-col overflow-hidden select-none"
    >
      <div className="flex h-full items-center">
        <CoverView />
        <div className="flex min-w-0 flex-1 flex-col space-y-1 px-2">
          <div className="flex min-w-0 flex-col">
            {title ? (
              <span className="truncate text-center text-sm font-medium">{title}</span>
            ) : (
              <span className="text-center text-sm text-zinc-500">暂无播放内容</span>
            )}
          </div>
          <div data-tauri-drag-region="false" className="window-no-drag group mt-1 flex h-5 items-center">
            <input
              type="range"
              aria-label="播放进度"
              min={0}
              max={max}
              step={1}
              value={value}
              disabled={disabled}
              onChange={e => handleSeek(Number(e.currentTarget.value))}
              className={clx(
                "h-[4px] w-full flex-1 cursor-pointer appearance-none rounded-full bg-transparent outline-none",
                "disabled:cursor-default disabled:opacity-50",
                "[&::-webkit-slider-thumb]:bg-primary [&::-webkit-slider-thumb]:h-3 [&::-webkit-slider-thumb]:w-3 [&::-webkit-slider-thumb]:appearance-none [&::-webkit-slider-thumb]:rounded-full [&::-webkit-slider-thumb]:opacity-0 [&::-webkit-slider-thumb]:shadow-sm",
                "[&::-moz-range-thumb]:bg-primary [&::-moz-range-thumb]:h-3 [&::-moz-range-thumb]:w-3 [&::-moz-range-thumb]:rounded-full [&::-moz-range-thumb]:border-0 [&::-moz-range-thumb]:opacity-0",
                !disabled &&
                  "group-hover:[&::-moz-range-thumb]:opacity-100 group-hover:[&::-webkit-slider-thumb]:opacity-100",
              )}
              style={{
                background: `linear-gradient(to right, hsl(var(--heroui-primary)) ${percent}%, hsl(var(--heroui-default-300)) ${percent}%)`,
              }}
            />
          </div>
          <div className="flex items-center justify-between space-x-1">
            <IconButton onClick={() => postMessage("togglePlayMode")} aria-label="播放模式">
              {playModeIcon}
            </IconButton>
            <div className="flex items-center space-x-1">
              <IconButton disabled={disabled || isSingle} onClick={() => postMessage("prev")} aria-label="上一首">
                <RiSkipBackFill size={18} />
              </IconButton>
              <IconButton
                disabled={disabled}
                onClick={() => postMessage("togglePlay")}
                aria-label={isPlaying ? "暂停" : "播放"}
              >
                {isPlaying ? <RiPauseCircleFill size={28} /> : <RiPlayCircleFill size={28} />}
              </IconButton>
              <IconButton disabled={disabled || isSingle} onClick={() => postMessage("next")} aria-label="下一首">
                <RiSkipForwardFill size={18} />
              </IconButton>
            </div>
            <IconButton onClick={() => void toggleMiniMode()} aria-label="展开主窗口">
              <RiExpandDiagonalLine size={16} />
            </IconButton>
          </div>
        </div>
      </div>
    </div>
  );
};

export default MiniPlayer;
