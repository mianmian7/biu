import { describe, expect, it } from "vitest";

import { classifyUpdateError } from "../../electron/updater/classify-error";

describe("classifyUpdateError", () => {
  it.each([
    "ERR_UPDATER_CHANNEL_FILE_NOT_FOUND",
    "ERR_UPDATER_LATEST_VERSION_NOT_FOUND",
    "ERR_UPDATER_NO_PUBLISHED_VERSIONS",
  ])("classifies %s as unpublished", code => {
    expect(classifyUpdateError(Object.assign(new Error("release missing"), { code }))).toBe("unpublished");
    expect(classifyUpdateError({ code })).toBe("unpublished");
  });
  it.each(["ENOTFOUND", "ECONNRESET", "ETIMEDOUT", "net::ERR_INTERNET_DISCONNECTED"])(
    "classifies %s as network",
    code => {
      expect(classifyUpdateError({ code })).toBe("network");
    },
  );
  it.each([new Error("net::ERR_CONNECTION_REFUSED"), "net::ERR_TIMED_OUT"])(
    "classifies Chromium messages as network",
    error => {
      expect(classifyUpdateError(error)).toBe("network");
    },
  );
  it.each([
    undefined,
    null,
    "unexpected",
    new Error("checksum mismatch"),
    { code: "ERR_UPDATER_INVALID_SIGNATURE" },
    { code: "EACCES" },
  ])("classifies other failures safely: %s", error => {
    expect(classifyUpdateError(error)).toBe("other");
  });
});
