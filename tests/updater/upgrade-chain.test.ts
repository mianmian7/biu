// @vitest-environment node
// Feeds the generated release files to the real electron-updater 6.6.2 GitHubProvider,
// the version Electron `.6` and the `.7` bridge ship, and checks which release each client downloads.
import fs from "node:fs";
import { createRequire } from "node:module";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import { generateFeedsFromDir, writeBridgeFeeds } from "../../scripts/electron-updater-feed.mjs";

// builder-util-runtime is electron-updater's own dependency; load the same instance it uses.
const requireFromUpdater = createRequire(createRequire(import.meta.url).resolve("electron-updater"));
const { HttpError } = requireFromUpdater("builder-util-runtime") as {
  HttpError: new (statusCode: number, description: string) => Error;
};
// Loaded untyped: electron-updater's .d.ts files need the Electron types, which this repo no longer has.
type ResolvedFile = { url: URL };
type Provider = {
  getLatestVersion: () => Promise<{ version: string }>;
  resolveFiles: (info: unknown) => ResolvedFile[];
};
const { GitHubProvider } = requireFromUpdater("./providers/GitHubProvider") as {
  GitHubProvider: new (options: unknown, updater: unknown, runtimeOptions: unknown) => Provider;
};

const REPO = "/mianmian7/biu/releases";
const TAURI_VERSION = "2.0.0";
const TAURI_TAG = `v${TAURI_VERSION}`;
const BRIDGE_TAG = "v1.16.1-autolyrics.7";
const fixtureDir = path.join(import.meta.dirname, "fixtures", "bridge");

const tmpDirs: string[] = [];
afterEach(() => {
  delete process.env.TEST_UPDATER_ARCH;
  for (const dir of tmpDirs.splice(0)) fs.rmSync(dir, { recursive: true, force: true });
});

function buildTauriRelease(): string {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "biu-chain-"));
  tmpDirs.push(dir);
  const v = TAURI_VERSION;
  const names = [
    `Biu-${v}-mac-arm64.zip`,
    `Biu-${v}-mac-x64.zip`,
    `Biu-${v}-mac-arm64.dmg`,
    `Biu-${v}-mac-x64.dmg`,
    `Biu-${v}-win-setup-x64.exe`,
    `Biu-${v}-win-setup-arm64.exe`,
    `Biu-${v}-linux-x86_64.AppImage`,
    `Biu-${v}-linux-amd64.deb`,
    `Biu-${v}-linux-arm64.AppImage`,
    `Biu-${v}-linux-arm64.deb`,
  ];
  for (const name of names) fs.writeFileSync(path.join(dir, name), name);
  generateFeedsFromDir({ dir, version: v, tag: TAURI_TAG, releaseDate: "2026-11-05T00:00:00.000Z" });
  writeBridgeFeeds({ bridgeDir: fixtureDir, bridgeTag: BRIDGE_TAG, outDir: dir });
  return dir;
}

function atomFeed(tags: string[]): string {
  const entries = tags
    .map(
      tag =>
        `<entry><id>${tag}</id><updated>2026-11-05T00:00:00Z</updated>` +
        `<link rel="alternate" type="text/html" href="https://github.com${REPO}/tag/${tag}"/>` +
        `<title>${tag}</title><content type="html">notes</content></entry>`,
    )
    .join("");
  return `<?xml version="1.0" encoding="UTF-8"?><feed xmlns="http://www.w3.org/2005/Atom">${entries}</feed>`;
}

async function resolveUpdate(opts: {
  releaseDir: string;
  platform: "darwin" | "win32" | "linux";
  channel?: string;
  currentVersion: string;
}) {
  const updater = {
    allowPrerelease: false,
    channel: opts.channel ?? null,
    currentVersion: opts.currentVersion,
    fullChangelog: false,
  };
  const requested: string[] = [];
  const executor = {
    request: async (options: { path: string }) => {
      const pathname = options.path.split("?")[0];
      requested.push(pathname);
      if (pathname === `${REPO}.atom`) return atomFeed([TAURI_TAG, BRIDGE_TAG, "v1.16.1-autolyrics.6"]);
      if (pathname === `${REPO}/latest`) return JSON.stringify({ tag_name: TAURI_TAG });
      const prefix = `${REPO}/download/${TAURI_TAG}/`;
      if (pathname.startsWith(prefix)) {
        const file = path.join(opts.releaseDir, pathname.slice(prefix.length));
        if (fs.existsSync(file)) return fs.readFileSync(file, "utf8");
      }
      throw new HttpError(404, `not found: ${pathname}`);
    },
  };
  const provider = new GitHubProvider({ provider: "github", owner: "mianmian7", repo: "biu" }, updater, {
    platform: opts.platform,
    executor,
    isUseMultipleRangeRequest: true,
  });
  const info = await provider.getLatestVersion();
  const files = provider.resolveFiles(info).map(f => f.url.pathname);
  return { version: info.version, files, requested };
}

describe("Electron → bridge → Tauri upgrade chain (electron-updater 6.6.2)", () => {
  it("sends Electron .6 clients to the .7 bridge on every platform", async () => {
    const releaseDir = buildTauriRelease();
    const cases = [
      { platform: "darwin", arch: undefined, feed: "latest-mac.yml" },
      { platform: "win32", arch: undefined, feed: "latest.yml" },
      { platform: "linux", arch: "x64", feed: "latest-linux.yml" },
      { platform: "linux", arch: "arm64", feed: "latest-linux-arm64.yml" },
    ] as const;
    for (const c of cases) {
      if (c.arch) process.env.TEST_UPDATER_ARCH = c.arch;
      const r = await resolveUpdate({ releaseDir, platform: c.platform, currentVersion: "1.16.1-autolyrics.6" });
      expect(r.requested).toContain(`${REPO}/download/${TAURI_TAG}/${c.feed}`);
      expect(r.version).toBe("1.16.1-autolyrics.7");
      expect(r.files.length).toBeGreaterThan(0);
      for (const file of r.files) {
        expect(file.startsWith(`${REPO}/download/${BRIDGE_TAG}/Biu-1.16.1-autolyrics.7-`)).toBe(true);
      }
    }
  });

  it("sends the .7 bridge (channel tauri) to the Tauri installers", async () => {
    const releaseDir = buildTauriRelease();
    const base = `${REPO}/download/${TAURI_TAG}/Biu-${TAURI_VERSION}`;
    const cases = [
      { platform: "darwin", channel: "tauri", arch: undefined, first: `${base}-mac-arm64.zip` },
      { platform: "win32", channel: "tauri", arch: undefined, first: `${base}-win-setup-x64.exe` },
      { platform: "win32", channel: "tauri-arm64", arch: undefined, first: `${base}-win-setup-arm64.exe` },
      { platform: "linux", channel: "tauri", arch: "x64", first: `${base}-linux-x86_64.AppImage` },
      { platform: "linux", channel: "tauri", arch: "arm64", first: `${base}-linux-arm64.AppImage` },
    ] as const;
    for (const c of cases) {
      if (c.arch) process.env.TEST_UPDATER_ARCH = c.arch;
      const r = await resolveUpdate({
        releaseDir,
        platform: c.platform,
        channel: c.channel,
        currentVersion: "1.16.1-autolyrics.7",
      });
      expect(r.version).toBe(TAURI_VERSION);
      if (c.platform === "darwin") {
        // MacUpdater picks the zip matching the CPU, so both arches must be present.
        expect(r.files).toContain(`${base}-mac-arm64.zip`);
        expect(r.files).toContain(`${base}-mac-x64.zip`);
      } else {
        // NsisUpdater and AppImageUpdater take the first matching file.
        expect(r.files[0]).toBe(c.first);
      }
      if (c.platform === "win32") expect(r.files).toHaveLength(1);
    }
  });
});
