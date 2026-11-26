import { defineConfig } from "@rsbuild/core";
import { pluginReact } from "@rsbuild/plugin-react";
import { pluginSvgr } from "@rsbuild/plugin-svgr";

/**
 * 多页面入口：主窗口 + 两个极轻量子窗口。
 * 子窗口入口不能引入 @heroui/react、framer-motion、react-router、axios、kuromoji、moment、
 * overlayscrollbars 或 store/play-list（会 new Audio()），tests/sub-window-entries.test.ts 会检查。
 * 主进程仍加载 `index.html#mini-player` / `index.html#desktop-lyrics`，由 index.html 内联脚本跳转。
 */
const SUB_WINDOW_TITLES: Record<string, string> = {
  "desktop-lyrics": "Biu Desktop Lyrics",
  "mini-player": "Biu Mini Player",
};

export default defineConfig({
  source: {
    entry: {
      index: "./src/index.tsx",
      "desktop-lyrics": "./src/entries/desktop-lyrics.tsx",
      "mini-player": "./src/entries/mini-player.tsx",
    },
  },
  output: {
    distPath: {
      root: "./dist/web",
    },
    // 生产环境相对路径，保证通过 file:// 加载时静态资源能正确引用
    assetPrefix: "./",
    cleanDistPath: true,
  },
  performance: {
    removeMomentLocale: true,
  },
  html: {
    template: ({ entryName }) =>
      entryName in SUB_WINDOW_TITLES ? "./src/entries/sub-window.html" : "./src/index.html",
    title: ({ entryName }) => SUB_WINDOW_TITLES[entryName] ?? "Biu",
  },
  plugins: [
    pluginReact(),
    pluginSvgr({
      svgrOptions: {
        exportType: "named",
        // Enable SVGO to optimize inline SVGs
        svgo: true,
        svgoConfig: {
          plugins: [
            {
              name: "preset-default",
              params: { overrides: { removeViewBox: false } },
            },
          ],
        },
      },
    }),
  ],
  dev: {
    writeToDisk: true,
    lazyCompilation: false,
    cliShortcuts: false,
    // 开发环境相对路径，保证通过 file:// 加载时静态资源能正确引用
    assetPrefix: "./",
  },
  server: {
    port: 5678,
    strictPort: false,
    printUrls: false,
    open: false,
    compress: false,
  },
});
