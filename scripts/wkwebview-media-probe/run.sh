#!/bin/zsh
# macOS only. Plays proxy-shaped media responses in a real WKWebView and prints one JSON line per case.
# Optional: put a real Bilibili DASH audio segment at ./real.m4s to add the r1/r2 cases.
set -e
cd "$(dirname "$0")"
WORK=$(mktemp -d "${TMPDIR:-/tmp}/biu-wkwebview-media-probe.XXXXXX")
cp server.py page.html "$WORK/"
[[ -f real.m4s ]] && cp real.m4s "$WORK/"
FF=${FFMPEG:-$(command -v ffmpeg)}
"$FF" -loglevel error -y -f lavfi -i "sine=frequency=440:duration=6" -c:a aac -b:a 128k -f mp4 -movflags +frag_keyframe+empty_moov+default_base_moof "$WORK/a.m4s"
"$FF" -loglevel error -y -f lavfi -i "sine=frequency=440:duration=6" -c:a aac -b:a 128k -f adts "$WORK/a.aac"
swiftc -O -o "$WORK/probe" probe.swift -framework Cocoa -framework WebKit
PORT=${PORT:-18931}
python3 "$WORK/server.py" $PORT 2> "$WORK/server.log" &
SP=$!
trap 'kill $SP 2>/dev/null || true; rm -rf "$WORK"' EXIT
sleep 1
QUERY=""; [[ -f "$WORK/real.m4s" ]] && QUERY="?real=1"
# Page on localhost, media on 127.0.0.1: cross-origin, like tauri://localhost -> loopback proxy.
"$WORK/probe" "http://localhost:$PORT/page$QUERY"
