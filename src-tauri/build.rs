use std::env;
use std::fs;
use std::path::PathBuf;

fn stage_ffmpeg_sidecar() {
    println!("cargo:rerun-if-changed=ffmpeg");
    let Ok(target) = env::var("TARGET") else {
        return;
    };
    let manifest_dir = PathBuf::from(env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into()));
    let binaries_dir = manifest_dir.join("binaries");
    let _ = fs::create_dir_all(&binaries_dir);

    let is_windows = target.contains("windows");
    let ext = if is_windows { ".exe" } else { "" };
    let dest_name = format!("ffmpeg-{target}{ext}");
    let dest_path = binaries_dir.join(&dest_name);

    let ffmpeg_root = manifest_dir.join("ffmpeg");
    let src_candidate = if is_windows {
        if target.starts_with("aarch64-") && ffmpeg_root.join("ffmpeg-win-arm64.exe").exists() {
            Some(ffmpeg_root.join("ffmpeg-win-arm64.exe"))
        } else {
            Some(ffmpeg_root.join("ffmpeg.exe"))
        }
    } else if target.contains("apple-darwin") {
        if target.starts_with("aarch64-") {
            Some(ffmpeg_root.join("ffmpeg-mac-arm64"))
        } else {
            Some(ffmpeg_root.join("ffmpeg-mac-x64"))
        }
    } else if target.contains("linux") {
        if target.starts_with("aarch64-") {
            Some(ffmpeg_root.join("ffmpeg-linux-arm64"))
        } else {
            Some(ffmpeg_root.join("ffmpeg-linux"))
        }
    } else {
        None
    };

    if let Some(src) = src_candidate.filter(|p| p.exists()) {
        let _ = fs::copy(&src, &dest_path);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&dest_path, fs::Permissions::from_mode(0o755));
        }
        return;
    }

    // Fallback for targets without a prebuilt binary in src-tauri/ffmpeg (e.g. aarch64-unknown-linux-gnu local dev/test):
    // copy system ffmpeg if present or create a small wrapper script so Tauri externalBin bundling succeeds.
    if !dest_path.exists() {
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let script = b"#!/bin/sh\nexec ffmpeg \"$@\"\n";
            let _ = fs::write(&dest_path, script);
            let _ = fs::set_permissions(&dest_path, fs::Permissions::from_mode(0o755));
        }
        #[cfg(windows)]
        {
            let _ = fs::write(&dest_path, b"");
        }
    }
}

fn main() {
    stage_ffmpeg_sidecar();
    tauri_build::build()
}
