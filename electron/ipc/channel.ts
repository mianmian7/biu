export const channel = {
  store: {
    getSettings: "settings:get",
    setSettings: "settings:set",
    clearSettings: "settings:clear",
    get: "store:get",
    set: "store:set",
    clear: "store:clear",
  },
  dialog: {
    selectDirectory: "dialog:select-directory",
    showFileInFolder: "dialog:show-file-in-folder",
    selectFile: "dialog:select-file",
    openDirectory: "dialog:open-directory",
    openExternal: "dialog:open-external",
  },
  font: {
    getFonts: "font:get-fonts",
  },
  file: {
    getSize: "file:get-size",
  },
  download: {
    getList: "download:get-list",
    getDownloadData: "download:get-download-data",
    add: "download:add",
    addList: "download:add-list",
    pause: "download:pause",
    resume: "download:resume",
    cancel: "download:cancel",
    retry: "download:retry",
    sync: "download:sync",
    clear: "download:clear",
  },
  autoLyrics: {
    start: "auto-lyrics:start",
    prefetch: "auto-lyrics:prefetch",
    cancel: "auto-lyrics:cancel",
    progress: "auto-lyrics:progress",
  },
  lyrics: {
    searchNeteaseSongs: "lyrics:netease-search",
    getNeteaseLyrics: "lyrics:netease-lyrics",
    searchLrclib: "lyrics:lrclib-search",
    /** 为日语歌词行本地生成罗马音 */
    romanize: "lyrics:romanize",
  },
  router: {
    navigate: "router:navigate",
  },
  http: {
    get: "http:get",
    post: "http:post",
  },
  player: {
    state: "player:state",
    prev: "player:prev",
    next: "player:next",
    toggle: "player:toggle",
  },
  shortcut: {
    triggered: "shortcut:triggered",
    register: "shortcut:register",
    unregister: "shortcut:unregister",
    registerAll: "shortcut:register-all",
    unregisterAll: "shortcut:unregister-all",
  },
  app: {
    getVersion: "app:get-version",
    checkUpdate: "app:check-update",
    onUpdateAvailable: "app:on-update-available",
    downloadUpdate: "app:download-update",
    updateMessage: "app:update-message",
    quitAndInstall: "app:quit-and-install",
    openInstallerDirectory: "app:open-installer-directory",
    onBeforeQuit: "app:on-before-quit",
    isDev: "app:is-dev",
    setProxySettings: "app:set-proxy-settings",
  },
  cookie: {
    get: "cookie:get",
    set: "cookie:set",
  },
  localMusic: {
    scan: "local-music:scan",
    deleteFile: "local-music:delete-file",
  },
  window: {
    toggleMini: "window:toggle-mini",
    minimize: "window:minimize",
    toggleMaximize: "window:toggle-maximize",
    close: "window:close",
    maximize: "window:maximize",
    unmaximize: "window:unmaximize",
    isMaximized: "window:is-maximized",
    enterFullScreen: "window:enter-full-screen",
    leaveFullScreen: "window:leave-full-screen",
    isFullScreen: "window:is-full-screen",
    toggleDevTools: "window:toggle-dev-tools",
  },
  desktopLyrics: {
    /** 主窗口 -> 主进程：同步状态与内容 */
    sync: "desktop-lyrics:sync",
    /** 主进程 -> 桌面歌词窗口：推送展示内容 */
    content: "desktop-lyrics:content",
    /** 桌面歌词窗口/托盘 -> 主进程 -> 主窗口：请求切换显示 */
    toggleRequested: "desktop-lyrics:toggle-requested",
    /** 桌面歌词窗口/托盘 -> 主进程 -> 主窗口：请求切换锁定 */
    lockToggleRequested: "desktop-lyrics:lock-toggle-requested",
    getContent: "desktop-lyrics:get-content",
    /** 桌面歌词窗口 -> 主进程：工具栏操作（播放控制/样式切换等），大多转发给主窗口 */
    command: "desktop-lyrics:command",
    /** 桌面歌词窗口 -> 主进程：开始/结束拖动窗口 */
    dragStart: "desktop-lyrics:drag-start",
    dragEnd: "desktop-lyrics:drag-end",
    /** 桌面歌词窗口 -> 主进程：锁定状态下临时切换鼠标穿透（悬停解锁按钮） */
    setIgnoreMouse: "desktop-lyrics:set-ignore-mouse",
    /** 桌面歌词窗口 -> 主进程：内容高度变化，窗口高度跟随 */
    setContentHeight: "desktop-lyrics:set-content-height",
    /** 设置页 -> 主进程：重置窗口位置 */
    resetPosition: "desktop-lyrics:reset-position",
  },
};
