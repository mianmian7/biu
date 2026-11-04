import { createHash } from "node:crypto";
import fs from "node:fs";
import path from "node:path";
import { fileURLToPath } from "node:url";

/**
 * Compute base64-encoded SHA-512 hash and byte size of a file,
 * matching electron-builder's `hashFile(file, "sha512", "base64")` format.
 */
export function statAndHashFile(filePath) {
  const buf = fs.readFileSync(filePath);
  const sha512 = createHash("sha512").update(buf).digest("base64");
  return {
    url: path.basename(filePath),
    sha512,
    size: buf.byteLength,
  };
}

/**
 * Format an electron-updater YAML feed (`latest.yml`, `latest-mac.yml`,
 * `latest-linux.yml`, `latest-linux-arm64.yml`) matching electron-builder's output.
 */
export function formatUpdaterYaml({
  version,
  files,
  primaryFile = "",
  releaseDate = "",
  releaseNotes = "",
}) {
  if (!version) {
    throw new Error("version is required");
  }
  if (!Array.isArray(files) || files.length === 0) {
    throw new Error("files must be a non-empty array");
  }
  const primary =
    (primaryFile && files.find(f => f.url === primaryFile)) || files[0];
  const dateStr = releaseDate || new Date().toISOString();

  const lines = [`version: ${version}`, "files:"];
  for (const file of files) {
    lines.push(`  - url: ${file.url}`);
    lines.push(`    sha512: ${file.sha512}`);
    lines.push(`    size: ${file.size}`);
    if (typeof file.blockMapSize === "number") {
      lines.push(`    blockMapSize: ${file.blockMapSize}`);
    }
  }
  lines.push(`path: ${primary.url}`);
  lines.push(`sha512: ${primary.sha512}`);
  if (typeof releaseNotes === "string" && releaseNotes.length > 0) {
    lines.push(`releaseNotes: ${JSON.stringify(releaseNotes)}`);
  }
  lines.push(`releaseDate: '${dateStr}'`);
  return `${lines.join("\n")}\n`;
}

/**
 * Parse a simple electron-updater YAML string for validation/testing.
 */
export function parseUpdaterYaml(yamlText) {
  const lines = yamlText.trim().split(/\r?\n/);
  const result = {
    version: "",
    files: [],
    path: "",
    sha512: "",
    releaseDate: "",
  };
  let currentFile = null;
  for (const line of lines) {
    if (line.startsWith("version: ")) {
      result.version = line.slice("version: ".length).trim();
    } else if (line.startsWith("  - url: ")) {
      currentFile = { url: line.slice("  - url: ".length).trim(), sha512: "", size: 0 };
      result.files.push(currentFile);
    } else if (line.startsWith("    sha512: ") && currentFile) {
      currentFile.sha512 = line.slice("    sha512: ".length).trim();
    } else if (line.startsWith("    size: ") && currentFile) {
      currentFile.size = Number(line.slice("    size: ".length).trim());
    } else if (line.startsWith("    blockMapSize: ") && currentFile) {
      currentFile.blockMapSize = Number(line.slice("    blockMapSize: ".length).trim());
    } else if (line.startsWith("path: ")) {
      currentFile = null;
      result.path = line.slice("path: ".length).trim();
    } else if (line.startsWith("sha512: ")) {
      result.sha512 = line.slice("sha512: ".length).trim();
    } else if (line.startsWith("releaseDate: ")) {
      result.releaseDate = line
        .slice("releaseDate: ".length)
        .trim()
        .replace(/^['"]|['"]$/g, "");
    }
  }
  return result;
}

/**
 * Electron clients on `.6` or older must pass through the bridge release, which exports
 * their login before Tauri takes over. electron-updater 6.6.2's GitHubProvider returns
 * `{ tag, ...yml }`, so a `tag:` line in the yml makes it download from the bridge release.
 */
export function bridgeFeedYaml(yamlText, bridgeTag) {
  if (!bridgeTag) {
    throw new Error("bridgeTag is required");
  }
  const kept = yamlText
    .trimEnd()
    .split(/\r?\n/)
    .filter(line => !line.startsWith("tag: "));
  return `${[...kept, `tag: ${bridgeTag}`].join("\n")}\n`;
}

/**
 * Copy the bridge release's `latest*.yml` into `outDir`, each pointing at `bridgeTag`.
 */
export function writeBridgeFeeds({ bridgeDir, bridgeTag, outDir }) {
  const generated = {};
  for (const name of fs.readdirSync(bridgeDir)) {
    if (!/^latest.*\.yml$/.test(name)) continue;
    const outPath = path.join(outDir, name);
    const text = fs.readFileSync(path.join(bridgeDir, name), "utf8");
    fs.writeFileSync(outPath, bridgeFeedYaml(text, bridgeTag), "utf8");
    generated[name] = outPath;
  }
  for (const required of ["latest.yml", "latest-mac.yml", "latest-linux.yml", "latest-linux-arm64.yml"]) {
    if (!generated[required]) {
      throw new Error(`bridge feed ${required} missing in ${bridgeDir}`);
    }
  }
  return generated;
}

/**
 * Generate the electron-updater feeds that the bridge release reads (channel `tauri`):
 * `tauri-mac.yml`, `tauri.yml` (win x64), `tauri-arm64.yml` (win arm64),
 * `tauri-linux.yml`, `tauri-linux-arm64.yml`, plus Tauri's own `latest.json`.
 * NsisUpdater takes the first exe in a feed, so each Windows arch gets its own feed.
 */
export function generateFeedsFromDir({
  dir,
  outDir = dir,
  version,
  releaseDate = new Date().toISOString(),
  releaseNotes = "",
  repoUrl = "https://github.com/mianmian7/biu",
  tag = `v${version}`,
  channel = "tauri",
}) {
  fs.mkdirSync(outDir, { recursive: true });
  const entries = fs.readdirSync(dir);
  const findEntry = name => (entries.includes(name) ? path.join(dir, name) : null);

  const generated = {};

  // 1. macOS: latest-mac.yml
  const macFileNames = [
    `Biu-${version}-mac-x64.zip`,
    `Biu-${version}-mac-arm64.zip`,
    `Biu-${version}-mac-x64.dmg`,
    `Biu-${version}-mac-arm64.dmg`,
  ];
  const macFiles = macFileNames
    .map(findEntry)
    .filter(Boolean)
    .map(statAndHashFile);
  if (macFiles.some(f => f.url.endsWith(".zip"))) {
    const primaryMac =
      macFiles.find(f => f.url === `Biu-${version}-mac-x64.zip`)?.url ||
      macFiles.find(f => f.url.endsWith(".zip"))?.url;
    const yml = formatUpdaterYaml({
      version,
      files: macFiles,
      primaryFile: primaryMac,
      releaseDate,
      releaseNotes,
    });
    const name = `${channel}-mac.yml`;
    const outPath = path.join(outDir, name);
    fs.writeFileSync(outPath, yml, "utf8");
    generated[name] = outPath;
  }

  // 2. Windows: one feed per arch
  for (const [arch, name] of [
    ["x64", `${channel}.yml`],
    ["arm64", `${channel}-arm64.yml`],
  ]) {
    const exePath = findEntry(`Biu-${version}-win-setup-${arch}.exe`);
    if (!exePath) continue;
    const yml = formatUpdaterYaml({
      version,
      files: [statAndHashFile(exePath)],
      releaseDate,
      releaseNotes,
    });
    const outPath = path.join(outDir, name);
    fs.writeFileSync(outPath, yml, "utf8");
    generated[name] = outPath;
  }

  // 3. Linux x64: latest-linux.yml
  const linuxX64Names = [
    `Biu-${version}-linux-x86_64.AppImage`,
    `Biu-${version}-linux-amd64.deb`,
  ];
  const linuxX64Files = linuxX64Names
    .map(findEntry)
    .filter(Boolean)
    .map(statAndHashFile);
  if (linuxX64Files.some(f => f.url.endsWith(".AppImage"))) {
    const yml = formatUpdaterYaml({
      version,
      files: linuxX64Files,
      primaryFile: `Biu-${version}-linux-x86_64.AppImage`,
      releaseDate,
      releaseNotes,
    });
    const name = `${channel}-linux.yml`;
    const outPath = path.join(outDir, name);
    fs.writeFileSync(outPath, yml, "utf8");
    generated[name] = outPath;
  }

  // 4. Linux arm64: latest-linux-arm64.yml
  const linuxArm64Names = [
    `Biu-${version}-linux-arm64.AppImage`,
    `Biu-${version}-linux-arm64.deb`,
  ];
  const linuxArm64Files = linuxArm64Names
    .map(findEntry)
    .filter(Boolean)
    .map(statAndHashFile);
  if (linuxArm64Files.some(f => f.url.endsWith(".AppImage"))) {
    const yml = formatUpdaterYaml({
      version,
      files: linuxArm64Files,
      primaryFile: `Biu-${version}-linux-arm64.AppImage`,
      releaseDate,
      releaseNotes,
    });
    const name = `${channel}-linux-arm64.yml`;
    const outPath = path.join(outDir, name);
    fs.writeFileSync(outPath, yml, "utf8");
    generated[name] = outPath;
  }

  // 5. Tauri updater: latest.json (when .sig files are present)
  const tauriTargets = [
    {
      keys: ["darwin-aarch64", "darwin-aarch64-app"],
      asset: `Biu-${version}-mac-arm64.app.tar.gz`,
    },
    {
      keys: ["darwin-x86_64", "darwin-x86_64-app"],
      asset: `Biu-${version}-mac-x64.app.tar.gz`,
    },
    {
      keys: ["windows-x86_64", "windows-x86_64-nsis"],
      asset: `Biu-${version}-win-setup-x64.exe`,
    },
    {
      keys: ["windows-aarch64", "windows-aarch64-nsis"],
      asset: `Biu-${version}-win-setup-arm64.exe`,
    },
    {
      keys: ["linux-x86_64", "linux-x86_64-appimage"],
      asset: `Biu-${version}-linux-x86_64.AppImage`,
    },
    {
      keys: ["linux-aarch64", "linux-aarch64-appimage"],
      asset: `Biu-${version}-linux-arm64.AppImage`,
    },
  ];

  const platforms = {};
  for (const t of tauriTargets) {
    const assetPath = findEntry(t.asset);
    const sigPath = findEntry(`${t.asset}.sig`);
    if (assetPath && sigPath) {
      const signature = fs.readFileSync(sigPath, "utf8").trim();
      const downloadUrl = `${repoUrl.replace(/\/$/, "")}/releases/download/${tag}/${t.asset}`;
      for (const key of t.keys) {
        platforms[key] = {
          signature,
          url: downloadUrl,
        };
      }
    }
  }

  if (Object.keys(platforms).length > 0) {
    const latestJson = {
      version,
      notes: releaseNotes || `Release v${version}`,
      pub_date: releaseDate,
      platforms,
    };
    const outPath = path.join(outDir, "latest.json");
    fs.writeFileSync(outPath, `${JSON.stringify(latestJson, null, 2)}\n`, "utf8");
    generated["latest.json"] = outPath;
  }

  return generated;
}

function parseCliArgs(argv) {
  const args = {};
  for (let i = 0; i < argv.length; i++) {
    const arg = argv[i];
    if (arg === "--dir" && argv[i + 1]) {
      args.dir = argv[++i];
    } else if (arg === "--out-dir" && argv[i + 1]) {
      args.outDir = argv[++i];
    } else if (arg === "--version" && argv[i + 1]) {
      args.version = argv[++i];
    } else if (arg === "--release-date" && argv[i + 1]) {
      args.releaseDate = argv[++i];
    } else if (arg === "--notes" && argv[i + 1]) {
      args.releaseNotes = argv[++i];
    } else if (arg === "--repo-url" && argv[i + 1]) {
      args.repoUrl = argv[++i];
    } else if (arg === "--tag" && argv[i + 1]) {
      args.tag = argv[++i];
    } else if (arg === "--bridge-dir" && argv[i + 1]) {
      args.bridgeDir = argv[++i];
    } else if (arg === "--bridge-tag" && argv[i + 1]) {
      args.bridgeTag = argv[++i];
    }
  }
  return args;
}

if (process.argv[1] && path.resolve(process.argv[1]) === fileURLToPath(import.meta.url)) {
  const opts = parseCliArgs(process.argv.slice(2));
  if (!opts.dir || !opts.version) {
    console.error(
      "Usage: node scripts/electron-updater-feed.mjs --dir <artifacts-dir> --version <semver> [--out-dir <dir>] [--release-date <ISO>] [--notes <text>] [--tag <vX.Y.Z>] [--bridge-dir <dir> --bridge-tag <tag>]",
    );
    process.exit(1);
  }
  const generated = generateFeedsFromDir(opts);
  if (opts.bridgeDir) {
    Object.assign(
      generated,
      writeBridgeFeeds({ bridgeDir: opts.bridgeDir, bridgeTag: opts.bridgeTag, outDir: opts.outDir ?? opts.dir }),
    );
  }
  console.log("Generated feeds:", Object.keys(generated));
}
