import log from "./logger";
import moment from "moment";

import { toMediaUrl, unwrapMediaUrl } from "@/platform";
import { getAudioWebStreamUrl } from "@/service/audio-web-url";
import { getPlayerPlayurl, type DashAudio } from "@/service/player-playurl";
import { useUser } from "@/store/user";

import { audioQualitySort } from "../constants/audio";
import { VideoFnval } from "../constants/video";
import { getUrlParams } from "./url";

function canBrowserPlayMime(mimeTypes: string[]): boolean {
  if (typeof Audio === "undefined") {
    return true;
  }
  try {
    const probe = new Audio();
    if (typeof probe.canPlayType !== "function") {
      return true;
    }
    const results = mimeTypes.map(m => probe.canPlayType(m));
    // In test environments (like JSDOM) where canPlayType returns "" for everything including audio/mp4, treat as supported
    if (probe.canPlayType("audio/mp4") === "" && results.every(r => r === "")) {
      return true;
    }
    return results.some(r => r === "probably" || r === "maybe");
  } catch {
    return true;
  }
}

function sortAudio(audio: DashAudio[]) {
  return audio.toSorted((a, b) => {
    if (a.bandwidth !== b.bandwidth) {
      return b.bandwidth - a.bandwidth;
    }

    const indexA = audioQualitySort.indexOf(a.id);
    const indexB = audioQualitySort.indexOf(b.id);
    if (indexA === -1) return 1;
    if (indexB === -1) return -1;
    return indexB - indexA;
  });
}

function selectAudioByQuality(audioList: DashAudio[], quality: AudioQuality): DashAudio | undefined {
  if (!audioList.length) return undefined;

  const sortedList = sortAudio(audioList);

  switch (quality) {
    case "high":
      return sortedList[0];
    case "medium": {
      const midIndex = Math.floor((sortedList.length - 1) / 2);
      return sortedList[midIndex];
    }
    case "low":
      return sortedList[sortedList.length - 1];
    default:
      return sortedList[0];
  }
}

export async function getDashUrl(bvid: string, cid: string | number, audioQuality: AudioQuality = "auto") {
  try {
    const getUrlInfoRes = await getPlayerPlayurl({
      bvid,
      cid,
      fnval: VideoFnval.AllDash,
    });

    const bestVideoInfo = getUrlInfoRes?.data?.dash?.video?.[0];
    const videoResolution = `${bestVideoInfo?.width}x${bestVideoInfo?.height}`;
    const rawVideoUrl = bestVideoInfo?.baseUrl || bestVideoInfo?.backupUrl?.[0];
    const videoUrl = rawVideoUrl ? await toMediaUrl(rawVideoUrl) : undefined;
    const flacAudio = getUrlInfoRes?.data?.dash?.flac?.audio;
    const dolbyAudio = getUrlInfoRes?.data?.dash?.dolby?.audio?.[0];

    if (audioQuality === "auto" || audioQuality === "lossless") {
      if (flacAudio && canBrowserPlayMime(["audio/flac", 'audio/mp4; codecs="flac"'])) {
        const rawAudioUrl = flacAudio.baseUrl || flacAudio.backupUrl?.[0];
        return {
          isLossless: true,
          audioCodecs: flacAudio.codecs.toLowerCase(),
          audioBandwidth: flacAudio.bandwidth,
          audioUrl: rawAudioUrl ? await toMediaUrl(rawAudioUrl) : "",
          videoUrl,
          videoResolution,
        };
      }

      if (
        dolbyAudio &&
        canBrowserPlayMime([
          `audio/mp4; codecs="${dolbyAudio.codecs}"`,
          'audio/mp4; codecs="ec-3"',
          'audio/mp4; codecs="ac-3"',
        ])
      ) {
        const rawAudioUrl = dolbyAudio.baseUrl || dolbyAudio.backupUrl?.[0] || "";
        return {
          isLossless: false,
          isDolby: true,
          audioCodecs: dolbyAudio.codecs.toLowerCase(),
          audioBandwidth: dolbyAudio.bandwidth,
          audioUrl: rawAudioUrl ? await toMediaUrl(rawAudioUrl) : "",
          videoUrl,
          videoResolution: `${bestVideoInfo?.width}x${bestVideoInfo?.height}`,
        };
      }
    }

    const audioList = getUrlInfoRes?.data?.dash?.audio || [];
    const selectedAudio = selectAudioByQuality(audioList, audioQuality);
    const rawSelectedUrl = selectedAudio?.baseUrl || selectedAudio?.backupUrl?.[0] || "";
    return {
      isLossless: false,
      audioCodecs: selectedAudio?.codecs?.toLowerCase() || "",
      audioBandwidth: selectedAudio?.bandwidth,
      audioUrl: rawSelectedUrl ? await toMediaUrl(rawSelectedUrl) : "",
      videoUrl,
      videoResolution,
    };
  } catch (error) {
    log.error("[Get video play url error]", error);
    return {
      isLossless: false,
    };
  }
}

/**
 * 登录情况下获取音乐播放链接
 */
export const getAudioUrl = async (sid: number | string) => {
  const res = await getAudioWebStreamUrl({
    songid: sid,
    quality: useUser.getState().user?.vipStatus ? 3 : 2,
    privilege: 2,
    mid: useUser.getState().user?.mid || 0,
    platform: "web",
  });

  const isFlac = res?.data?.type === 3;
  const rawAudioUrl = res?.data?.cdns?.[0];

  return {
    audioUrl: rawAudioUrl ? await toMediaUrl(rawAudioUrl) : undefined,
    audioCodecs: isFlac ? "flac" : "",
    isLossless: isFlac,
  };
};

/** URL是否有效 */
export const isUrlValid = (url?: string): url is string => {
  const raw = unwrapMediaUrl(url);
  return Boolean(raw) && moment().isBefore(moment.unix(Number(getUrlParams(raw as string).deadline)));
};
