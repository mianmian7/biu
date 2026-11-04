/**
 * Electron does not wait for async before-quit work. This handler holds every quit until one
 * migration export has finished (idle → exporting → exported), including repeated quits and
 * quits during an explicit install's own export. Only a handed-off installer skips the hold.
 */
export function createQuitHold(deps: {
  exportWithin: (ms: number) => Promise<string | null>;
  isInstallerHandedOff: () => boolean;
  /** Called when the export failed, before quitting again. */
  onExportFailed: () => void;
  quit: () => void;
  timeoutMs?: number;
  warn?: (message: string, error: unknown) => void;
}) {
  let state: "idle" | "exporting" | "exported" = "idle";
  return (event: { preventDefault: () => void }) => {
    if (state === "exported" || deps.isInstallerHandedOff()) return;
    event.preventDefault();
    if (state === "exporting") return;
    state = "exporting";
    void (async () => {
      let exported: string | null = null;
      try {
        exported = await deps.exportWithin(deps.timeoutMs ?? 5000);
      } catch (error) {
        deps.warn?.("[main] quit export failed:", error);
      }
      if (!exported) deps.onExportFailed();
      state = "exported";
      deps.quit();
    })();
  };
}
