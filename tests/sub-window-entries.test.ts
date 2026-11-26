import { existsSync, readFileSync, statSync } from "node:fs";
import path from "node:path";
import { describe, expect, test } from "vitest";

const ROOT = path.resolve(__dirname, "..");
const ALIASES: Record<string, string> = { "@/": "src/", "@shared/": "shared/" };
const EXTENSIONS = [".ts", ".tsx", "/index.ts", "/index.tsx"];

/** 子窗口入口禁止引入的第三方包与本地模块 */
const FORBIDDEN_PACKAGES = [
  "@heroui/react",
  "framer-motion",
  "react-router",
  "axios",
  "kuromoji",
  "moment",
  "moment-duration-format",
  "overlayscrollbars",
  "overlayscrollbars-react",
];
const FORBIDDEN_FILES = ["src/store/play-list.ts", "src/platform/index.ts", "src/app.tsx", "src/app.css"];

const resolveLocal = (from: string, spec: string): string | null => {
  let base: string | null = null;
  for (const [alias, dir] of Object.entries(ALIASES)) {
    if (spec.startsWith(alias)) base = path.join(ROOT, dir, spec.slice(alias.length));
  }
  if (spec.startsWith(".")) base = path.resolve(path.dirname(from), spec);
  if (!base) return null;
  if (existsSync(base) && statSync(base).isFile()) return base;
  for (const ext of EXTENSIONS) if (existsSync(base + ext)) return base + ext;
  throw new Error(`cannot resolve ${spec} from ${from}`);
};

/** 解析运行时 import（忽略 `import type` / `export type`），返回可达的本地文件与第三方包 */
const collectGraph = (entry: string) => {
  const files = new Set<string>();
  const packages = new Set<string>();
  const queue = [path.join(ROOT, entry)];
  const importRe =
    /(?:^|\n)\s*(?:import|export)\s+(?!type\s)(?:[^"'\n;]*?\s+from\s+)?["']([^"']+)["']|import\(\s*["']([^"']+)["']\s*\)/g;
  while (queue.length) {
    const file = queue.pop()!;
    if (files.has(file)) continue;
    files.add(file);
    if (!/\.(t|j)sx?$/.test(file)) continue;
    const source = readFileSync(file, "utf8").replace(/import\s+type\s[^;]+;/g, "");
    for (const match of source.matchAll(importRe)) {
      const spec = match[1] ?? match[2];
      const local = resolveLocal(file, spec);
      if (local) queue.push(local);
      else packages.add(spec.startsWith("@") ? spec.split("/").slice(0, 2).join("/") : spec.split("/")[0]);
    }
  }
  return { files: [...files].map(f => path.relative(ROOT, f)), packages: [...packages] };
};

describe.each(["src/entries/desktop-lyrics.tsx", "src/entries/mini-player.tsx"])("%s stays lightweight", entry => {
  const graph = collectGraph(entry);

  test("does not import heavy main-window packages", () => {
    for (const pkg of FORBIDDEN_PACKAGES) {
      expect(graph.packages, `${entry} -> ${pkg}`).not.toContain(pkg);
    }
  });

  test("does not import play-list store (new Audio()), platform, app or app.css", () => {
    for (const file of FORBIDDEN_FILES) {
      expect(graph.files, `${entry} -> ${file}`).not.toContain(file);
    }
  });

  test("uses the lightweight sub-window stylesheet", () => {
    expect(graph.files).toContain("src/entries/sub-window.css");
  });
});

describe("main entry", () => {
  test("no runtime code imports moment anymore", () => {
    const graph = collectGraph("src/app.tsx");
    // 健全性：同一套遍历在主入口能看到这些重型依赖，证明上面的子窗口断言不是空转
    expect(graph.packages).toEqual(expect.arrayContaining(["@heroui/react", "react-router", "axios"]));
    expect(graph.files).toContain("src/store/play-list.ts");
    expect(graph.packages).not.toContain("moment");
    expect(graph.packages).not.toContain("moment-duration-format");
  });

  test("sub-windows open dedicated lightweight HTML entries and index.html retains hash fallback", () => {
    const html = readFileSync(path.join(ROOT, "src/index.html"), "utf8");
    expect(html).toContain('"desktop-lyrics"');
    expect(html).toContain('"mini-player"');
    expect(html).toContain("location.replace");
    const rust = readFileSync(path.join(ROOT, "src-tauri/src/desktop_lyrics.rs"), "utf8");
    expect(rust).toContain('"desktop-lyrics.html"');
    expect(rust).toContain('"mini-player.html"');
  });
});
