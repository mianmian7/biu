import { BrowserWindow, ipcMain } from "electron";
import { z } from "zod";

import { autoLyricsService } from "../auto-lyrics/runtime";
import { sources } from "../auto-lyrics/sources";
import { channel } from "./channel";

const fullSchema = z.object({
  bvid: z.string().regex(/^BV[\da-zA-Z]{10}$/),
  cid: z.number().int().positive(),
  title: z.string(),
  pageTitle: z.string().optional(),
  duration: z.number().positive().finite(),
  tid: z.number().int(),
  ownerName: z.string(),
  bypassCache: z.boolean().optional(),
});
/** The renderer may send only bvid/cid; the rest comes from the view API. */
const paramsSchema = z.union([fullSchema, fullSchema.pick({ bvid: true, cid: true, bypassCache: true })]);

async function resolveParams(input: unknown) {
  const params = paramsSchema.parse(input);
  if ("tid" in params) return params;
  const resolved = await sources.params(params.bvid, params.cid, AbortSignal.timeout(15000));
  return { ...resolved, bypassCache: params.bypassCache };
}

export function registerAutoLyricsHandlers() {
  ipcMain.handle(channel.autoLyrics.start, async (_, params: unknown) =>
    autoLyricsService.start(await resolveParams(params)),
  );
  ipcMain.handle(channel.autoLyrics.prefetch, async (_, params: unknown) =>
    autoLyricsService.prefetch(await resolveParams(params)),
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
