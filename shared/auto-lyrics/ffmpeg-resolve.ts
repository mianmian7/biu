export const DECODE_UNAVAILABLE = "decode-unavailable";

/** ffmpeg cannot be executed on this machine. Auto lyrics stops; the app does not. */
export class DecodeUnavailableError extends Error {
  readonly code = DECODE_UNAVAILABLE;

  constructor(detail: string) {
    super(`${DECODE_UNAVAILABLE}: ${detail}`);
    this.name = "DecodeUnavailableError";
  }
}

export function isDecodeUnavailable(error: unknown): boolean {
  return (
    error instanceof DecodeUnavailableError ||
    (typeof error === "object" &&
      error !== null &&
      "code" in error &&
      (error as { code?: unknown }).code === DECODE_UNAVAILABLE) ||
    (error instanceof Error && error.message.startsWith(`${DECODE_UNAVAILABLE}:`))
  );
}

/** Common install locations. User and bundled paths are supplied by the caller. */
export const SYSTEM_FFMPEG_PATHS = [
  "/opt/homebrew/bin/ffmpeg",
  "/usr/local/bin/ffmpeg",
  "/usr/bin/ffmpeg",
  "/snap/bin/ffmpeg",
] as const;

const ELF_MACHINES: Record<number, NodeJS.Architecture> = {
  3: "ia32",
  40: "arm",
  62: "x64",
  183: "arm64",
};

const MACHO_CPU: Record<number, NodeJS.Architecture> = {
  7: "ia32",
  12: "arm",
  0x01000007: "x64",
  0x0100000c: "arm64",
};

const PE_MACHINES: Record<number, NodeJS.Architecture> = {
  0x14c: "ia32",
  0x1c0: "arm",
  0x8664: "x64",
  0xaa64: "arm64",
};

/**
 * Positive architecture of an ELF, thin Mach-O, or PE header.
 * Returns null when the container is unknown so the caller probes instead of guessing.
 */
export function executableArch(header: Uint8Array): NodeJS.Architecture | null {
  if (header.length >= 20 && header[0] === 0x7f && header[1] === 0x45 && header[2] === 0x4c && header[3] === 0x46) {
    const little = header[5] === 1;
    if (!little && header[5] !== 2) return null;
    const machine = little ? header[18] | (header[19] << 8) : (header[18] << 8) | header[19];
    return ELF_MACHINES[machine] ?? null;
  }
  if (header.length >= 8) {
    // On-disk magic, not the host-endian constant. Fat images are left unknown and probed.
    const signature = (header[0] | (header[1] << 8) | (header[2] << 16) | (header[3] << 24)) >>> 0;
    const little = signature === 0xfeedface || signature === 0xfeedfacf;
    const big = signature === 0xcefaedfe || signature === 0xcffaedfe;
    if (little || big) {
      const cpu = readI32(header, 4, little);
      return MACHO_CPU[cpu >>> 0] ?? null;
    }
  }
  if (header.length >= 0x40 && header[0] === 0x4d && header[1] === 0x5a) {
    const lfanew = readI32(header, 0x3c, true);
    if (lfanew < 0 || lfanew + 6 > header.length) return null;
    if (
      header[lfanew] !== 0x50 ||
      header[lfanew + 1] !== 0x45 ||
      header[lfanew + 2] !== 0 ||
      header[lfanew + 3] !== 0
    ) {
      return null;
    }
    const machine = header[lfanew + 4] | (header[lfanew + 5] << 8);
    return PE_MACHINES[machine] ?? null;
  }
  return null;
}

export function bundledFfmpegFileName(platform: NodeJS.Platform, arch: string): string {
  if (platform === "win32") return "ffmpeg.exe";
  if (platform === "darwin") return arch === "arm64" ? "ffmpeg-mac-arm64" : "ffmpeg-mac-x64";
  if (platform === "linux") return "ffmpeg-linux";
  return "ffmpeg";
}

export interface FfmpegPickInput {
  /** Settings path, then optional env override. Tried before system and bundled binaries. */
  userPaths: readonly string[];
  systemPaths: readonly string[];
  bundledPath: string;
  processArch: string;
  exists(candidate: string): boolean;
  readHeader(candidate: string): Uint8Array | undefined;
  /** Run the candidate. `enoexec` means the kernel rejected the image. */
  probe(candidate: string): "ok" | "enoexec" | "failed";
}

/**
 * Prefer a runnable user binary, then a system binary, then the bundled one.
 * A bundled (or any) binary whose header is a different CPU, or whose exec fails
 * with ENOEXEC, is skipped. Nothing runnable becomes decode-unavailable.
 */
export function selectRunnableFfmpeg(input: FfmpegPickInput): string {
  const ordered = [...input.userPaths, ...input.systemPaths, input.bundledPath];
  const seen = new Set<string>();
  const rejected: string[] = [];
  for (const candidate of ordered) {
    if (!candidate || seen.has(candidate)) continue;
    seen.add(candidate);
    if (!input.exists(candidate)) {
      rejected.push(`${candidate}: missing`);
      continue;
    }
    const header = input.readHeader(candidate);
    const arch = header ? executableArch(header) : null;
    if (arch && arch !== input.processArch) {
      rejected.push(`${candidate}: arch ${arch}`);
      continue;
    }
    const probed = input.probe(candidate);
    if (probed !== "ok") {
      rejected.push(`${candidate}: ${probed}`);
      continue;
    }
    return candidate;
  }
  throw new DecodeUnavailableError(rejected.join("; ") || "no ffmpeg candidate");
}

function readI32(header: Uint8Array, offset: number, little: boolean): number {
  if (offset + 4 > header.length) return 0;
  const view = new DataView(header.buffer, header.byteOffset, header.byteLength);
  return view.getInt32(offset, little);
}
