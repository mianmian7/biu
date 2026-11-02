import { romanizeKana } from "../auto-lyrics/kana";

/** 分词结果中转换罗马音需要的字段（与 kuromoji 的 IpadicFeatures 兼容） */
export interface RomajiToken {
  surface_form: string;
  reading?: string;
  pos: string;
  pos_detail_1: string;
}

const KANA = /[぀-ゟ゠-ヿ]/;
const CJK_OR_KANA = /[぀-ゟ゠-ヿ㐀-鿿々〆]/;

/** 歌词行是否包含假名 */
const hasKana = (text: string) => KANA.test(text);

/**
 * 判断整首歌是否是日语：至少两行、且不少于两成的行含假名。
 * 中文歌词几乎不会出现假名，以此避免给中文歌生成罗马音。
 */
export function isJapaneseLyrics(lines: readonly string[]) {
  const withKana = lines.filter(hasKana).length;
  return withKana >= 2 && withKana >= lines.length * 0.2;
}

/** 日语歌里需要生成罗马音的行：含假名或汉字（纯英文/数字行不生成） */
export const needsRomaji = (text: string) => CJK_OR_KANA.test(text);

/** 助词按发音写：は→wa、へ→e、を→wo（与网易云罗马音写法一致） */
const PARTICLE_READING: Record<string, string> = { は: "wa", へ: "e", を: "wo" };

/** 与前一个假名合成一个音节的小写假名（拗音、外来语小元音） */
const SMALL_KANA = /[ゃゅょぁぃぅぇぉゎャュョァィゥェォヮ]/;

/**
 * 把一段假名切成音节：小写假名并入前一个，促音「っ」并入后一个，长音「ー」单独成音节。
 * 例：まって → ま / って，しゃしん → しゃ / し / ん
 */
function splitMorae(kana: string) {
  const morae: string[] = [];
  let pendingSokuon = "";
  for (const char of kana) {
    if (/[っッ]/.test(char)) {
      pendingSokuon += char;
    } else if (SMALL_KANA.test(char) && morae.length && !pendingSokuon) {
      morae[morae.length - 1] += char;
    } else {
      morae.push(pendingSokuon + char);
      pendingSokuon = "";
    }
  }
  return morae;
}

/** 假名转为空格分隔的音节罗马音；长音「ー」写成前一音节的元音 */
function romanizeMorae(kana: string) {
  const syllables: string[] = [];
  for (const mora of splitMorae(kana)) {
    if (mora === "ー") {
      const vowel = syllables.at(-1)?.match(/[aeiou]$/)?.[0];
      if (vowel) syllables.push(vowel);
      continue;
    }
    const romaji = romanizeKana(mora);
    if (romaji) syllables.push(romaji);
  }
  return syllables;
}

/**
 * 把一行歌词的分词结果转换为罗马音。
 *
 * 采用网易云罗马音的写法：按音节空格分隔（ka ze ni yu re ru），促音并入后一音节（ma tte），
 * 这样补齐的行与歌词源自带的罗马音风格一致。相邻分词的读音拼接后再切音节，
 * 促音「っ」跨分词时（なかっ|た）也不会丢。
 */
export function tokensToRomaji(tokens: readonly RomajiToken[]): string {
  const parts: string[] = [];
  let kanaRun = "";
  const flush = () => {
    if (kanaRun) parts.push(...romanizeMorae(kanaRun));
    kanaRun = "";
  };

  for (const token of tokens) {
    const { surface_form: surface, reading, pos } = token;
    if (pos === "記号" || !surface.trim()) {
      flush();
      continue;
    }

    if (pos === "助詞" && PARTICLE_READING[surface]) {
      flush();
      parts.push(PARTICLE_READING[surface]);
      continue;
    }

    // 数字按读音写（１ → i chi），与网易云罗马音一致
    if (pos === "名詞" && token.pos_detail_1 === "数" && reading && reading !== "*") {
      kanaRun += reading;
      continue;
    }

    // 英文等没有读音的部分保持原样
    if (!CJK_OR_KANA.test(surface)) {
      flush();
      parts.push(surface.trim());
      continue;
    }

    kanaRun += reading && reading !== "*" ? reading : surface;
  }
  flush();

  return parts.join(" ").toLowerCase();
}
