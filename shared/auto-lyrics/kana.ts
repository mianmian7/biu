/** Hepburn romanization of kana, enough to recognize artist names written in Latin letters (TOGENASHI TOGEARI). */
const BASE: Record<string, string> = {
  あ: "a",
  い: "i",
  う: "u",
  え: "e",
  お: "o",
  か: "ka",
  き: "ki",
  く: "ku",
  け: "ke",
  こ: "ko",
  が: "ga",
  ぎ: "gi",
  ぐ: "gu",
  げ: "ge",
  ご: "go",
  さ: "sa",
  し: "shi",
  す: "su",
  せ: "se",
  そ: "so",
  ざ: "za",
  じ: "ji",
  ず: "zu",
  ぜ: "ze",
  ぞ: "zo",
  た: "ta",
  ち: "chi",
  つ: "tsu",
  て: "te",
  と: "to",
  だ: "da",
  ぢ: "ji",
  づ: "zu",
  で: "de",
  ど: "do",
  な: "na",
  に: "ni",
  ぬ: "nu",
  ね: "ne",
  の: "no",
  は: "ha",
  ひ: "hi",
  ふ: "fu",
  へ: "he",
  ほ: "ho",
  ば: "ba",
  び: "bi",
  ぶ: "bu",
  べ: "be",
  ぼ: "bo",
  ぱ: "pa",
  ぴ: "pi",
  ぷ: "pu",
  ぺ: "pe",
  ぽ: "po",
  ま: "ma",
  み: "mi",
  む: "mu",
  め: "me",
  も: "mo",
  や: "ya",
  ゆ: "yu",
  よ: "yo",
  ら: "ra",
  り: "ri",
  る: "ru",
  れ: "re",
  ろ: "ro",
  わ: "wa",
  ゐ: "i",
  ゑ: "e",
  を: "o",
  ん: "n",
  ゔ: "vu",
  ぁ: "a",
  ぃ: "i",
  ぅ: "u",
  ぇ: "e",
  ぉ: "o",
};
const SMALL_Y: Record<string, string> = { ゃ: "a", ゅ: "u", ょ: "o" };

/** Katakana and hiragana to Hepburn; returns undefined when the text is not pure kana. */
export function romanizeKana(text: string): string | undefined {
  // Katakana → hiragana (U+30A1–U+30F6 map 0x60 lower); ー and spaces are kept for the loop below.
  const kana = text.normalize("NFKC").replace(/[ァ-ヶ]/g, c => String.fromCharCode(c.charCodeAt(0) - 0x60));
  if (!/^[ぁ-ゖー・\s]+$/.test(kana)) return undefined;
  let out = "";
  let geminate = false;
  for (const char of kana) {
    if (char === "っ") {
      geminate = true;
      continue;
    }
    let syllable: string;
    if (char in SMALL_Y) {
      // き + ゃ → kya, し + ゃ → sha, ち + ゃ → cha, じ + ゃ → ja.
      const vowel = SMALL_Y[char];
      out = /(?:sh|ch|j)i$/.test(out) ? out.slice(0, -1) + vowel : out.replace(/i$/, "y" + vowel);
      continue;
    } else if (char === "ー") {
      syllable = out.slice(-1);
    } else if (/[・\s]/.test(char)) {
      syllable = "";
    } else {
      syllable = BASE[char] ?? "";
    }
    if (geminate && syllable) syllable = (syllable.startsWith("ch") ? "t" : syllable[0]) + syllable;
    geminate = false;
    out += syllable;
  }
  return out;
}
