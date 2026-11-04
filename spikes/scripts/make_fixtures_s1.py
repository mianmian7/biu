#!/usr/bin/env python3
"""Generate S1 synthetic fMP4 FLAC and E-AC-3 (Dolby) fixtures using ffmpeg and verify box structure via ffprobe."""
import json
import re
import subprocess
import sys
from pathlib import Path

if len(sys.argv) < 2:
    raise SystemExit("Usage: make_fixtures_s1.py <out_dir>")

out = Path(sys.argv[1])
out.mkdir(parents=True, exist_ok=True)

MOVFLAGS = "+empty_moov+default_base_moof+frag_keyframe+delay_moov"

SPECS = [
    (
        "flac-1.m4a",
        "sine=frequency=440:duration=120",
        "aformat=sample_fmts=s16:sample_rates=44100:channel_layouts=stereo",
        ["-c:a", "flac", "-sample_fmt", "s16", "-bits_per_raw_sample", "16"],
    ),
    (
        "flac-2.m4a",
        "sine=frequency=523.25:duration=120",
        "aformat=sample_fmts=s32:sample_rates=48000:channel_layouts=stereo",
        ["-c:a", "flac", "-sample_fmt", "s32", "-bits_per_raw_sample", "24"],
    ),
    (
        "flac-3.m4a",
        "sine=frequency=659.25:duration=120",
        "aformat=sample_fmts=s32:sample_rates=96000:channel_layouts=stereo",
        ["-c:a", "flac", "-sample_fmt", "s32", "-bits_per_raw_sample", "24"],
    ),
    (
        "dolby-1.m4a",
        "sine=frequency=330:duration=120",
        "aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=stereo",
        ["-c:a", "eac3", "-b:a", "192k"],
    ),
    (
        "dolby-2.m4a",
        "sine=frequency=392:duration=120",
        "aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=5.1",
        ["-c:a", "eac3", "-b:a", "384k"],
    ),
    (
        "dolby-3.m4a",
        "sine=frequency=493.88:duration=120",
        "aformat=sample_fmts=fltp:sample_rates=48000:channel_layouts=5.1",
        ["-c:a", "eac3", "-b:a", "640k"],
    ),
]

summary = []
for name, src, aformat, codec_args in SPECS:
    dest = out / name
    cmd = [
        "ffmpeg",
        "-y",
        "-hide_banner",
        "-loglevel",
        "error",
        "-f",
        "lavfi",
        "-i",
        src,
        "-af",
        aformat,
        *codec_args,
        "-strict",
        "experimental",
        "-f",
        "mp4",
        "-movflags",
        MOVFLAGS,
        "-frag_duration",
        "2000000",
        str(dest),
    ]
    subprocess.run(cmd, check=True)

    probe_json = subprocess.check_output(
        [
            "ffprobe",
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            str(dest),
        ],
        text=True,
    )
    info = json.loads(probe_json)
    stream = info["streams"][0]
    fmt = info["format"]

    trace = subprocess.run(
        ["ffprobe", "-v", "trace", str(dest)],
        stdout=subprocess.PIPE,
        stderr=subprocess.STDOUT,
        text=True,
        check=True,
    ).stdout
    boxes = re.findall(r"type:'(ftyp|moov|moof|mdat|mfra)' parent:'root'", trace)
    moof_count = boxes.count("moof")
    mdat_count = boxes.count("mdat")
    assert "moov" in boxes and moof_count >= 10 and mdat_count >= 10, (
        f"{name} is not fragmented MP4: boxes={boxes}"
    )

    row = {
        "file": name,
        "format_name": fmt.get("format_name"),
        "major_brand": fmt.get("tags", {}).get("major_brand"),
        "duration": float(fmt.get("duration", 0)),
        "size_bytes": int(fmt.get("size", 0)),
        "codec_name": stream.get("codec_name"),
        "codec_tag_string": stream.get("codec_tag_string"),
        "sample_rate": int(stream.get("sample_rate", 0)),
        "channels": int(stream.get("channels", 0)),
        "channel_layout": stream.get("channel_layout"),
        "bits_per_raw_sample": stream.get("bits_per_raw_sample"),
        "bit_rate": stream.get("bit_rate") or fmt.get("bit_rate"),
        "root_boxes_head": boxes[:8],
        "moof_count": moof_count,
        "mdat_count": mdat_count,
    }
    summary.append(row)
    print(json.dumps(row))

assert len(summary) == 6
