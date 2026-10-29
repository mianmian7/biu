import got from "got";

import { UserAgent } from "../../network/user-agent";

export function getSongByNetease(params: SearchSongByNeteaseParams, signal?: AbortSignal) {
  return got
    .get("https://interface.music.163.com/api/search/get", {
      signal,
      searchParams: {
        ...params,
      },
      timeout: { request: 10000 },
      retry: { limit: 3 },
      headers: {
        Referer: "https://music.163.com/",
        origin: "https://music.163.com",
        "user-agent": UserAgent,
      },
    })
    .json<SearchSongByNeteaseResponse>();
}

export function getLyricsByNetease(params: GetLyricsByNeteaseParams, signal?: AbortSignal) {
  return got
    .get("https://interface.music.163.com/api/song/lyric", {
      signal,
      searchParams: {
        ...params,
        tv: -1,
        lv: -1,
        rv: -1,
        kv: -1,
        _nmclfl: 1,
      },
      timeout: { request: 10000 },
      retry: { limit: 3 },
      headers: {
        Referer: "https://music.163.com/",
        origin: "https://music.163.com",
        "user-agent": UserAgent,
      },
    })
    .json<GetLyricsByNeteaseResponse>();
}
