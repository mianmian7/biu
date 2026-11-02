import { useEffect, useLayoutEffect, useRef } from "react";

/** 当前行进度：0-1；1 表示整行已高亮 */
export type ProgressGetter = () => number;

export interface DesktopLyricsViewProps {
  fontSize: number;
  style: DesktopLyricsStyle;
  colors: DesktopLyricsColors;
  text?: string;
  romaji?: string;
  translation?: string;
  nextText?: string;
  status?: string;
  hasTranslation: boolean;
  hasRomaji: boolean;
  /** 当前行下标，-1 表示没有歌词（显示歌名与状态） */
  index: number;
  playId?: string;
  /** 读取当前行进度，窗口内每帧调用；不传按整行高亮 */
  getProgress?: ProgressGetter;
}

const NBSP = "\u00A0";

/** 主歌词行高（em），副歌词行高（em） */
const LINE_HEIGHT = 1.3;

/** 由 hex 计算相对亮度，用来决定描边/阴影用深色还是浅色 */
function isLightColor(hex: string) {
  const full = hex.length === 4 ? hex.replace(/^#(.)(.)(.)$/, "#$1$1$2$2$3$3") : hex;
  const r = Number.parseInt(full.slice(1, 3), 16);
  const g = Number.parseInt(full.slice(3, 5), 16);
  const b = Number.parseInt(full.slice(5, 7), 16);
  if ([r, g, b].some(Number.isNaN)) return true;
  return (0.299 * r + 0.587 * g + 0.114 * b) / 255 > 0.5;
}

/** 文字效果层样式：描边或阴影都画在独立的透明文字层上，避免与渐变填充互相遮挡 */
function getEffectStyle(effect: DesktopLyricsEffect, fontSize: number, light: boolean): React.CSSProperties | null {
  const dark = light ? "rgba(0, 0, 0, 0.72)" : "rgba(255, 255, 255, 0.85)";

  if (effect === "stroke") {
    // 描边居中于字形轮廓，内侧一半被填充层盖住，所以宽度取两倍
    const width = Math.max(2.5, Math.round(fontSize * 0.11 * 10) / 10);
    return { WebkitTextStroke: `${width}px ${dark}`, color: "transparent" };
  }

  if (effect === "shadow") {
    const soft = light ? "rgba(0, 0, 0, 0.5)" : "rgba(255, 255, 255, 0.55)";
    const blur = Math.max(3, Math.round(fontSize * 0.12));
    return { textShadow: `0 1px 2px ${dark}, 0 0 ${blur}px ${soft}`, color: "transparent" };
  }

  return null;
}

const gradient = ([from, to]: [string, string]) => `linear-gradient(180deg, ${from} 0%, ${to} 100%)`;

const fillStyle = (colors: [string, string]): React.CSSProperties => ({
  backgroundImage: gradient(colors),
  WebkitBackgroundClip: "text",
  backgroundClip: "text",
  color: "transparent",
});

interface LyricTextProps {
  text: string;
  fontSize: number;
  colors: DesktopLyricsColors;
  effect: DesktopLyricsEffect;
  /** played：整行已唱色；unplayed：整行未唱色；karaoke：按进度裁切 */
  mode: "played" | "unplayed" | "karaoke";
  justify: "flex-start" | "center" | "flex-end";
  getProgress?: ProgressGetter;
}

/**
 * 单行歌词：效果层 + 未唱层 + 已唱层叠放在同一个 grid 单元里。
 *
 * 进度高亮与超长行横向滚动都在 rAF 中直接改 DOM，不触发 React 重渲染。
 */
const LyricText = ({ text, fontSize, colors, effect, mode, justify, getProgress }: LyricTextProps) => {
  const boxRef = useRef<HTMLDivElement | null>(null);
  const stackRef = useRef<HTMLDivElement | null>(null);
  const playedRef = useRef<HTMLSpanElement | null>(null);
  const overflowRef = useRef(0);

  const effectStyle = getEffectStyle(effect, fontSize, isLightColor(colors.unplayed[0]));

  // 测量超出宽度：超长行按进度横向滚动，否则保持对齐方式
  useLayoutEffect(() => {
    const box = boxRef.current;
    const stack = stackRef.current;
    if (!box || !stack) return;

    const measure = () => {
      overflowRef.current = Math.max(0, stack.scrollWidth - box.clientWidth);
      box.style.justifyContent = overflowRef.current > 0 ? "flex-start" : justify;
      if (overflowRef.current === 0) stack.style.transform = "";
    };

    measure();
    const observer = new ResizeObserver(measure);
    observer.observe(box);
    observer.observe(stack);
    return () => observer.disconnect();
  }, [text, fontSize, justify]);

  useEffect(() => {
    const played = playedRef.current;
    const stack = stackRef.current;
    if (!stack) return;

    const apply = (progress: number) => {
      const p = Math.min(1, Math.max(0, progress));
      if (played && mode === "karaoke") {
        played.style.clipPath = p >= 1 ? "" : `inset(-20% ${((1 - p) * 100).toFixed(2)}% -20% 0)`;
      }
      const overflow = overflowRef.current;
      // 前 15% 保持行首可见，最后 15% 停在行尾
      stack.style.transform =
        overflow > 0 ? `translateX(${-overflow * Math.min(1, Math.max(0, (p - 0.15) / 0.7))}px)` : "";
    };

    if (!getProgress) {
      apply(mode === "unplayed" ? 0 : 1);
      return;
    }

    let rafId = 0;
    const tick = () => {
      apply(getProgress());
      rafId = requestAnimationFrame(tick);
    };
    tick();
    return () => cancelAnimationFrame(rafId);
  }, [getProgress, mode, text]);

  return (
    <div
      ref={boxRef}
      className="flex w-full overflow-hidden"
      // 上下留出空间容纳描边/阴影，避免溢出盒子后透明窗口无法完整重绘
      style={{ justifyContent: justify, padding: `${Math.round(fontSize * 0.12)}px ${Math.round(fontSize * 0.2)}px` }}
    >
      <div ref={stackRef} className="grid shrink-0 whitespace-pre" style={{ lineHeight: LINE_HEIGHT }}>
        {effectStyle ? (
          <span aria-hidden className="[grid-area:1/1]" style={effectStyle}>
            {text}
          </span>
        ) : null}
        {mode !== "played" ? (
          <span className="[grid-area:1/1]" style={fillStyle(colors.unplayed)}>
            {text}
          </span>
        ) : null}
        {mode !== "unplayed" ? (
          <span
            ref={playedRef}
            aria-hidden={mode === "karaoke"}
            className="[grid-area:1/1]"
            style={fillStyle(colors.played)}
          >
            {text}
          </span>
        ) : null}
      </div>
    </div>
  );
};

const alignToJustify = (align: DesktopLyricsAlign) =>
  align === "left" ? "flex-start" : align === "right" ? "flex-end" : "center";

/**
 * 桌面歌词展示内容（桌面歌词窗口与设置页预览共用）
 *
 * - 单行：当前行 + 罗马音 + 翻译，按对齐方式排列
 * - 双行：当前行与下一行上下交错（上行居左、下行居右），当前行在两行之间交替
 * - 整首歌有翻译/罗马音时按设置预留副歌词行，逐行切换时高度不变
 */
const DesktopLyricsView = ({
  fontSize,
  style,
  colors,
  text,
  romaji,
  translation,
  nextText,
  status,
  hasTranslation,
  hasRomaji,
  index,
  playId,
  getProgress,
}: DesktopLyricsViewProps) => {
  const subFontSize = Math.max(12, Math.round(fontSize * style.subScale));
  const fontWeight = style.bold ? 700 : 500;
  const mainMode: LyricTextProps["mode"] = style.karaoke && getProgress ? "karaoke" : "played";
  const hasLyrics = index >= 0;
  const justify = alignToJustify(style.align);

  const common = {
    colors,
    effect: style.effect,
  };

  // 翻译/罗马音属于当前行：开启同步时与原文使用同一进度，按各自文本宽度填充
  const subMode: LyricTextProps["mode"] = style.karaokeSubLines ? mainMode : "unplayed";

  const renderSub = (value: string | undefined, key: string, mode: LyricTextProps["mode"] = "unplayed") => (
    <div
      key={mode === "unplayed" ? key : `${key}-${playId}-${index}`}
      className={mode === "unplayed" ? undefined : "desktop-lyrics-fade"}
      style={{ fontSize: subFontSize }}
    >
      <LyricText
        {...common}
        text={value || NBSP}
        fontSize={subFontSize}
        mode={mode}
        justify={justify}
        getProgress={mode === "karaoke" ? getProgress : undefined}
      />
    </div>
  );

  let mainRows: React.ReactNode;

  if (!hasLyrics) {
    mainRows = (
      <>
        <div style={{ fontSize }}>
          <LyricText {...common} text={text || "Biu"} fontSize={fontSize} mode="played" justify={justify} />
        </div>
        {status ? renderSub(status, "status") : null}
      </>
    );
  } else if (style.lineMode === "double") {
    const current = (
      <LyricText
        {...common}
        text={text || NBSP}
        fontSize={fontSize}
        mode={mainMode}
        justify={index % 2 === 0 ? "flex-start" : "flex-end"}
        getProgress={mainMode === "karaoke" ? getProgress : undefined}
      />
    );
    const next = (
      <LyricText
        {...common}
        text={nextText || NBSP}
        fontSize={fontSize}
        mode="unplayed"
        justify={index % 2 === 0 ? "flex-end" : "flex-start"}
      />
    );

    mainRows = (
      <>
        <div
          key={`a-${playId}-${index % 2 === 0 ? index : index + 1}`}
          className="desktop-lyrics-fade"
          style={{ fontSize }}
        >
          {index % 2 === 0 ? current : next}
        </div>
        <div
          key={`b-${playId}-${index % 2 === 1 ? index : index + 1}`}
          className="desktop-lyrics-fade"
          style={{ fontSize }}
        >
          {index % 2 === 0 ? next : current}
        </div>
      </>
    );
  } else {
    mainRows = (
      <div key={`${playId}-${index}`} className="desktop-lyrics-fade" style={{ fontSize }}>
        <LyricText
          {...common}
          text={text || NBSP}
          fontSize={fontSize}
          mode={mainMode}
          justify={justify}
          getProgress={mainMode === "karaoke" ? getProgress : undefined}
        />
      </div>
    );
  }

  return (
    <div
      className="flex w-full flex-col"
      style={{
        opacity: style.opacity,
        fontWeight,
        fontFamily: style.fontFamily || undefined,
      }}
    >
      {mainRows}
      {hasLyrics && style.showRomaji && hasRomaji ? renderSub(romaji, "romaji", subMode) : null}
      {hasLyrics && style.showTranslation && hasTranslation ? renderSub(translation, "translation", subMode) : null}
    </div>
  );
};

export default DesktopLyricsView;
