#!/usr/bin/env python3
"""Bounded headless probes of existing app, without UI clicks or source changes.
A timeout is recorded, never treated as feature success. Only owned app PIDs are stopped.
"""
import json,os,subprocess,time,runpy
from pathlib import Path
ROOT=Path(__file__).resolve().parents[1]
APP=ROOT/'m0/src-tauri/target/release/bundle/macos/Biu M0.app/Contents/MacOS/biu-m0'
E=ROOT/'evidence';PRIVATE=ROOT/'m0/private'
processes=runpy.run_path(str(E/'mac-run-s2.py'))['processes']

def run(label,mode,seconds,proxy=False):
 path=E/f'mac-{label}.jsonl'
 if path.exists():raise SystemExit(f'STOP: {path.name} already exists')
 baseline={r['pid'] for r in processes() if 'com.apple.WebKit.' in r['name']}
 env=os.environ.copy();env.update(BIU_MODE=mode,BIU_SPIKE_FIXTURES='1',BIU_FIXTURES=str(ROOT/'m0/fixtures'),BIU_LOG=str(path),BIU_SOAK_SECONDS='75')
 env.pop('BIU_WEBVIEW_PROXY',None)
 if proxy:env['BIU_WEBVIEW_PROXY']='http://127.0.0.1:18765'
 with (PRIVATE/f'mac-{label}-console.log').open('w') as log:
  app=subprocess.Popen([str(APP)],cwd=ROOT/'m0',env=env,stdout=log,stderr=subprocess.STDOUT)
  start=time.monotonic();samples=[]
  while app.poll() is None and time.monotonic()-start<seconds:
   raw=subprocess.check_output(['ps','-axo','pid=,ppid=,rss=,%cpu=,time=,comm='],text=True)
   included=[]
   for line in raw.splitlines():
    v=line.split(None,5)
    if len(v)!=6:continue
    pid,ppid,rss,cpu,cpu_time,name=v;pid=int(pid)
    if pid==app.pid or (pid not in baseline and name.startswith('/System/Library/Frameworks/WebKit.framework/') and 'com.apple.WebKit.' in name):
     included.append({'pid':pid,'ppid':int(ppid),'rss_bytes':int(rss)*1024,'cpu_percent':float(cpu),'cpu_time':cpu_time,'name':name})
   samples.append({'seconds':time.monotonic()-start,'unix_seconds':time.time(),'processes':included,'rss_bytes':sum(r['rss_bytes'] for r in included),'cpu_percent_sum':sum(r['cpu_percent'] for r in included)})
   time.sleep(5)
  timed_out=app.poll() is None
  if timed_out:app.terminate()
  try:code=app.wait(timeout=10)
  except subprocess.TimeoutExpired:raise SystemExit('STOP: test app did not terminate; no force kill')
 result={'label':label,'mode':mode,'proxy_enabled':proxy,'app_pid':app.pid,'elapsed_seconds':time.monotonic()-start,'bounded_probe_seconds':seconds,'stopped_by_executor_at_bound':timed_out,'exit':code,'cpu_note':'ps %cpu snapshots for native-new-XPC cohort; not a rendered-lyrics CPU acceptance result if no lyrics_frames','samples':samples}
 (E/f'mac-{label}-probe.json').write_text(json.dumps(result,indent=2)+'\n')
 rows=[json.loads(s) for s in path.read_text().splitlines()] if path.exists() else []
 print(json.dumps({k:result[k] for k in result if k!='samples'}),flush=True)
 print('events:',[r['event'] for r in rows],flush=True)
 if code not in [0,-15]:raise SystemExit('STOP: uncovered headless probe crash')

if __name__=='__main__':
 run('s5','full',80)
 run('s6','controls',35)
 run('s6-proxy','controls',35,proxy=True)
