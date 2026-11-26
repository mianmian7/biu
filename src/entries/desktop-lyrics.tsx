import DesktopLyrics from "@/pages/desktop-lyrics";

import { bootSubWindow } from "./sub-window-boot";
import { initSubWindowTheme } from "./sub-window-theme";

import "./sub-window.css";

// 桌面歌词没有字体设置时继承应用字体，和主入口的 Theme 组件效果一致
void initSubWindowTheme();

// 页面根节点就是 #root 的直接子元素，主进程转发的 cursor-inside / cursor-leave 能落到它上面
bootSubWindow(<DesktopLyrics />);
