#!/usr/bin/env python3
"""Fixture upstream that emits the exact response shapes the Biu loopback proxy can send."""
import gzip, os, socket, sys, threading

HERE = os.path.dirname(os.path.abspath(__file__))
FMP4 = open(os.path.join(HERE, "a.m4s"), "rb").read()
REAL = open(os.path.join(HERE, "real.m4s"), "rb").read() if os.path.exists(os.path.join(HERE, "real.m4s")) else b""
ADTS = open(os.path.join(HERE, "a.aac"), "rb").read()
CORS = "Access-Control-Allow-Origin: *\r\nAccess-Control-Expose-Headers: Content-Range, Accept-Ranges, Content-Length\r\nCache-Control: no-store\r\nConnection: close\r\n"

UNSATISFIABLE = "unsatisfiable"

def parse_range(h, n):
    """None = no usable Range header (serve 200); UNSATISFIABLE = answer 416."""
    if not h or not h.startswith("bytes=") or "," in h: return None
    s, _, e = h[6:].strip().partition("-")
    try:
        if s == "":
            if not e or int(e) == 0: return UNSATISFIABLE
            start, end = max(0, n - int(e)), n - 1
        else:
            start, end = int(s), (int(e) if e else n - 1)
    except ValueError:
        return None
    if start >= n or end < start: return UNSATISFIABLE
    return start, min(end, n - 1)

def send(conn, status, headers, body=b""):
    conn.sendall(f"HTTP/1.1 {status}\r\n{CORS}{headers}\r\n".encode() + body)

def ranged(conn, rng, data, ctype, extra=""):
    r = parse_range(rng, len(data))
    if r == UNSATISFIABLE:
        send(conn, "416 Range Not Satisfiable", f"Content-Range: bytes */{len(data)}\r\nContent-Length: 0\r\n")
    elif r:
        s, e = r; part = data[s:e+1]
        send(conn, "206 Partial Content", f"Content-Type: {ctype}\r\nContent-Length: {len(part)}\r\nContent-Range: bytes {s}-{e}/{len(data)}\r\nAccept-Ranges: bytes\r\n{extra}", part)
    else:
        send(conn, "200 OK", f"Content-Type: {ctype}\r\nContent-Length: {len(data)}\r\nAccept-Ranges: bytes\r\n{extra}", data)

def chunked(conn, data, ctype, complete):
    conn.sendall(f"HTTP/1.1 200 OK\r\n{CORS}Content-Type: {ctype}\r\nTransfer-Encoding: chunked\r\naccept-ranges: bytes\r\n\r\n".encode())
    body = data if complete else data[: len(data) // 2]
    for i in range(0, len(body), 8192):
        c = body[i:i+8192]; conn.sendall(f"{len(c):x}\r\n".encode() + c + b"\r\n")
    if complete: conn.sendall(b"0\r\n\r\n")

def handle(conn):
    try:
        req = b""
        while b"\r\n\r\n" not in req:
            d = conn.recv(65536)
            if not d: return
            req += d
        lines = req.decode("latin1").split("\r\n")
        method, path, _ = lines[0].split(" ")
        hdr = {l.split(":", 1)[0].lower(): l.split(":", 1)[1].strip() for l in lines[1:] if ":" in l}
        rng = hdr.get("range"); case = path.strip("/").split("?")[0].replace(".m4s","")
        sys.stderr.write(f"{method} {case} range={rng}\n")
        if case == "page": 
            html = open(os.path.join(HERE, "page.html"), "rb").read()
            send(conn, "200 OK", f"Content-Type: text/html\r\nContent-Length: {len(html)}\r\n", html); return
        if case == "c1_mp4_len": ranged(conn, rng, FMP4, "audio/mp4")
        elif case == "c2_octet": ranged(conn, rng, FMP4, "application/octet-stream")
        elif case == "c3_chunked_ok": chunked(conn, FMP4, "audio/mp4", True)
        elif case == "c4_chunked_trunc": chunked(conn, FMP4, "audio/mp4", False)
        elif case == "c5_close_trunc":
            conn.sendall(f"HTTP/1.1 200 OK\r\n{CORS}Content-Type: audio/mp4\r\naccept-ranges: bytes\r\n\r\n".encode() + FMP4[: len(FMP4)//2])
        elif case == "c12_close_ok":
            conn.sendall(f"HTTP/1.1 200 OK\r\n{CORS}Content-Type: audio/mp4\r\naccept-ranges: bytes\r\n\r\n".encode() + FMP4)
        elif case == "c13_no_range_200":
            send(conn, "200 OK", f"Content-Type: audio/mp4\r\nContent-Length: {len(FMP4)}\r\n", FMP4)
        elif case == "c14_octet_no_range":
            send(conn, "200 OK", f"Content-Type: application/octet-stream\r\nContent-Length: {len(FMP4)}\r\n", FMP4)
        elif case == "c15_octet_m4s_ext":
            ranged(conn, rng, FMP4, "application/octet-stream")
        elif case == "r1_real_octet": ranged(conn, rng, REAL, "application/octet-stream")
        elif case == "r2_real_mp4": ranged(conn, rng, REAL, "audio/mp4")
        elif case == "r3_real_octet_norange": send(conn, "200 OK", f"Content-Type: application/octet-stream\r\nContent-Length: {len(REAL)}\r\n", REAL)
        elif case == "c6_adts_aac": ranged(conn, rng, ADTS, "audio/aac")
        elif case == "c7_adts_as_mp4": ranged(conn, rng, ADTS, "audio/mp4")
        elif case == "c8_403_html": b = b"<html>expired</html>"; send(conn, "403 Forbidden", f"Content-Type: text/html\r\nContent-Length: {len(b)}\r\n", b)
        elif case == "c9_200_html": b = b"<html>challenge</html>"; send(conn, "200 OK", f"Content-Type: text/html\r\nContent-Length: {len(b)}\r\n", b)
        elif case == "c10_gzip":
            z = gzip.compress(FMP4); send(conn, "200 OK", f"Content-Type: audio/mp4\r\nContent-Encoding: gzip\r\nContent-Length: {len(z)}\r\n", z)
        elif case == "c11_502_empty": send(conn, "502 Bad Gateway", "Content-Length: 0\r\n")
        else: send(conn, "404 Not Found", "Content-Length: 0\r\n")
    except (BrokenPipeError, ConnectionResetError): pass
    finally:
        try: conn.shutdown(socket.SHUT_RDWR)
        except OSError: pass
        conn.close()

s = socket.socket(); s.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
s.bind(("127.0.0.1", int(sys.argv[1]))); s.listen(64)
while True:
    c, _ = s.accept(); threading.Thread(target=handle, args=(c,), daemon=True).start()
