// @vitest-environment node
import type { IpadicFeatures, Tokenizer } from "kuromoji";

import kuromoji from "kuromoji";
import { readFile } from "node:fs/promises";
import path from "node:path";
import { gunzipSync } from "node:zlib";
import { beforeAll, describe, expect, test } from "vitest";

import { buildTokenizer, trimTrailingZeros } from "@/platform/romaji-worker";
import { romanizeKana } from "@shared/auto-lyrics/kana";
import { isJapaneseLyrics, needsRomaji, tokensToRomaji } from "@shared/lyrics/romaji";

let tokenizer: Tokenizer<IpadicFeatures>;
const dicPath = path.resolve(__dirname, "../node_modules/kuromoji/dict");

beforeAll(async () => {
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

test("zero trimming preserves alignment and string terminators", () => {
  const bytes = new Uint8Array([1, 2, 3, 4, 0, 0, 0, 0]);
  expect(new Uint8Array(trimTrailingZeros(bytes.buffer, 4))).toEqual(bytes.slice(0, 4));
  expect(new Uint8Array(trimTrailingZeros(bytes.buffer, 1, 1))).toEqual(bytes.slice(0, 5));
  expect(trimTrailingZeros(new ArrayBuffer(8), 4).byteLength).toBe(4);
  expect(trimTrailingZeros(new ArrayBuffer(8), 2).byteLength).toBe(2);
  expect(trimTrailingZeros(bytes.buffer, 10)).toBe(bytes.buffer);
});

test("real IPADIC retains complete lookup tables and byte-identical tokenization and romaji", async () => {
  const trimmed = await buildTokenizer(async name => {
    const raw = gunzipSync(await readFile(path.join(dicPath, name)));
    return Uint8Array.from(raw).buffer;
  });

  // Audit every mapped record and its NUL-terminated POS entry, including the final record
  // where a numeric field can end in zeroes that are still part of a valid read.
  const internals = trimmed as unknown as Record<
    string,
    {
      dictionary: { buffer: Uint8Array<ArrayBuffer> };
      pos_buffer: { buffer: Uint8Array<ArrayBuffer> };
      target_map: Record<number, number[]>;
    }
  >;
  for (const [name, prefix] of [
    ["token_info_dictionary", "tid"],
    ["unknown_dictionary", "unk"],
  ] as const) {
    const { dictionary, pos_buffer, target_map } = internals[name];
    const words = dictionary.buffer;
    const positions = pos_buffer.buffer;
    const wordView = new DataView(words.buffer);
    const map = gunzipSync(await readFile(path.join(dicPath, `${prefix}_map.dat.gz`)));
    let cursor = 4;
    let mismatch = false;
    for (let i = 0; i < map.readInt32LE(0); i++) {
      const key = map.readInt32LE(cursor);
      const count = map.readInt32LE(cursor + 4);
      cursor += 8;
      const entries = target_map[key];
      if (entries.length !== count) mismatch = true;
      for (let j = 0; j < count; j++, cursor += 4) {
        const offset = map.readInt32LE(cursor);
        if (entries[j] !== offset || offset < 0 || offset + 10 > words.length) {
          mismatch = true;
          continue;
        }
        const pos = wordView.getInt32(offset + 6, true);
        if (pos < 0 || positions.indexOf(0, pos) < 0) mismatch = true;
      }
    }
    expect(mismatch).toBe(false);
    expect(map.subarray(cursor).every(byte => byte === 0)).toBe(true);
    expect(words.length + positions.length).toBeLessThan(prefix === "tid" ? 40_000_000 : 2_000);
  }

  const japaneseLyrics = [
    "風に揺れる街の灯り",
    "君は僕に会いたくなかったのかな",
    "遠く響くメロディーへ",
    "Lemon、あなたの光",
    "勇者を待っている　彼が叫んだ",
    "僕にとってあなたは１です",
    "あの日見渡した渚を 今も思い出すんだ",
    "ずっと真夜中でいいのに。𠮷野家🦊 αβ ΓД 한글 ＡＢＣ 123",
    "っティー、ファンタジー？　カタカナ＋ひらがな！",
    "ツィ、ウィ、シェア、夢を見ていた",
  ];
  for (const lyric of japaneseLyrics) {
    const expected = tokenizer.tokenize(lyric);
    const actual = trimmed.tokenize(lyric);
    expect(JSON.stringify(actual)).toBe(JSON.stringify(expected));
    expect(tokensToRomaji(actual)).toBe(tokensToRomaji(expected));
  }
}, 30_000);
