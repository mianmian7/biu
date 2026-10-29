// @vitest-environment node
import { mkdtemp, readFile, rm, writeFile } from "node:fs/promises";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it, vi } from "vitest";
import { z } from "zod";

const mocks = vi.hoisted(() => ({ view: vi.fn(), start: vi.fn(), identify: vi.fn(), unsubscribe: vi.fn() }));
vi.mock("electron", () => ({
  app: { getAppMetrics: () => [{ pid: 1, type: "Browser", memory: { workingSetSize: 123, privateBytes: 100 } }] },
}));
vi.mock("electron-log", () => ({ default: { info: vi.fn() } }));
vi.mock("../../electron/auto-lyrics/sources", () => ({ sources: { view: mocks.view } }));
vi.mock("../../electron/auto-lyrics/runtime", () => ({
  autoLyricsService: { start: mocks.start, identify: mocks.identify, onProgress: () => mocks.unsubscribe },
}));

import { runAutoLyricsBench } from "../../electron/auto-lyrics/bench-runner";

const rowsSchema = z.array(
  z.object({
    coldStart: z.boolean(),
    status: z.string(),
    error: z.string().optional(),
    expect: z.unknown().optional(),
    memory: z.object({ peaks: z.array(z.object({ workingSetKB: z.number() })) }),
  }),
);

const directories: string[] = [];
afterEach(async () => {
  await Promise.all(directories.splice(0).map(directory => rm(directory, { recursive: true, force: true })));
});

async function config(value: object) {
  const directory = await mkdtemp(path.join(os.tmpdir(), "lyrics-bench-test-"));
  directories.push(directory);
  const filename = path.join(directory, "config.json");
  await writeFile(filename, JSON.stringify({ ...value, outDir: "results" }));
  return { directory, filename };
}

describe("Electron bench runner", () => {
  it("runs repeats without alignment cache and keeps identify-only out of the pipeline", async () => {
    mocks.view.mockResolvedValue({
      bvid: "BV1StSoBPEwu",
      title: "合集",
      tid: 3,
      ownerName: "UP",
      pages: [{ page: 2, cid: 20, part: "song", duration: 200 }],
    });
    mocks.start.mockResolvedValue({ key: "BV1StSoBPEwu-20", status: "matched", timings: { coarse: 30, fine: 50 } });
    mocks.identify.mockResolvedValue({ status: "unidentified", confidence: 0 });
    const { filename, directory } = await config({
      cases: [{ bvid: "BV1StSoBPEwu", page: 2 }],
      identifyOnly: [{ bvid: "BV1StSoBPEwu", page: 2, expect: "unidentified" }],
      repeat: 2,
    });
    await runAutoLyricsBench(filename);
    expect(mocks.start).toHaveBeenCalledTimes(2);
    expect(mocks.start).toHaveBeenLastCalledWith(
      expect.objectContaining({ cid: 20, pageTitle: "song", bypassCache: true }),
    );
    expect(mocks.identify).toHaveBeenCalledTimes(1);
    const rows = rowsSchema.parse(JSON.parse(await readFile(path.join(directory, "results/results.json"), "utf8")));
    expect(rows.map((row: { coldStart: boolean }) => row.coldStart)).toEqual([true, false, false]);
    expect(rows[0].memory.peaks[0].workingSetKB).toBe(123);
    expect(rows[2].expect).toBe("unidentified");
    expect(mocks.unsubscribe).toHaveBeenCalledTimes(3);
  });

  it("records view failures and continues with the following case", async () => {
    mocks.view.mockRejectedValue(new Error("HTTP 412"));
    const { filename, directory } = await config({
      identifyOnly: [{ bvid: "BV1StSoBPEwu" }, { bvid: "BV14f4y1W7jY" }],
    });
    await runAutoLyricsBench(filename);
    const rows = rowsSchema.parse(JSON.parse(await readFile(path.join(directory, "results/results.json"), "utf8")));
    expect(rows).toHaveLength(2);
    expect(rows[0]).toMatchObject({ status: "error", error: "HTTP 412" });
    expect(mocks.start).not.toHaveBeenCalled();
    expect(mocks.identify).not.toHaveBeenCalled();
  });
});
