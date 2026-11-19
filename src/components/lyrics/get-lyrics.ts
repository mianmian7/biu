import { getLyric } from "@/service/ai-lyrics";
import { getWebPlayerInfo, type WebPlayerParams } from "@/service/web-player";

export async function getLyricsByBili(params: WebPlayerParams) {
  const res = await getWebPlayerInfo(params);
  const subTitleUrl =
    res?.data?.subtitle?.subtitles?.[0]?.subtitle_url || res?.data?.subtitle?.subtitles?.[0]?.subtitle_url_v2;

  if (subTitleUrl) {
    const getLyricsRes = await getLyric(subTitleUrl);
    const lines =
      getLyricsRes?.body
        ?.map(item => {
          const raw = item.content ?? "";
          const cleaned = raw.replace(/^[♪♫]+|[♪♫]+$/g, "").trim();
          return {
            time: Math.max(0, Math.round((item.from ?? 0) * 1000)),
            endTime: typeof item.to === "number" && Number.isFinite(item.to) ? Math.round(item.to * 1000) : undefined,
            text: cleaned,
          };
        })
        .filter(item => item.text)
        .toSorted((a, b) => a.time - b.time) ?? [];
    let latestEnd = -Infinity;
    return lines.map((line, index) => {
      const next = lines[index + 1];
      const { endTime, ...rest } = line;
      if (endTime !== undefined && endTime > line.time) latestEnd = Math.max(latestEnd, endTime);
      return latestEnd > line.time && (!next || next.time - latestEnd >= 1000) ? { ...rest, endTime: latestEnd } : rest;
    });
  }

  return null;
}
