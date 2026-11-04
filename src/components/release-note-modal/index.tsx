import { addToast, Button, Modal, ModalBody, ModalContent, ModalFooter, ModalHeader, Progress } from "@heroui/react";
import { RiInformationLine } from "@remixicon/react";
import { filesize } from "filesize";
import { useShallow } from "zustand/react/shallow";

import { useAppUpdateStore } from "@/store/app-update";
import { useModalStore } from "@/store/modal";

import Typography from "../typography";

const ReleaseNoteModal = () => {
  const { isReleaseNoteModalOpen, onReleaseNoteModalOpenChange } = useModalStore(
    useShallow(state => ({
      isReleaseNoteModalOpen: state.isReleaseNoteModalOpen,
      onReleaseNoteModalOpenChange: state.onReleaseNoteModalOpenChange,
    })),
  );
  const {
    releaseNotes,
    mode,
    downloadStatus: status,
    downloadProgress,
    downloadInfo,
    downloadError: error,
    setDownload,
  } = useAppUpdateStore(
    useShallow(state => ({
      releaseNotes: state.releaseNotes,
      mode: state.mode,
      downloadStatus: state.downloadStatus,
      downloadProgress: state.downloadProgress,
      downloadInfo: state.downloadInfo,
      downloadError: state.downloadError,
      setDownload: state.setDownload,
    })),
  );

  const startDownload = async () => {
    setDownload({ status: "downloading" });
    try {
      await window.electron.downloadAppUpdate();
    } catch (e) {
      setDownload({ status: "error", error: e instanceof Error ? e.message : String(e) });
    }
  };

  const handleOpenInstaller = async () => {
    try {
      if (downloadInfo?.filePath) {
        const ok = await window.electron.showFileInFolder(downloadInfo.filePath);
        if (!ok) {
          addToast({
            title: "无法打开安装包文件夹",
            color: "danger",
          });
        }
        return ok;
      }
    } catch (e) {
      addToast({
        title: e instanceof Error ? e.message : String(e),
        color: "danger",
      });
    }
  };

  return (
    <>
      <Modal
        radius="md"
        shouldBlockScroll={false}
        scrollBehavior="inside"
        size="lg"
        isOpen={isReleaseNoteModalOpen}
        onOpenChange={onReleaseNoteModalOpenChange}
        isKeyboardDismissDisabled
        isDismissable={false}
        disableAnimation
      >
        <ModalContent>
          <ModalHeader>✨ 有新版本更新</ModalHeader>
          <ModalBody className="px-0">
            {releaseNotes?.trim() ? (
              <Typography content={releaseNotes} />
            ) : (
              <div className="text-center text-sm text-zinc-500 dark:text-zinc-400">暂无更新日志</div>
            )}
          </ModalBody>
          {status === "downloading" && (
            <Progress radius="none" className="w-full" color="primary" value={downloadProgress?.percent} />
          )}
          <ModalFooter className="items-center justify-between">
            <div className="min-w-0 flex-auto">
              {status === "downloading" && (
                <div className="items-center">{filesize(downloadProgress?.bytesPerSecond || 0)}/s</div>
              )}
              {status === "error" && <span className="text-danger block truncate">{error}</span>}
            </div>
            {status === "downloaded" ? (
              <div className="inline-flex items-center space-x-2">
                <span className="inline-flex items-center space-x-1">
                  <RiInformationLine size={16} />
                  <span className="text-sm text-zinc-500 dark:text-zinc-400">
                    {mode === "auto" ? "也可以稍后，退出时自动安装" : "安装包已下载完成"}
                  </span>
                </span>
                {mode === "auto" ? (
                  <Button
                    color="primary"
                    onPress={() => {
                      void window.electron.quitAndInstall();
                    }}
                  >
                    立即重启更新
                  </Button>
                ) : (
                  <Button color="primary" onPress={handleOpenInstaller}>
                    打开安装包文件夹
                  </Button>
                )}
              </div>
            ) : (
              <div className="inline-flex items-center space-x-2">
                {Boolean(downloadProgress) && (
                  <span>
                    {downloadProgress?.transferred ? filesize(downloadProgress.transferred) : "-"}/
                    {downloadProgress?.total ? filesize(downloadProgress.total) : "-"}
                  </span>
                )}

                {mode === "manual" ? (
                  <Button color="primary" isLoading={status === "downloading"} onPress={startDownload}>
                    {status === "downloading" ? "正在下载" : "下载更新"}
                  </Button>
                ) : mode === "auto" && status === "error" ? (
                  <Button color="primary" onPress={startDownload}>
                    重试下载
                  </Button>
                ) : status === "downloading" ? (
                  <span>正在下载 {downloadProgress?.percent.toFixed(2) ?? "0"}%</span>
                ) : mode === "auto" && status === "idle" ? (
                  <span>正在后台准备更新</span>
                ) : null}
              </div>
            )}
          </ModalFooter>
        </ModalContent>
      </Modal>
    </>
  );
};

export default ReleaseNoteModal;
