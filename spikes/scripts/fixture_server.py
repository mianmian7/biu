#!/usr/bin/env python3
"""Range-aware loopback fixture server and a synthetic HTTP proxy. No external traffic."""
import argparse, json, time, re, sys
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path
from urllib.parse import urlsplit, parse_qs
p=argparse.ArgumentParser();p.add_argument('--fixtures',type=Path,required=True);p.add_argument('--log',type=Path,required=True);p.add_argument('--port',type=int,default=18765);args=p.parse_args()
class Handler(BaseHTTPRequestHandler):
    def log_message(self,*args): pass
    def do_GET(self):
        path=urlsplit(self.path).path
        with args.log.open('a') as f: f.write(json.dumps({'time':time.time(),'path':re.sub(r'/[0-9a-f]{8}-[0-9a-f-]{27,}/', '/<ephemeral-token>/', path),'round':parse_qs(urlsplit(self.path).query).get('round',[None])[0],'range':self.headers.get('Range'),'referer':self.headers.get('Referer'),'proxy':self.path.startswith('http://')})+'\n')
        if path=='/expired': self.send_response(403);self.send_header('Content-Length','0');self.end_headers();return
        if path=='/art.svg':
            data=b'<svg xmlns="http://www.w3.org/2000/svg" width="128" height="128"><rect width="128" height="128" fill="cyan"/></svg>'
            self.send_response(200);self.send_header('Content-Type','image/svg+xml');self.send_header('Content-Length',str(len(data)));self.end_headers();self.wfile.write(data);return
        if path not in ['/aac.m4a','/tone.flac']:self.send_error(404);return
        file=args.fixtures/path.lstrip('/');length=file.stat().st_size
        start,end=0,length-1
        r=self.headers.get('Range')
        if r:
            spec=r.removeprefix('bytes=');a,b=spec.split('-',1)
            if not a:start=max(0,length-int(b));end=length-1
            else:start=int(a);end=min(int(b) if b else length-1,length-1)
        if start>=length:self.send_response(416);self.send_header('Content-Range',f'bytes */{length}');self.end_headers();return
        data=file.read_bytes()[start:end+1]
        self.send_response(206 if r else 200)
        if r:self.send_header('Content-Range',f'bytes {start}-{end}/{length}')
        self.send_header('Accept-Ranges','bytes');self.send_header('Content-Length',str(len(data)));self.send_header('Content-Type','audio/mp4' if path.endswith('m4a') else 'audio/flac');self.send_header('Access-Control-Allow-Origin','http://tauri.localhost' if sys.platform=='win32' else 'tauri://localhost');self.send_header('Cache-Control','no-store');self.end_headers()
        try:
            for offset in range(0,len(data),16384):
                self.wfile.write(data[offset:offset+16384]);self.wfile.flush();time.sleep(0.03125)
        except (BrokenPipeError,ConnectionResetError):pass
ThreadingHTTPServer(('127.0.0.1',args.port),Handler).serve_forever()
