import type { IpadicFeatures, Tokenizer } from "kuromoji";

import log from "electron-log";
import kuromoji from "kuromoji";
import { createRequire } from "node:module";
import path from "node:path";

import { isJapaneseLyrics, needsRomaji, tokensToRomaji } from "../shared/lyrics/romaji";

const require = createRequire(import.meta.url);

/** 词典常驻内存约几十 MB：闲置一段时间后释放，下次需要时再加载 */
const IDLE_RELEASE_MS = 5 * 60 * 1000;

let tokenizerPromise: Promise<Tokenizer<IpadicFeatures>> | null = null;
let releaseTimer: NodeJS.Timeout | null = null;

/** 懒加载 kuromoji 分词器（词典随 kuromoji 包一起打进 asar，fs 可直接读取） */
function getTokenizer() {
  if (!tokenizerPromise) {
    const dicPath = path.join(path.dirname(require.resolve("kuromoji/package.json")), "dict");
    tokenizerPromise = new Promise<Tokenizer<IpadicFeatures>>((resolve, reject) => {
      kuromoji.builder({ dicPath }).build((err, tokenizer) => (err ? reject(err) : resolve(tokenizer)));
    }).catch(err => {
      tokenizerPromise = null;
      throw err;
    });
  }

  if (releaseTimer) clearTimeout(releaseTimer);
  releaseTimer = setTimeout(() => {
    releaseTimer = null;
    tokenizerPromise = null;
  }, IDLE_RELEASE_MS);

  return tokenizerPromise;
}

/**
 * 为日语歌词行生成罗马音。
 *
 * 网易云很多日语歌只有翻译没有罗马音（或只覆盖部分行），这里按汉字读音本地生成补齐。
 * 不是日语的歌整首返回 null；纯英文/数字行返回 null。
 */
export async function romanizeLyricLines(lines: readonly string[]): Promise<(string | null)[]> {
  if (!Array.isArray(lines) || !lines.length || !isJapaneseLyrics(lines)) return lines.map(() => null);

  try {
    const tokenizer = await getTokenizer();
    return lines.map(line => {
      if (typeof line !== "string" || !needsRomaji(line)) return null;
      return tokensToRomaji(tokenizer.tokenize(line)) || null;
    });
  } catch (err) {
    log.warn("[lyrics-romaji] romanize failed:", err);
    return lines.map(() => null);
  }
}
