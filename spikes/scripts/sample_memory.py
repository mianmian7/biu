#!/usr/bin/env python3
"""Portable process-tree RSS sampler. macOS WebKit services reparented to launchd need manual inclusion."""
import argparse,json,platform,subprocess,time
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--pid',type=int,required=True);p.add_argument('--seconds',type=int,default=1800);p.add_argument('--out',type=Path,required=True);args=p.parse_args()
args.out.parent.mkdir(parents=True,exist_ok=True);start=time.monotonic()
with args.out.open('w') as f:
    while time.monotonic()-start<=args.seconds:
        if platform.system()=='Windows':
            cmd=['powershell','-NoProfile','-Command','Get-CimInstance Win32_Process | Select-Object ProcessId,ParentProcessId,WorkingSetSize | ConvertTo-Json -Compress']
            data=json.loads(subprocess.check_output(cmd,text=True,encoding='utf-8-sig'));rows=[(int(r['ProcessId']),int(r['ParentProcessId']),int(r['WorkingSetSize'] or 0)) for r in data]
        else:
            text=subprocess.check_output(['ps','-axo','pid=,ppid=,rss='],text=True);rows=[tuple(map(int,s.split())) for s in text.splitlines() if s.strip()];rows=[(a,b,c*1024) for a,b,c in rows]
        found={args.pid};previous=set()
        while previous!=found:
            previous=found.copy();found.update(a for a,b,c in rows if b in found)
        active=[r for r in rows if r[0] in found]
        if not any(r[0]==args.pid for r in active):break
        f.write(json.dumps({'seconds':time.monotonic()-start,'unix_seconds':time.time(),'rss_bytes':sum(r[2] for r in active),'pids':[r[0] for r in active]})+'\n');f.flush();time.sleep(10)
print('Saved process-tree RSS; inspect missing/reparented WebKit processes before interpreting a plateau.')
