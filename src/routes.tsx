import { lazy, Suspense } from "react";
import type { RouteObject } from "react-router";

import Layout from "./layout";
import MusicRecommend from "./pages/music-recommend";

// 首页（Layout + 推荐页）同步加载保证首屏；其余页面按路由拆包，
// Layout 里的 <Suspense> 包住 <Outlet />。
const DesktopLyrics = lazy(() => import("./pages/desktop-lyrics"));
const DownloadList = lazy(() => import("./pages/download-list"));
const DynamicFeed = lazy(() => import("./pages/dynamic-feed"));
const EmptyPage = lazy(() => import("./pages/empty"));
const FollowList = lazy(() => import("./pages/follow-list"));
const History = lazy(() => import("./pages/history"));
const Later = lazy(() => import("./pages/later"));
const LocalMusicPage = lazy(() => import("./pages/local-music"));
// 子窗口正常走独立 HTML 入口（src/entries），这里只保留 hash 路由兜底
const MiniPlayer = lazy(() => import("./pages/mini-player"));
const NotFound = lazy(() => import("./pages/not-found"));
const Search = lazy(() => import("./pages/search"));
const Settings = lazy(() => import("./pages/settings"));
const UserProfile = lazy(() => import("./pages/user-profile"));
const Folder = lazy(() => import("./pages/video-collection"));

const routes: RouteObject[] = [
  {
    path: "/",
    element: <Layout />,
    children: [
      {
        index: true,
        element: <MusicRecommend />,
      },
      {
        path: "later",
        element: <Later />,
      },
      {
        path: "history",
        element: <History />,
      },
      {
        path: "follow",
        element: <FollowList />,
      },
      {
        path: "collection/:id",
        element: <Folder />,
      },
      {
        path: "user/:id",
        element: <UserProfile />,
      },
      {
        path: "settings",
        element: <Settings />,
      },
      {
        path: "download-list",
        element: <DownloadList />,
      },
      {
        path: "dynamic-feed",
        element: <DynamicFeed />,
      },
      {
        path: "local-music",
        element: <LocalMusicPage />,
      },
      {
        path: "search",
        element: <Search />,
      },
      {
        path: "empty",
        element: <EmptyPage />,
      },
    ],
  },
  {
    path: "mini-player",
    element: (
      <Suspense fallback={null}>
        <MiniPlayer />
      </Suspense>
    ),
  },
  {
    path: "desktop-lyrics",
    element: (
      <Suspense fallback={null}>
        <DesktopLyrics />
      </Suspense>
    ),
  },
  {
    path: "*",
    element: (
      <Suspense fallback={null}>
        <NotFound />
      </Suspense>
    ),
  },
];

export default routes;
