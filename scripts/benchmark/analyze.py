#!/usr/bin/env python3
"""Summarize benchmark JSONs into Markdown tables."""
import json
import statistics as st
import sys
import os

RES = sys.argv[1] if len(sys.argv) > 1 else "."
CLS_NAME = {"s": "small 1.0deg", "m": "medium 4.0deg", "l": "large 10deg", "xl": "x-large 20deg"}
CLS_ORDER = ["s", "m", "l", "xl"]


def load(name):
    p = os.path.join(RES, name)
    if not os.path.exists(p):
        return None
    with open(p) as f:
        return json.load(f)


def pct(v, p):
    v = sorted(v)
    if not v:
        return float("nan")
    k = (len(v) - 1) * p
    f = int(k)
    c = min(f + 1, len(v) - 1)
    return v[f] + (v[c] - v[f]) * (k - f)


def fmt_s(x):
    return "%.2f" % x


def mb(x):
    return "%.0f" % (x / 1048576.0)


def random_table(data, name):
    rows = []
    for cls in CLS_ORDER:
        rs = [r for r in data["results"] if r["cls"] == cls]
        ok = [r for r in rs if r["status"] == "success"]
        walls = [r["wall_total"] for r in ok]
        cpus = [r["cpu_sec"] for r in ok]
        errs = [r["err_center_deg"] * 3600 for r in ok if "err_center_deg" in r]
        peak = max((r["mem_peak_during"] for r in rs), default=0)
        rows.append((cls, len(rs), len(ok), walls, cpus, errs, peak))
    print("\n### %s - random 256\n" % name)
    print("| FOV class | n | solved | median wall s | mean s | p95 s | min s | max s | median CPU s | peak RSS MB |")
    print("|---|---|---|---|---|---|---|---|---|---|")
    for cls, n, nok, walls, cpus, errs, peak in rows:
        if not walls:
            print("| %s | %d | 0 | - | - | - | - | - | - | %s |" % (CLS_NAME[cls], n, mb(peak)))
            continue
        print("| %s | %d | %d | %s | %s | %s | %s | %s | %s | %s |" % (
            CLS_NAME[cls], n, nok, fmt_s(st.median(walls)), fmt_s(st.mean(walls)),
            fmt_s(pct(walls, 0.95)), fmt_s(min(walls)), fmt_s(max(walls)),
            fmt_s(st.median(cpus)), mb(peak)))
    allok = [r for r in data["results"] if r["status"] == "success"]
    allw = [r["wall_total"] for r in allok]
    allc = [r["cpu_sec"] for r in allok]
    tot = len(data["results"])
    print("\nOverall: %d/%d solved (%.1f%%), total wall %.1f s, total CPU %.1f s, "
          "median wall %.2f s, first-solve-of-class excluded-median %.2f s" % (
              len(allok), tot, 100.0 * len(allok) / tot, sum(allw), sum(allc),
              st.median(allw),
              st.median([r["wall_total"] for r in allok if not r["label"].endswith("[0]")])))
    fails = [r for r in data["results"] if r["status"] != "success"]
    if fails:
        print("Failures: " + ", ".join("%s(%s)" % (r["id"], r["status"]) for r in fails))
    errs_all = [r["err_center_deg"] * 3600 for r in allok if "err_center_deg" in r]
    if errs_all:
        print("Center error vs requested pointing: median %.1f arcsec, p95 %.1f arcsec" % (
            st.median(errs_all), pct(errs_all, 0.95)))


def scenario_table(datasets):
    print("\n### Warm-start scenarios (wall seconds)\n")
    labels = ["cold", "repeat1", "repeat2", "repeat3", "repeat4", "repeat5",
              "near1", "near2", "near3", "near4", "distant", "distant_repeat"]
    for cls in CLS_ORDER:
        print("\n**%s**\n" % CLS_NAME[cls])
        print("| server | " + " | ".join(labels) + " |")
        print("|---" * (len(labels) + 1) + "|")
        for name, data in datasets:
            row = {r["label"]: r for r in data["results"] if r["cls"] == cls}
            cells = []
            for lab in labels:
                r = row.get("%s_%s" % (cls, lab))
                if r is None:
                    cells.append("-")
                elif r["status"] != "success":
                    cells.append("FAIL")
                else:
                    cells.append(fmt_s(r["wall_total"]))
            print("| %s | %s |" % (name, " | ".join(cells)))


def mem_summary(data, name):
    peak = max((r["mem_peak_during"] for r in data["results"]), default=0)
    base = data["meta"].get("baseline_mem", {})
    cpus = [r["cpu_sec"] for r in data["results"] if r["status"] == "success"]
    print("%s: overall peak %s MB, baselines after restart: %s, total CPU %.0f s" % (
        name, mb(peak),
        {k: mb(v) + " MB" for k, v in base.items()}, sum(cpus)))


SERVERS = [
    ("faint_light", "fl"),
    ("astrometry.net (job system)", "anet"),
    ("astrometry.net (direct)", "anet_direct"),
    ("ASTAP (CLI)", "astap"),
]

randoms, scenarios = {}, {}
for name, key in SERVERS:
    r = load(key + "_random.json")
    sc = load(key + "_scenario.json")
    if r:
        randoms[key] = r
        random_table(r, name)
        mem_summary(r, name + " random")
    if sc:
        scenarios[key] = sc
if scenarios:
    scenario_table([(n, scenarios[k]) for n, k in SERVERS if k in scenarios])
    for name, key in SERVERS:
        if key in scenarios:
            mem_summary(scenarios[key], name + " scenario")

if "fl" in randoms:
    print("\n### Speedup vs faint_light (median wall)\n")
    for name, key in SERVERS[1:]:
        if key not in randoms:
            continue
        for cls in CLS_ORDER:
            a = [r["wall_total"] for r in randoms[key]["results"] if r["cls"] == cls and r["status"] == "success"]
            b = [r["wall_total"] for r in randoms["fl"]["results"] if r["cls"] == cls and r["status"] == "success"]
            if a and b:
                print("%s %s: %.2f / %.2f = %.1fx" % (name, CLS_NAME[cls], st.median(a),
                                                      st.median(b), st.median(a) / st.median(b)))
