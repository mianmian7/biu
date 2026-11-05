import { describe, expect, it } from "vitest";

import { formatUrlProtocol } from "@/common/utils/url";

describe("formatUrlProtocol", () => {
  it("adds https to protocol-relative covers", () => {
    expect(formatUrlProtocol("//i1.hdslb.com/bfs/archive/a.jpg")).toBe("https://i1.hdslb.com/bfs/archive/a.jpg");
  });

  it("upgrades http covers that WKWebView would block", () => {
    expect(formatUrlProtocol("http://i1.hdslb.com/bfs/archive/a.jpg")).toBe("https://i1.hdslb.com/bfs/archive/a.jpg");
  });

  it("keeps https, empty and undefined values", () => {
    expect(formatUrlProtocol("https://i0.hdslb.com/a.jpg")).toBe("https://i0.hdslb.com/a.jpg");
    expect(formatUrlProtocol("")).toBe("");
    expect(formatUrlProtocol(undefined)).toBeUndefined();
  });
});
