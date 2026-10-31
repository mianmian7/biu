// @vitest-environment node
import { readFileSync } from "node:fs";
import { describe, expect, it, vi } from "vitest";

import {
  DecodeUnavailableError,
  executableArch,
  selectRunnableFfmpeg,
} from "../../electron/auto-lyrics/ffmpeg-resolve";

function elf(machine: number): Uint8Array {
  const header = new Uint8Array(64);
  header.set([0x7f, 0x45, 0x4c, 0x46, 2, 1]);
  header[18] = machine & 0xff;
  header[19] = (machine >> 8) & 0xff;
  return header;
}

function pick(
  headers: Record<string, Uint8Array | undefined>,
  probe: (candidate: string) => "ok" | "enoexec" | "failed" = () => "ok",
) {
  return selectRunnableFfmpeg({
    userPaths: ["/settings/ffmpeg", "/env/ffmpeg"],
    systemPaths: ["/usr/bin/ffmpeg"],
    bundledPath: "/app/ffmpeg-linux",
    processArch: "arm64",
    exists: candidate => headers[candidate] !== undefined,
    readHeader: candidate => headers[candidate],
    probe,
  });
}

describe("ffmpeg selection on a mismatched bundle", () => {
  it("reads ELF, Mach-O, and PE machines", () => {
    expect(executableArch(elf(62))).toBe("x64");
    expect(executableArch(elf(183))).toBe("arm64");
    const macho = new Uint8Array(32);
    macho.set([0xcf, 0xfa, 0xed, 0xfe]);
    new DataView(macho.buffer).setInt32(4, 0x0100000c, true);
    expect(executableArch(macho)).toBe("arm64");
    const pe = new Uint8Array(0x80);
    pe.set([0x4d, 0x5a]);
    new DataView(pe.buffer).setInt32(0x3c, 0x40, true);
    pe.set([0x50, 0x45, 0, 0, 0x64, 0x86], 0x40);
    expect(executableArch(pe)).toBe("x64");
    expect(executableArch(new Uint8Array([0x23, 0x21]))).toBeNull();
  });

  it("recognizes the shipped linux ffmpeg as x64", () => {
    const shipped = readFileSync("electron/ffmpeg/ffmpeg-linux").subarray(0, 64);
    expect(executableArch(shipped)).toBe("x64");
  });

  it("uses the user binary when the bundle is the wrong architecture", () => {
    const probe = vi.fn(() => "ok" as const);
    expect(
      pick(
        {
          "/settings/ffmpeg": elf(183),
          "/app/ffmpeg-linux": elf(62),
        },
        probe,
      ),
    ).toBe("/settings/ffmpeg");
    expect(probe).toHaveBeenCalledTimes(1);
    expect(probe).toHaveBeenCalledWith("/settings/ffmpeg");
  });

  it("skips a wrong-arch user path, then an ENOEXEC system binary, and keeps a runnable bundle", () => {
    const probe = vi.fn((candidate: string) => (candidate === "/usr/bin/ffmpeg" ? "enoexec" : "ok"));
    expect(
      pick(
        {
          "/settings/ffmpeg": elf(62),
          "/usr/bin/ffmpeg": new Uint8Array([0x7f, 0x45, 0x4c, 0x46]),
          "/app/ffmpeg-linux": elf(183),
        },
        probe,
      ),
    ).toBe("/app/ffmpeg-linux");
    expect(probe.mock.calls.map(call => call[0])).toEqual(["/usr/bin/ffmpeg", "/app/ffmpeg-linux"]);
  });

  it("uses BIU_FFMPEG_PATH after the settings path is rejected", () => {
    const probe = vi.fn(() => "ok" as const);
    expect(
      pick(
        {
          "/settings/ffmpeg": elf(62),
          "/env/ffmpeg": elf(183),
          "/app/ffmpeg-linux": elf(62),
        },
        probe,
      ),
    ).toBe("/env/ffmpeg");
    expect(probe).toHaveBeenCalledTimes(1);
  });

  it("uses a system binary when the bundle cannot execute", () => {
    expect(
      pick({
        "/usr/bin/ffmpeg": new Uint8Array([0x23, 0x21]),
        "/app/ffmpeg-linux": elf(62),
      }),
    ).toBe("/usr/bin/ffmpeg");
  });

  it("reports decode-unavailable when every candidate is missing or the wrong architecture", () => {
    const probe = vi.fn(() => "ok" as const);
    expect(() => pick({ "/app/ffmpeg-linux": elf(62) }, probe)).toThrow(DecodeUnavailableError);
    expect(probe).not.toHaveBeenCalled();
    expect(() => pick({ "/settings/ffmpeg": elf(62), "/app/ffmpeg-linux": elf(183) }, () => "enoexec")).toThrow(
      /decode-unavailable:.*enoexec/,
    );
  });
});
