// @vitest-environment node
import { createHash } from "node:crypto";
import fs from "node:fs";
import os from "node:os";
import path from "node:path";
import { afterEach, describe, expect, it } from "vitest";

import {
  formatUpdaterYaml,
  generateFeedsFromDir,
  parseUpdaterYaml,
  statAndHashFile,
} from "../../scripts/electron-updater-feed.mjs";

const tmpDirs: string[] = [];

afterEach(() => {
  for (const dir of tmpDirs.splice(0)) {
    fs.rmSync(dir, { recursive: true, force: true });
  }
});

function makeTempDir() {
  const dir = fs.mkdtempSync(path.join(os.tmpdir(), "biu-feed-test-"));
  tmpDirs.push(dir);
  return dir;
}

describe("electron-updater-feed generator", () => {
  it("computes base64 sha512 and exact byte size", () => {
    const dir = makeTempDir();
    const file = path.join(dir, "Biu-1.16.1-autolyrics.6-linux-arm64.AppImage");
    const payload = Buffer.from("biu-test-appimage-payload-bytes", "utf8");
    fs.writeFileSync(file, payload);

    const expectedSha512 = createHash("sha512").update(payload).digest("base64");
    const stat = statAndHashFile(file);
    expect(stat).toEqual({
      url: "Biu-1.16.1-autolyrics.6-linux-arm64.AppImage",
      sha512: expectedSha512,
      size: payload.byteLength,
    });
    expect(stat.sha512).toMatch(/^[A-Za-z0-9+/]{86}==$/);
  });

  it("formats latest*.yml bit-for-bit identical to v1.16.1-autolyrics.6 electron-builder output", () => {
    const macYml = formatUpdaterYaml({
      version: "1.16.1-autolyrics.6",
      files: [
        {
          url: "Biu-1.16.1-autolyrics.6-mac-x64.zip",
          sha512: "HmaR9LVAAMm9z5X4+VmmJgXL+aaAj+TL6xXxoWB6KsDSNINtu4+RbHOkQhu96kylWw1fra3nWWpuz5HlJvFKvg==",
          size: 135010444,
        },
        {
          url: "Biu-1.16.1-autolyrics.6-mac-arm64.zip",
          sha512: "oskmrHWrCYVLijvEomeloI2gYAefHy5JNkoKJQZeR1uUgHB+6jWXKwKrY97bMtQMw47xvZZe7xhy9XKnxynbhw==",
          size: 127176427,
        },
        {
          url: "Biu-1.16.1-autolyrics.6-mac-x64.dmg",
          sha512: "TNW+zX40/uhyaN0e9j4Cm+dZ40Li8xU8zgDr22bx56+wFzP5/X6JXpoepLM8zCW+2EvabjJZO8WBe59+22vOEQ==",
          size: 139155913,
        },
        {
          url: "Biu-1.16.1-autolyrics.6-mac-arm64.dmg",
          sha512: "6nWEyGMB85HHvDrNI2dIhA5Naip23L6OPcykjIdrknoYMGsP7x3Yqqsp7tAM4sT1O1Uoyram5tA3Dg8oo//6Lg==",
          size: 131265944,
        },
      ],
      primaryFile: "Biu-1.16.1-autolyrics.6-mac-x64.zip",
      releaseDate: "2026-11-04T06:47:51.435Z",
    });

    expect(macYml).toBe(
      [
        "version: 1.16.1-autolyrics.6",
        "files:",
        "  - url: Biu-1.16.1-autolyrics.6-mac-x64.zip",
        "    sha512: HmaR9LVAAMm9z5X4+VmmJgXL+aaAj+TL6xXxoWB6KsDSNINtu4+RbHOkQhu96kylWw1fra3nWWpuz5HlJvFKvg==",
        "    size: 135010444",
        "  - url: Biu-1.16.1-autolyrics.6-mac-arm64.zip",
        "    sha512: oskmrHWrCYVLijvEomeloI2gYAefHy5JNkoKJQZeR1uUgHB+6jWXKwKrY97bMtQMw47xvZZe7xhy9XKnxynbhw==",
        "    size: 127176427",
        "  - url: Biu-1.16.1-autolyrics.6-mac-x64.dmg",
        "    sha512: TNW+zX40/uhyaN0e9j4Cm+dZ40Li8xU8zgDr22bx56+wFzP5/X6JXpoepLM8zCW+2EvabjJZO8WBe59+22vOEQ==",
        "    size: 139155913",
        "  - url: Biu-1.16.1-autolyrics.6-mac-arm64.dmg",
        "    sha512: 6nWEyGMB85HHvDrNI2dIhA5Naip23L6OPcykjIdrknoYMGsP7x3Yqqsp7tAM4sT1O1Uoyram5tA3Dg8oo//6Lg==",
        "    size: 131265944",
        "path: Biu-1.16.1-autolyrics.6-mac-x64.zip",
        "sha512: HmaR9LVAAMm9z5X4+VmmJgXL+aaAj+TL6xXxoWB6KsDSNINtu4+RbHOkQhu96kylWw1fra3nWWpuz5HlJvFKvg==",
        "releaseDate: '2026-11-04T06:47:51.435Z'",
        "",
      ].join("\n"),
    );

    const parsed = parseUpdaterYaml(macYml);
    expect(parsed.version).toBe("1.16.1-autolyrics.6");
    expect(parsed.path).toBe("Biu-1.16.1-autolyrics.6-mac-x64.zip");
    expect(parsed.files).toHaveLength(4);
  });

  it("generates all 4 electron-updater YAML files and latest.json from an artifact directory", () => {
    const dir = makeTempDir();
    const version = "1.16.2";
    const assets: Record<string, string> = {
      [`Biu-${version}-mac-x64.zip`]: "mac-x64-zip",
      [`Biu-${version}-mac-arm64.zip`]: "mac-arm64-zip",
      [`Biu-${version}-mac-x64.dmg`]: "mac-x64-dmg",
      [`Biu-${version}-mac-arm64.dmg`]: "mac-arm64-dmg",
      [`Biu-${version}-mac-x64.app.tar.gz`]: "mac-x64-targz",
      [`Biu-${version}-mac-x64.app.tar.gz.sig`]: "sig-mac-x64",
      [`Biu-${version}-mac-arm64.app.tar.gz`]: "mac-arm64-targz",
      [`Biu-${version}-mac-arm64.app.tar.gz.sig`]: "sig-mac-arm64",
      [`Biu-${version}-win-setup-x64.exe`]: "win-x64-exe",
      [`Biu-${version}-win-setup-x64.exe.sig`]: "sig-win-x64",
      [`Biu-${version}-win-setup-arm64.exe`]: "win-arm64-exe",
      [`Biu-${version}-win-setup-arm64.exe.sig`]: "sig-win-arm64",
      [`Biu-${version}-linux-x86_64.AppImage`]: "linux-x64-appimage",
      [`Biu-${version}-linux-x86_64.AppImage.sig`]: "sig-linux-x64",
      [`Biu-${version}-linux-amd64.deb`]: "linux-x64-deb",
      [`Biu-${version}-linux-arm64.AppImage`]: "linux-arm64-appimage",
      [`Biu-${version}-linux-arm64.AppImage.sig`]: "sig-linux-arm64",
      [`Biu-${version}-linux-arm64.deb`]: "linux-arm64-deb",
    };

    for (const [name, content] of Object.entries(assets)) {
      fs.writeFileSync(path.join(dir, name), content, "utf8");
    }

    const generated = generateFeedsFromDir({
      dir,
      version,
      releaseDate: "2026-11-05T00:00:00.000Z",
      releaseNotes: "Step 4 release",
      tag: `v${version}`,
    });

    expect(Object.keys(generated).sort()).toEqual([
      "latest-linux-arm64.yml",
      "latest-linux.yml",
      "latest-mac.yml",
      "latest.json",
      "latest.yml",
    ]);

    const winYml = parseUpdaterYaml(fs.readFileSync(generated["latest.yml"], "utf8"));
    expect(winYml.path).toBe(`Biu-${version}-win-setup-x64.exe`);
    expect(winYml.files.map((f: { url: string }) => f.url)).toEqual([
      `Biu-${version}-win-setup-x64.exe`,
      `Biu-${version}-win-setup-arm64.exe`,
    ]);

    const linuxArmYml = parseUpdaterYaml(fs.readFileSync(generated["latest-linux-arm64.yml"], "utf8"));
    expect(linuxArmYml.path).toBe(`Biu-${version}-linux-arm64.AppImage`);
    expect(linuxArmYml.files.map((f: { url: string }) => f.url)).toEqual([
      `Biu-${version}-linux-arm64.AppImage`,
      `Biu-${version}-linux-arm64.deb`,
    ]);

    const latestJson = JSON.parse(fs.readFileSync(generated["latest.json"], "utf8")) as {
      version: string;
      notes: string;
      pub_date: string;
      platforms: Record<string, { signature: string; url: string }>;
    };
    expect(latestJson.version).toBe(version);
    expect(latestJson.notes).toBe("Step 4 release");
    expect(latestJson.platforms["darwin-aarch64"]).toEqual({
      signature: "sig-mac-arm64",
      url: `https://github.com/mianmian7/biu/releases/download/v${version}/Biu-${version}-mac-arm64.app.tar.gz`,
    });
    expect(latestJson.platforms["windows-x86_64"]).toEqual({
      signature: "sig-win-x64",
      url: `https://github.com/mianmian7/biu/releases/download/v${version}/Biu-${version}-win-setup-x64.exe`,
    });
    expect(latestJson.platforms["linux-aarch64"]).toEqual({
      signature: "sig-linux-arm64",
      url: `https://github.com/mianmian7/biu/releases/download/v${version}/Biu-${version}-linux-arm64.AppImage`,
    });
  });
});
