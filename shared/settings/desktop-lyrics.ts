/** 桌面歌词默认样式 */
export const defaultDesktopLyricsStyle: DesktopLyricsStyle = {
  lineMode: "single",
  align: "center",
  showTranslation: true,
  showRomaji: true,
  subScale: 0.6,
  fontFamily: "",
  bold: true,
  colorPreset: "jade",
  customPlayedColor: "#ffd34e",
  customUnplayedColor: "#ffffff",
  effect: "stroke",
  karaoke: true,
  karaokeSubLines: true,
  opacity: 1,
};

export const DESKTOP_LYRICS_FONT_SIZE_MIN = 16;
export const DESKTOP_LYRICS_FONT_SIZE_MAX = 72;
export const DESKTOP_LYRICS_SUB_SCALE_MIN = 0.4;
export const DESKTOP_LYRICS_SUB_SCALE_MAX = 1;
export const DESKTOP_LYRICS_OPACITY_MIN = 0.3;

const WHITE: [string, string] = ["#ffffff", "#e9edf2"];

/** 内置配色方案（`theme` 与 `custom` 在解析时根据设置计算） */
export const desktopLyricsColorPresets: { id: string; name: string; colors: DesktopLyricsColors }[] = [
  { id: "jade", name: "翡翠", colors: { played: ["#9dffcb", "#22d97a"], unplayed: WHITE } },
  { id: "crimson", name: "赤焰", colors: { played: ["#ffb199", "#ff3d3d"], unplayed: WHITE } },
  { id: "azure", name: "晴空", colors: { played: ["#a8e4ff", "#2f8cff"], unplayed: WHITE } },
  { id: "amber", name: "琥珀", colors: { played: ["#fff1a8", "#ffb020"], unplayed: WHITE } },
  { id: "sakura", name: "樱粉", colors: { played: ["#ffd1e6", "#ff5fa2"], unplayed: WHITE } },
  { id: "violet", name: "紫罗兰", colors: { played: ["#e2ccff", "#9a5bff"], unplayed: WHITE } },
  { id: "snow", name: "素雪", colors: { played: ["#ffffff", "#f2f4f7"], unplayed: ["#b8bec8", "#8e96a3"] } },
  { id: "night", name: "墨夜", colors: { played: ["#ffe08a", "#ffc531"], unplayed: ["#4a4f59", "#2b2f36"] } },
];

/** 配色方案切换顺序（桌面歌词工具栏「换配色」使用） */
export const desktopLyricsColorCycle = [...desktopLyricsColorPresets.map(item => item.id), "theme"];

const clamp = (value: number, min: number, max: number) => Math.min(max, Math.max(min, value));

const isHexColor = (value: unknown): value is string =>
  typeof value === "string" && /^#(?:[0-9a-f]{3}|[0-9a-f]{6})$/i.test(value);

/** 把旧版本/部分缺失的样式与默认值合并，并把数值限制在合法范围内 */
export function resolveDesktopLyricsStyle(style?: Partial<DesktopLyricsStyle> | null): DesktopLyricsStyle {
  const merged = { ...defaultDesktopLyricsStyle, ...style };

  return {
    ...merged,
    lineMode: merged.lineMode === "double" ? "double" : "single",
    align: ["left", "center", "right"].includes(merged.align) ? merged.align : "center",
    effect: ["shadow", "stroke", "none"].includes(merged.effect) ? merged.effect : "stroke",
    subScale: clamp(Number(merged.subScale) || defaultDesktopLyricsStyle.subScale, 0.4, 1),
    opacity: clamp(Number(merged.opacity) || 1, DESKTOP_LYRICS_OPACITY_MIN, 1),
    fontFamily: typeof merged.fontFamily === "string" ? merged.fontFamily : "",
    customPlayedColor: isHexColor(merged.customPlayedColor)
      ? merged.customPlayedColor
      : defaultDesktopLyricsStyle.customPlayedColor,
    customUnplayedColor: isHexColor(merged.customUnplayedColor)
      ? merged.customUnplayedColor
      : defaultDesktopLyricsStyle.customUnplayedColor,
  };
}

/** 把 #rgb / #rrggbb 按比例与白色（amount>0）或黑色（amount<0）混合 */
function mixHex(hex: string, amount: number) {
  const full =
    hex.length === 4
      ? `#${hex
          .slice(1)
          .split("")
          .map(ch => ch + ch)
          .join("")}`
      : hex;
  const target = amount > 0 ? 255 : 0;
  const ratio = Math.abs(amount);
  const channels = [1, 3, 5].map(start => {
    const value = Number.parseInt(full.slice(start, start + 2), 16);
    return Math.round(value + (target - value) * ratio)
      .toString(16)
      .padStart(2, "0");
  });
  return `#${channels.join("")}`;
}

/** 单色扩展为上浅下深的渐变 */
const toGradient = (hex: string): [string, string] => [mixHex(hex, 0.35), hex];

/** 根据样式解析出实际配色；`theme` 使用应用主题色 */
export function resolveDesktopLyricsColors(style: DesktopLyricsStyle, primaryColor?: string): DesktopLyricsColors {
  if (style.colorPreset === "custom") {
    return {
      played: toGradient(style.customPlayedColor),
      unplayed: [style.customUnplayedColor, mixHex(style.customUnplayedColor, -0.08)],
    };
  }

  if (style.colorPreset === "theme") {
    return {
      played: toGradient(isHexColor(primaryColor) ? primaryColor : "#1ed760"),
      unplayed: WHITE,
    };
  }

  const preset = desktopLyricsColorPresets.find(item => item.id === style.colorPreset) ?? desktopLyricsColorPresets[0];
  return preset.colors;
}
