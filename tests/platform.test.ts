import { beforeEach, describe, expect, test, vi } from "vitest";

import {
  applyMigratedLocalStorage,
  createTauriElectronAPI,
  toMediaUrl,
  toMediaUrlSync,
  UNIMPLEMENTED_ELECTRON_METHODS,
  unwrapMediaUrl,
} from "@/platform";
import { biliAdapter } from "@/service/request";
import { StoreNameMap } from "@shared/store";

describe("platform ElectronAPI compatibility and media URL wrapping", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    delete (window as any).__TAURI__;
  });

  test("UNIMPLEMENTED_ELECTRON_METHODS is empty after Step 4 updater implementation", () => {
    expect(UNIMPLEMENTED_ELECTRON_METHODS).toEqual([]);
  });

  test("Step 4 updater methods invoke corresponding Tauri commands and listen to events", async () => {
    const invokeMock = vi.fn(async (cmd: string) => {
      if (cmd === "check_app_update") {
        return { isUpdateAvailable: true, latestVersion: "1.16.2", releaseNotes: "notes" };
      }
      if (cmd === "get_update_mode") return "auto";
      if (cmd === "download_app_update") return undefined;
      if (cmd === "quit_and_install") return true;
      return undefined;
    });
    const listenMock = vi.fn(async () => () => {});
    (window as any).__TAURI__ = {
      core: { invoke: invokeMock },
      event: { listen: listenMock },
    };

    const api = createTauriElectronAPI();
    expect(await api.checkAppUpdate()).toEqual({
      isUpdateAvailable: true,
      latestVersion: "1.16.2",
      releaseNotes: "notes",
    });
    expect(invokeMock).toHaveBeenCalledWith("check_app_update", undefined);

    expect(await api.getUpdateMode()).toBe("auto");
    expect(invokeMock).toHaveBeenCalledWith("get_update_mode", undefined);

    await api.downloadAppUpdate();
    expect(invokeMock).toHaveBeenCalledWith("download_app_update", undefined);

    expect(await api.quitAndInstall()).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("quit_and_install", undefined);

    const offAvail = api.onUpdateAvailable(() => {});
    expect(listenMock).toHaveBeenCalledWith("app:update-available", expect.any(Function));
    offAvail();

    const offProg = api.onDownloadAppProgress(() => {});
    expect(listenMock).toHaveBeenCalledWith("app:update-message", expect.any(Function));
    offProg();
  });

  test("Step 1 methods invoke corresponding Tauri commands", async () => {
    const invokeMock = vi.fn(async (cmd: string) => {
      if (cmd === "store_get") return { appSettings: { audioQuality: "auto" } };
      if (cmd === "store_clear") return true;
      if (cmd === "cookie_get") return "cookie-val";
      if (cmd === "media_base") return "http://127.0.0.1:43210/secret-token/";
      return undefined;
    });
    (window as any).__TAURI__ = {
      core: { invoke: invokeMock },
    };

    const api = createTauriElectronAPI();
    const settings = await api.getStore(StoreNameMap.AppSettings);
    expect(settings).toEqual({ appSettings: { audioQuality: "auto" } });
    expect(invokeMock).toHaveBeenCalledWith("store_get", { name: "app-settings" });

    await api.setStore(StoreNameMap.UserLoginInfo, { isLogin: true } as any);
    expect(invokeMock).toHaveBeenCalledWith("store_set", {
      name: "user-login-info",
      value: { isLogin: true },
    });

    await api.clearStore(StoreNameMap.UserLoginInfo);
    expect(invokeMock).toHaveBeenCalledWith("store_clear", { name: "user-login-info" });

    const cookie = await api.getCookie("SESSDATA");
    expect(cookie).toBe("cookie-val");
    expect(invokeMock).toHaveBeenCalledWith("cookie_get", { key: "SESSDATA" });

    await api.setCookie("bili_jct", "csrf-123", 1900000000);
    expect(invokeMock).toHaveBeenCalledWith("cookie_set", {
      payload: { name: "bili_jct", value: "csrf-123", expirationDate: 1900000000 },
    });

    const wrapped = await toMediaUrl(
      "https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/01/02/123.m4s?deadline=1999999999&upsig=abc",
    );
    expect(wrapped).toBe(
      "http://127.0.0.1:43210/secret-token/remote?u=https%3A%2F%2Fupos-sz-mirrorcos.bilivideo.com%2Fupgcxcode%2F01%2F02%2F123.m4s%3Fdeadline%3D1999999999%26upsig%3Dabc",
    );
    expect(unwrapMediaUrl(wrapped)).toBe(
      "https://upos-sz-mirrorcos.bilivideo.com/upgcxcode/01/02/123.m4s?deadline=1999999999&upsig=abc",
    );

    // A playlist saved by an earlier launch holds a proxy URL with that launch's port and token
    const fromEarlierLaunch = wrapped.replace("43210/secret-token", "51234/old-token");
    expect(await toMediaUrl(fromEarlierLaunch)).toBe(wrapped);
    expect(toMediaUrlSync(fromEarlierLaunch)).toBe(wrapped);
  });

  test("biliAdapter forwards axios request config to bili_request command", async () => {
    const invokeMock = vi.fn(async () => ({
      status: 200,
      headers: { "content-type": "application/json" },
      body: JSON.stringify({ code: 0, data: { ok: true } }),
    }));
    (window as any).__TAURI__ = {
      core: { invoke: invokeMock },
    };

    const resp = await biliAdapter({
      url: "/x/web-interface/nav",
      baseURL: "https://api.bilibili.com",
      method: "get",
      params: { foo: "bar" },
      headers: {} as any,
    });

    expect(resp.status).toBe(200);
    expect(resp.data).toEqual({ code: 0, data: { ok: true } });
    expect(invokeMock).toHaveBeenCalledWith("bili_request", {
      request: expect.objectContaining({
        method: "GET",
        url: "https://api.bilibili.com/x/web-interface/nav",
        params: { foo: "bar" },
        timeoutMs: 15000,
      }),
    });
  });

  test("Step 2 methods invoke lyrics, desktop lyrics, mini player, shortcut, and media key commands", async () => {
    const invokeMock = vi.fn(async (cmd: string) => {
      if (cmd === "lyrics_search_netease") return { result: { songs: [], songCount: 0 } };
      if (cmd === "lyrics_get_netease") return { lrc: { lyric: "[00:00.00]hi" } };
      if (cmd === "lyrics_search_lrclib") return [{ id: 1, plainLyrics: "hi" }];
      if (cmd === " shortcut_register" || cmd === "shortcut_register") return true;
      return undefined;
    });
    (window as any).__TAURI__ = {
      core: { invoke: invokeMock },
      event: {
        listen: vi.fn(async () => () => {}),
        emit: vi.fn(async () => {}),
      },
    };

    const api = createTauriElectronAPI();

    expect(await api.searchNeteaseSongs({ s: "test", offset: 0, limit: 5, type: 1 })).toEqual({
      result: { songs: [], songCount: 0 },
    });
    expect(invokeMock).toHaveBeenCalledWith("lyrics_search_netease", {
      params: { s: "test", offset: 0, limit: 5, type: 1 },
    });

    expect(await api.getNeteaseLyrics({ id: 42 })).toEqual({
      lrc: { lyric: "[00:00.00]hi" },
    });
    expect(invokeMock).toHaveBeenCalledWith("lyrics_get_netease", {
      params: { id: 42 },
    });

    expect(await api.searchLrclibLyrics({ q: "query" })).toEqual([{ id: 1, plainLyrics: "hi" }]);
    expect(invokeMock).toHaveBeenCalledWith("lyrics_search_lrclib", {
      params: { q: "query" },
    });

    api.sendDesktopLyricsCommand("togglePlay");
    expect(invokeMock).toHaveBeenCalledWith("desktop_lyrics_command", { command: "togglePlay" });

    await api.toggleMiniPlayer();
    expect(invokeMock).toHaveBeenCalledWith("toggle_mini_player", undefined);

    expect(await api.registerShortcut({ id: "togglePlay", accelerator: "CommandOrControl+Shift+P" })).toBe(true);
    expect(invokeMock).toHaveBeenCalledWith("shortcut_register", {
      id: "togglePlay",
      accelerator: "CommandOrControl+Shift+P",
    });

    api.updatePlaybackState(true);
    expect(invokeMock).toHaveBeenCalledWith("update_playback_state", {
      isPlaying: true,
    });
  });
  test("Step 3 methods invoke corresponding Tauri commands and wrap file:// URLs", async () => {
    const invokeMock = vi.fn(async (cmd: string, _args?: Record<string, unknown>) => {
      if (cmd === "media_base") return "http://127.0.0.1:43210/secret-token/";
      if (cmd === "select_directory") return "/home/user/Music";
      if (cmd === "select_file") return "/usr/bin/ffmpeg";
      if (cmd === "open_directory") return true;
      if (cmd === "show_file_in_folder") return true;
      if (cmd === "get_fonts") return ["DejaVu Sans", "Liberation Serif"];
      if (cmd === "scan_local_music") return [{ id: "abc", title: "Song", filePath: "/home/user/Music/a.mp3" }];
      if (cmd === "delete_local_music_file") return true;
      if (cmd === "download_get_list")
        return [{ id: "dl-1", title: "Video", status: "completed", outputFileType: "audio" }];
      return undefined;
    });
    const listenMock = vi.fn(async () => () => {});
    (window as any).__TAURI__ = {
      core: { invoke: invokeMock },
      event: { listen: listenMock },
    };

    const api = createTauriElectronAPI();
    expect(await api.selectDirectory("Pick")).toBe("/home/user/Music");
    expect(invokeMock).toHaveBeenCalledWith("select_directory", { title: "Pick" });

    expect(await api.selectFile()).toBe("/usr/bin/ffmpeg");
    expect(await api.openDirectory("/home/user/Music")).toBe(true);
    expect(await api.showFileInFolder("/home/user/Music/a.mp3")).toBe(true);
    expect(await api.getFonts()).toEqual(["DejaVu Sans", "Liberation Serif"]);
    expect(await api.scanLocalMusic(["/home/user/Music"])).toHaveLength(1);
    expect(await api.deleteLocalMusicFile("/home/user/Music/a.mp3")).toBe(true);

    expect(await api.getMediaDownloadTaskList()).toHaveLength(1);
    await api.addMediaDownloadTask({ outputFileType: "audio", title: "Test", bvid: "BV1xx411c7mD" });
    expect(invokeMock).toHaveBeenCalledWith("download_add", {
      task: { outputFileType: "audio", title: "Test", bvid: "BV1xx411c7mD" },
    });
    await api.pauseMediaDownloadTask("dl-1");
    await api.resumeMediaDownloadTask("dl-1");
    await api.retryMediaDownloadTask("dl-1");
    await api.cancelMediaDownloadTask("dl-1");
    await api.clearMediaDownloadTaskList();

    const unsub = api.syncMediaDownloadTaskList(() => {});
    expect(listenMock).toHaveBeenCalledWith("download:sync", expect.any(Function));
    unsub();

    const localWrapped = await toMediaUrl("file:///home/user/Music/song.flac");
    expect(localWrapped).toBe("http://127.0.0.1:43210/secret-token/local?p=%2Fhome%2Fuser%2FMusic%2Fsong.flac");
    expect(unwrapMediaUrl(localWrapped)).toBe("file:///home/user/Music/song.flac");
  });

  test("auto-lyrics worker alignment and Tauri AutoLyrics controller match AUTO_LYRICS_ALGO_VERSION", async () => {
    const { alignAudioInWorker, createTauriAutoLyricsController, pcmS16leToFloat32 } =
      await import("@/platform/auto-lyrics-worker");
    const { AUTO_LYRICS_ALGO_VERSION } = await import("../shared/auto-lyrics/service");

    expect(AUTO_LYRICS_ALGO_VERSION).toBe("p1-gap-dtw-6");

    // 1. PCM s16le conversion
    const s16 = new Int16Array([0, 16384, -32768]);
    const f32 = pcmS16leToFloat32(new Uint8Array(s16.buffer));
    expect(f32[0]).toBeCloseTo(0);
    expect(f32[1]).toBeCloseTo(0.5);
    expect(f32[2]).toBeCloseTo(-1.0);

    // 2. Worker alignment on synthetic 22050Hz signal (studio vs 1.0s delayed live)
    const sr = 22050;
    const studioPcm = new Float32Array(sr * 12);
    const livePcm = new Float32Array(sr * 13);
    for (let i = 0; i < studioPcm.length; i++) {
      const t = i / sr;
      if (t >= 1 && t <= 10) {
        const val = Math.sin(2 * Math.PI * (220 + 20 * Math.floor(t)) * t) * 0.6;
        studioPcm[i] = val;
        if (i + sr < livePcm.length) {
          livePcm[i + sr] = val;
        }
      }
    }

    let coarseCalled = false;
    const aligned = await alignAudioInWorker(
      {
        studios: [studioPcm],
        live: livePcm,
        lines: [
          { idx: 0, studio_start: 2.0, text: "First line" },
          { idx: 1, studio_start: 5.0, text: "Second line" },
        ],
      },
      new AbortController().signal,
      () => {
        coarseCalled = true;
      },
    );
    expect(coarseCalled).toBe(true);
    expect(aligned.lines).toHaveLength(2);
    expect(aligned.lines[0].liveStartSec).toBeGreaterThan(2.2);

    // 3. Cancellation on controller
    const controller = createTauriAutoLyricsController(async () => undefined as any);
    await expect(controller.cancel("BV1xx411c7mD-1")).resolves.toBeUndefined();
  });

  test("applyMigratedLocalStorage writes only missing string entries, then acks", async () => {
    const calls: string[] = [];
    (window as any).__TAURI__ = {
      core: {
        invoke: vi.fn(async (cmd: string) => {
          calls.push(cmd);
          if (cmd === "migration_local_storage") {
            return { "user-token": '{"state":{"tokenData":{"refresh_token":"r"}}}', "play-list": "new", bad: 1 };
          }
          return null;
        }),
      },
    };
    localStorage.clear();
    localStorage.setItem("play-list", "existing");
    const written = await applyMigratedLocalStorage(localStorage);
    expect(written).toBe(1);
    expect(localStorage.getItem("user-token")).toContain("refresh_token");
    expect(localStorage.getItem("play-list")).toBe("existing");
    expect(localStorage.getItem("bad")).toBeNull();
    expect(calls).toEqual(["migration_local_storage", "migration_local_storage_done"]);
  });
});
