export const getUrlParams = (url: string) => {
  const urlParams = new URLSearchParams(url.split("?")[1]);
  return Object.fromEntries(urlParams.entries());
};

// Bilibili returns covers as "//host/..." or "http://host/...". WKWebView blocks
// http subresources on the tauri:// origin, so both forms are upgraded to https.
export const formatUrlProtocol = (url?: string) => {
  if (url?.startsWith("//")) {
    return `https:${url}`;
  }

  if (url?.startsWith("http://")) {
    return `https://${url.slice("http://".length)}`;
  }

  return url;
};

const getBiliVideoLink = (data: { type: "mv" | "audio"; bvid?: string; sid?: string | number; pageIndex?: number }) => {
  return `https://www.bilibili.com/${data?.type === "mv" ? `video/${data?.bvid}${(data.pageIndex ?? 0) > 1 ? `?p=${data.pageIndex}` : ""}` : `audio/au${data?.sid}`}`;
};

export const openBiliVideoLink = (data: {
  type: "mv" | "audio";
  bvid?: string;
  sid?: string | number;
  pageIndex?: number;
}) => {
  window.electron.openExternal(getBiliVideoLink(data));
};
