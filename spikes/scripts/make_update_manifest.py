#!/usr/bin/env python3
"""Create a loopback-only updater manifest from an already signed artifact. Never prints keys/signatures."""
import argparse, json, shutil
from pathlib import Path
p=argparse.ArgumentParser();p.add_argument('--artifact',type=Path,required=True);p.add_argument('--target',required=True,choices=['linux-aarch64','darwin-aarch64','windows-x86_64']);p.add_argument('--version',default='0.0.2');p.add_argument('--out',type=Path,required=True);args=p.parse_args()
args.out.mkdir(parents=True,exist_ok=True)
artifact=args.artifact.resolve();dest=args.out/artifact.name
if artifact!=dest.resolve():shutil.copy2(artifact,dest)
signature=Path(str(artifact)+'.sig').read_text().strip()
# URL-encode names containing spaces, keeping platform naming intact.
from urllib.parse import quote
manifest={'version':args.version,'notes':'M0 loopback-only update test','pub_date':'2026-11-02T00:00:00Z','platforms':{args.target:{'url':'http://127.0.0.1:18766/'+quote(dest.name),'signature':signature}}}
(args.out/'latest.json').write_text(json.dumps(manifest,indent=2)+'\n')
print(f'Prepared local manifest: target={args.target}, version={args.version}, bytes={dest.stat().st_size}')
