export function classifyUpdateError(error: unknown): "unpublished" | "network" | "other" {
  const code = String((error as { code?: unknown } | null)?.code ?? "");
  const message = error instanceof Error ? error.message : String(error);
  if (
    [
      "ERR_UPDATER_CHANNEL_FILE_NOT_FOUND",
      "ERR_UPDATER_LATEST_VERSION_NOT_FOUND",
      "ERR_UPDATER_NO_PUBLISHED_VERSIONS",
    ].includes(code)
  ) {
    return "unpublished";
  }
  if (["ENOTFOUND", "ECONNRESET", "ETIMEDOUT"].includes(code) || /net::ERR_/.test(`${code} ${message}`)) {
    return "network";
  }
  return "other";
}
