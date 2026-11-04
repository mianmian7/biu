import { StoreNameMap } from "@shared/store";
import { beforeEach, describe, expect, test, vi } from "vitest";

import {
  createTauriElectronAPI,
  NOT_IMPLEMENTED_MESSAGE,
  toMediaUrl,
  UNIMPLEMENTED_ELECTRON_METHODS,
  unwrapMediaUrl,
} from "@/platform";
import { biliAdapter } from "@/service/request";

describe("platform ElectronAPI compatibility and media URL wrapping", () => {
  beforeEach(() => {
    vi.restoreAllMocks();
    delete (window as any).__TAURI__;
  });

  test("all Step 2+ unimplemented methods return explicit '未实现' errors or warnings without throwing synchronously", async () => {
    const warnSpy = vi.spyOn(console, "warn").mockImplementation(() => {});
    const api = createTauriElectronAPI() as Record<string, any>;

    for (const key of UNIMPLEMENTED_ELECTRON_METHODS) {
      const target = key.includes(".")
        ? key.split(".").reduce((acc, part) => acc?.[part], api)
        : api[key];
      expect(typeof target).toBe("function");

      const result = target();
      if (result && typeof result.then === "function") {
        await expect(result).rejects.toThrow(NOT_IMPLEMENTED_MESSAGE);
      } else if (typeof result === "function") {
        // Unsubscribe function returned by event listener stubs
        expect(() => result()).not.toThrow();
        expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining(NOT_IMPLEMENTED_MESSAGE));
      } else {
        expect(warnSpy).toHaveBeenCalledWith(expect.stringContaining(NOT_IMPLEMENTED_MESSAGE));
      }
    }
  });

  test("Step 1 methods invoke corresponding Tauri commands", async () => {
    const invokeMock = vi.fn(async (cmd: string, _args?: Record<string, unknown>) => {
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
      }),
    });
  });
});
