import { useEffect } from "react";

import { initSubWindowTheme } from "@/entries/sub-window-theme";
import { defaultAppSettings } from "@shared/settings/app-settings";

export const useStyle = () => {
  useEffect(() => {
    let active = true;
    document.documentElement.style.background = "transparent";
    document.body.style.background = "transparent";
    document.body.style.margin = "0";
    document.body.style.overflow = "hidden";

    const rootEl: HTMLDivElement | null = document.querySelector("#root");
    if (rootEl) {
      rootEl.style.background = "hsl(var(--heroui-background))";
      rootEl.style.overflow = "hidden";
      rootEl.style.borderRadius = `${defaultAppSettings.borderRadius}px`;
    }

    // 迷你播放器入口没有 Theme 组件：直接读持久化设置（store_get）应用深浅色、主色与圆角
    void initSubWindowTheme().then(settings => {
      if (!active || !rootEl || typeof settings.borderRadius !== "number") return;
      rootEl.style.borderRadius = `${settings.borderRadius}px`;
    });

    return () => {
      active = false;
      const rootEl: HTMLDivElement | null = document.querySelector("#root");
      if (rootEl) {
        document.documentElement.style.removeProperty("background");
        document.body.style.removeProperty("background");
        document.body.style.removeProperty("margin");
        document.body.style.removeProperty("overflow");
        rootEl.style.removeProperty("background");
        rootEl.style.removeProperty("overflow");
        rootEl.style.removeProperty("border-radius");
      }
    };
  }, []);
};
