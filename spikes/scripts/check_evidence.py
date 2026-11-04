#!/usr/bin/env python3
"""Assert actual harness events, not merely successful command exits."""
import argparse, json
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('kind',choices=['playback','missing','controls','update']);p.add_argument('file',type=Path);args=p.parse_args()
rows=[json.loads(s) for s in args.file.read_text().splitlines() if s.strip()]
def events(name):return [r for r in rows if r['event']==name]
assert not events('failure'),events('failure')
if args.kind=='playback':
    assert events('complete'),'no completion event'
    assert events('aac_playing')[0]['duration']>=300
    assert events('flac_playing')[0]['time']>0
    assert events('waveform')[0]['max']>0
    seek=events('seek');assert len(seek)==10,f'expected 10 seeks, got {len(seek)}'
    assert max(s['ms'] for s in seek)<=1500,seek
    assert all(abs(s['time']-s['target'])<2 for s in seek)
    assert events('url_refresh'),'no injected URL expiry refresh'
    ranged=[r for r in events('http_range') if r['range'] is not None]
    assert len(ranged)>=5,f'only {len(ranged)} network Range transfers (some seeks may use cache)'
    assert all(r['status']==206 for r in ranged)
    soak=events('soak');assert soak and all(not r['paused'] and r['error'] is None for r in soak)
    if events('soak_start')[0].get('kind')=='reload-and-range':
        reloads=events('soak_reload');assert reloads
        for reload in reloads:
            assert any(r.get('round')==reload['round'] and r['bytes']>0 for r in ranged),reload['round']
    # Playback assertions do not judge the RSS slope. analyze_soak.py owns memory verdicts.
    print(f"PASS playback: seeks=10 max={max(s['ms'] for s in seek):.1f} ms; Range/206={len(ranged)}; waveform={events('waveform')[0]['max']:.6f}; soak={events('soak_start')[0]['seconds']} s")
elif args.kind=='missing':
    missing=events('missing_decoder')[-1];assert missing['detected'] and missing['ms']<=3000 and missing['updates']==0
    print(f"PASS missing libav: detected in {missing['ms']:.1f} ms; error={missing.get('error')}")
elif args.kind=='controls':
    assert events('native_media_ready')
    actions=events('native_frontend_action');assert any(r['action']=='Pause' and r['paused'] for r in actions);assert any(r['action']=='Play' and not r['paused'] for r in actions)
    assert events('hdslb_image')[-1]['loaded']
    print('PASS MPRIS fallback: D-Bus Pause/Play controlled HTMLAudio; hdslb image loaded')
else:
    assert [r['version'] for r in events('startup')]==['0.0.1','0.0.2']
    assert events('update_verified')[-1]['version']=='0.0.2' and events('update_installed')
    verify=events('update_restart_verified')[-1]
    assert verify['version']=='0.0.2' and verify['settings_preserved'] and verify['login_fixture_preserved']
    print('PASS updater: signature verified, 0.0.1 -> 0.0.2 installed, restart and settings/login-fixture preserved')
