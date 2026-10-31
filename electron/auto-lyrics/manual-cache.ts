/** Renderer edits have priority over the automatic pipeline, including old entries. */
export function markManualEdits(
  previous: Record<string, MusicLyrics>,
  incoming: Record<string, MusicLyrics>,
): Record<string, MusicLyrics> {
  return Object.fromEntries(
    Object.entries(incoming).map(([key, entry]) => {
      const old = previous[key];
      const edited =
        old?.lyrics !== entry.lyrics ||
        old?.tLyrics !== entry.tLyrics ||
        old?.rLyrics !== entry.rLyrics ||
        (old?.offset ?? 0) !== (entry.offset ?? 0);
      return [key, edited ? { ...entry, source: "manual" } : entry];
    }),
  );
}
