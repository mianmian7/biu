import { act, type ReactNode } from "react";
import { createRoot, type Root } from "react-dom/client";

import { afterEach, beforeEach, describe, expect, it, vi } from "vitest";

import ReleaseNoteModal from "@/components/release-note-modal";
import { useAppUpdateStore } from "@/store/app-update";

vi.mock("@heroui/react", () => {
  const Container = ({ children }: { children: ReactNode }) => <div>{children}</div>;
  return {
    addToast: vi.fn(),
    Modal: Container,
    ModalBody: Container,
    ModalContent: Container,
    ModalFooter: Container,
    ModalHeader: Container,
    Progress: () => null,
    Button: ({ children, onPress, isLoading }: { children: ReactNode; onPress: () => void; isLoading?: boolean }) => (
      <button type="button" disabled={isLoading} onClick={onPress}>
        {children}
      </button>
    ),
  };
});
vi.mock("@/components/typography", () => ({ default: () => null }));

let container: HTMLDivElement;
let root: Root;
const download = vi.fn();
beforeEach(() => {
  Object.assign(globalThis, { IS_REACT_ACT_ENVIRONMENT: true });
  Object.assign(window, { electron: { downloadAppUpdate: download } });
  download.mockResolvedValue(undefined);
  useAppUpdateStore.setState({
    mode: "auto",
    downloadStatus: "error",
    downloadError: "download failed",
    downloadProgress: undefined,
  });
  container = document.createElement("div");
  document.body.append(container);
  root = createRoot(container);
});
afterEach(() => {
  act(() => root.unmount());
  container.remove();
  Object.assign(window, { electron: undefined });
});

describe("automatic download recovery", () => {
  it("offers retry after an auto download error and calls the download entry point", async () => {
    act(() => root.render(<ReleaseNoteModal />));
    const retry = Array.from(container.querySelectorAll("button")).find(button => button.textContent === "重试下载");
    expect(retry).toBeDefined();
    await act(async () => retry!.click());
    expect(download).toHaveBeenCalledOnce();
    expect(useAppUpdateStore.getState().downloadStatus).toBe("downloading");
  });
  it("retains the retry action if a retry rejects", async () => {
    download.mockRejectedValue(new Error("retry failed"));
    act(() => root.render(<ReleaseNoteModal />));
    await act(async () => container.querySelector("button")!.click());
    expect(useAppUpdateStore.getState().downloadError).toBe("retry failed");
    expect(container.textContent).toContain("重试下载");
  });
});
