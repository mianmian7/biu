import React, { lazy, Suspense, useEffect } from "react";
import { ErrorBoundary } from "react-error-boundary";
import { Outlet, useLocation } from "react-router";

import log from "@/common/utils/logger";
import ConfirmModal from "@/components/confirm-modal";
import Fallback from "@/components/error-fallback";
import PlayBar from "@/layout/playbar";
import { useModalStore } from "@/store/modal";
import { useUser } from "@/store/user";

import Navbar from "./navbar";
import SideNav from "./side";
import TopLyrics from "./top-lyrics";
import { useLingeringOpen } from "./use-lingering-open";
import { useWindowUiVisible } from "./window-visibility";

const FavoritesSelectModal = lazy(() => import("@/components/favorites-select-modal"));
const FullScreenPlayer = lazy(() => import("@/components/full-screen-player"));
const PlayListDrawer = lazy(() => import("@/components/music-playlist-drawer"));
const ReleaseNoteModal = lazy(() => import("@/components/release-note-modal"));
const VideoPagesDownloadSelectModal = lazy(() => import("@/components/video-pages-download-select-modal"));

/** 只在打开（及退出动画期间）挂载弹层，冷启动不创建隐藏的弹层树 */
const MountWhenOpen = ({ isOpen, children }: { isOpen: boolean; children: React.ReactNode }) => {
  const mounted = useLingeringOpen(isOpen);
  return mounted ? <Suspense fallback={null}>{children}</Suspense> : null;
};

const Overlays = () => {
  const isFavSelectModalOpen = useModalStore(s => s.isFavSelectModalOpen);
  const isConfirmModalOpen = useModalStore(s => s.isConfirmModalOpen);
  const isVideoPageDownloadModalOpen = useModalStore(s => s.isVideoPageDownloadModalOpen);
  const isReleaseNoteModalOpen = useModalStore(s => s.isReleaseNoteModalOpen);
  const isPlayListDrawerOpen = useModalStore(s => s.isPlayListDrawerOpen);
  const isFullScreenPlayerOpen = useModalStore(s => s.isFullScreenPlayerOpen);

  return (
    <>
      <MountWhenOpen isOpen={isFavSelectModalOpen}>
        <FavoritesSelectModal />
      </MountWhenOpen>
      <MountWhenOpen isOpen={isConfirmModalOpen}>
        <ConfirmModal />
      </MountWhenOpen>
      <MountWhenOpen isOpen={isVideoPageDownloadModalOpen}>
        <VideoPagesDownloadSelectModal />
      </MountWhenOpen>
      <MountWhenOpen isOpen={isReleaseNoteModalOpen}>
        <ReleaseNoteModal />
      </MountWhenOpen>
      <MountWhenOpen isOpen={isPlayListDrawerOpen}>
        <PlayListDrawer />
      </MountWhenOpen>
      <MountWhenOpen isOpen={isFullScreenPlayerOpen}>
        <FullScreenPlayer />
      </MountWhenOpen>
    </>
  );
};

/** 主窗口的可视 UI：窗口隐藏到托盘后整体卸载 */
const LayoutContent = () => {
  const location = useLocation();

  return (
    <ErrorBoundary
      FallbackComponent={Fallback}
      resetKeys={[location.pathname]}
      onError={(error, info) => {
        log.error("[ErrorBoundary]", error, info);
      }}
    >
      <div className="flex h-full flex-col">
        <div className="flex min-h-0 w-full flex-1">
          <SideNav />
          <div className="flex min-h-0 min-w-0 flex-1 flex-col">
            <div className="h-16 flex-none">
              <Navbar />
            </div>
            <TopLyrics />
            <div className="min-h-0 flex-1 overflow-hidden">
              <Suspense fallback={null}>
                <Outlet />
              </Suspense>
            </div>
          </div>
        </div>
        <div className="relative z-50 h-[88px] w-full flex-none shadow-2xl">
          <PlayBar />
        </div>
      </div>
      <Overlays />
    </ErrorBoundary>
  );
};

const Layout = () => {
  const updateUser = useUser(state => state.updateUser);
  // 隐藏到托盘 3 秒后卸载可视 UI；播放、快捷键、歌词同步与迷你播放器同步都在 App/模块级，不受影响。
  // 路由在 HashRouter 中、业务状态在 zustand 中，重新显示时立即恢复。
  const isUiVisible = useWindowUiVisible();

  useEffect(() => {
    updateUser();
  }, []);

  return isUiVisible ? <LayoutContent /> : null;
};

export default Layout;
