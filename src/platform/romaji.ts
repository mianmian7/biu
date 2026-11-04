import DynamicDictionaries from "kuromoji/src/dict/DynamicDictionaries";
import Tokenizer from "kuromoji/src/Tokenizer";

import { isJapaneseLyrics, needsRomaji, type RomajiToken, tokensToRomaji } from "@shared/lyrics/romaji";

import { ensureMediaBase, invokeTauri } from "./index";

const IDLE_RELEASE_MS = 5 * 60 * 1000;

interface KuromojiTokenizer {
  tokenize: (text: string) => RomajiToken[];
}

let tokenizerPromise: Promise<KuromojiTokenizer> | null = null;
let releaseTimer: ReturnType<typeof setTimeout> | null = null;

async function fetchDecompressedDict(base: string, filename: string): Promise<ArrayBuffer> {
  const resp = await fetch(`${base}kuromoji?name=${encodeURIComponent(filename)}`);
  if (!resp.ok || !resp.body) {
    throw new Error(`Failed to fetch kuromoji dictionary ${filename}: HTTP ${resp.status}`);
  }
  const stream = resp.body.pipeThrough(new DecompressionStream("gzip"));
  return new Response(stream).arrayBuffer();
}

async function buildTokenizer(): Promise<KuromojiTokenizer> {
  await invokeTauri<void>("kuromoji_ensure_dict");
  const base = await ensureMediaBase();

  const [
    baseBuf,
    checkBuf,
    tidBuf,
    tidPosBuf,
    tidMapBuf,
    ccBuf,
    unkBuf,
    unkPosBuf,
    unkMapBuf,
    unkCharBuf,
    unkCompatBuf,
    unkInvokeBuf,
  ] = await Promise.all([
    fetchDecompressedDict(base, "base.dat.gz"),
    fetchDecompressedDict(base, "check.dat.gz"),
    fetchDecompressedDict(base, "tid.dat.gz"),
    fetchDecompressedDict(base, "tid_pos.dat.gz"),
    fetchDecompressedDict(base, "tid_map.dat.gz"),
    fetchDecompressedDict(base, "cc.dat.gz"),
    fetchDecompressedDict(base, "unk.dat.gz"),
    fetchDecompressedDict(base, "unk_pos.dat.gz"),
    fetchDecompressedDict(base, "unk_map.dat.gz"),
    fetchDecompressedDict(base, "unk_char.dat.gz"),
    fetchDecompressedDict(base, "unk_compat.dat.gz"),
    fetchDecompressedDict(base, "unk_invoke.dat.gz"),
  ]);

  const dic = new DynamicDictionaries();
  dic.loadTrie(new Int32Array(baseBuf), new Int32Array(checkBuf));
  dic.loadTokenInfoDictionaries(new Uint8Array(tidBuf), new Uint8Array(tidPosBuf), new Uint8Array(tidMapBuf));
  dic.loadConnectionCosts(new Int16Array(ccBuf));
  dic.loadUnknownDictionaries(
    new Uint8Array(unkBuf),
    new Uint8Array(unkPosBuf),
    new Uint8Array(unkMapBuf),
    new Uint8Array(unkCharBuf),
    new Uint32Array(unkCompatBuf),
    new Uint8Array(unkInvokeBuf),
  );

  return new Tokenizer(dic) as KuromojiTokenizer;
}

function getTokenizer(): Promise<KuromojiTokenizer> {
  if (!tokenizerPromise) {
    tokenizerPromise = buildTokenizer().catch(err => {
      tokenizerPromise = null;
      throw err;
    });
  }

  if (releaseTimer) {
    clearTimeout(releaseTimer);
  }
  releaseTimer = setTimeout(() => {
    releaseTimer = null;
    tokenizerPromise = null;
  }, IDLE_RELEASE_MS);

  return tokenizerPromise;
}

export async function romanizeLyricLines(lines: readonly string[]): Promise<(string | null)[]> {
  if (!Array.isArray(lines) || !lines.length || !isJapaneseLyrics(lines)) {
    return Array.isArray(lines) ? lines.map(() => null) : [];
  }

  try {
    const tokenizer = await getTokenizer();
    return lines.map(line => {
      if (typeof line !== "string" || !needsRomaji(line)) return null;
      return tokensToRomaji(tokenizer.tokenize(line)) || null;
    });
  } catch (err) {
    void invokeTauri("frontend_log", {
      level: "warn",
      message: `[lyrics-romaji] romanize failed: ${err instanceof Error ? err.message : String(err)}`,
    }).catch(() => {});
    return lines.map(() => null);
  }
}
