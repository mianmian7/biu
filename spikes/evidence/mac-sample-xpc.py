#!/usr/bin/env python3
"""Supplement original ps tree sampler with all newly created native WebKit XPC PIDs.
Cohort inclusion uses prelaunch baseline; individual attribution evidence is separate.
No unrelated pre-existing WebKit or iOS Simulator process is included in the sum.
"""
import argparse,json,subprocess,time
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--pid',type=int,required=True);p.add_argument('--baseline',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args()
baseline=set(json.loads(a.baseline.read_text())['baseline_native_webkit_pids'])
start=time.monotonic()
with a.out.open('w') as out:
 while True:
  raw=subprocess.check_output(['ps','-axo','pid=,ppid=,rss=,%cpu=,time=,comm='],text=True)
  rows=[]
  for line in raw.splitlines():
   v=line.split(None,5)
   if len(v)!=6:continue
   pid,ppid,rss,cpu,cpu_time,name=v
   rows.append({'pid':int(pid),'ppid':int(ppid),'rss_bytes':int(rss)*1024,'cpu_percent':float(cpu),'cpu_time':cpu_time,'name':name})
  tree={a.pid}
  while True:
   new=tree|{r['pid'] for r in rows if r['ppid'] in tree}
   if new==tree:break
   tree=new
  native=[r for r in rows if r['name'].startswith('/System/Library/Frameworks/WebKit.framework/') and 'com.apple.WebKit.' in r['name']]
  cohort=[r for r in native if r['pid'] not in baseline]
  included=[r for r in rows if r['pid'] in tree or r in cohort]
  alive=any(r['pid']==a.pid for r in rows)
  record={'seconds':time.monotonic()-start,'unix_seconds':time.time(),'app_pid':a.pid,'app_alive':alive,'rss_bytes':sum(r['rss_bytes'] for r in included),'cpu_percent_sum':sum(r['cpu_percent'] for r in included),'processes':included,'cohort_native_webkit_pids':[r['pid'] for r in cohort],'baseline_native_webkit_pids':sorted(baseline)}
  out.write(json.dumps(record)+'\n');out.flush()
  if not alive:break
  if time.monotonic()-start>3900:break
  time.sleep(10)
