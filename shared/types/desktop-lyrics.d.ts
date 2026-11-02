declare global {
  /** 桌面歌词显示行数：单行 / 双行交错 */
  type DesktopLyricsLineMode = "single" | "double";

  /** 桌面歌词单行模式的对齐方式 */
  type DesktopLyricsAlign = "left" | "center" | "right";

  /** 桌面歌词文字效果 */
  type DesktopLyricsEffect = "shadow" | "stroke" | "none";

  /** 桌面歌词样式（设置里可调整的部分） */
  interface DesktopLyricsStyle {
    /** 单行 / 双行交错 */
    lineMode: DesktopLyricsLineMode;
    /** 单行模式对齐方式 */
    align: DesktopLyricsAlign;
    /** 是否显示翻译 */
    showTranslation: boolean;
    /** 是否显示罗马音 */
    showRomaji: boolean;
    /** 翻译/罗马音字号相对主歌词字号的比例（0.4 - 1） */
    subScale: number;
    /** 字体，空字符串表示跟随应用字体 */
    fontFamily: string;
    /** 是否加粗 */
    bold: boolean;
    /** 配色方案 id，`custom` 使用自定义颜色，`theme` 跟随主题色 */
    colorPreset: string;
    /** 自定义已播放颜色 */
    customPlayedColor: string;
    /** 自定义未播放颜色 */
    customUnplayedColor: string;
    /** 文字效果 */
    effect: DesktopLyricsEffect;
    /** 是否按行内进度高亮已唱部分 */
    karaoke: boolean;
    /** 翻译/罗马音是否与原文同步高亮（关闭时保持未唱色） */
    karaokeSubLines: boolean;
    /** 文字不透明度（0.3 - 1） */
    opacity: number;
  }

  /** 解析后的配色（渐变上/下两色） */
  interface DesktopLyricsColors {
    played: [string, string];
    unplayed: [string, string];
  }

  /** 桌面悬浮歌词窗口的展示内容 */
  interface DesktopLyricsPayload {
    /** 设置里是否开启桌面歌词 */
    enabled: boolean;
    /** 是否锁定（鼠标穿透） */
    locked: boolean;
    /** 是否有可显示的内容（有正在播放的歌曲） */
    active: boolean;
    /** 当前歌词行文本；没有歌词时为歌曲标题 */
    text?: string;
    /** 当前歌词行罗马音 */
    romaji?: string;
    /** 当前歌词行翻译 */
    translation?: string;
    /** 下一行歌词文本（双行模式使用） */
    nextText?: string;
    /** 没有歌词时的提示（加载中/暂无歌词/自动歌词状态） */
    status?: string;
    /** 整首歌是否有翻译（用于预留行高，避免逐行抖动） */
    hasTranslation: boolean;
    /** 整首歌是否有罗马音 */
    hasRomaji: boolean;
    /** 当前行开始时间（毫秒，已含偏移） */
    lineStartMs?: number;
    /** 当前行进度高亮结束时间（毫秒） */
    lineEndMs?: number;
    /** 同步时的播放位置（毫秒，已含偏移） */
    positionMs: number;
    /** 同步时刻（Date.now()），桌面歌词窗口据此插值播放位置 */
    syncedAt: number;
    /** 播放倍速 */
    rate: number;
    /** 是否正在播放 */
    isPlaying: boolean;
    /** 当前播放项 id，用于切歌时重置动画 */
    playId?: string;
    /** 当前歌词行下标，-1 表示没有歌词 */
    index: number;
    /** 字号（px） */
    fontSize: number;
    /** 样式 */
    style: DesktopLyricsStyle;
    /** 解析后的配色 */
    colors: DesktopLyricsColors;
  }

  /** 桌面歌词窗口发往主窗口的操作 */
  type DesktopLyricsCommand =
    | "prev"
    | "next"
    | "togglePlay"
    | "toggleLock"
    | "close"
    | "fontLarger"
    | "fontSmaller"
    | "nextColor"
    | "toggleLineMode"
    | "toggleTranslation"
    | "toggleRomaji"
    | "openSettings";

  /** 桌面歌词窗口位置尺寸（DIP） */
  interface DesktopLyricsBounds {
    x: number;
    y: number;
    width: number;
    height: number;
  }

  /** 桌面歌词开关状态（用于托盘菜单勾选） */
  interface DesktopLyricsWindowState {
    enabled: boolean;
    locked: boolean;
  }
}

export {};
