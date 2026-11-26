import { readableColor } from "color2k";

import { Themes } from "@/common/constants/theme";
import { hexToHsl, isHex, resolveTheme } from "@/common/utils/color";
import { defaultAppSettings } from "@shared/settings/app-settings";
import { StoreNameMap } from "@shared/store";

import { getStore } from "./sub-window-bridge";

type ThemeSettings = Partial<
  Pick<AppSettings, "themeMode" | "fontFamily" | "primaryColor" | "borderRadius" | "backgroundColor">
>;

/**
 * 把主题设置应用到 :root，逻辑与 `src/components/theme` 相同，
 * 但不依赖 zustand settings store / HeroUIProvider，供子窗口入口使用。
 */
export function applyThemeSettings(settings: ThemeSettings, root: HTMLElement = document.documentElement) {
  const themeName = resolveTheme(settings.themeMode ?? "system");
  root.classList.remove("light", "dark");
  root.classList.add(themeName);
  root.style.colorScheme = themeName;

  const primary = isHex(settings.primaryColor)
    ? settings.primaryColor!
    : (Themes[themeName].colors?.primary as string | undefined);
  const background = isHex(settings.backgroundColor)
    ? settings.backgroundColor!
    : (Themes[themeName].colors?.background as string | undefined);

  if (primary) root.style.setProperty("--heroui-primary", hexToHsl(primary));
  if (background) {
    root.style.setProperty("--heroui-background", hexToHsl(background));
    root.style.setProperty("--heroui-foreground", hexToHsl(readableColor(background)));
  }
  if (typeof settings.borderRadius === "number") {
    root.style.setProperty("--heroui-radius-medium", `${settings.borderRadius}px`);
  }
  const font = settings.fontFamily === "system-default" ? "system-ui" : settings.fontFamily;
  if (font) root.style.fontFamily = font;

  return themeName;
}

/** 读取持久化的应用设置并应用主题；跟随系统时监听系统深浅色变化 */
export async function initSubWindowTheme(): Promise<ThemeSettings> {
  const store = await getStore(StoreNameMap.AppSettings);
  // 与 settings store 的 persist 合并语义一致：持久化值覆盖默认值
  const settings: ThemeSettings = { ...defaultAppSettings, ...store?.appSettings };
  applyThemeSettings(settings);

  if ((settings.themeMode ?? "system") === "system" && typeof window.matchMedia === "function") {
    window.matchMedia("(prefers-color-scheme: dark)").addEventListener("change", () => applyThemeSettings(settings));
  }
  return settings;
}
