#!/usr/bin/env python3
"""Benchmark ASTAP (Windows CLI via WSL interop) on the same image set and
record shape as bench.py, so analyze.py and Benchmark.md can compare them.

Runs on the WSL side of the Windows benchmark host. Per solve, a PowerShell wrapper starts
astap_cli.exe and reports wall seconds (stopwatch around the process),
CPU seconds (TotalProcessorTime) and peak working set of that process.
Solutions are read from the .ini ASTAP writes next to the image; a
"solved" result whose center is off by more than 10% of the FOV is
classified `false_match` (ASTAP is run position-blind but with the true
FOV as a hint, and a loose quad tolerance can produce confident wrong
solutions on these survey renders).

Modes:
  sample    try candidate (db, stars, tolerance) configs on the first N
            images of each class and report correct-solve counts
  random    the 256 random images, per-class config from CONFIG
  scenario  the warm-start sequences (ASTAP is stateless; measures OS
            file-cache effects only)
"""
import argparse
import json
import math
import os
import subprocess
import time

WSL_IMG = os.path.expanduser("~/bench/images")
WIN_IMG = r"C:\Temp\flbench\images"
MNT_IMG = "/mnt/c/Temp/flbench/images"
PS = "/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
PS1 = r"C:\Temp\flbench\solve_one.ps1"

# per-class solver config, chosen by `--mode sample` results:
# (database, max_stars, quad_tolerance)
# (database, max_stars, quad_tolerance, extra_args)
CONFIG = {
    "s": ("d50", "500", "0.010", ""),
    "m": ("d50", "500", "0.010", ""),
    "l": ("g05", "1000", "0.015", ""),
    "xl": ("d20", "500", "0.010", ""),
}

CANDIDATES = {
    "s": [("d50", "500", "0.007", "-speed slow"), ("d50", "500", "0.010", ""), ("d50", "500", "0.007", "-z 2")],
    "m": [("d50", "500", "0.007", "-speed slow"), ("d50", "500", "0.010", ""), ("d50", "500", "0.007", "-z 2"), ("d20", "500", "0.007", "-speed slow")],
    "l": [("g05", "500", "0.007", "-speed slow"), ("g05", "500", "0.010", ""), ("d20", "500", "0.007", "-speed slow"), ("g05", "1000", "0.007", "-speed slow")],
    "xl": [("d20", "500", "0.007", "-speed slow"), ("g05", "500", "0.007", "-speed slow"), ("d20", "500", "0.010", ""), ("g05", "500", "0.010", "")],
}


def ang_sep_deg(ra1, dec1, ra2, dec2):
    r1, d1, r2, d2 = map(math.radians, (ra1, dec1, ra2, dec2))
    c = (math.sin(d1) * math.sin(d2) +
         math.cos(d1) * math.cos(d2) * math.cos(r1 - r2))
    return math.degrees(math.acos(max(-1.0, min(1.0, c))))


def parse_ini(path):
    out = {}
    try:
        with open(path, encoding="latin-1") as f:
            for line in f:
                if "=" in line:
                    k, v = line.split("=", 1)
                    out[k.strip()] = v.strip()
    except FileNotFoundError:
        pass
    return out


def solve(job, db, smax, tol, extra=""):
    img_win = WIN_IMG + "\\" + job["file"]
    ini = os.path.join(MNT_IMG, job["id"] + ".ini")
    if os.path.exists(ini):
        os.remove(ini)
    t0 = time.monotonic()
    proc = subprocess.run(
        [PS, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", PS1,
         "-img", img_win, "-fov", str(job["fov"]), "-db", db,
         "-smax", smax, "-tol", tol] + (["-extra", extra] if extra else []),
        capture_output=True, text=True, timeout=400)
    client_wall = time.monotonic() - t0
    kv = {}
    for line in proc.stdout.splitlines():
        line = line.strip()
        if "=" in line:
            k, v = line.split("=", 1)
            kv[k] = v
    rec = {
        "wall_total": float(kv.get("WALL") or client_wall),
        "cpu_sec": round(float(kv.get("CPU") or 0), 3),
        "mem_peak_during": int(kv.get("PEAK") or 0),
        "exit_code": int(kv.get("EXIT") or -1),
        "client_wall": round(client_wall, 3),
        "astap_db": db, "astap_stars": smax, "astap_tol": tol, "astap_extra": extra,
    }
    rec.update({k: job[k] for k in ("id", "role", "cls", "fov", "ra", "dec", "file")})
    info = parse_ini(ini)
    if info.get("PLTSOLVD") == "T":
        calib = {
            "ra": float(info["CRVAL1"]),
            "dec": float(info["CRVAL2"]),
            "pixscale": float(info["CDELT2"]) * 3600.0,
            "orientation": float(info.get("CROTA2", "0")),
        }
        rec["calib"] = calib
        err = ang_sep_deg(job["ra"], job["dec"], calib["ra"], calib["dec"])
        rec["err_center_deg"] = round(err, 5)
        rec["status"] = "success" if err <= 0.10 * job["fov"] else "false_match"
        if "WARNING" in info:
            rec["warning"] = info["WARNING"]
    else:
        rec["status"] = "failure"
        if "ERROR" in info:
            rec["error"] = info["ERROR"]
    return rec


def report(rec, label=""):
    err = ("err=%7.1f\"" % (rec["err_center_deg"] * 3600)) if "err_center_deg" in rec else "            "
    print("  %-14s %-11s %6.2fs cpu=%5.2fs %s %s" % (
        label or rec["id"], rec["status"], rec["wall_total"],
        rec["cpu_sec"], err, rec.get("astap_db", "")), flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--mode", choices=["sample", "random", "scenario"], required=True)
    ap.add_argument("--out")
    ap.add_argument("--limit", type=int, default=0)
    ap.add_argument("--sample-n", type=int, default=10)
    args = ap.parse_args()

    with open(os.path.join(WSL_IMG, "manifest.json")) as f:
        manifest = json.load(f)
    classes = []
    for j in manifest:
        if j["cls"] not in classes:
            classes.append(j["cls"])

    if args.mode == "sample":
        for cls in classes:
            jobs = [j for j in manifest if j["cls"] == cls and j["role"] == "random"][:args.sample_n]
            for db, smax, tol, extra in CANDIDATES[cls]:
                good = walls = 0
                for job in jobs:
                    rec = solve(job, db, smax, tol, extra)
                    if rec["status"] == "success":
                        good += 1
                        walls += rec["wall_total"]
                print("class %-2s db=%-4s s=%-4s t=%-5s x=[%s] -> %d/%d correct, mean wall %.2fs" % (
                    cls, db, smax, tol, extra, good, len(jobs),
                    (walls / good) if good else -1), flush=True)
        return

    results = []
    meta = {
        "server": "astap", "host": "benchmark host (Windows 11, ASTAP CLI)",
        "mode": args.mode,
        "astap_version": "CLI-2026.07.16",
        "config": {k: {"db": v[0], "max_stars": v[1], "quad_tolerance": v[2], "extra": v[3]}
                   for k, v in CONFIG.items()},
        "notes": "position-blind (-r 180), true FOV given per image; "
                 "wall/cpu/peak of the astap_cli.exe process itself "
                 "(PowerShell Start-Process wrapper)",
        "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }

    if args.mode == "random":
        for cls in classes:
            jobs = [j for j in manifest if j["cls"] == cls and j["role"] == "random"]
            if args.limit:
                jobs = jobs[:args.limit]
            db, smax, tol, extra = CONFIG[cls]
            print("== class %s: %d images (db=%s stars=%s tol=%s extra=%s)" %
                  (cls, len(jobs), db, smax, tol, extra), flush=True)
            for i, job in enumerate(jobs):
                rec = solve(job, db, smax, tol, extra)
                rec["label"] = "%s[%d]" % (cls, i)
                report(rec, rec["label"])
                results.append(rec)
    else:
        for cls in classes:
            print("== scenario class %s" % cls, flush=True)
            by_role = {j["role"]: j for j in manifest if j["cls"] == cls}
            db, smax, tol, extra = CONFIG[cls]
            seq = [("cold", by_role["base"])]
            seq += [("repeat%d" % k, by_role["base"]) for k in range(1, 6)]
            seq += [("near%d" % k, by_role["near%d" % k]) for k in range(1, 5)]
            seq += [("distant", by_role["distant"]), ("distant_repeat", by_role["distant"])]
            for label, job in seq:
                rec = solve(job, db, smax, tol, extra)
                rec["label"] = "%s_%s" % (cls, label)
                report(rec, rec["label"])
                results.append(rec)

    meta["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    out = os.path.expanduser(args.out)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w") as f:
        json.dump({"meta": meta, "results": results}, f, indent=1)
    ok = sum(1 for r in results if r["status"] == "success")
    fm = sum(1 for r in results if r["status"] == "false_match")
    print("wrote %s: %d/%d success, %d false matches" % (out, ok, len(results), fm), flush=True)


if __name__ == "__main__":
    main()
