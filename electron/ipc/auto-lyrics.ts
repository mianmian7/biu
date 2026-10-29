import { BrowserWindow, ipcMain } from "electron";
import { z } from "zod";

import { autoLyricsService } from "../auto-lyrics/runtime";
import { channel } from "./channel";

const paramsSchema = z.object({
  bvid: z.string().regex(/^BV[\da-zA-Z]{10}$/),
  cid: z.number().int().positive(),
  title: z.string(),
  pageTitle: z.string().optional(),
  duration: z.number().positive().finite(),
  tid: z.number().int(),
  ownerName: z.string(),
  bypassCache: z.boolean().optional(),
});

export function registerAutoLyricsHandlers() {
  ipcMain.handle(channel.autoLyrics.start, (_, params: unknown) => autoLyricsService.start(paramsSchema.parse(params)));
  ipcMain.handle(channel.autoLyrics.prefetch, (_, params: unknown) =>
    autoLyricsService.prefetch(paramsSchema.parse(params)),
  );
  ipcMain.handle(channel.autoLyrics.cancel, (_, key: unknown) =>
    autoLyricsService.cancel(z.string().optional().parse(key)),
  );
  autoLyricsService.onProgress(progress => {
    for (const window of BrowserWindow.getAllWindows()) {
      if (!window.isDestroyed() && !window.webContents.isDestroyed()) {
        window.webContents.send(channel.autoLyrics.progress, progress);
      }
    }
  });
}
