// @vitest-environment node
import { afterEach, expect, test, vi } from "vitest";

import type { RomajiRequest, RomajiResponse } from "@/platform/romaji-worker";

import { romanizeLyricLines } from "@/platform/romaji";

vi.mock("@/platform/index", () => ({
  ensureMediaBase: async () => "http://localhost/",
  invokeTauri: async () => {},
}));

afterEach(() => {
  vi.useRealTimers();
  vi.unstubAllGlobals();
});

test("the romaji worker terminates after eight idle seconds and the normalized result stays cached", async () => {
  const workers: TestWorker[] = [];
  class TestWorker {
    onmessage?: (event: { data: RomajiResponse }) => void;
    postMessage = vi.fn<(input: RomajiRequest) => void>();
    terminate = vi.fn();
    constructor() {
      workers.push(this);
    }
  }
  vi.stubGlobal("Worker", TestWorker);
  vi.useFakeTimers();

  const lyrics = ["風に揺れる", "日本の街"];
  const expected = ["ka ze ni yu re ru", "ni ho n no ma chi"];
  const first = romanizeLyricLines(lyrics);
  const duplicate = romanizeLyricLines([" 風に揺れる ", "日本の街"]);
  await vi.advanceTimersByTimeAsync(8_000);
  expect(workers).toHaveLength(1);
  expect(workers[0].postMessage).toHaveBeenCalledTimes(1);
  expect(workers[0].terminate).not.toHaveBeenCalled();

  const [{ id }] = workers[0].postMessage.mock.calls[0];
  workers[0].onmessage?.({ data: { id, result: expected } });
  expect(await first).toEqual(expected);
  expect(await duplicate).toEqual(expected);
  await vi.advanceTimersByTimeAsync(7_999);
  expect(workers[0].terminate).not.toHaveBeenCalled();
  await vi.advanceTimersByTimeAsync(1);
  expect(workers[0].terminate).toHaveBeenCalledOnce();

  expect(await romanizeLyricLines(lyrics)).toEqual(expected);
  expect(workers).toHaveLength(1);
});
