#!/usr/bin/env python3
"""Turn solves.csv into chart-ready summaries.

Outputs (in the results directory):
  summary_by_group_candidate.csv       one row per (group, candidate)
  summary_by_group_candidate_class.csv one row per (group, candidate, FOV class)
  solves_enriched.csv                  every solve + consensus-based accuracy

Consensus accuracy: for each image we take the median RA/Dec across all
candidates that solved it (in any group). Comparing each solve to that
consensus removes any bias in the reference WCS - useful for the TESS classes,
where a TAN fit to a distorted 12 deg field legitimately disagrees with the
pipeline centre by a few pixels.
"""
import csv
import math
import os
import statistics as st
import sys
from collections import defaultdict

RES = sys.argv[1] if len(sys.argv) > 1 else os.path.expanduser("~/bench/great/results")
SRC = os.path.join(RES, "solves.csv")

GOOD = ("success",)


def sep_arcsec(ra1, dec1, ra2, dec2):
    r1, d1, r2, d2 = map(math.radians, (ra1, dec1, ra2, dec2))
    c = (math.sin(d1) * math.sin(d2) +
         math.cos(d1) * math.cos(d2) * math.cos(r1 - r2))
    return math.degrees(math.acos(max(-1.0, min(1.0, c)))) * 3600.0


def f(x):
    try:
        return float(x)
    except (TypeError, ValueError):
        return None


rows = list(csv.DictReader(open(SRC)))
print("%d solve records" % len(rows))

# ---- per-image consensus position from every successful solve --------------
by_img = defaultdict(list)
for r in rows:
    if r["status"] in GOOD and f(r["solved_ra"]) is not None:
        by_img[r["image_id"]].append((f(r["solved_ra"]), f(r["solved_dec"])))
consensus = {}
for img, pts in by_img.items():
    if len(pts) >= 3:
        consensus[img] = (st.median([p[0] for p in pts]), st.median([p[1] for p in pts]))

for r in rows:
    ra, dec = f(r["solved_ra"]), f(r["solved_dec"])
    c = consensus.get(r["image_id"])
    r["err_consensus_arcsec"] = (round(sep_arcsec(ra, dec, c[0], c[1]), 4)
                                 if (ra is not None and c) else "")
    r["n_consensus"] = len(by_img.get(r["image_id"], []))

with open(os.path.join(RES, "solves_enriched.csv"), "w", newline="") as fh:
    w = csv.DictWriter(fh, fieldnames=list(rows[0].keys()))
    w.writeheader()
    w.writerows(rows)


def pct(v, p):
    v = sorted(v)
    if not v:
        return None
    k = (len(v) - 1) * p
    lo = int(k)
    hi = min(lo + 1, len(v) - 1)
    return v[lo] + (v[hi] - v[lo]) * (k - lo)


def summarise(group_keys, out_name, header):
    buckets = defaultdict(list)
    for r in rows:
        buckets[tuple(r[k] for k in group_keys)].append(r)
    out = []
    for key, rs in sorted(buckets.items()):
        ok = [r for r in rs if r["status"] in GOOD]
        walls = [f(r["wall_s"]) for r in ok if f(r["wall_s"]) is not None]
        cpus = [f(r["cpu_s"]) for r in ok if f(r["cpu_s"]) is not None]
        mems = [f(r["peak_mem_bytes"]) for r in rs if f(r["peak_mem_bytes"])]
        errs = [f(r["err_arcsec"]) for r in ok if f(r["err_arcsec"]) is not None]
        cerr = [f(r["err_consensus_arcsec"]) for r in ok if f(r["err_consensus_arcsec"]) is not None]
        scerr = [f(r["scale_err_pct"]) for r in ok if f(r["scale_err_pct"]) is not None]
        n_false = sum(1 for r in rs if r["status"] == "false_match")
        n_to = sum(1 for r in rs if r["status"] in ("timeout", "no_output", "no_result"))
        rec = dict(zip(header, key))
        rec.update({
            "n_solves": len(rs), "n_success": len(ok),
            "success_rate_pct": round(100.0 * len(ok) / len(rs), 2) if rs else "",
            "n_false_match": n_false, "n_timeout_or_noout": n_to,
            "wall_median_s": round(st.median(walls), 4) if walls else "",
            "wall_mean_s": round(st.mean(walls), 4) if walls else "",
            "wall_p95_s": round(pct(walls, 0.95), 4) if walls else "",
            "wall_min_s": round(min(walls), 4) if walls else "",
            "wall_max_s": round(max(walls), 4) if walls else "",
            "wall_total_s": round(sum(walls), 2) if walls else "",
            "cpu_median_s": round(st.median(cpus), 4) if cpus else "",
            "cpu_total_s": round(sum(cpus), 2) if cpus else "",
            "mem_peak_mb": round(max(mems) / 1048576.0, 1) if mems else "",
            "mem_median_mb": round(st.median(mems) / 1048576.0, 1) if mems else "",
            "err_median_arcsec": round(st.median(errs), 3) if errs else "",
            "err_p95_arcsec": round(pct(errs, 0.95), 3) if errs else "",
            "err_consensus_median_arcsec": round(st.median(cerr), 3) if cerr else "",
            "scale_err_median_pct": round(st.median(scerr), 4) if scerr else "",
        })
        out.append(rec)
    cols = header + ["n_solves", "n_success", "success_rate_pct", "n_false_match",
                     "n_timeout_or_noout", "wall_median_s", "wall_mean_s", "wall_p95_s",
                     "wall_min_s", "wall_max_s", "wall_total_s", "cpu_median_s",
                     "cpu_total_s", "mem_peak_mb", "mem_median_mb", "err_median_arcsec",
                     "err_p95_arcsec", "err_consensus_median_arcsec", "scale_err_median_pct"]
    with open(os.path.join(RES, out_name), "w", newline="") as fh:
        w = csv.DictWriter(fh, fieldnames=cols)
        w.writeheader()
        w.writerows(out)
    return out


s1 = summarise(["group", "candidate"], "summary_by_group_candidate.csv", ["group", "candidate"])
summarise(["group", "candidate", "fov_class"], "summary_by_group_candidate_class.csv",
          ["group", "candidate", "fov_class"])

print("\n%-6s %-14s %6s %7s %9s %9s %9s %8s %9s" % (
    "group", "candidate", "n", "ok%", "wall_med", "wall_p95", "cpu_med", "mem_MB", "err\""))
for r in s1:
    print("%-6s %-14s %6s %7s %9s %9s %9s %8s %9s" % (
        r["group"], r["candidate"], r["n_solves"], r["success_rate_pct"],
        r["wall_median_s"], r["wall_p95_s"], r["cpu_median_s"], r["mem_peak_mb"],
        r["err_median_arcsec"]))
print("\nwrote summaries to %s" % RES)
