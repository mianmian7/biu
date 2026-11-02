#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
mkdir -p "$root/m0/fixtures" "$root/m0/web/assets" "$root/m0/runtime" "$root/evidence"
if [[ $# -gt 0 ]]; then
  test -f "$1/index.html"
  cp -a "$1/." "$root/m0/web/assets/"
fi
test -f "$root/m0/web/assets/index.html" || { echo 'Supply existing dist/web as argument, or unpack m0-inputs.tar.gz first.' >&2; exit 1; }
# These are original, synthetic sine-wave fixtures; no third-party music is downloaded.
gst-launch-1.0 -q audiotestsrc wave=sine freq=440 samplesperbuffer=4410 num-buffers=3000 ! 'audio/x-raw,rate=44100,channels=2' ! audioconvert ! avenc_aac bitrate=128000 ! aacparse ! mp4mux faststart=true ! filesink location="$root/m0/fixtures/aac.m4a"
gst-launch-1.0 -q audiotestsrc wave=sine freq=440 samplesperbuffer=4410 num-buffers=100 ! 'audio/x-raw,rate=44100,channels=2' ! audioconvert ! flacenc ! filesink location="$root/m0/fixtures/tone.flac"
plugins="$(dirname "$(LC_ALL=C gst-inspect-1.0 coreelements | awk '/Filename/{print $2; exit}')")"
mkdir -p "$root/m0/runtime/gst-no-libav"
for p in "$plugins"/*.so; do
  case "$p" in */libgstlibav.so) ;; *) ln -sf "$p" "$root/m0/runtime/gst-no-libav/" ;; esac
done
printf 'Prepared synthetic AAC (300 s), FLAC (10 s), isolated plugins without libav.\n'
