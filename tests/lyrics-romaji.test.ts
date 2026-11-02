// @vitest-environment node
import type { IpadicFeatures, Tokenizer } from "kuromoji";

import kuromoji from "kuromoji";
import path from "node:path";
import { beforeAll, describe, expect, test } from "vitest";

import { romanizeKana } from "@shared/auto-lyrics/kana";
import { isJapaneseLyrics, needsRomaji, tokensToRomaji } from "@shared/lyrics/romaji";

let tokenizer: Tokenizer<IpadicFeatures>;

beforeAll(async () => {
  const dicPath = path.resolve(__dirname, "../node_modules/kuromoji/dict");
  tokenizer = await new Promise((resolve, reject) => {
    kuromoji.builder({ dicPath }).build((err, built) => (err ? reject(err) : resolve(built)));
  });
}, 30_000);

const romanize = (line: string) => tokensToRomaji(tokenizer.tokenize(line));

describe("tokensToRomaji", () => {
  test("reads kanji and splits syllables like NetEase romaji", () => {
    expect(romanize("風に揺れる街の灯り")).toBe("ka ze ni yu re ru ma chi no a ka ri");
  });

  test("keeps sokuon with the next syllable and writes particles by sound", () => {
    // 促音跨分词（なかっ|た）并入后一音节
    expect(romanize("君は僕に会いたくなかったのかな")).toBe("ki mi wa bo ku ni a i ta ku na ka tta no ka na");
    expect(romanize("夢を見ていた")).toBe("yu me wo mi te i ta");
    expect(romanize("遠く響くメロディーへ")).toBe("to o ku hi bi ku me ro di i e");
  });

  test("keeps latin words and drops punctuation", () => {
    expect(romanize("Lemon、あなたの光")).toBe("lemon a na ta no hi ka ri");
  });
});

test("matches NetEase romaji for lines that have it", () => {
  // 网易云《勇者を待っている》等行自带的罗马音
  expect(romanize("勇者を待っている")).toBe("yu u sha wo ma tte i ru");
  expect(romanize("彼が叫んだ")).toBe("ka re ga sa ke n da");
  expect(romanize("時だけが過ぎてゆく")).toBe("to ki da ke ga su gi te yu ku");
  expect(romanize("僕にとってあなたは１です")).toBe("bo ku ni to tte a na ta wa i chi de su");
});

describe("romanizeKana small vowels", () => {
  test("handles loanword combinations", () => {
    expect(romanizeKana("ファンタジー")).toBe("fantajii");
    expect(romanizeKana("ティー")).toBe("tii");
    expect(romanizeKana("シェア")).toBe("shea");
    expect(romanizeKana("ツィ")).toBe("tsi");
    expect(romanizeKana("ウィ")).toBe("wi");
  });
});

describe("language detection", () => {
  test("only Japanese lyrics get romaji, latin-only lines are skipped", () => {
    expect(isJapaneseLyrics(["風に揺れる", "街の灯り", "Hello"])).toBe(true);
    expect(isJapaneseLyrics(["夜空中最亮的星", "能否听清"])).toBe(false);
    expect(needsRomaji("Hello world")).toBe(false);
    expect(needsRomaji("灯り")).toBe(true);
  });
});
