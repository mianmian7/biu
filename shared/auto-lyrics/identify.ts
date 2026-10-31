/** Rule-only port of the archive's song identification. All durations are seconds. */
import { romanizeKana } from "./kana";
import { buildSearchQueries, GENERIC_CANDIDATE_PATTERN } from "./title-candidates";

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
  /** `alternates`: other releases of the song by the same artist, tried when `song` has no usable reference. */
  | { status: "matched"; song: SongCandidate; confidence: number; alternates?: SongCandidate[] }
  /** Rules cannot tell these songs apart (or the video may not be music); alignment must arbitrate. */
  | { status: "ambiguous"; contenders: SongCandidate[]; confidence: number }
  | { status: "unidentified" | "not-music" | "collection"; confidence: number };

export const LIVE = /(?<![a-z])(?:live|cover)(?![a-z])|现场|現場|音乐节|音樂節|翻唱|カバー|ライブ|演唱[会會]/i;
const COVERED = /(?<![a-z])cover(?![a-z])|翻唱|カバー/i;
export const WRONG_VERSION =
  /(?<![a-z])(?:instrumental|remix|off[\s_-]*vocal|karaoke)(?![a-z])|伴奏|纯音乐|純音楽|カラオケ|ニコカラ|填词|填詞|改词|改詞|替え歌/i;
export const COLLECTION = /合集|全集|串烧|串燒|メドレー|全曲|(?<![a-z])(?:medley|playlist)(?![a-z])/i;
export const NON_SONG = /教程|教学|教學|乐谱|樂譜|总谱|總譜|贝斯谱|貝斯譜|吉他谱|指北|(?<![a-z])tab(?:谱)?(?![a-z])/i;
export const NOISE =
  /(?<![a-z])(?:official|music\s*video|video|audio|lyrics?|mv|pv|hd|4k|1080p|hires|hi-res|live|cover)(?![a-z])|官方(?:音源|音乐|音樂)?|中字|中日字幕|双语字幕|双语|无损|高音质|高音質|现场版?|現場版?|翻唱|完整版|单曲循环|纯享版|字幕版|伴奏/gi;
export const BRACKETS = /[（(【[「『《〈]([^（）()【】[\]「」『』《》〈〉]*)[）)】\]」』》〉]/g;
export const SHOW_NOISE =
  /(?<![\p{L}\p{N}_])(?:M-\d+|#\d+|No\.\s*\d+|Vo\.\s*\S+|\d+(?:st|nd|rd|th)\s*(?:live|one-?man|tour)?|day\s*\d+)/giu;
const QUOTED = /《[^》]+》|「[^」]+」|『[^』]+』/;
export const QUERY_SPLIT = /[／/|｜\-【】「」\s]+/;
export const VERSION =
  /(?<![a-z])(?:ver\.?|version|edit|feat\.?|with|duet|tv\s*size)(?![a-z])|合唱|版本?|ヴァージョン|バージョン|デュエット|[&/×、]/i;

const PERFORMANCE_TID = 59;
const SUNG = /唱|(?<![a-z])vocals?(?![a-z])|歌ってみた/i;
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

/** Text after a full stop is commentary: "爆ぜて咲く。现场互动拉满…" searches as "爆ぜて咲く". */
const dropChatter = (text: string) => trimNoise(text.replace(/(?<=\S)\s*。[\s\S]*$/, ""));

/** Only songs are searched; a "weak" video is searched and then confirmed by alignment. */
const searchable = (input: IdentifyInput) => [undefined, "weak"].includes(classify(input));

/** Setlist marks are not part of the name: "EN2 名もなき何もかも(reprise)-トゲナシトゲアリ | 1st". */
const withoutShowNoise = (text: string) => trimNoise(text.replace(SHOW_NOISE, " ").replace(/\s+/g, " "));

export function identifyPrimaryQueries(input: IdentifyInput): string[] {
  if (!searchable(input)) return [];
  const raw = songText(input.title, input.pageTitle);
  const text = withoutShowNoise(dropChatter(clean(raw)));
  // A quoted name is the strongest hint: [4k60帧/live优化]「空の箱」-トゲナシトゲアリ.
  const quoted = QUOTED.exec(raw)?.[0].slice(1, -1).trim() ?? "";
  const pieces = text.split(/\s+[-|｜/]\s*|\s*[-|｜/]\s+/);
  const queries = [text, quoted, withoutShowNoise(dropChatter(clean(input.title))), ...pieces];
  return [...new Set(queries.filter(q => normalize(clean(q))))].slice(0, 3);
}

/** Initial queries followed by the archive's fallback query fragments. */
export function identifyQueries(input: IdentifyInput): string[] {
  if (!searchable(input)) return [];
  const text = songText(input.title, input.pageTitle);
  const initial = identifyPrimaryQueries(input);
  const cleaned = trimNoise(
    dropChatter(clean(text))
      .replace(SHOW_NOISE, " ")
      .replace(/[／/|｜]\s*[a-zA-Z0-9\s._-]*$/, " ")
      .replace(/\s+/g, " "),
  );
  const pieces = cleaned
    .split(QUERY_SPLIT)
    .filter(p => /[\u3040-\u30ff\u3400-\u9fff]/.test(p) && normalize(p).length >= 3)
    .sort((a, b) => normalize(b).length - normalize(a).length);
  const fallback = [...new Set([cleaned, ...pieces, clean(input.title)].filter(q => normalize(q)))].slice(0, 6);
  // A part named only by its song is searched with the video title, which often names the artist.
  const withVideo = input.pageTitle ? [`${clean(input.pageTitle)} ${clean(input.title)}`] : [];
  // Quoted names like 《雨爱》 that the archive rules drop (CJK pieces shorter than 3 chars).
  const borrowed = buildSearchQueries(text).slice(0, 3);
  return [...new Set([...initial, ...fallback, ...borrowed, ...withVideo])];
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
  // NetEase annotates aliases: 无需既定的未来（少女乐队的呐喊 剧场版…主题曲）.
  const aliases = (song.aliases ?? []).flatMap(a => [a, a.replace(/\s*[（(][^（）()]*[）)]\s*$/, "")]);
  // An alias that is only display noise (完整版) or a role (插曲) names nothing.
  const named = aliases.filter(a => normalize(clean(a)) && !GENERIC_CANDIDATE_PATTERN.test(a.trim()));
  return [...new Set([...titleVariants(song.name, song.artists), ...named])];
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
    // Quoting marks the name's end: 「arrow」TOGENASHI TOGEARI.
    return (
      words.length > 1 ||
      [...text.matchAll(BRACKETS)].some(m => normalize(m[1]) === key) ||
      new RegExp(`${pattern}(?![\\t ]+[a-z])`, "i").test(clean(text))
    );
  }
  if (key.length < 3) {
    return (
      [...text.matchAll(BRACKETS)]
        .map(m => m[1])
        // Brackets end a span too: 【Official Music Video】栞 （Shiori）.
        .concat(text.split(/[\s/|:：・—–\-【】[\]（）()「」『』《》]+/))
        .some(span => normalize(span) === key)
    );
  }
  return normalize(text).includes(key);
}

/** A trailing credit such as 視界の隅 朽ちる音 (新川崎(仮)), including nested brackets. */
export function withoutTrailingNote(title: string): string {
  title = title.normalize("NFKC").trim();
  if (!title.endsWith(")")) return title;
  let depth = 0;
  for (let i = title.length - 1; i > 0; i--) {
    if (title[i] === ")") depth++;
    else if (title[i] === "(" && --depth === 0) return title.slice(0, i).trim() || title;
  }
  return title;
}

/** A release note (中文版, guitar, パラレルver.) the video does not mention: 影色舞(中文版) for a video of 影色舞. */
export function unmentionedNote(name: string, context: string): boolean {
  const note = normalize(name.normalize("NFKC").trim().slice(withoutTrailingNote(name).length));
  return !!note && !normalize(context).includes(note);
}

/** Shi/si, chi/ti, tsu/tu, fu/hu and ji/zi are the same name; long vowels may be written once or twice. */
const latinKey = (text: string) =>
  normalize(text)
    .replace(/sh(?=i)/g, "s")
    .replace(/ch(?=i)/g, "t")
    .replace(/ts(?=u)/g, "t")
    .replace(/f(?=u)/g, "h")
    .replace(/j(?=i)/g, "z")
    .replace(/([aeiou])\1/g, "$1")
    .replace(/ou/g, "o");

/** The artist appears in the title, also as romaji of a kana name: トゲナシトゲアリ = TOGENASHI TOGEARI. */
export function artistInText(artist: string, text: string): boolean {
  if (strongMatch(artist, text)) return true;
  const romaji = romanizeKana(artist);
  if (!romaji || romaji.length < 6) return false;
  const key = latinKey(romaji);
  return text
    .normalize("NFKC")
    .split(/[^a-zA-Z\s]+/)
    .some(span => latinKey(span).includes(key));
}

interface RuleMatch {
  song: SongCandidate;
  title: string;
  /** Normalized names, aliases and translations of the song that appear in the video title. */
  titles: string[];
  artistMatch: boolean;
  /** Found only as a variant spelling (夜隐染 for 夜隠染). */
  variant?: boolean;
}

const CJK = /[\u3400-\u9fff]/;

/**
 * The title's spelling of a name written with other Han variants: 夜隐染 for 夜隠染, 証命讚歌 for 証命讃歌,
 * 伤つき伤つけ for 傷つき傷つけ. Only Han characters may differ, at most one in four (and one for short names).
 */
function variantSpan(title: string, text: string): string | undefined {
  const key = normalize(title);
  if (key.length < 3 || !CJK.test(key)) return;
  const hay = normalize(text);
  const limit = Math.max(1, Math.floor(key.length / 4));
  for (let start = 0; start + key.length <= hay.length; start++) {
    let diff = 0;
    for (let i = 0; i < key.length && diff <= limit; i++) {
      if (hay[start + i] === key[i]) continue;
      diff += CJK.test(key[i]) && CJK.test(hay[start + i]) ? 1 : limit + 1;
    }
    if (diff > 0 && diff <= limit) return hay.slice(start, start + key.length);
  }
}

export function ruleMatches(
  songs: readonly SongCandidate[],
  text: string,
  context: string,
  extra = false,
): RuleMatch[] {
  const matches: RuleMatch[] = [];
  // A song named "1st" is not in "トゲナシトゲアリ | 1st".
  text = text.replace(SHOW_NOISE, " ");
  for (const song of songs.slice(0, 10)) {
    if (
      !song.id ||
      !song.artists.length ||
      WRONG_VERSION.test(song.name) ||
      // A cover release only stands for a video that says it is a cover: swim (Cover) / MyGO!!!!!.
      (LIVE.test(song.name) && !(COVERED.test(song.name) && COVERED.test(context))) ||
      !Number.isFinite(song.duration) ||
      song.duration <= 0 ||
      song.duration > 1200
    )
      continue;
    const artistMatch = song.artists.some(a => artistInText(a, context));
    const names = songTitles(song);
    const forms = [...new Set([...names, ...names.map(withoutTrailingNote)])]
      .reverse()
      .filter(t => normalize(clean(t)));
    const strict = forms.filter(t => strongMatch(t, text));
    const variants = strict.length ? [] : forms.flatMap(t => variantSpan(t, text) ?? []);
    const titles = strict.length ? strict : variants;
    const title = titles[0];
    if (!title) continue;
    const escaped = title.normalize("NFKC").replace(/[.*+?^${}()|[\]\\]/g, "\\$&");
    if (extra && new RegExp(`${escaped}-[A-Za-z0-9]`).test(text.normalize("NFKC"))) continue;
    matches.push({
      song,
      title,
      titles: titles.map(normalize),
      artistMatch,
      variant: !strict.length,
    });
  }
  return matches;
}

/** Splits items into groups that are transitively linked. */
function components<T>(items: readonly T[], linked: (a: T, b: T) => boolean): T[][] {
  const groups: T[][] = [];
  for (const item of items) {
    const joined = groups.filter(group => group.some(other => linked(item, other)));
    if (!joined.length) groups.push([item]);
    else joined[0].push(item, ...joined.slice(1).flatMap(group => groups.splice(groups.indexOf(group), 1)[0]));
  }
  return groups;
}

/** Releases of one song: candidates linked by a shared artist name or alias. */
function artistGroups(pool: readonly RuleMatch[], duration: number): RuleMatch[][] {
  const gap = (c: RuleMatch) => Math.abs(c.song.duration - duration);
  return components(pool, (a, b) => a.song.artists.some(x => b.song.artists.some(y => sameName(x, y)))).map(group =>
    group.sort((a, b) => gap(a) - gap(b) || a.song.id - b.song.id),
  );
}

/**
 * Releases of one song name (a shared name, alias or translation: 名もなき何もかも = 皆无其名); artist aliases
 * connect its releases. Without the artist in the video title, covers are ambiguous unless one artist holds a
 * strict majority of the releases; then up to three contenders, most releases first, go to alignment.
 */
export function chooseRule(
  candidates: readonly RuleMatch[],
  duration: number,
): { match: RuleMatch; majority: boolean; releases: RuleMatch[] } | { contenders: SongCandidate[] } {
  const explicit = candidates.filter(c => c.artistMatch);
  const pool = explicit.length ? explicit : candidates;
  const groups = artistGroups(pool, duration);
  const majority = !explicit.length && groups.length > 1;
  const group = majority ? groups.find(g => g.length * 2 > pool.length) : groups.length === 1 ? groups[0] : undefined;
  if (group) return { match: group[0], majority, releases: group };
  const gap = (g: RuleMatch[]) => Math.abs(g[0].song.duration - duration);
  return {
    contenders: groups
      .sort((a, b) => b.length - a.length || gap(a) - gap(b))
      .slice(0, 3)
      .map(g => g[0].song),
  };
}

/** How much a matched name says: a CJK character carries about three Latin letters or digits. */
function nameWeight(title: string): number {
  return [...title].reduce((sum, char) => sum + (/[\p{Script=Latin}\p{N}]/u.test(char) ? 0.35 : 1), 0);
}

const TAGS = /【[^】]*】|\[[^\]]*\]/g;
const NAMED = /「([^」]+)」|『([^』]+)』|《([^》]+)》/g;
const LYRIC_LINE = /“[^”]*”|"[^"]*"/g;
const READING = /[（(][^（）()]*[）)]/g;

/**
 * Where the title puts a name says how much it is the song: quoted 「名無声」 is the name, plain text usually is,
 * a reading in brackets 栞（Shiori） or a quoted lyric line “若不是你…” is about it, and a 【颂乐人偶 插曲】 tag is
 * metadata.
 */
function placement(title: string, text: string): number {
  // ruleMatches already checked the word boundaries; here only the region of the title matters.
  const key = normalize(title);
  if ([...text.matchAll(NAMED)].some(m => normalize(m.slice(1).find(Boolean) ?? "") === key)) return QUOTED_NAME;
  const untagged = text.replace(TAGS, " ");
  if (normalize(untagged.replace(LYRIC_LINE, " ").replace(READING, " ")).includes(key)) return 1;
  if (normalize(untagged).includes(key)) return 0.5;
  return 0.3;
}

const QUOTED_NAME = 1.5;

interface NameGroup {
  matches: RuleMatch[];
  weight: number;
  artist: boolean;
  /** The uploader quoted the name: 「arrow」, 《雨爱》. */
  quoted: boolean;
  /** Smallest relative duration gap to the video. */
  gap: number;
}

/**
 * Different song names in one title compete on evidence instead of cancelling each other: the artist named in
 * the title, then a much more specific name with a plausible duration. A stray word that happens to be a song
 * (BD, 1st, GBC, Encore) loses to 空白とカタルシス; names with no clear winner go to alignment.
 */
/** A variant spelling is weaker evidence; a song named after its artist (Ave Mujica) may only be the artist. */
const evidence = (c: RuleMatch, title: string) =>
  (c.variant ? 0.5 : 1) * (c.song.artists.some(a => normalize(a) === title) ? 0.3 : 1);

function rankNames(matches: readonly RuleMatch[], text: string, duration: number): NameGroup[] {
  return components(matches, (a, b) => a.titles.some(t => b.titles.includes(t)))
    .map(group => ({
      matches: group,
      weight: Math.max(...group.flatMap(c => c.titles.map(t => nameWeight(t) * placement(t, text) * evidence(c, t)))),
      artist: group.some(c => c.artistMatch),
      quoted: group.some(c => c.titles.some(t => placement(t, text) === QUOTED_NAME)),
      gap: Math.min(...group.map(c => Math.abs(c.song.duration - duration) / duration)),
    }))
    .sort((a, b) => Number(b.artist) - Number(a.artist) || b.weight - a.weight || a.gap - b.gap);
}

/** Video length over song length: a TV size is about 0.35, a Live with talk under 2.5. */
const MIN_LENGTH_RATIO = 0.3;
const MAX_LENGTH_RATIO = 2.5;

/** Names of three CJK characters or nine Latin letters are song names in their own right, not stray words. */
const SPECIFIC_NAME = 3;

const dominates = (a: NameGroup, b: NameGroup) =>
  (a.artist && !b.artist) ||
  // The uploader quoted one name: 颂乐人偶OP「KiLLKiSS」.
  (a.quoted && !b.quoted && a.artist === b.artist) ||
  (a.artist === b.artist && b.weight < SPECIFIC_NAME && a.weight >= 2 * b.weight && a.gap <= 0.35);

/** Concert parts between songs: "intro", "MC", "encore", "SE". */
const STAGE_PART =
  /^(?:intro|outro|opening|ending|overture|mc\d*|se\d*|bgm|encore|talk|アンコール|開演|开场|序曲|谢幕)$/;

/** "weak": no music category or wording, but a quoted name such as 港刺《爆ぜて咲く》 may still be a song. */
function classify(input: IdentifyInput): "not-music" | "collection" | "weak" | undefined {
  const context = `${input.title}\n${input.pageTitle ?? ""}`;
  if (STAGE_PART.test(normalize(clean(input.pageTitle ?? "")))) return "not-music";
  // A multi-part video is described by its parts: 应援不完全指北 holds one song per part.
  if (NON_SONG.test(input.pageTitle ? input.pageTitle : input.title) || WRONG_VERSION.test(context)) return "not-music";
  // The performance category (演奏) holds instrumental covers unless someone sings.
  if (input.tid === PERFORMANCE_TID && !SUNG.test(context)) return "not-music";
  if (COLLECTION.test(songText(input.title, input.pageTitle))) return "collection";
  if (!MUSIC_TIDS.has(input.tid) && !MUSIC_TITLE.test(context) && !LIVE.test(context))
    return QUOTED.test(context) ? "weak" : "not-music";
}

export function identifyRules(
  input: IdentifyInput,
  candidates: readonly SongCandidate[],
  extra = false,
): Identification {
  const status = classify(input);
  if (status && status !== "weak") return { status, confidence: 1 };
  if (!Number.isFinite(input.duration) || input.duration <= 0 || input.duration > 1200)
    return { status: "unidentified", confidence: 0 };
  const text = songText(input.title, input.pageTitle);
  // UP names are metadata, not evidence that the uploader is the original artist.
  const context = `${input.title}\n${input.pageTitle ?? ""}`;
  const matches = candidates.flatMap(song => ruleMatches([song], text, context, extra));
  const unique = [...new Map(matches.map(c => [c.song.id, c])).values()];
  // 爆ぜて咲 only matches because the title says 爆ぜて咲く, which another candidate is named.
  // Only names the title spells out may cover others; a variant spelling (用音一会) may straddle two words.
  const longer = [...new Set(unique.filter(c => !c.variant).flatMap(c => c.titles))];
  // 「名もなき何もかも」- 皆无其名 joins two names of one song: it covers neither and is not a third name.
  const joined = (title: string) => {
    const parts = longer.filter(other => other.length < title.length && title.includes(other));
    const maximal = parts.filter(part => !parts.some(other => other !== part && other.includes(part)));
    // ETERNAL FLAME 〜空の箱 is its own name even though Eternal Flame and 空の箱 are other songs.
    return maximal.length > 1 && unique.some(c => maximal.every(part => c.titles.includes(part)));
  };
  const covered = (title: string) =>
    longer.some(other => other.length > title.length && other.includes(title) && !joined(other));
  // A 10 s clip or a 15 min reaction is not one play of a 4 min song.
  const plausible = (c: RuleMatch) =>
    input.duration >= MIN_LENGTH_RATIO * c.song.duration && input.duration <= MAX_LENGTH_RATIO * c.song.duration;
  const names = rankNames(
    unique.filter(c => plausible(c) && !c.titles.every(title => covered(title) || joined(title))),
    text,
    input.duration,
  );
  if (!names.length) return { status: "unidentified", confidence: 0 };
  if (names.length > 1 && !dominates(names[0], names[1])) {
    // Two real song names: a medley or a two-song video, which one song's lyrics would not cover.
    if (names[1].weight >= SPECIFIC_NAME) return { status: "unidentified", confidence: 0 };
    const contenders = names.slice(0, 3).map(name => {
      const chosen = chooseRule(name.matches, input.duration);
      return "match" in chosen ? chosen.match.song : chosen.contenders[0];
    });
    return { status: "ambiguous", contenders, confidence: 0 };
  }
  // A short name without the artist or quotes (IMG, part.1, a lyric phrase) is too common to name the song.
  const [top] = names;
  if (!top.artist && !top.quoted && top.weight < SPECIFIC_NAME) return { status: "unidentified", confidence: 0 };
  const chosen = chooseRule(names[0].matches, input.duration);
  if ("contenders" in chosen) return { status: "ambiguous", contenders: chosen.contenders, confidence: 0 };
  const { match, majority } = chosen;
  if (status === "weak") return { status: "ambiguous", contenders: [match.song], confidence: 0 };
  return {
    status: "matched",
    song: match.song,
    confidence: match.artistMatch ? 0.95 : majority ? 0.65 : 0.8,
    alternates: chosen.releases.slice(1).map(c => c.song),
  };
}

/**
 * Searches the title's queries until one song stands out: the first query may only find stray words (未使用, Shiori)
 * that happen to be songs, so a missing or ambiguous identity keeps the fallback fragments going.
 */
export async function identifyBySearch(
  input: IdentifyInput,
  search: (query: string) => Promise<SongCandidate[]>,
): Promise<Identification> {
  const initial = identifyRules(input, []);
  if (initial.status === "collection" || initial.status === "not-music") return initial;
  const candidates = new Map<number, SongCandidate>();
  const text = songText(input.title, input.pageTitle);
  const context = `${input.title}\n${input.pageTitle ?? ""}`;
  const run = async (query: string, extra: boolean) => {
    for (const match of ruleMatches(await search(query), text, context, extra))
      candidates.set(match.song.id, match.song);
  };
  const current = () => identifyRules(input, [...candidates.values()]);
  const primary = identifyPrimaryQueries(input);
  for (const query of primary) await run(query, false);
  for (const query of identifyQueries(input)) {
    if (current().status === "matched") break;
    if (!primary.includes(query)) await run(query, true);
  }
  const identified = current();
  if (identified.status !== "ambiguous") return identified;
  // Releases of one name keep search order: NetEase ranks the canonical release first. Different names stay
  // in evidence order.
  const [first] = identified.contenders;
  if (identified.contenders.every(c => sameName(c.name, first.name))) {
    const rank = [...candidates.keys()];
    identified.contenders.sort((a, b) => rank.indexOf(a.id) - rank.indexOf(b.id));
  }
  return identified;
}
