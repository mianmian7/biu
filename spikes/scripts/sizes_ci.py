#!/usr/bin/env python3
"""Measure S4 installer bundles and verify non-empty embedded web/assets."""
import argparse
import hashlib
import json
import sys
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument("--bundle-dir", type=Path, default=Path("m0/src-tauri/target/release/bundle"))
p.add_argument("--web-dir", type=Path, default=Path("m0/web"))
p.add_argument("--out", type=Path)
args = p.parse_args()

web_dir = args.web_dir.resolve()
assets_dir = web_dir / "assets"
if not assets_dir.is_dir():
    sys.exit(f"ERROR: web/assets directory missing: {assets_dir}")

assets_files = [f for f in sorted(assets_dir.rglob("*")) if f.is_file() and not f.is_symlink()]
if not assets_files:
    sys.exit(f"ERROR: web/assets is empty: {assets_dir}")

web_files = [f for f in sorted(web_dir.rglob("*")) if f.is_file() and not f.is_symlink()]
assets_bytes = sum(f.stat().st_size for f in assets_files)
web_bytes = sum(f.stat().st_size for f in web_files)
if assets_bytes == 0:
    sys.exit("ERROR: web/assets has 0 total bytes")

bundle_dir = args.bundle_dir.resolve()
if not bundle_dir.is_dir():
    sys.exit(f"ERROR: bundle directory not found: {bundle_dir}")

INSTALLER_EXTS = {".dmg", ".exe", ".deb", ".rpm", ".appimage", ".msi"}
installers = []
for f in sorted(bundle_dir.rglob("*")):
    if f.is_file() and not f.is_symlink() and f.suffix.lower() in INSTALLER_EXTS:
        size = f.stat().st_size
        installers.append(
            {
                "name": f.name,
                "relpath": f.relative_to(bundle_dir).as_posix(),
                "bytes": size,
                "MB": size / 1_000_000,
                "sha256": hashlib.sha256(f.read_bytes()).hexdigest(),
                "frontend_files": len(web_files),
                "frontend_bytes": web_bytes,
                "web_assets_files": len(assets_files),
                "web_assets_bytes": assets_bytes,
            }
        )

if not installers:
    sys.exit(f"ERROR: no installer packages found under {bundle_dir}")

app_bundles = []
for app in sorted(bundle_dir.glob("macos/*.app")):
    if app.is_dir():
        files = [f for f in app.rglob("*") if f.is_file() and not f.is_symlink()]
        size = sum(f.stat().st_size for f in files)
        app_bundles.append(
            {
                "name": app.name,
                "files": len(files),
                "bytes": size,
                "MB": size / 1_000_000,
            }
        )

result = {
    "unit": "MB = 1000000 bytes",
    "frontend": {
        "files": len(web_files),
        "bytes": web_bytes,
        "MB": web_bytes / 1_000_000,
        "web_assets_non_empty": True,
        "web_assets_files": len(assets_files),
        "web_assets_bytes": assets_bytes,
        "web_assets_MB": assets_bytes / 1_000_000,
    },
    "installers": installers,
}
if app_bundles:
    result["macos_app_bundles"] = app_bundles

output = json.dumps(result, indent=2) + "\n"
if args.out:
    args.out.parent.mkdir(parents=True, exist_ok=True)
    args.out.write_text(output)
print(output, end="")
