#!/usr/bin/env python3
"""Run S3 dependency & playback verification inside a clean Linux container (Ubuntu, Fedora, Arch)."""
import argparse
import glob
import json
import os
import subprocess
import sys
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument("--distro", required=True)
p.add_argument("--bundle-dir", type=Path, required=True)
p.add_argument("--root", type=Path, required=True)
p.add_argument("--out", type=Path, required=True)
args = p.parse_args()

root = args.root.resolve()
bundle_dir = args.bundle_dir.resolve()
evidence = root / "evidence"
evidence.mkdir(parents=True, exist_ok=True)
fixtures = root / "m0/fixtures"
fixtures.mkdir(parents=True, exist_ok=True)

distro_raw = args.distro
distro_slug = distro_raw.replace(":", "-").replace("/", "-")


def sh(cmd, check=True, env=None):
    return subprocess.run(
        cmd,
        shell=isinstance(cmd, str),
        text=True,
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        check=check,
        env=env,
    )


def gen_fixtures():
    sh(
        [
            "ffmpeg",
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=300",
            "-af",
            "aformat=sample_fmts=fltp:sample_rates=44100:channel_layouts=stereo",
            "-c:a",
            "aac",
            "-b:a",
            "128k",
            "-movflags",
            "+faststart",
            str(fixtures / "aac.m4a"),
        ]
    )
    sh(
        [
            "ffmpeg",
            "-y",
            "-hide_banner",
            "-loglevel",
            "error",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=10",
            "-af",
            "aformat=sample_fmts=s16:sample_rates=44100:channel_layouts=stereo",
            "-c:a",
            "flac",
            str(fixtures / "tone.flac"),
        ]
    )


def run_harness(name, mode, seconds, binary_path, extra_env=None):
    cmd = [
        "dbus-run-session",
        "--",
        "xvfb-run",
        "-a",
        "python3",
        str(root / "scripts/run_linux.py"),
        "--root",
        str(root),
        "--mode",
        mode,
        "--seconds",
        str(seconds),
        "--name",
        name,
        "--binary",
        str(binary_path),
    ]
    env = os.environ.copy()
    if extra_env:
        env.update(extra_env)
    res = sh(cmd, check=False, env=env)
    log_file = evidence / f"{name}.jsonl"
    events = []
    if log_file.exists():
        events = [json.loads(line) for line in log_file.read_text().splitlines() if line.strip()]
    return res.returncode, res.stdout, events


def make_isolated_gst_no_libav():
    res = sh("LC_ALL=C gst-inspect-1.0 coreelements | awk '/Filename/{print $2; exit}'", check=False)
    plugin_file = res.stdout.strip()
    if not plugin_file:
        for cand in ["/usr/lib/x86_64-linux-gnu/gstreamer-1.0", "/usr/lib64/gstreamer-1.0", "/usr/lib/gstreamer-1.0"]:
            if Path(cand).is_dir():
                plugin_Dir = Path(cand)
                break
        else:
            return {}
    else:
        plugin_Dir = Path(plugin_file).parent
    isolated = root / "m0/runtime/gst-no-libav"
    isolated.mkdir(parents=True, exist_ok=True)
    for so in plugin_Dir.glob("*.so"):
        if "libav" not in so.name:
            target = isolated / so.name
            if not target.exists():
                target.symlink_to(so)
    reg = root / "m0/runtime/isolated-registry.bin"
    if reg.exists():
        reg.unlink()
    return {
        "GST_PLUGIN_SYSTEM_PATH_1_0": str(isolated),
        "GST_PLUGIN_PATH_1_0": "",
        "GST_REGISTRY": str(reg),
    }


missing_ms = None
dep_result = {}
aac_ok = False
flac_ok = False
failure_reasons = []

try:
    if distro_raw.startswith("ubuntu"):
        sh("apt-get update")
        sh("DEBIAN_FRONTEND=noninteractive apt-get install -y python3 xvfb dbus-x11 openbox ffmpeg ca-certificates")
        gen_fixtures()

        # Install .deb dependencies EXCEPT gstreamer1.0-libav to test missing-decoder first
        sh(
            "DEBIAN_FRONTEND=noninteractive apt-get install -y libwebkit2gtk-4.1-0 libayatana-appindicator3-1 libgtk-3-0 gstreamer1.0-plugins-base gstreamer1.0-plugins-good gstreamer1.0-tools"
        )
        debs = sorted(bundle_dir.rglob("*.deb"))
        if not debs:
            raise RuntimeError(f"No .deb found in {bundle_dir}")
        deb = debs[0]

        # Extract binary from deb for step 1 (before apt install pulls gstreamer1.0-libav)
        extract_dir = Path("/tmp/biu-deb-extract")
        extract_dir.mkdir(parents=True, exist_ok=True)
        sh(["dpkg-deb", "-x", str(deb), str(extract_dir)])
        bin_path = extract_dir / "usr/bin/biu-m0"

        rc, out, evs = run_harness(f"s3-{distro_slug}-missing", "missing", 5, bin_path)
        md = [e for e in evs if e.get("event") == "missing_decoder"]
        if md and md[-1].get("detected") and md[-1].get("ms", 9999) <= 3000 and md[-1].get("updates") == 0:
            missing_ms = md[-1]["ms"]
        else:
            failure_reasons.append(f"missing_decoder check failed: rc={rc}, events={md}")

        # Step 2: apt install ./*.deb and verify gstreamer1.0-libav is pulled in
        apt_res = sh(f"DEBIAN_FRONTEND=noninteractive apt-get install -y '{deb}'", check=False)
        dpkg_res = sh("dpkg -s gstreamer1.0-libav", check=False)
        libav_installed = dpkg_res.returncode == 0 and "Status: install ok installed" in dpkg_res.stdout
        dep_result = {
            "manager": "apt",
            "package": deb.name,
            "apt_exit": apt_res.returncode,
            "gstreamer1.0-libav_pulled": libav_installed,
            "apt_output_tail": apt_res.stdout.splitlines()[-15:],
        }
        if apt_res.returncode != 0 or not libav_installed:
            failure_reasons.append(f"apt install failed or did not pull gstreamer1.0-libav: exit={apt_res.returncode}")

        # Step 3: AAC and FLAC playback
        rc, out, evs = run_harness(f"s3-{distro_slug}-playback", "full", 35, Path("/usr/bin/biu-m0"))
        aac_ev = [e for e in evs if e.get("event") == "aac_playing"]
        flac_ev = [e for e in evs if e.get("event") == "flac_playing"]
        aac_ok = bool(aac_ev and aac_ev[0].get("duration", 0) >= 300)
        flac_ok = bool(flac_ev and flac_ev[0].get("time", 0) > 0)
        if not aac_ok or not flac_ok or any(e.get("event") == "failure" for e in evs):
            failure_reasons.append(f"playback failed: rc={rc}, aac={aac_ok}, flac={flac_ok}")

    elif distro_raw.startswith("fedora"):
        rpms = sorted(bundle_dir.rglob("*.rpm"))
        if not rpms:
            raise RuntimeError(f"No .rpm found in {bundle_dir}")
        rpm = rpms[0]

        # Step 2a: Record default repo dnf install ./*.rpm first
        default_dnf = sh(["dnf", "install", "-y", str(rpm)], check=False)
        default_ok = default_dnf.returncode == 0

        # Install test harness tools (ffmpeg-free is in default Fedora repos)
        sh(
            "dnf install -y python3 xorg-x11-server-Xvfb dbus-x11 openbox ffmpeg-free cpio rpm WebKitGTK4.1 gtk3 libayatana-appindicator-gtk3 gstreamer1 gstreamer1-plugins-base gstreamer1-plugins-good",
            check=False,
        )
        gen_fixtures()

        rpmfusion_tried = False
        rpmfusion_dnf_exit = None
        rpmfusion_output_tail = []
        if not default_ok:
            rpmfusion_tried = True
            fed_ver = sh("rpm -E %fedora").stdout.strip()
            sh(
                f"dnf install -y https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-{fed_ver}.noarch.rpm https://mirrors.rpmfusion.org/nonfree/fedora/rpmfusion-nonfree-release-{fed_ver}.noarch.rpm",
                check=False,
            )
            rf_res = sh(["dnf", "install", "-y", str(rpm)], check=False)
            rpmfusion_dnf_exit = rf_res.returncode
            rpmfusion_output_tail = rf_res.stdout.splitlines()[-15:]

        dep_result = {
            "manager": "dnf",
            "package": rpm.name,
            "default_repo_exit": default_dnf.returncode,
            "default_repo_resolved": default_ok,
            "default_repo_output_tail": default_dnf.stdout.splitlines()[-15:],
            "rpmfusion_tested": rpmfusion_tried,
            "rpmfusion_exit": rpmfusion_dnf_exit,
            "rpmfusion_resolved": rpmfusion_dnf_exit == 0 if rpmfusion_tried else None,
            "rpmfusion_output_tail": rpmfusion_output_tail,
        }
        if not default_ok and rpmfusion_dnf_exit != 0:
            failure_reasons.append("dnf install failed with both default repos and RPM Fusion")

        bin_path = Path("/usr/bin/biu-m0")
        if not bin_path.exists():
            extract_dir = Path("/tmp/biu-rpm-extract")
            extract_dir.mkdir(parents=True, exist_ok=True)
            sh(f"cd {extract_dir} && rpm2cpio '{rpm}' | cpio -idm")
            bin_path = extract_dir / "usr/bin/biu-m0"

        # Step 1: Test missing gst-libav using isolated plugin directory without libgstlibav.so
        iso_env = make_isolated_gst_no_libav()
        rc, out, evs = run_harness(f"s3-{distro_slug}-missing", "missing", 5, bin_path, extra_env=iso_env)
        md = [e for e in evs if e.get("event") == "missing_decoder"]
        if md and md[-1].get("detected") and md[-1].get("ms", 9999) <= 3000 and md[-1].get("updates") == 0:
            missing_ms = md[-1]["ms"]
        else:
            failure_reasons.append(f"missing_decoder check failed: rc={rc}, events={md}")

        # Step 3: AAC and FLAC playback with gst-libav installed
        rc, out, evs = run_harness(f"s3-{distro_slug}-playback", "full", 35, bin_path)
        aac_ev = [e for e in evs if e.get("event") == "aac_playing"]
        flac_ev = [e for e in evs if e.get("event") == "flac_playing"]
        aac_ok = bool(aac_ev and aac_ev[0].get("duration", 0) >= 300)
        flac_ok = bool(flac_ev and flac_ev[0].get("time", 0) > 0)
        if not aac_ok or not flac_ok or any(e.get("event") == "failure" for e in evs):
            failure_reasons.append(f"playback failed: rc={rc}, aac={aac_ok}, flac={flac_ok}")

    elif distro_raw.startswith("arch"):
        appimages = sorted(bundle_dir.rglob("*.AppImage"))
        if not appimages:
            raise RuntimeError(f"No .AppImage found in {bundle_dir}")
        appimage = appimages[0]
        appimage.chmod(0o755)

        pacman_pkgs = [
            "python",
            "xorg-server-xvfb",
            "dbus",
            "openbox",
            "ffmpeg",
            "fuse2",
            "webkit2gtk-4.1",
            "gtk3",
            "libayatana-appindicator",
            "gstreamer",
            "gst-plugins-base",
            "gst-plugins-good",
        ]
        pac_base = sh(["pacman", "-Syu", "--noconfirm", *pacman_pkgs], check=False)
        gen_fixtures()

        # Extract AppImage to test binary without bundled gst-libav first, or run AppImage with isolated GST env
        iso_env = make_isolated_gst_no_libav()
        iso_env["APPIMAGE_EXTRACT_AND_RUN"] = "1"
        rc, out, evs = run_harness(f"s3-{distro_slug}-missing", "missing", 5, appimage, extra_env=iso_env)
        md = [e for e in evs if e.get("event") == "missing_decoder"]
        if md and md[-1].get("detected") and md[-1].get("ms", 9999) <= 3000 and md[-1].get("updates") == 0:
            missing_ms = md[-1]["ms"]
        else:
            failure_reasons.append(f"missing_decoder check failed: rc={rc}, events={md}")

        # Install gst-libav on Arch and test playback via AppImage (--appimage-extract-and-run)
        pac_libav = sh(["pacman", "-S", "--noconfirm", "gst-libav"], check=False)
        dep_result = {
            "manager": "pacman",
            "artifact": appimage.name,
            "run_flag": "--appimage-extract-and-run",
            "pacman_packages": pacman_pkgs + ["gst-libav"],
            "pacman_base_exit": pac_base.returncode,
            "pacman_gst_libav_exit": pac_libav.returncode,
        }
        if pac_base.returncode != 0 or pac_libav.returncode != 0:
            failure_reasons.append(f"pacman install failed: base={pac_base.returncode}, libav={pac_libav.returncode}")

        # Create a wrapper script that passes --appimage-extract-and-run
        wrapper = Path("/tmp/run-appimage.sh")
        wrapper.write_text(f"#!/usr/bin/env bash\nexec '{appimage}' --appimage-extract-and-run \"$@\"\n")
        wrapper.chmod(0o755)

        rc, out, evs = run_harness(f"s3-{distro_slug}-playback", "full", 35, wrapper)
        aac_ev = [e for e in evs if e.get("event") == "aac_playing"]
        flac_ev = [e for e in evs if e.get("event") == "flac_playing"]
        aac_ok = bool(aac_ev and aac_ev[0].get("duration", 0) >= 300)
        flac_ok = bool(flac_ev and flac_ev[0].get("time", 0) > 0)
        if not aac_ok or not flac_ok or any(e.get("event") == "failure" for e in evs):
            failure_reasons.append(f"playback failed: rc={rc}, aac={aac_ok}, flac={flac_ok}")
    else:
        failure_reasons.append(f"unsupported distro: {distro_raw}")
except Exception as exc:
    failure_reasons.append(str(exc))

report_obj = {
    "distro": distro_raw,
    "missing_libav_detection_ms": missing_ms,
    "dependency_resolution": dep_result,
    "AAC": aac_ok,
    "FLAC": flac_ok,
    "failure_reason": "; ".join(failure_reasons) if failure_reasons else None,
}

args.out.parent.mkdir(parents=True, exist_ok=True)
args.out.write_text(json.dumps(report_obj, indent=2, ensure_ascii=False) + "\n")
print(json.dumps(report_obj, ensure_ascii=False))
if failure_reasons:
    sys.exit(1)
