#!/usr/bin/env python3
"""Run inside dbus-run-session + xvfb-run. Record whole app process-tree RSS and MPRIS names."""
import argparse, json, os, subprocess, time, shutil
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--root',type=Path,required=True);p.add_argument('--mode',default='full');p.add_argument('--seconds',type=int,default=1800);p.add_argument('--name',default='linux');p.add_argument('--reuse-server',action='store_true');p.add_argument('--binary',type=Path);args=p.parse_args()
root=args.root.resolve();out=root/'evidence';out.mkdir(exist_ok=True)
env=os.environ|{'BIU_SPIKE_FIXTURES':'1','BIU_MODE':args.mode,'BIU_SOAK_SECONDS':str(args.seconds),'BIU_FIXTURES':str(root/'m0/fixtures'),'BIU_LOG':str(out/f'{args.name}.jsonl'),'WEBKIT_DISABLE_COMPOSITING_MODE':'1','G_MESSAGES_DEBUG':''}
Path(env['BIU_LOG']).write_text('')
if args.binary:env['APPIMAGE_EXTRACT_AND_RUN']='1'
if args.mode=='controls':env['BIU_NATIVE_MEDIA']='1'
serverlog=(out/f'{args.name}-server.jsonl').open('w');serverlog.close()
server=None if args.reuse_server else subprocess.Popen(['python3',str(root/'scripts/fixture_server.py'),'--fixtures',str(root/'m0/fixtures'),'--log',str(out/f'{args.name}-server.jsonl')])
wm=subprocess.Popen(['openbox'],stdout=subprocess.DEVNULL,stderr=subprocess.DEVNULL)
log=(out/f'{args.name}-stderr.log').open('w')
binary=args.binary.resolve() if args.binary else root/'m0/runtime'/args.name/'biu-m0'
if not args.binary:
    binary.parent.mkdir(parents=True,exist_ok=True)
    shutil.copy2(root/'m0/src-tauri/target/release/biu-m0',binary)
app=subprocess.Popen([str(binary)],cwd=root,env=env,stdout=log,stderr=log)
def descendants(pid):
    found={pid};previous=set()
    while found!=previous:
        previous=found.copy()
        for f in Path('/proc').glob('[0-9]*/stat'):
            try:
                s=f.read_text(); fields=s[s.rindex(')')+2:].split();ppid=int(fields[1])
                if ppid in found:found.add(int(f.parent.name))
            except (OSError,ValueError,IndexError):pass
    return found
start=time.monotonic();last=-10;native_tested=False
try:
    with (out/f'{args.name}-memory.jsonl').open('w') as samples:
        while app.poll() is None and time.monotonic()-start<args.seconds+120:
            elapsed=time.monotonic()-start
            if elapsed-last>=10:
                pids=descendants(app.pid);rss=0;cpu=0;processes=[]
                for pid in pids:
                    try:
                        s=Path(f'/proc/{pid}/stat').read_text();v=s[s.rindex(')')+2:].split();size=int(v[21])*os.sysconf('SC_PAGE_SIZE');rss+=size;cpu+=int(v[11])+int(v[12]);processes.append({'pid':pid,'name':s[s.index('(')+1:s.rindex(')')],'rss_bytes':size})
                    except (OSError,ValueError,IndexError):pass
                bus=subprocess.run(['dbus-send','--session','--print-reply','--dest=org.freedesktop.DBus','/org/freedesktop/DBus','org.freedesktop.DBus.ListNames'],capture_output=True,text=True)
                names=[s.strip() for s in bus.stdout.splitlines() if 'mpris' in s.lower()]
                if names and args.mode=='controls' and not native_tested:
                    with (out/f'{args.name}-dbus.txt').open('w') as dbuslog:
                        for method in ['org.freedesktop.DBus.Properties.GetAll','org.mpris.MediaPlayer2.Player.Pause','org.mpris.MediaPlayer2.Player.Play']:
                            command=['dbus-send','--session','--print-reply','--dest=org.mpris.MediaPlayer2.biu_m0','/org/mpris/MediaPlayer2',method]
                            if method.endswith('GetAll'):command+=['string:org.mpris.MediaPlayer2.Player']
                            result=subprocess.run(command,capture_output=True,text=True,timeout=5);dbuslog.write(result.stdout+result.stderr);time.sleep(1)
                    native_tested=True
                samples.write(json.dumps({'seconds':elapsed,'rss_bytes':rss,'cpu_ticks':cpu,'pids':len(pids),'app_pid':app.pid,'processes':processes,'mpris':names,'unix_seconds':time.time()})+'\n');samples.flush();last=elapsed
            time.sleep(1)
        if app.poll() is None:app.terminate();app.wait(timeout=10)
    print(json.dumps({'exit':app.returncode,'elapsed_seconds':time.monotonic()-start,'name':args.name}))
finally:
    for proc in [app,server,wm]:
        if proc is not None and proc.poll() is None:
            proc.terminate()
            try:proc.wait(timeout=10)
            except subprocess.TimeoutExpired:proc.kill();proc.wait()
    log.close()
if app.returncode != 0:raise SystemExit(f'app failed: {app.returncode}')
