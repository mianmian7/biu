/** Rule-only port of the archive's song identification. All durations are seconds. */
import { buildSearchQueries } from "./title-candidates";

export interface IdentifyInput {
  title: string;
  pageTitle?: string;
  duration: number;
  tid: number;
  ownerName: string;
}

export interface SongCandidate {
  id: number;
  name: string;
  artists: string[];
  duration: number;
  aliases?: string[];
}

export type Identification =
  | { status: "matched"; song: SongCandidate; confidence: number }
  | { status: "unidentified" | "not-music" | "collection"; confidence: number };

export const LIVE = /(?<![a-z])(?:live|cover)(?![a-z])|现场|現場|翻唱|カバー|ライブ|演唱[会會]/i;
export const WRONG_VERSION =
  /(?<![a-z])(?:instrumental|remix|off[\s_-]*vocal|karaoke)(?![a-z])|伴奏|纯音乐|純音楽|カラオケ|ニコカラ/i;
export const COLLECTION = /合集|全集|串烧|串燒|メドレー|全曲|(?<![a-z])(?:medley|playlist)(?![a-z])/i;
export const NON_SONG = /教程|教学|教學|乐谱|樂譜|总谱|總譜|贝斯谱|貝斯譜|吉他谱|指北|(?<![a-z])tab(?:谱)?(?![a-z])/i;
export const NOISE =
  /(?<![a-z])(?:official|music\s*video|video|audio|lyrics?|mv|pv|hd|4k|1080p|hires|hi-res|live|cover)(?![a-z])|官方(?:音源|音乐|音樂)?|中字|中日字幕|双语字幕|双语|无损|高音质|高音質|现场版?|現場版?|翻唱|完整版|单曲循环|纯享版|字幕版|伴奏/gi;
export const BRACKETS = /[（(【[「『《〈]([^（）()【】[\]「」『』《》〈〉]*)[）)】\]」』》〉]/g;
export const SHOW_NOISE =
  /(?<![\p{L}\p{N}_])(?:M-\d+|#\d+|No\.\s*\d+|Vo\.\s*\S+|\d+(?:st|nd|rd|th)\s*(?:live|one-?man|tour)?|day\s*\d+)/giu;
export const QUERY_SPLIT = /[／/|｜\-【】「」\s]+/;
export const VERSION =
  /(?<![a-z])(?:ver\.?|version|edit|feat\.?|with|duet|tv\s*size)(?![a-z])|合唱|版本?|ヴァージョン|バージョン|デュエット|[&/×、]/i;

const MUSIC_TIDS = new Set([3, 5, 6, 28, 29, 30, 31, 59, 130, 193, 243, 244, 265, 266, 267, 268, 1003]);
const MUSIC_TITLE =
  /(?<![a-z])(?:mv|pv|music|song|audio|lyrics?|tv\s*size|op|ed)(?![a-z])|原曲|歌[曲词詞]|音[乐樂源]|罗马音|假名|唱|踊ってみた/i;

export function normalize(text: string): string {
  // Unicode NFKC + case folding (the explicit folds cover nontrivial song/name cases).
  return text
    .normalize("NFKC")
    .toLowerCase()
    .replace(/ß/g, "ss")
    .replace(/ς/g, "σ")
    .replace(/[^\p{L}\p{N}]/gu, "");
}

const trimNoise = (text: string) => text.replace(/^[\s\-_|/·:：]+|[\s\-_|/·:：]+$/g, "");

export function clean(text: string): string {
  return trimNoise(
    text
      .normalize("NFKC")
      .replace(/BV[0-9A-Za-z]{10}|\bP\d+\b/gi, " ")
      .replace(NOISE, " ")
      .replace(/[【】[\]「」『』《》〈〉()]/g, " ")
      .replace(/\s+/g, " "),
  );
}

export function titleVariants(title: string, artists: readonly string[] = []): string[] {
  title = title.normalize("NFKC").trim();
  const suffix = /\s*\(([^()]*)\)\s*$/.exec(title);
  const core =
    suffix && (VERSION.test(suffix[1]) || artists.some(a => normalize(a) === normalize(suffix[1])))
      ? title.slice(0, suffix.index).trim()
      : title;
  return [...new Set([title, core].filter(Boolean))];
}

export function songText(videoTitle: string, pageTitle = ""): string {
  const page = clean(pageTitle);
  return page && !["正片", "字幕", "full", "本篇"].includes(normalize(page)) && !/^\d+$/.test(page)
    ? pageTitle
    : videoTitle;
}

export function identifyPrimaryQueries(input: IdentifyInput): string[] {
  if (classify(input)) return [];
  const text = songText(input.title, input.pageTitle);
  return [
    ...new Set([clean(text), clean(input.title), ...clean(text).split(/\s+[-|/]\s+/)].filter(q => normalize(q))),
  ].slice(0, 3);
}

/** Initial queries followed by the archive's fallback query fragments. */
export function identifyQueries(input: IdentifyInput): string[] {
  if (classify(input)) return [];
  const text = songText(input.title, input.pageTitle);
  const initial = identifyPrimaryQueries(input);
  const cleaned = trimNoise(
    clean(text)
      .replace(SHOW_NOISE, " ")
      .replace(/[／/|｜]\s*[a-zA-Z0-9\s._-]*$/, " ")
      .replace(/\s+/g, " "),
  );
  const pieces = cleaned
    .split(QUERY_SPLIT)
    .filter(p => /[\u3040-\u30ff\u3400-\u9fff]/.test(p) && normalize(p).length >= 3)
    .sort((a, b) => normalize(b).length - normalize(a).length);
  const fallback = [...new Set([cleaned, ...pieces, clean(input.title)].filter(q => normalize(q)))].slice(0, 6);
  // Quoted names like 《雨爱》 that the archive rules drop (CJK pieces shorter than 3 chars).
  const borrowed = buildSearchQueries(text).slice(0, 3);
  return [...new Set([...initial, ...fallback, ...borrowed])];
}

export function nameForms(text: string): Set<string> {
  text = text.normalize("NFKC").trim();
  const forms = [text, ...text.split(/[/／×&、,]/)];
  for (const raw of [...forms]) {
    const part = raw.trim();
    if (!part.endsWith(")")) continue;
    let depth = 0;
    for (let i = part.length - 1; i >= 0; i--) {
      if (part[i] === ")") depth++;
      else if (part[i] === "(" && --depth === 0) {
        forms.push(part.slice(0, i).trim(), part.slice(i + 1, -1).trim());
        break;
      }
    }
  }
  return new Set(forms.map(normalize).filter(Boolean));
}

export function sameName(a: string, b: string): boolean {
  const names = nameForms(b);
  return [...nameForms(a)].some(name => names.has(name));
}

export function songTitles(song: SongCandidate): string[] {
  return [...new Set([...titleVariants(song.name, song.artists), ...(song.aliases ?? []).filter(a => a.trim())])];
}

export function strongMatch(title: string, text: string): boolean {
  const key = normalize(title);
  if (!key) return false;
  if ([...title].every(char => char.charCodeAt(0) <= 127)) {
    const words =
      title
        .normalize("NFKC")
        .toLowerCase()
        .match(/[a-z0-9]+/g) ?? [];
    if (!words.length) return false;
    const pattern = `(?<![a-z0-9])${words.join("[\\W_]*")}(?![a-z0-9])`;
    if (!new RegExp(pattern, "i").test(text.normalize("NFKC"))) return false;
    // A one-word title must not be the first word of a longer English name: GIRLS vs "Girls Band Cry".
    return words.length > 1 || new RegExp(`${pattern}(?![\\t ]+[a-z])`, "i").test(clean(text));
  }
  if (key.length < 3) {
    return [...text.matchAll(BRACKETS)]
      .map(m => m[1])
      .concat(text.split(/[\s/|:：・—–-]+/))
      .some(span => normalize(span) === key);
  }
  return normalize(text).includes(key);
}

interface RuleMatch {
  song: SongCandidate;
  title: string;
  artistMatch: boolean;
}

export function ruleMatches(
  songs: readonly SongCandidate[],
  text: string,
  context: string,
  extra = false,
): RuleMatch[] {
  const matches: RuleMatch[] = [];
  for (const song of songs.slice(0, 10)) {
    if (
      !song.id ||
      !song.artists.length ||
      WRONG_VERSION.test(song.name) ||
      LIVE.test(song.name) ||
      !Number.isFinite(song.duration) ||
      song.duration <= 0 ||
      song.duration > 1200
    )
      continue;
    const title = songTitles(song)
      .reverse()
      .find(t => strongMatch(t, text));
    if (!title) continue;
    const escaped = title.normalize("NFKC").replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    if (extra && new RegExp(`${escaped}-[A-Za-z0-9]`).test(text.normalize("NFKC"))) continue;
    matches.push({ song, title, artistMatch: song.artists.some(a => strongMatch(a, context)) });
  }
  return matches;
}

/** Ambiguous identities are rejected; artist aliases may connect otherwise equal titles. */
export function chooseRule(candidates: readonly RuleMatch[], duration: number): RuleMatch | undefined {
  if (!candidates.length || new Set(candidates.map(c => normalize(c.title))).size > 1) return;
  const explicit = candidates.filter(c => c.artistMatch);
  const remaining = [...(explicit.length ? explicit : candidates)];
  const group = remaining.splice(0, 1);
  let changed = true;
  while (changed) {
    changed = false;
    for (let i = remaining.length - 1; i >= 0; i--) {
      const candidate = remaining[i];
      if (group.some(c => c.song.artists.some(a => candidate.song.artists.some(b => sameName(a, b))))) {
        group.push(...remaining.splice(i, 1));
        changed = true;
      }
    }
  }
  if (remaining.length) return;
  return group.sort(
    (a, b) => Math.abs(a.song.duration - duration) - Math.abs(b.song.duration - duration) || a.song.id - b.song.id,
  )[0];
}

function classify(input: IdentifyInput): "not-music" | "collection" | undefined {
  const context = `${input.title}\n${input.pageTitle ?? ""}`;
  if (NON_SONG.test(context) || WRONG_VERSION.test(context)) return "not-music";
  if (COLLECTION.test(songText(input.title, input.pageTitle))) return "collection";
  if (!MUSIC_TIDS.has(input.tid) && !MUSIC_TITLE.test(context) && !LIVE.test(context)) return "not-music";
}

export function identifyRules(
  input: IdentifyInput,
  candidates: readonly SongCandidate[],
  extra = false,
): Identification {
  const status = classify(input);
  if (status) return { status, confidence: 1 };
  if (!Number.isFinite(input.duration) || input.duration <= 0 || input.duration > 1200)
    return { status: "unidentified", confidence: 0 };
  const text = songText(input.title, input.pageTitle);
  // UP names are metadata, not evidence that the uploader is the original artist.
  const context = `${input.title}\n${input.pageTitle ?? ""}`;
  const matches = candidates.flatMap(song => ruleMatches([song], text, context, extra));
  const unique = [...new Map(matches.map(c => [c.song.id, c])).values()];
  const chosen = chooseRule(unique, input.duration);
  return chosen
    ? { status: "matched", song: chosen.song, confidence: chosen.artistMatch ? 0.95 : 0.8 }
    : { status: "unidentified", confidence: 0 };
}
