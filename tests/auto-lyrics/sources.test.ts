// @vitest-environment node
import { describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({
  get: vi.fn(),
  view: vi.fn(),
  dash: vi.fn(),
  search: vi.fn(),
  lyric: vi.fn(),
  sign: vi.fn(),
}));

vi.mock("got", () => ({ default: { get: mocks.get } }));
vi.mock("../../electron/ipc/api/web-interface-view", () => ({ getWebInterfaceView: mocks.view }));
vi.mock("../../electron/ipc/api/dash-url", () => ({ getDashurl: mocks.dash }));
vi.mock("../../electron/ipc/api/netease-lyric", () => ({
  getSongByNetease: mocks.search,
  getLyricsByNetease: mocks.lyric,
}));
vi.mock("../../electron/ipc/api/wbi", () => ({ encodeParamsWbi: mocks.sign }));
vi.mock("../../electron/network/cookie", () => ({ getCookieString: async () => "" }));

import { sources } from "../../electron/auto-lyrics/sources";

describe("auto lyrics sources", () => {
  const song = { id: 1, name: "Test", artists: ["Artist"], duration: 180 };

  it("retries v_voucher at most twice and reports the blocked search", async () => {
    mocks.sign.mockImplementation(async params => params);
    mocks.get.mockReturnValue({ json: async () => ({ code: 0, data: { v_voucher: "blocked" } }) });
    await expect(sources.references(song, new AbortController().signal)).rejects.toThrow("after 2 retries");
    expect(mocks.get).toHaveBeenCalledTimes(3);
  });

  it("cancels during voucher backoff without starting another request", async () => {
    const controller = new AbortController();
    mocks.sign.mockImplementation(async params => params);
    mocks.get.mockReturnValue({
      json: async () => {
        controller.abort();
        return { code: 0, data: { v_voucher: "blocked" } };
      },
    });
    await expect(sources.references(song, controller.signal)).rejects.toMatchObject({ name: "AbortError" });
    expect(mocks.get).toHaveBeenCalledTimes(1);
  });

  it("resolves a reference cid and downloads only its lowest bandwidth audio", async () => {
    const signal = new AbortController().signal;
    mocks.view.mockResolvedValue({
      code: 0,
      data: { bvid: "BVtest", cid: 2, title: "Test", duration: 180, tid: 3, owner: { name: "UP" }, pages: [] },
    });
    mocks.dash.mockResolvedValue({
      code: 0,
      data: {
        dash: {
          audio: [
            { bandwidth: 192000, baseUrl: "https://audio.invalid/high" },
            { bandwidth: 53000, base_url: "https://audio.invalid/low" },
          ],
        },
      },
    });
    const audio = Buffer.from([1, 2, 3]);
    mocks.get.mockReturnValue({ buffer: async () => audio });
    await expect(sources.audio("BVtest", undefined, signal)).resolves.toBe(audio);
    expect(mocks.dash).toHaveBeenCalledWith(expect.objectContaining({ bvid: "BVtest", cid: 2 }), signal);
    expect(mocks.get).toHaveBeenCalledWith(
      "https://audio.invalid/low",
      expect.objectContaining({
        signal,
        headers: expect.objectContaining({ Referer: "https://www.bilibili.com/video/BVtest" }),
      }),
    );
  });

  it("keeps Netease durations in seconds and maps translated and romanized lyrics", async () => {
    const signal = new AbortController().signal;
    mocks.search.mockResolvedValue({
      code: 200,
      result: { songs: [{ id: 1, name: "Test", duration: 180000, artists: [{ name: "Artist" }], alias: ["Alias"] }] },
    });
    mocks.lyric.mockResolvedValue({
      code: 200,
      lrc: { lyric: "original" },
      tlyric: { lyric: "translated" },
      romalrc: { lyric: "romanized" },
    });
    await expect(sources.search("Test", signal)).resolves.toEqual([{ ...song, aliases: ["Alias"] }]);
    await expect(sources.lyric(1, signal)).resolves.toEqual({
      lrc: "original",
      tlyric: "translated",
      romalrc: "romanized",
    });
  });
});
