import { afterEach, describe, expect, it, vi } from "vitest";

const mocks = vi.hoisted(() => ({ build: vi.fn(), error: vi.fn(), success: vi.fn() }));
vi.mock("electron-builder", () => ({ build: mocks.build }));
vi.mock("@rsbuild/core", () => ({ logger: { error: mocks.error, success: mocks.success } }));
afterEach(() => vi.unstubAllEnvs());

describe("update build configuration", () => {
  it("uses a generic provider only when the local feed override is present", async () => {
    vi.stubEnv("BIU_UPDATE_FEED_URL", "http://127.0.0.1:8765");
    mocks.build.mockResolvedValue([]);
    const { buildElectron } = await import("../../plugins/electron-build");
    await buildElectron();
    expect(mocks.build).toHaveBeenLastCalledWith(
      expect.objectContaining({
        config: expect.objectContaining({ publish: { provider: "generic", url: "http://127.0.0.1:8765" } }),
      }),
    );
    vi.stubEnv("BIU_UPDATE_FEED_URL", "");
    await buildElectron();
    expect(mocks.build).toHaveBeenLastCalledWith(
      expect.objectContaining({
        config: expect.objectContaining({
          publish: expect.objectContaining({ provider: "github", releaseType: null }),
        }),
      }),
    );
  });
  it("propagates build failures after logging", async () => {
    const error = new Error("build failed");
    mocks.build.mockRejectedValue(error);
    const { buildElectron } = await import("../../plugins/electron-build");
    await expect(buildElectron()).rejects.toThrow("build failed");
    expect(mocks.error).toHaveBeenCalledWith(error);
  });
});
