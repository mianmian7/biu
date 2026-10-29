import { describe, expect, it } from "vitest";

import { markManualEdits } from "../../electron/auto-lyrics/manual-cache";

describe("renderer lyrics edits", () => {
  const entry: MusicLyrics = { lyrics: "[00:01]line", source: "auto", offset: 0 };
  it("marks offset changes and replacement lyrics as manual", () => {
    expect(markManualEdits({ key: entry }, { key: { ...entry, offset: 100 } }).key.source).toBe("manual");
    expect(markManualEdits({ key: entry }, { key: { ...entry, lyrics: "replacement" } }).key.source).toBe("manual");
  });
  it("preserves automatic status for font changes and legacy manual entries", () => {
    expect(markManualEdits({ key: entry }, { key: { ...entry, fontSize: 20 } }).key.source).toBe("auto");
    expect(markManualEdits({ key: {} }, { key: {} }).key.source).toBeUndefined();
  });
});
