import { describe, expect, it, vi } from "vitest";

import { createQuitHold } from "../electron/quit-hold";

function deferred<T>() {
  let resolve!: (value: T) => void;
  const promise = new Promise<T>(r => (resolve = r));
  return { promise, resolve };
}

function setup(handedOff = false) {
  const exportResult = deferred<string | null>();
  const deps = {
    exportWithin: vi.fn(() => exportResult.promise),
    isInstallerHandedOff: vi.fn(() => handedOff),
    onExportFailed: vi.fn(),
    quit: vi.fn(),
  };
  const hold = createQuitHold(deps);
  const quitEvent = () => {
    const event = { preventDefault: vi.fn() };
    hold(event);
    return event;
  };
  return { deps, exportResult, quitEvent };
}

describe("createQuitHold", () => {
  it("holds every quit until one export finishes, then lets the next quit through", async () => {
    const { deps, exportResult, quitEvent } = setup();
    expect(quitEvent().preventDefault).toHaveBeenCalled();
    // A second quit while exporting is held too and does not start another export.
    expect(quitEvent().preventDefault).toHaveBeenCalled();
    expect(deps.exportWithin).toHaveBeenCalledTimes(1);
    expect(deps.quit).not.toHaveBeenCalled();

    exportResult.resolve("/tmp/tauri-migration.json");
    await vi.waitFor(() => expect(deps.quit).toHaveBeenCalledTimes(1));
    expect(deps.onExportFailed).not.toHaveBeenCalled();
    expect(quitEvent().preventDefault).not.toHaveBeenCalled();
  });

  it("skips the install on this quit when the export fails", async () => {
    const { deps, exportResult, quitEvent } = setup();
    quitEvent();
    exportResult.resolve(null);
    await vi.waitFor(() => expect(deps.quit).toHaveBeenCalledTimes(1));
    expect(deps.onExportFailed).toHaveBeenCalledTimes(1);
  });

  it("does not hold once the installer has taken over", () => {
    const { deps, quitEvent } = setup(true);
    expect(quitEvent().preventDefault).not.toHaveBeenCalled();
    expect(deps.exportWithin).not.toHaveBeenCalled();
  });
});
