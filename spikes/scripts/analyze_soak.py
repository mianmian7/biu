#!/usr/bin/env python3
"""Judge a fresh-load + Range >=60 min soak. Commit the actual RSS series and per-round traffic proof.
Rule declared before running: 600 s warmup; both post-warmup and final-30-min OLS slopes
must be <=0 MB/min to mark memory passing. Positive slope remains pending, regardless of a late GC drop.
"""
import argparse,json,shutil,hashlib
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--events',type=Path,required=True);p.add_argument('--memory',type=Path,required=True);p.add_argument('--server',type=Path,required=True);p.add_argument('--out',type=Path,required=True);a=p.parse_args()
def rows(p):return [json.loads(s) for s in p.read_text().splitlines() if s.strip()]
events=rows(a.events);memory=rows(a.memory);upstream=rows(a.server)
start=next(r for r in events if r['event']=='soak_start')
assert start['kind']=='reload-and-range' and start['seconds']>=3600
observed=(max(r['wall_time_ms'] for r in events if r['event']=='soak')-start['wall_time_ms'])/1000
assert observed>=start['seconds'],f'only {observed} seconds of observed soak'
assert events[-1]['event']=='complete' and not any(r['event']=='failure' for r in events)
assert all(not r['paused'] and r['error'] is None for r in events if r['event']=='soak')
reloads=[r for r in events if r['event']=='soak_reload'];assert len(reloads)>=120
all_range=[r for r in events if r['event'] in ('http_range','range') and r.get('range') is not None and r.get('status')==206 and r.get('bytes',0)>0]
assert all_range,'no Range/206 events recorded'
traffic=[]
for reload in reloads:
    n=reload['round'];transfers=[r for r in events if r['event'] in ('http_range','range') and r.get('round')==n]
    ranged=[r for r in transfers if r['range'] is not None]
    requests=[r for r in upstream if r.get('round')==str(n)]
    if transfers:
        assert ranged and all(r['status']==206 and r['bytes']>0 for r in ranged),(n,ranged)
    assert any(r['range'] is not None for r in requests),(n,'no upstream Range')
    assert any(r['range'] is None for r in requests) or all(r['range'] is not None for r in requests),(n,'no upstream requests')
    traffic.append({'round':n,'wall_time_ms':reload['wall_time_ms'],'target':reload['target'],'seek_ms':reload['ms'],'upstream_requests':len(requests),'upstream_range_requests':sum(r['range'] is not None for r in requests),'transfers':[{k:r[k] for k in ['range','status','bytes','ms']} for r in transfers]})
origin=start['wall_time_ms']/1000
samples=[{'soak_seconds':r['unix_seconds']-origin,**r} for r in memory]
post=[r for r in samples if r['soak_seconds']>=600];late=[r for r in samples if r['soak_seconds']>=start['seconds']-1800]
assert len(post)>=290 and len(late)>=170
# Equal-interval raw samples, not endpoint differences; no removal of GC cycles/outliers.
def slope(rows):
    xs=[r['soak_seconds']/60 for r in rows];ys=[r['rss_bytes']/1e6 for r in rows]
    mx=sum(xs)/len(xs);my=sum(ys)/len(ys)
    return sum((x-mx)*(y-my) for x,y in zip(xs,ys))/sum((x-mx)**2 for x in xs)
s=slope(post);tail=slope(late);passing=s<=0 and tail<=0
out=a.out;out.mkdir(parents=True,exist_ok=True)
rss=out/'soak-60min-rss.jsonl';shutil.copyfile(a.memory,rss)
(out/'soak-60min-rounds.json').write_text(json.dumps(traffic,indent=2)+'\n')
result={'status':'通过（仅本次测试）' if passing else '待定（warmup后仍有正斜率，不能记通过）','workload':'fresh URL + no-store + forced Range seek every 30 s; no audio.loop','configured_soak_seconds':start['seconds'],'observed_soak_seconds':observed,'rounds':len(reloads),'rounds_with_verified_proxy_and_upstream_range':len(traffic),'warmup_seconds':600,'decision_rule':'both post-warmup and final-30-min OLS slopes <= 0 MB/min; otherwise pending','post_warmup_ols_MB_per_min':s,'last_30min_ols_MB_per_min':tail,'post_warmup_samples':len(post),'rss_samples':len(memory),'rss_peak_bytes':max(r['rss_bytes'] for r in samples),'rss_last_bytes':samples[-1]['rss_bytes'],'rss_sequence_file':rss.name,'rss_sequence_sha256':hashlib.sha256(rss.read_bytes()).hexdigest(),'per_round_traffic_file':'soak-60min-rounds.json','limitations':'Process-tree RSS includes WebKit. A finite run cannot rule out long-term leaks; positive slopes are not excused by GC drops.'}
(out/'soak-60min-summary.json').write_text(json.dumps(result,ensure_ascii=False,indent=2)+'\n')
print(f"SOAK: {len(reloads)} fresh-load rounds, all with proxy/upstream Range; warmup=600 s; OLS={s:+.6f} MB/min; last30={tail:+.6f} MB/min; {result['status']}")
