#!/usr/bin/env python3
"""Evaluate S2 memory OLS slopes (warmup 600 s, post-warmup and final 30 min) per process and overall."""
import argparse
import json
import sys
from pathlib import Path

p = argparse.ArgumentParser()
p.add_argument("--memory", type=Path, required=True)
p.add_argument("--control", type=Path)
p.add_argument("--out", type=Path, required=True)
p.add_argument("--warmup-seconds", type=float, default=600.0)
p.add_argument("--tail-window-seconds", type=float, default=1800.0)
args = p.parse_args()


def load_jsonl(path: Path):
    rows = [json.loads(line) for line in path.read_text().splitlines() if line.strip()]
    if not rows:
        sys.exit(f"ERROR: empty memory log: {path}")
    return rows


def ols_slope_mb_per_min(points):
    if len(points) < 2:
        return 0.0
    xs = [t / 60.0 for t, _ in points]
    ys = [b / 1_000_000.0 for _, b in points]
    mx = sum(xs) / len(xs)
    my = sum(ys) / len(ys)
    denom = sum((x - mx) ** 2 for x in xs)
    if denom == 0:
        return 0.0
    return sum((x - mx) * (y - my) for x, y in zip(xs, ys)) / denom


def is_rust_main(name: str, pid: int, main_pid: int) -> bool:
    base = name.rsplit("/", 1)[-1].lower()
    if pid == main_pid:
        return True
    return base in ("biu-m0", "biu-m0.exe", "biu")


def is_webview_proc(name: str, pid: int, main_pid: int) -> bool:
    if is_rust_main(name, pid, main_pid):
        return False
    low = name.lower()
    return any(k in low for k in ("webkit", "webcontent", "webview", "msedgewebview2", "wpe")) or pid != main_pid


def analyze_series(rows, warmup: float, tail_window: float):
    max_sec = max(r["seconds"] for r in rows)
    tail_start = max(warmup, max_sec - tail_window)
    total_pts = [(r["seconds"], r["rss_bytes"]) for r in rows]
    total_post = [(s, b) for s, b in total_pts if s >= warmup]
    total_tail = [(s, b) for s, b in total_pts if s >= tail_start]
    if len(total_post) < 2 or len(total_tail) < 2:
        sys.exit(f"ERROR: insufficient post-warmup samples (max_sec={max_sec}, post={len(total_post)}, tail={len(total_tail)})")

    first_pids = rows[0].get("pids") or []
    main_pid = rows[0].get("app_pid") or (first_pids[0] if first_pids else 0)

    by_proc = {}
    for r in rows:
        sec = r["seconds"]
        procs = r.get("processes")
        if procs:
            for proc in procs:
                pid = int(proc["pid"])
                pname = str(proc.get("name", pid)).rsplit("/", 1)[-1]
                by_proc.setdefault((pid, pname), []).append((sec, int(proc["rss_bytes"])))
        else:
            by_proc.setdefault((main_pid or 0, "process_tree_total"), []).append((sec, int(r["rss_bytes"])))

    processes_out = []
    rust_main_procs = []
    webview_procs = []
    for (pid, pname), pts in sorted(by_proc.items()):
        p_post = [(s, b) for s, b in pts if s >= warmup]
        p_tail = [(s, b) for s, b in pts if s >= tail_start]
        s_post = ols_slope_mb_per_min(p_post) if len(p_post) >= 2 else 0.0
        s_tail = ols_slope_mb_per_min(p_tail) if len(p_tail) >= 2 else 0.0
        role = (
            "total_only"
            if pname == "process_tree_total"
            else ("rust_main" if is_rust_main(pname, pid, main_pid) else ("webview" if is_webview_proc(pname, pid, main_pid) else "other"))
        )
        entry = {
            "pid": pid,
            "name": pname,
            "role": role,
            "samples": len(pts),
            "post_warmup_samples": len(p_post),
            "last_30min_samples": len(p_tail),
            "post_warmup_ols_MB_per_min": s_post,
            "last_30min_ols_MB_per_min": s_tail,
            "peak_rss_bytes": max(b for _, b in pts),
            "last_rss_bytes": pts[-1][1],
        }
        processes_out.append(entry)
        if role == "rust_main":
            rust_main_procs.append(entry)
        elif role == "webview":
            webview_procs.append(entry)

    total_summary = {
        "samples": len(total_pts),
        "max_seconds": max_sec,
        "warmup_seconds": warmup,
        "tail_start_seconds": tail_start,
        "post_warmup_samples": len(total_post),
        "last_30min_samples": len(total_tail),
        "post_warmup_ols_MB_per_min": ols_slope_mb_per_min(total_post),
        "last_30min_ols_MB_per_min": ols_slope_mb_per_min(total_tail),
        "peak_rss_bytes": max(b for _, b in total_pts),
        "last_rss_bytes": total_pts[-1][1],
    }

    rust_main_summary = None
    if rust_main_procs:
        rust_main_summary = {
            "pids": [p["pid"] for p in rust_main_procs],
            "post_warmup_ols_MB_per_min": sum(p["post_warmup_ols_MB_per_min"] for p in rust_main_procs),
            "last_30min_ols_MB_per_min": sum(p["last_30min_ols_MB_per_min"] for p in rust_main_procs),
        }

    webview_summary = {
        "pids": [p["pid"] for p in webview_procs],
        "post_warmup_ols_MB_per_min": sum(p["post_warmup_ols_MB_per_min"] for p in webview_procs),
        "last_30min_ols_MB_per_min": sum(p["last_30min_ols_MB_per_min"] for p in webview_procs),
        "processes": webview_procs,
    }

    return {
        "total": total_summary,
        "rust_main": rust_main_summary,
        "webview": webview_summary,
        "processes": processes_out,
    }


primary = analyze_series(load_jsonl(args.memory), args.warmup_seconds, args.tail_window_seconds)
control = (
    analyze_series(load_jsonl(args.control), args.warmup_seconds, args.tail_window_seconds)
    if args.control
    else None
)

tot_post = primary["total"]["post_warmup_ols_MB_per_min"]
tot_tail = primary["total"]["last_30min_ols_MB_per_min"]

# Rule 1: 全部进程两段均 <= 0 -> pass
all_procs_non_positive = (
    tot_post <= 0.0
    and tot_tail <= 0.0
    and all(
        p["post_warmup_ols_MB_per_min"] <= 0.0 and p["last_30min_ols_MB_per_min"] <= 0.0
        for p in primary["processes"]
    )
)

verdict = "fail"
ratio = None
reasons = []

if all_procs_non_positive or (tot_post <= 0.0 and tot_tail <= 0.0 and (primary["rust_main"] is None or (primary["rust_main"]["post_warmup_ols_MB_per_min"] <= 0.0 and primary["rust_main"]["last_30min_ols_MB_per_min"] <= 0.0))):
    if all_procs_non_positive:
        verdict = "pass"
        reasons.append("all processes and total post-warmup & last-30min slopes <= 0")
    elif tot_post <= 0.0 and tot_tail <= 0.0:
        verdict = "pass"
        reasons.append("total post-warmup & last-30min slopes <= 0")

if verdict != "pass":
    rust_main = primary["rust_main"]
    webview_post = primary["webview"]["post_warmup_ols_MB_per_min"]
    max_wv_post = max((p["post_warmup_ols_MB_per_min"] for p in primary["webview"]["processes"]), default=0.0)
    rust_ok = (
        rust_main is not None
        and rust_main["post_warmup_ols_MB_per_min"] <= 0.05
        and rust_main["last_30min_ols_MB_per_min"] <= 0.05
    )
    total_ok = tot_post <= 1.5
    wv_concentrated = len(primary["webview"]["processes"]) > 0 and (
        webview_post > (rust_main["post_warmup_ols_MB_per_min"] if rust_main else 0.0)
        and max_wv_post > 0.05
        and (tot_post <= 0 or webview_post >= 0.5 * tot_post)
    )
    if not rust_ok:
        reasons.append(f"rust_main slope condition failed: {rust_main}")
    if not total_ok:
        reasons.append(f"total post-warmup slope {tot_post:.6f} > 1.5 MB/min")
    if not wv_concentrated:
        reasons.append(f"growth not concentrated in webview: webview_post={webview_post:.6f}, max_wv_post={max_wv_post:.6f}")

    if rust_ok and total_ok and wv_concentrated:
        if control is None:
            verdict = "needs_control"
            reasons.append("rust_main <= 0.05, total <= 1.5, growth in webview, but --control not provided")
        else:
            ctrl_post = control["total"]["post_warmup_ols_MB_per_min"]
            if ctrl_post > 0 and tot_post > 0:
                ratio = tot_post / ctrl_post
                if 0.5 <= ratio <= 2.0:
                    verdict = "known_limit"
                    reasons.append(f"control slope ratio {ratio:.4f} within [0.5, 2.0]")
                else:
                    verdict = "fail"
                    reasons.append(f"control slope ratio {ratio:.4f} outside [0.5, 2.0]")
            else:
                verdict = "fail"
                reasons.append(f"cannot form valid positive slope ratio in [0.5, 2.0] (primary={tot_post:.6f}, control={ctrl_post:.6f})")

output_obj = {
    "verdict": verdict,
    "reasons": reasons,
    "control_total_post_warmup_slope_ratio": ratio,
    "primary": primary,
    "control": control,
}

args.out.parent.mkdir(parents=True, exist_ok=True)
args.out.write_text(json.dumps(output_obj, indent=2) + "\n")
print(json.dumps({"verdict": verdict, "total_post_MB_per_min": tot_post, "total_tail_MB_per_min": tot_tail, "ratio": ratio, "reasons": reasons}))
