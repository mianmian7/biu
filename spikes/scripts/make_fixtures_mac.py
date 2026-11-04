#!/usr/bin/env python3
"""Mac CI: 生成与 prepare_linux.sh 等价的原创正弦波夹具（AAC 300 s、FLAC 10 s），用系统 afconvert 编码。"""
import math, struct, subprocess, sys, wave
from pathlib import Path
out = Path(sys.argv[1]); out.mkdir(parents=True, exist_ok=True)
def tone(path, seconds):
    with wave.open(str(path), 'wb') as w:
        w.setnchannels(2); w.setsampwidth(2); w.setframerate(44100)
        frame = [struct.pack('<hh', v, v) for v in (int(16000*math.sin(2*math.pi*440*i/44100)) for i in range(44100))]
        sec = b''.join(frame)
        for _ in range(seconds): w.writeframes(sec)
tone(out/'aac.wav', 300); tone(out/'tone.wav', 10)
subprocess.run(['afconvert', '-f', 'm4af', '-d', 'aac', '-b', '128000', str(out/'aac.wav'), str(out/'aac.m4a')], check=True)
subprocess.run(['afconvert', '-f', 'flac', '-d', 'flac', str(out/'tone.wav'), str(out/'tone.flac')], check=True)
for p in out.glob('*.wav'): p.unlink()
print(sorted(p.name for p in out.iterdir()))
