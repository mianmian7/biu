import { execFile } from "node:child_process";
import { constants, promises as fs } from "node:fs";
import path from "node:path";
import { promisify } from "node:util";

export type UpdateMode = "auto" | "manual";
type CodeSignatureKind = "certificate" | "adhoc" | "unsigned";

interface UpdateModeOptions {
  platform: NodeJS.Platform;
  env: NodeJS.ProcessEnv;
  execPath: string;
  codeSignatureKind?: CodeSignatureKind;
  bundleParentWritable?: boolean;
}

export function resolveUpdateMode(options: UpdateModeOptions): UpdateMode {
  const { platform, env, execPath, codeSignatureKind, bundleParentWritable } = options;
  if (platform === "win32") {
    return env.PORTABLE_EXECUTABLE_DIR || env.PORTABLE_EXECUTABLE_FILE ? "manual" : "auto";
  }
  if (platform === "linux") return env.APPIMAGE ? "auto" : "manual";
  if (platform === "darwin") {
    if (execPath.includes("/AppTranslocation/") || execPath.startsWith("/Volumes/")) return "manual";
    return codeSignatureKind === "certificate" && bundleParentWritable ? "auto" : "manual";
  }
  return "manual";
}

export function parseCodesignOutput(stderr: string): CodeSignatureKind {
  if (stderr.includes("Signature=adhoc")) return "adhoc";
  if (stderr.includes("code object is not signed")) return "unsigned";
  if (stderr.includes("Authority=")) return "certificate";
  return "unsigned";
}

let cachedMode: Promise<UpdateMode> | undefined;

// Keep IPC mode queries and install guards consistent after a native signature failure.
export function disableAutoUpdate(): void {
  cachedMode = Promise.resolve("manual");
}

export function detectUpdateMode(): Promise<UpdateMode> {
  cachedMode ??= (async () => {
    const { app } = await import("electron");
    const { default: log } = await import("electron-log");
    const { default: isDev } = await import("electron-is-dev");
    if (isDev || !app.isPackaged) {
      log.info("[updater] mode=manual");
      return "manual";
    }
    const execPath = app.getPath("exe");
    let codeSignatureKind: CodeSignatureKind = "unsigned";
    let bundleParentWritable = false;
    if (process.platform === "darwin") {
      const bundlePath = path.resolve(execPath, "../../..");
      try {
        const { stderr } = await promisify(execFile)("/usr/bin/codesign", ["-dv", "--verbose=2", bundlePath]);
        codeSignatureKind = parseCodesignOutput(stderr);
      } catch (error) {
        // Unsigned apps cause codesign to exit non-zero. Unknown failures stay manual.
        codeSignatureKind = parseCodesignOutput(String((error as { stderr?: string }).stderr ?? ""));
      }
      try {
        await fs.access(path.dirname(bundlePath), constants.W_OK);
        bundleParentWritable = true;
      } catch {
        // An unwritable installation cannot be replaced by Squirrel.
      }
    }
    const mode = resolveUpdateMode({
      platform: process.platform,
      env: process.env,
      execPath,
      codeSignatureKind,
      bundleParentWritable,
    });
    log.info(`[updater] mode=${mode}`);
    return mode;
  })();
  return cachedMode;
}
