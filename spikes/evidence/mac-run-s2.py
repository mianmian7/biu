#!/usr/bin/env python3
"""Run unchanged README S2 modes; supplement original sampler with attributed XPC RSS.
No UI automation, source edits, sudo, permission changes or audio fixtures are used here.
"""
import json, os, re, subprocess, time
from pathlib import Path
ROOT = Path(__file__).resolve().parents[1]
PRIVATE = ROOT / 'm0/private'
APP = ROOT / 'm0/src-tauri/target/release/bundle/macos/Biu M0.app/Contents/MacOS/biu-m0'
EVIDENCE = ROOT / 'evidence'

def processes():
    text = subprocess.check_output(['ps', '-axo', 'pid=,ppid=,rss=,comm='], text=True)
    rows = []
    for line in text.splitlines():
        parts = line.split(None, 3)
        if len(parts) == 4:
            pid, ppid, rss, name = parts
            rows.append({'pid': int(pid), 'ppid': int(ppid), 'rss_bytes': int(rss)*1024, 'name': name})
    return rows

def webkit(row):
    return row['name'].startswith('/System/Library/Frameworks/WebKit.framework/') and 'com.apple.WebKit.' in row['name']

def tool_probe(pid, app_pid, mode):
    reports = {}
    for name, command in [('vmmap', ['/usr/bin/vmmap', '-summary', str(pid)]), ('footprint', ['/usr/bin/footprint', '-p', str(pid), '-f', 'bytes'])]:
        try:
            r = subprocess.run(command, capture_output=True, text=True, timeout=20)
            text = r.stdout + r.stderr
            (PRIVATE/f'mac-{mode}-{name}-{pid}.txt').write_text(text)
            # Keep only process attribution, memory summaries and explicit tool errors.
            lines = [line for line in text.splitlines() if re.search(r'^(Process:|Path:|Identifier:|Parent Process:|Responsible:|Physical footprint:|Physical footprint \(peak\):)|error|denied|permission|requires|cannot|failed|not permitted', line, re.I)]
            reports[name] = {'exit': r.returncode, 'summary': lines[:35]}
        except subprocess.TimeoutExpired:
            reports[name] = {'timeout_seconds': 20}
    text = (PRIVATE/f'mac-{mode}-vmmap-{pid}.txt').read_text() if (PRIVATE/f'mac-{mode}-vmmap-{pid}.txt').exists() else ''
    responsible = bool(re.search(r'Responsible:\s+biu-m0\s+\['+str(app_pid)+r'\]', text))
    try:
        r = subprocess.run(['lsof', '-nP', '-p', str(pid), '-Fn'], capture_output=True, text=True, timeout=10)
        bundle_path = any('com.biu.spike.m0' in line or '/biu-m0-transfer/' in line for line in r.stdout.splitlines())
        reports['lsof'] = {'exit': r.returncode, 'app_path_association': bundle_path}
    except subprocess.TimeoutExpired:
        bundle_path = False
        reports['lsof'] = {'timeout_seconds': 10}
    return responsible or bundle_path, reports

def run(mode, label):
    baseline = {r['pid'] for r in processes() if webkit(r)}
    event_path = EVIDENCE/f'mac-s2-{label}.jsonl'
    if event_path.exists():
        raise SystemExit(f'STOP: evidence already exists: {event_path.name}')
    env = os.environ.copy()
    env.update(BIU_SPIKE_FIXTURES='1', BIU_FIXTURES=str(ROOT/'m0/fixtures'), BIU_LOG=str(event_path), BIU_MODE=mode, BIU_SOAK_SECONDS='3600')
    env.pop('BIU_WEBVIEW_PROXY', None)
    with (PRIVATE/f'mac-s2-{label}-console.log').open('w') as console:
        app = subprocess.Popen([str(APP)], cwd=ROOT/'m0', env=env, stdout=console, stderr=subprocess.STDOUT)
        (PRIVATE/'mac-active-app.pid').write_text(str(app.pid)+'\n')
        sampler = subprocess.Popen(['python3', str(ROOT/'scripts/sample_memory.py'), '--pid', str(app.pid), '--seconds', '3600', '--out', str(EVIDENCE/f'mac-s2-{label}-memory.jsonl')], stdout=console, stderr=subprocess.STDOUT)
        start = time.monotonic()
        probes, attributed = {}, set()
        with (EVIDENCE/f'mac-s2-{label}-xpc-memory.jsonl').open('w') as out:
            while app.poll() is None:
                tick = time.monotonic()
                rows = processes()
                tree = {app.pid}
                while True:
                    extended = tree | {r['pid'] for r in rows if r['ppid'] in tree}
                    if extended == tree: break
                    tree = extended
                candidates = [r for r in rows if webkit(r) and r['pid'] not in baseline]
                if tick-start >= 5:
                    for row in [r for r in rows if r['pid']==app.pid] + candidates:
                        pid = row['pid']
                        if str(pid) not in probes:
                            associated, details = tool_probe(pid, app.pid, label)
                            probes[str(pid)] = {'pid':pid, 'name':row['name'], 'attributed_to_app':pid==app.pid or associated, 'tools':details}
                            if associated: attributed.add(pid)
                            (EVIDENCE/f'mac-s2-{label}-memory-tools.json').write_text(json.dumps({'app_pid':app.pid,'baseline_native_webkit_pids':sorted(baseline),'probes':probes}, indent=2)+'\n')
                owned = tree | attributed
                included = [r for r in rows if r['pid'] in owned]
                cohort_pids = tree | {r['pid'] for r in candidates}
                cohort = [r for r in rows if r['pid'] in cohort_pids]
                record = {'seconds':time.monotonic()-start,'unix_seconds':time.time(),'app_pid':app.pid,'rss_bytes':sum(r['rss_bytes'] for r in included),'tree_rss_bytes':sum(r['rss_bytes'] for r in rows if r['pid'] in tree),'pids':[r['pid'] for r in included],'processes':included,'cohort_rss_bytes':sum(r['rss_bytes'] for r in cohort),'cohort_processes':cohort,'unattributed_new_webkit_pids':[r['pid'] for r in candidates if r['pid'] not in owned]}
                out.write(json.dumps(record)+'\n'); out.flush()
                if time.monotonic()-start > 3900:
                    app.terminate()
                    raise SystemExit('STOP: app exceeded 3900 s safety limit; no fallback assumed')
                time.sleep(max(0, 10-(time.monotonic()-tick)))
        exit_code = app.wait()
        sampler.wait(timeout=30)
    events = [json.loads(line) for line in event_path.read_text().splitlines()] if event_path.exists() else []
    failures = [r for r in events if r.get('event') in ['failure','protocol_error']]
    completed = any(r.get('event')=='complete' for r in events)
    result = {'mode':mode,'label':label,'app_pid':app.pid,'exit':exit_code,'elapsed_seconds':time.monotonic()-start,'complete':completed,'failures':failures}
    (EVIDENCE/f'mac-s2-{label}-run.json').write_text(json.dumps(result,indent=2)+'\n')
    print(json.dumps(result), flush=True)
    if exit_code != 0 and not failures and mode != 'custom':
        raise SystemExit('STOP: HTTP fallback application crashed; no further fallback in README')
    if not completed and not failures and exit_code == 0:
        raise SystemExit('STOP: no completion or covered protocol failure evidence')
    return completed and not failures

if __name__ == '__main__':
    custom_ok = run('custom', 'custom')
    if not custom_ok:
        run('full', 'http')
