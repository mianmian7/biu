import got from "got";
import { setTimeout } from "node:timers/promises";
import { z } from "zod";

import type { SongCandidate } from "../../shared/auto-lyrics/identify";
import type { ReferenceCandidate } from "../../shared/auto-lyrics/reference";

import { rankReferences } from "../../shared/auto-lyrics/reference";
import { getDashurl } from "../ipc/api/dash-url";
import { getLyricsByNetease, getSongByNetease } from "../ipc/api/netease-lyric";
import { encodeParamsWbi } from "../ipc/api/wbi";
import { getWebInterfaceView } from "../ipc/api/web-interface-view";
import { getCookieString } from "../network/cookie";
import { UserAgent } from "../network/user-agent";

const searchResponse = z.object({
  code: z.number(),
  message: z.string().optional(),
  data: z
    .object({
      v_voucher: z.string().optional(),
      result: z
        .array(z.object({ bvid: z.string(), title: z.string(), duration: z.union([z.string(), z.number()]) }))
        .optional(),
    })
    .nullish(),
});

function durationSeconds(value: string | number): number {
  if (typeof value === "number") return value;
  return value.split(":").reduce((total, part) => total * 60 + Number(part), 0);
}

type VideoView = Awaited<ReturnType<typeof sources.view>>;

/**
 * Identification input for one page. The page title is only trusted on multi-part videos: a single part is
 * usually named after the uploaded file (TG-2026-10-04-172657959).
 */
export function pageParams(video: VideoView, cid: number) {
  const page = video.pages.find(item => item.cid === cid);
  return {
    bvid: video.bvid,
    cid,
    title: video.title,
    pageTitle: video.pages.length > 1 ? page?.part : undefined,
    duration: page?.duration ?? video.duration,
    tid: video.tid,
    ownerName: video.ownerName,
  };
}

export const sources = {
  async search(query: string, signal: AbortSignal): Promise<SongCandidate[]> {
    const response = await getSongByNetease({ s: query, type: 1, limit: 20, offset: 0 }, signal);
    if (response.code !== 200) throw new Error(`Netease search: ${response.code}`);
    return (response.result?.songs ?? []).flatMap(song => {
      if (!song.id || !song.name || !song.duration) return [];
      return [
        {
          id: song.id,
          name: song.name,
          artists: (song.artists ?? []).flatMap(artist => (artist.name ? [artist.name] : [])),
          duration: song.duration / 1000,
          aliases: [...(song.alias ?? []), ...(song.transNames ?? [])],
        },
      ];
    });
  },

  async lyric(songId: number, signal: AbortSignal) {
    const response = await getLyricsByNetease({ id: songId }, signal);
    if (response.code !== 200) throw new Error(`Netease lyric: ${response.code}`);
    return { lrc: response.lrc?.lyric ?? "", tlyric: response.tlyric?.lyric, romalrc: response.romalrc?.lyric };
  },

  async view(bvid: string, signal: AbortSignal) {
    const response = await getWebInterfaceView({ bvid }, signal);
    if (response.code !== 0 || !response.data) throw new Error(`Bilibili view: ${response.code} ${response.message}`);
    const data = response.data;
    return {
      bvid: data.bvid,
      cid: data.cid,
      title: data.title,
      duration: data.duration,
      tid: data.tid,
      ownerName: data.owner.name,
      pages: data.pages,
    };
  },

  async params(bvid: string, cid: number, signal: AbortSignal) {
    return pageParams(await sources.view(bvid, signal), cid);
  },

  async references(song: SongCandidate, signal: AbortSignal): Promise<ReferenceCandidate[]> {
    const cookie = await getCookieString();
    signal.throwIfAborted();
    for (let attempt = 0; attempt <= 2; attempt++) {
      const params = await encodeParamsWbi(
        {
          search_type: "video",
          keyword: `${song.name} ${song.artists[0] ?? ""}`,
          page: 1,
          page_size: 50,
          order: "totalrank",
        },
        cookie,
        signal,
      );
      const response = searchResponse.parse(
        await got
          .get("https://api.bilibili.com/x/web-interface/wbi/search/type", {
            searchParams: params,
            signal,
            timeout: { request: 15000 },
            retry: { limit: 0 },
            headers: { Cookie: cookie, Referer: "https://www.bilibili.com/", "User-Agent": UserAgent },
          })
          .json<unknown>(),
      );
      if (response.data?.v_voucher) {
        if (attempt === 2) throw new Error("Bilibili search: v_voucher after 2 retries");
        await setTimeout(500 * 2 ** attempt, undefined, { signal });
        continue;
      }
      if (response.code !== 0) throw new Error(`Bilibili search: ${response.code} ${response.message ?? ""}`);
      return rankReferences(
        (response.data?.result ?? []).map(item => ({
          bvid: item.bvid,
          title: item.title.replace(/<[^>]*>/g, ""),
          duration: durationSeconds(item.duration),
        })),
        song.duration,
      );
    }
    return [];
  },

  async audio(bvid: string, cid: number | undefined, signal: AbortSignal): Promise<Uint8Array> {
    const resolvedCid = cid ?? (await sources.view(bvid, signal)).cid;
    const response = await getDashurl({ bvid, cid: resolvedCid, fnval: 16, fnver: 0, qn: 16 }, signal);
    if (response.code !== 0 || response.data?.v_voucher)
      throw new Error(
        `Bilibili playurl: ${response.code} ${response.data?.v_voucher ? "v_voucher" : response.message}`,
      );
    const audio = response.data?.dash?.audio?.toSorted((a, b) => a.bandwidth - b.bandwidth)[0];
    if (!audio) throw new Error("Bilibili playurl: no DASH audio");
    return got
      .get(audio.baseUrl || audio.base_url, {
        signal,
        timeout: { request: 30000 },
        retry: { limit: 1 },
        headers: { Referer: `https://www.bilibili.com/video/${bvid}`, "User-Agent": UserAgent },
      })
      .buffer();
  },
};

export default sources;
