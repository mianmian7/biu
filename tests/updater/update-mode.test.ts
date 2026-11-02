import { describe, expect, it } from "vitest";

import { parseCodesignOutput, resolveUpdateMode } from "../../electron/updater/update-mode";

const mac = {
  platform: "darwin" as const,
  env: {},
  execPath: "/Applications/Biu.app/Contents/MacOS/Biu",
  codeSignatureKind: "certificate" as const,
  bundleParentWritable: true,
};

describe("resolveUpdateMode", () => {
  it("allows installed Windows apps", () => {
    expect(resolveUpdateMode({ platform: "win32", env: {}, execPath: "C:\\Biu\\Biu.exe" })).toBe("auto");
  });
  it.each(["PORTABLE_EXECUTABLE_DIR", "PORTABLE_EXECUTABLE_FILE"])("keeps Windows %s manual", key => {
    expect(resolveUpdateMode({ platform: "win32", env: { [key]: "C:\\Biu" }, execPath: "Biu.exe" })).toBe("manual");
  });
  it("allows Linux AppImage only", () => {
    expect(
      resolveUpdateMode({ platform: "linux", env: { APPIMAGE: "/home/user/Biu.AppImage" }, execPath: "/tmp/Biu" }),
    ).toBe("auto");
    expect(resolveUpdateMode({ platform: "linux", env: {}, execPath: "/opt/Biu/biu" })).toBe("manual");
  });
  it("allows a certificate-signed writable Mac installation", () => {
    expect(resolveUpdateMode(mac)).toBe("auto");
  });
  it.each(["adhoc", "unsigned"] as const)("keeps %s Mac apps manual", codeSignatureKind => {
    expect(resolveUpdateMode({ ...mac, codeSignatureKind })).toBe("manual");
  });
  it.each([
    "/private/var/folders/test/AppTranslocation/UUID/d/Biu.app/Contents/MacOS/Biu",
    "/Volumes/Biu/Biu.app/Contents/MacOS/Biu",
  ])("keeps relocated Mac app manual: %s", execPath => {
    expect(resolveUpdateMode({ ...mac, execPath })).toBe("manual");
  });
  it("keeps unwritable or unknown Mac installations manual", () => {
    expect(resolveUpdateMode({ ...mac, bundleParentWritable: false })).toBe("manual");
    expect(resolveUpdateMode({ ...mac, codeSignatureKind: undefined })).toBe("manual");
    expect(resolveUpdateMode({ ...mac, bundleParentWritable: undefined })).toBe("manual");
  });
  it("fails closed on unsupported platforms", () => {
    expect(resolveUpdateMode({ platform: "freebsd", env: {}, execPath: "/bin/biu" })).toBe("manual");
  });
});

describe("parseCodesignOutput", () => {
  // Representative codesign -dv --verbose=2 stderr fragments. No macOS host is
  // available here; on-device output must still be checked at the spike gate.
  it.each([
    [
      "Executable=/Applications/Biu.app/Contents/MacOS/Biu\nIdentifier=com.biu.wood3n\nSignature=adhoc\nTeamIdentifier=not set",
      "adhoc",
    ],
    ["/Applications/Biu.app: code object is not signed at all", "unsigned"],
    [
      "Executable=/Applications/Biu.app/Contents/MacOS/Biu\nIdentifier=com.biu.wood3n\nAuthority=Biu Test Signing\nTeamIdentifier=not set",
      "certificate",
    ],
    ["", "unsigned"],
    ["codesign failed unexpectedly", "unsigned"],
    ["Signature=adhoc\nAuthority=unexpected", "adhoc"],
  ])("parses %s", (stderr, expected) => {
    expect(parseCodesignOutput(stderr)).toBe(expected);
  });
});
