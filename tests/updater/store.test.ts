import { describe, expect, it } from "vitest";

import { useAppUpdateStore } from "../../src/store/app-update";

describe("global download state", () => {
  it("retains progress and file info independently of the release modal", () => {
    const store = useAppUpdateStore.getState();
    store.setMode("auto");
    store.setUpdate({ isUpdateAvailable: true, latestVersion: "1.16.1-autolyrics.6" });
    const progress = { total: 100, transferred: 50, delta: 50, percent: 50, bytesPerSecond: 10 };
    store.setDownload({ status: "downloading", processInfo: progress });
    expect(useAppUpdateStore.getState()).toMatchObject({
      mode: "auto",
      downloadStatus: "downloading",
      downloadProgress: progress,
    });
    store.setDownload({ status: "downloaded", mode: "manual", downloadInfo: { filePath: "/tmp/Biu.zip" } });
    expect(useAppUpdateStore.getState()).toMatchObject({
      mode: "manual",
      downloadStatus: "downloaded",
      downloadInfo: { filePath: "/tmp/Biu.zip" },
      latestVersion: "1.16.1-autolyrics.6",
    });
  });
});
