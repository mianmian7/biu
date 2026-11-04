#!/usr/bin/env python3
"""Run under xvfb-run + dbus-run-session; replace only private/installed.AppImage."""
from pathlib import Path
import os, subprocess, argparse
p=argparse.ArgumentParser();p.add_argument('--root',type=Path,required=True);args=p.parse_args();root=args.root.resolve();private=root/'m0/private'
env=os.environ|{'APPIMAGE_EXTRACT_AND_RUN':'1','BIU_UPDATE_PRIVATE':str(private),'BIU_LOG':str(root/'evidence/update.jsonl'),'XDG_CONFIG_HOME':str(private/'config'),'XDG_DATA_HOME':str(private/'data'),'XDG_CACHE_HOME':str(private/'cache'),'WEBKIT_DISABLE_COMPOSITING_MODE':'1'}
Path(env['BIU_LOG']).write_text('')
server=subprocess.Popen(['python3','-m','http.server','18766','--bind','127.0.0.1','--directory',str(private/'http')],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
wm=subprocess.Popen(['openbox'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
try:
    with (root/'evidence/update-stderr.log').open('w') as log:
        for mode in ['update','verify-update']:
            result=subprocess.run([str(private/'installed.AppImage')],env=env|{'BIU_MODE':mode},cwd=private,stdout=log,stderr=log,timeout=180)
            print(f'{mode}: exit={result.returncode}')
            if result.returncode:break
finally:
    for proc in [server,wm]:
        proc.terminate();proc.wait(timeout=10)
