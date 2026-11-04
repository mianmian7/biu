#!/usr/bin/env python3
"""macOS: 采样 app 进程树 + 启动后新出现的 WebKit XPC 进程（被 launchd 接管，不在进程树里）。"""
import argparse, json, subprocess, time
from pathlib import Path
p = argparse.ArgumentParser(); p.add_argument('--pid', type=int, required=True); p.add_argument('--baseline', type=Path, required=True)
p.add_argument('--seconds', type=int, default=3700); p.add_argument('--out', type=Path, required=True); a = p.parse_args()
def ps():
    out = subprocess.check_output(['ps', '-axo', 'pid=,ppid=,rss=,comm='], text=True)
    rows = []
    for line in out.splitlines():
        s = line.split(None, 3)
        if len(s) == 4: rows.append((int(s[0]), int(s[1]), int(s[2]) * 1024, s[3]))
    return rows
base = set(json.loads(a.baseline.read_text()))
start = time.monotonic()
with a.out.open('w') as f:
    while time.monotonic() - start <= a.seconds:
        rows = ps(); tree = {a.pid}
        while True:
            ext = tree | {r[0] for r in rows if r[1] in tree}
            if ext == tree: break
            tree = ext
        if not any(r[0] == a.pid for r in rows): break
        xpc = [r for r in rows if 'com.apple.WebKit.' in r[3] and r[0] not in base and r[0] not in tree]
        inc = [r for r in rows if r[0] in tree] + xpc
        f.write(json.dumps({'seconds': time.monotonic() - start, 'unix_seconds': time.time(), 'rss_bytes': sum(r[2] for r in inc),
                            'tree_rss_bytes': sum(r[2] for r in rows if r[0] in tree), 'pids': [r[0] for r in inc],
                            'processes': [{'pid': r[0], 'rss_bytes': r[2], 'name': r[3].rsplit('/', 1)[-1]} for r in inc]}) + '\n'); f.flush()
        time.sleep(10)
