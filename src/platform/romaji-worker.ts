import { needsRomaji, type RomajiToken, tokensToRomaji } from "@shared/lyrics/romaji";

export interface RomajiRequest {
  id: number;
  base: string;
  lines: string[];
}

export type RomajiResponse = { id: number } & ({ result: (string | null)[] } | { error: string });

export function trimTrailingZeros(buf: ArrayBuffer, alignment: number, terminatorBytes = 0): ArrayBuffer {
  const bytes = new Uint8Array(buf);
  let end = bytes.length;
  while (end > 0 && bytes[end - 1] === 0) end--;
  const length = Math.max(alignment, Math.ceil((end + terminatorBytes) / alignment) * alignment);
  return length < bytes.length ? buf.slice(0, length) : buf;
}

export async function buildTokenizer(load: (file: string) => Promise<ArrayBuffer>) {
  const [{ default: DynamicDictionaries }, { default: Tokenizer }] = await Promise.all([
    import("kuromoji/src/dict/DynamicDictionaries"),
    import("kuromoji/src/Tokenizer"),
  ]);
  const dic = new DynamicDictionaries();
  const padded = async (name: string, alignment: number, terminatorBytes?: number) =>
    trimTrailingZeros(await load(name), alignment, terminatorBytes);

  // Trie, connection costs and character maps are indexed by position and keep their full dimensions.
  dic.loadTrie(new Int32Array(await load("base.dat.gz")), new Int32Array(await load("check.dat.gz")));
  dic.loadConnectionCosts(new Int16Array(await load("cc.dat.gz")));
  // IPADIC uses 10-byte token records, 32-bit target map fields, and NUL-terminated strings.
  dic.loadTokenInfoDictionaries(
    new Uint8Array(await padded("tid.dat.gz", 10)),
    new Uint8Array(await padded("tid_pos.dat.gz", 1, 1)),
    new Uint8Array(await padded("tid_map.dat.gz", 4)),
  );
  dic.loadUnknownDictionaries(
    new Uint8Array(await padded("unk.dat.gz", 10)),
    new Uint8Array(await padded("unk_pos.dat.gz", 1, 1)),
    new Uint8Array(await padded("unk_map.dat.gz", 4)),
    new Uint8Array(await load("unk_char.dat.gz")),
    new Uint32Array(await load("unk_compat.dat.gz")),
    new Uint8Array(await padded("unk_invoke.dat.gz", 1, 1)),
  );

  return new Tokenizer(dic) as { tokenize: (text: string) => RomajiToken[] };
}

export function loadTokenizer(base: string) {
  return buildTokenizer(async filename => {
    const response = await fetch(`${base}kuromoji?name=${encodeURIComponent(filename)}`);
    if (!response.ok || !response.body) {
      throw new Error(`Failed to fetch kuromoji dictionary ${filename}: HTTP ${response.status}`);
    }
    return new Response(response.body.pipeThrough(new DecompressionStream("gzip"))).arrayBuffer();
  });
}

export function romanizeWithTokenizer(tokenizer: Awaited<ReturnType<typeof buildTokenizer>>, lines: string[]) {
  return lines.map(line => (needsRomaji(line) ? tokensToRomaji(tokenizer.tokenize(line)) || null : null));
}

declare let __webpack_public_path__: string;

if (typeof self !== "undefined" && typeof window === "undefined") {
  // Rsbuild serves worker chunks from static/js/async/ and shares lazy vendor chunks with the page.
  if (typeof __webpack_public_path__ === "string") {
    __webpack_public_path__ = new URL("../../../", self.location.href).href;
  }
  let tokenizer: ReturnType<typeof loadTokenizer> | undefined;
  self.addEventListener("message", async (event: MessageEvent<RomajiRequest>) => {
    const { id, base, lines } = event.data;
    try {
      tokenizer ??= loadTokenizer(base).catch(error => {
        tokenizer = undefined;
        throw error;
      });
      const result = romanizeWithTokenizer(await tokenizer, lines);
      self.postMessage({ id, result } satisfies RomajiResponse);
    } catch (error) {
      self.postMessage({ id, error: error instanceof Error ? error.message : String(error) } satisfies RomajiResponse);
    }
  });
}
