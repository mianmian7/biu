/**
 * Song-name candidates from messy Bilibili titles, used only as extra search queries.
 * Pure functions copied from wood3n/biu PR #263 (heavendreamer, src/common/utils/lyric-match.ts):
 * cleanTrackTitle / buildTitleCandidates / buildSearchQueries. Scoring is not copied; identify.ts
 * still decides which search result is accepted.
 */

const DECORATION_PATTERN =
  /(翻唱|cover|高音质|无损|纯音乐|伴奏|官方|字幕|中字|完整版|动态歌词|歌词版|hi-?res|4k|1080p|720p|60fps|合集|联投|第\s*\d+\s*[集话期]|p\s*\d+|full\s*ver|music\s*video|现场|电台|live)/i;

const DECORATION_TOKENS =
  /(中日双语版?|双语版?|中文版|日文版|完整版|高清修复版?|修复版|超清|高清|高音质版?|无损音质|无损版?|纯享版?|男声版?|女声版?|合唱版?|ai翻唱|ai声线|伴奏版?|消音|和声版?|纯音乐|钢琴版?|吉他版?|8d环绕|环绕版?|双声道|全景声|杜比|母带|现场版?|试听版?|试听|电台版?|合集版?|剪辑版?|混剪|串烧|连唱|片段|副歌|车载|助眠|学习|专注|背景音乐|bgm|dj版?|慢速|加速|动态歌词|歌词版?|主题曲|剧情插曲|插曲|片尾曲|片头曲|官方版|变奏版|加长版|附歌词|中英歌词|中日歌词|中文字幕|录音棚|百万级|提取版|游戏版?|music\s*video|full\s*ver)/gi;

const QUOTE_PATTERN =
  /《([^》]+)》|〈([^〉]+)〉|【([^】]+)】|「([^」]+)」|『([^』]+)』|[“"]([^”"]+)[”"]|（([^）]+)）|\(([^)]+)\)|\[([^\]]+)\]/g;

const ALBUM_PATTERN = /(ost|soundtrack|原声|原声带|专辑|精选集|音乐集)/i;

const GENERIC_CANDIDATE_PATTERN =
  /^(bgm|ost|ep|op|ed|pv|mv|demo|remix|intro|outro|instrumental|i|ii|iii|iv|v|vi|vii|viii|ix|x|from|with|the|and|for|you|me|my|your|our|this|that|ver|version|feat|ft|full|part|pt|original|audio|video|song|track|theme|boss战?|战斗曲?|决战|序章|序曲|终章|插曲|主题曲|片尾曲|片头曲|前奏|间奏|尾声|伴奏|纯音乐|试听|完整版|加长版|变奏版|变奏|现场版|电台|合集|片段|剪辑|剧情|场景|鉴赏|纪念|庆典|动画|预告|主题|音乐|游戏|官方|原唱|翻唱|歌词|字幕|中字|双语|国语|日语|英语)$/i;

const BRACKET_PATTERN = /[【[（(〔［｛]([^】\]）)〕］｝]*)[】\]）)〕］｝]/g;
const SEPARATOR_PATTERN = /\s*[-–—－_|｜│┃/／·・~～&＆、]\s*/;
const SEPARATOR_PATTERN_ALL = new RegExp(SEPARATOR_PATTERN.source, "g");
const TOPIC_TAG_PATTERN = /[#＃][^\s#＃]+/g;

function normalizeForCompare(text?: string): string {
  if (!text) return "";
  return text
    .toLowerCase()
    .replace(/[\s\u3000]+/g, "")
    .replace(/[\u0021-\u002f\u003a-\u0040\u005b-\u0060\u007b-\u007e]/g, "")
    .replace(/[，。！？、；：“”‘’（）《》【】…—～·]/g, "");
}

export function cleanTrackTitle(raw?: string): string {
  if (!raw) return "";
  return raw
    .replace(TOPIC_TAG_PATTERN, " ")
    .replace(BRACKET_PATTERN, (_match, inner: string) => {
      const content = String(inner ?? "").trim();
      if (!content) return " ";
      return DECORATION_PATTERN.test(content) ? " " : ` ${content} `;
    })
    .replace(DECORATION_TOKENS, " ")
    .replace(/[《》「」『』“”"]/g, " ")
    .replace(/[_|｜]/g, " ")
    .replace(/\s+/g, " ")
    .trim();
}

/** 《鸣潮 3.2 OST》 -> 鸣潮; plain 《歌名》 is not a series. */
function getSeriesFingerprints(title: string): string[] {
  const fingerprints: string[] = [];
  for (const match of title.matchAll(QUOTE_PATTERN)) {
    const content = match.slice(1).find(Boolean)?.trim();
    if (!content || !ALBUM_PATTERN.test(content)) continue;
    const normalized = normalizeForCompare(content).replace(/(ost|soundtrack|原声带?|专辑|精选集|音乐集)/gi, "");
    const fingerprint = (normalized.match(/^[\u4e00-\u9fa5a-z]+/)?.[0] ?? "").slice(0, 4);
    if (fingerprint.length >= 2) fingerprints.push(fingerprint);
  }
  return [...new Set(fingerprints)];
}

/** Quoted text first, then separator pieces, then the whole cleaned title. */
export function buildTitleCandidates(raw?: string): string[] {
  if (!raw) return [];
  const candidates: string[] = [];
  const seriesFingerprints = getSeriesFingerprints(raw);
  const push = (value?: string) => {
    const text = value?.trim();
    if (!text || DECORATION_PATTERN.test(text) || GENERIC_CANDIDATE_PATTERN.test(text) || candidates.includes(text))
      return;
    if (/^\d+(?:[.\-_]\d+)*$/.test(text)) return;
    const normalized = normalizeForCompare(text);
    if (
      seriesFingerprints.some(
        fingerprint => normalized === fingerprint || (normalized.length >= 2 && fingerprint.startsWith(normalized)),
      )
    )
      return;
    candidates.push(text);
  };
  const pushSegments = (source: string) => {
    const cleaned = cleanTrackTitle(source);
    cleaned
      .replace(SEPARATOR_PATTERN_ALL, " ")
      .split(/\s+/)
      .filter(part => part.trim().length >= 2)
      .forEach(push);
    cleaned.split(SEPARATOR_PATTERN).forEach(push);
    push(cleaned);
  };
  const quotes: { content: string; end: number }[] = [];
  for (const match of raw.matchAll(QUOTE_PATTERN)) {
    const content = match.slice(1).find(Boolean)?.trim();
    if (content) quotes.push({ content, end: (match.index ?? 0) + match[0].length });
  }
  const albumQuote = quotes.find(quote => ALBUM_PATTERN.test(quote.content));
  if (albumQuote) pushSegments(raw.slice(albumQuote.end));
  quotes.forEach(quote => push(quote.content));
  pushSegments(raw);
  return candidates;
}

/** e.g. "中日双语版《雨爱》老头子携影分身#合唱" -> ["雨爱", ...] */
export function buildSearchQueries(title: string): string[] {
  return [...new Set(buildTitleCandidates(title).filter(text => text.length <= 28))].slice(0, 6);
}
