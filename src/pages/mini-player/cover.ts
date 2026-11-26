import { formatUrlProtocol } from "@/common/utils/url";

/** 迷你封面只有 100px 宽，B 站图床按 240px 方图裁剪即可 */
const MINI_COVER_PARAMS = "240w_240h_1c.webp";

/** B 站 `/bfs/` 图床地址追加缩放参数；已带参数或非 B 站图片原样返回 */
export function getMiniCoverSrc(src?: string) {
  const url = formatUrlProtocol(src);
  if (!url) return url;
  return url.includes("/bfs/") && !url.includes("@") ? `${url}@${MINI_COVER_PARAMS}` : url;
}
