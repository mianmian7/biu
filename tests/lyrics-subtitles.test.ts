import { beforeEach, describe, expect, test, vi } from "vitest";

import { getLyricsByBili } from "@/components/lyrics/get-lyrics";
import { getLyric } from "@/service/ai-lyrics";
import { getWebPlayerInfo } from "@/service/web-player";

vi.mock("@/service/ai-lyrics", () => ({ getLyric: vi.fn() }));
vi.mock("@/service/web-player", () => ({ getWebPlayerInfo: vi.fn() }));

beforeEach(() => {
  vi.mocked(getWebPlayerInfo).mockResolvedValue({
    data: { subtitle: { subtitles: [{ subtitle_url: "https://example.com/subtitles" }] } },
  } as Awaited<ReturnType<typeof getWebPlayerInfo>>);
});

describe("Bilibili subtitle silence boundaries", () => {
  test("does not clear shorter overlapping subtitles while an earlier line is still running", async () => {
    vi.mocked(getLyric).mockResolvedValue({
      body: [
        { from: 0, to: 10, content: "A" },
        { from: 2, to: 4, content: "B" },
        { from: 8, to: 9, content: "C" },
      ],
    });
    expect(await getLyricsByBili({ cid: 1 })).toEqual([
      { time: 0, text: "A" },
      { time: 2000, text: "B" },
      { time: 8000, endTime: 10000, text: "C" },
    ]);
  });

  test("measures overlap gaps from the latest end rather than the latest line's end", async () => {
    vi.mocked(getLyric).mockResolvedValue({
      body: [
        { from: 0, to: 10, content: "A" },
        { from: 2, to: 4, content: "B" },
        { from: 11, to: 15, content: "C" },
        { from: 12, to: 13, content: "D" },
        { from: 15.999, to: 17, content: "E" },
      ],
    });
    expect(await getLyricsByBili({ cid: 1 })).toEqual([
      { time: 0, text: "A" },
      { time: 2000, endTime: 10000, text: "B" },
      { time: 11000, text: "C" },
      { time: 12000, text: "D" },
      { time: 15999, endTime: 17000, text: "E" },
    ]);
  });

  test("retains one-second gaps and the final end, ignoring shorter gaps", async () => {
    vi.mocked(getLyric).mockResolvedValue({
      body: [
        { from: 10, to: 11, content: "last" },
        { from: 1, to: 2, content: "first" },
        { from: 3, to: 4.001, content: "second" },
        { from: 5, to: 10, content: "third" },
      ],
    });
    const lines = await getLyricsByBili({ cid: 1 });
    expect(lines).toEqual([
      { time: 1000, endTime: 2000, text: "first" },
      { time: 3000, text: "second" },
      { time: 5000, text: "third" },
      { time: 10000, endTime: 11000, text: "last" },
    ]);
  });

  test("ignores missing or invalid ends and empty subtitle rows", async () => {
    vi.mocked(getLyric).mockResolvedValue({
      body: [
        { from: 1, content: "missing" },
        { from: 2, to: 1, content: "invalid" },
        { from: 3, to: NaN, content: "not finite" },
        { from: 4, to: 5, content: "♪" },
      ],
    });
    expect(await getLyricsByBili({ cid: 1 })).toEqual([
      { time: 1000, text: "missing" },
      { time: 2000, text: "invalid" },
      { time: 3000, text: "not finite" },
    ]);
  });

  test("handles absent subtitles, empty bodies and network errors", async () => {
    vi.mocked(getLyric).mockResolvedValue({ body: [] });
    expect(await getLyricsByBili({ cid: 1 })).toEqual([]);
    vi.mocked(getLyric).mockRejectedValue(new Error("offline"));
    await expect(getLyricsByBili({ cid: 1 })).rejects.toThrow("offline");
    vi.mocked(getWebPlayerInfo).mockResolvedValue({} as Awaited<ReturnType<typeof getWebPlayerInfo>>);
    expect(await getLyricsByBili({ cid: 1 })).toBeNull();
  });
});
