#!/usr/bin/env python3
"""The multi-solver plate-solving benchmark.

256 images (86 small + 85 medium + 85 large) solved by every candidate, in
four strictly separated groups so that nothing unfair is ever compared:

  A  blind          no hints at all
  B  scale          field size / pixel scale known, position unknown
  C  hinted         position AND field size known
  D  warm/cached    same hints as C, but every caching or warm-start
                    mechanism left enabled and each image solved twice
                    back-to-back (rep 0 then rep 1)

Results stream into a CSV (one row per solve) so partial runs are usable and
the benchmark is resumable: an already-recorded (group, candidate, image, rep)
is skipped.

usage: great_bench.py <group> [candidate ...]
"""
import csv
import io
import json
import math
import mimetypes
import os
import subprocess
import sys
import threading
import time
import urllib.request
import uuid

BASE = os.path.expanduser("~/bench/great")
IMG_DIR = os.path.join(BASE, "images")
WIN_IMG_DIR = r"C:\astrometry\great\images"
CSV_PATH = os.path.join(BASE, "results", "solves.csv")
PS = "/mnt/c/Windows/System32/WindowsPowerShell/v1.0/powershell.exe"
PS1 = r"C:\astrometry\great\measure_win_batch.ps1"

TIMEOUT = {"A": 180, "B": 120, "C": 120, "D": 120}

# All Sky Plate Solver cannot solve the TESS fields at all (it hangs past 240 s
# on medium and errors out on large), so running all 85 would cost hours of
# pure timeout. It runs a documented 20-image pilot on those classes instead;
# the failure rate is reported over the pilot, not extrapolated.
CLASS_CAP = {"asps": {"medium": 20, "large": 20},
             # the Gaia TAP service answers 7 deg / 14 deg cone queries with
             # HTTP 408 no matter the row limit, so twirl can only run on the
             # small class. This is an archive limit, not a solver failure.
             "twirl": {"medium": 0, "large": 0}}

FIELDS = ["group", "candidate", "image_id", "file", "fov_class", "fov_deg",
          "true_ra", "true_dec", "true_pixscale", "rep", "status", "wall_s",
          "cpu_s", "peak_mem_bytes", "solved_ra", "solved_dec", "solved_scale",
          "err_arcsec", "scale_err_pct", "extra", "utc"]

CONTAINERS = {"faint_light": "faint_light",
              "anet_direct": "astrometrynet-astrometry-1",
              "anet_nova": "astrometrynet-astrometry-1",
              "twirl": "twirl_solver"}

GROUP_MEMBERS = {
    # ASPS always needs a scale (focal+pixel), so it cannot be blind.
    # twirl always needs position AND scale, so it only exists in C/D.
    "A": ["faint_light", "anet_direct", "anet_nova", "ansvr", "astap", "ps3"],
    "B": ["faint_light", "anet_direct", "anet_nova", "ansvr", "astap", "ps3", "asps"],
    "C": ["faint_light", "anet_direct", "anet_nova", "ansvr", "astap", "ps3", "asps", "twirl"],
    "D": ["faint_light", "anet_direct", "anet_nova", "ansvr", "astap", "ps3", "asps", "twirl"],
}


def sep_arcsec(ra1, dec1, ra2, dec2):
    r1, d1, r2, d2 = map(math.radians, (ra1, dec1, ra2, dec2))
    c = (math.sin(d1) * math.sin(d2) +
         math.cos(d1) * math.cos(d2) * math.cos(r1 - r2))
    return math.degrees(math.acos(max(-1.0, min(1.0, c)))) * 3600.0


# ------------------------------------------------------------------ cgroup
class CGroup:
    def __init__(self, container):
        self.container = container
        self.refresh()

    def refresh(self):
        cid = subprocess.check_output(["docker", "inspect", "-f", "{{.Id}}", self.container],
                                      text=True).strip()
        for b in ("/sys/fs/cgroup/system.slice/docker-%s.scope" % cid,
                  "/sys/fs/cgroup/docker/%s" % cid,
                  "/sys/fs/cgroup/unified/docker/%s" % cid):
            if os.path.isfile(os.path.join(b, "cpu.stat")):
                self.base = b
                return
        raise RuntimeError("no cgroup for %s" % self.container)

    def cpu(self):
        with open(os.path.join(self.base, "cpu.stat")) as f:
            for line in f:
                if line.startswith("usage_usec"):
                    return int(line.split()[1]) / 1e6
        return 0.0

    def mem(self):
        # anonymous memory only. memory.current would also count the page
        # cache of the 5 GB of memory-mapped index files, which is host cache
        # rather than solver footprint and would dwarf everything else. The
        # Windows candidates are measured by working set, which likewise
        # excludes the bulk of mapped-file cache, so anon is the comparable
        # quantity.
        try:
            with open(os.path.join(self.base, "memory.stat")) as f:
                for line in f:
                    if line.startswith("anon "):
                        return int(line.split()[1])
        except Exception:
            pass
        with open(os.path.join(self.base, "memory.current")) as f:
            return int(f.read())


class MemSampler(threading.Thread):
    def __init__(self, cg):
        super().__init__(daemon=True)
        self.cg, self.peak, self.stop = cg, 0, threading.Event()

    def run(self):
        while not self.stop.is_set():
            try:
                self.peak = max(self.peak, self.cg.mem())
            except Exception:
                pass
            self.stop.wait(0.05)

    def finish(self):
        self.stop.set()
        self.join(timeout=2)
        return self.peak


# -------------------------------------------------------------------- http
def post_multipart(url, fields, filepath=None, timeout=900):
    b = uuid.uuid4().hex
    body = io.BytesIO()
    for k, v in fields.items():
        body.write(b"--%s\r\n" % b.encode())
        body.write(b'Content-Disposition: form-data; name="%s"\r\n\r\n' % k.encode())
        body.write(v.encode() + b"\r\n")
    if filepath:
        fn = os.path.basename(filepath)
        ct = mimetypes.guess_type(fn)[0] or "application/octet-stream"
        body.write(b"--%s\r\n" % b.encode())
        body.write(b'Content-Disposition: form-data; name="file"; filename="%s"\r\n' % fn.encode())
        body.write(b"Content-Type: %s\r\n\r\n" % ct.encode())
        body.write(open(filepath, "rb").read())
        body.write(b"\r\n")
    body.write(b"--%s--\r\n" % b.encode())
    data = body.getvalue()
    req = urllib.request.Request(url, data=data,
                                 headers={"Content-Type": "multipart/form-data; boundary=%s" % b})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


def get_json(url, timeout=60):
    with urllib.request.urlopen(url, timeout=timeout) as r:
        return json.loads(r.read().decode())


def restart_container(name, ready_url, apikey=None):
    subprocess.run(["docker", "restart", name], check=True, stdout=subprocess.DEVNULL)
    t0 = time.monotonic()
    while time.monotonic() - t0 < 180:
        try:
            if apikey is None:
                urllib.request.urlopen(ready_url, timeout=5).read()
                return
            if post_multipart(ready_url, {"request-json": json.dumps({"apikey": apikey})},
                              timeout=10).get("status") == "success":
                return
        except Exception:
            time.sleep(0.5)
    raise RuntimeError("%s did not come back" % name)


# --------------------------------------------------------------- hint sets
def nova_hints(group, job):
    """Hints in nova request-json form (faint_light + astrometry.net + ansvr)."""
    h = {}
    if group in ("B", "C", "D"):
        h.update({"scale_units": "arcsecperpix", "scale_type": "ul",
                  "scale_lower": round(job["pixscale"] * 0.9, 4),
                  "scale_upper": round(job["pixscale"] * 1.1, 4)})
    if group in ("C", "D"):
        h.update({"center_ra": job["ra"], "center_dec": job["dec"], "radius": 5})
    return h


def direct_query(group, job):
    q = []
    if group in ("B", "C", "D"):
        q.append("scale_low=%.4f&scale_high=%.4f" % (job["pixscale"] * 0.9, job["pixscale"] * 1.1))
    if group in ("C", "D"):
        q.append("ra=%.5f&dec=%.5f&radius=5" % (job["ra"], job["dec"]))
    return "&".join(q)


# ------------------------------------------------------------ solve drivers
def solve_nova(host, container, apikey, group, job, restart):
    cg = CGroup(container)
    if restart:
        restart_container(container, "http://%s/api/login" % host, apikey)
        cg.refresh()
    sess = post_multipart("http://%s/api/login" % host,
                          {"request-json": json.dumps({"apikey": apikey})})["session"]
    req = {"session": sess}
    req.update(nova_hints(group, job))
    cpu0, ms = cg.cpu(), MemSampler(cg)
    ms.start()
    t0 = time.monotonic()
    deadline = TIMEOUT[group]
    status, jobid, calib = "timeout", None, None
    try:
        sub = post_multipart("http://%s/api/upload" % host,
                             {"request-json": json.dumps(req)},
                             filepath=os.path.join(IMG_DIR, job["file"]))["subid"]
        while time.monotonic() - t0 < deadline:
            try:
                jl = [j for j in (get_json("http://%s/api/submissions/%s" % (host, sub)).get("jobs") or []) if j]
                if jl:
                    jobid = jl[0]
                    break
            except Exception:
                pass
            time.sleep(0.1)
        while jobid is not None and time.monotonic() - t0 < deadline:
            try:
                st = get_json("http://%s/api/jobs/%s" % (host, jobid)).get("status", "solving")
                if st in ("success", "failure"):
                    status = st
                    break
            except Exception:
                pass
            time.sleep(0.1)
        if status == "success":
            calib = get_json("http://%s/api/jobs/%s/calibration" % (host, jobid))
    except Exception as e:
        status = "error:%s" % type(e).__name__
    wall = time.monotonic() - t0
    return {"status": status, "wall_s": wall, "cpu_s": cg.cpu() - cpu0,
            "peak_mem": ms.finish(),
            "ra": (calib or {}).get("ra"), "dec": (calib or {}).get("dec"),
            "scale": (calib or {}).get("pixscale")}


def solve_direct(host, container, group, job, restart):
    cg = CGroup(container)
    if restart:
        restart_container(container, "http://%s/" % host)
        cg.refresh()
    q = direct_query(group, job)
    url = "http://%s/solve%s" % (host, ("?" + q) if q else "")
    cpu0, ms = cg.cpu(), MemSampler(cg)
    ms.start()
    t0 = time.monotonic()
    try:
        req = urllib.request.Request(url, data=open(os.path.join(IMG_DIR, job["file"]), "rb").read(),
                                     headers={"Content-Type": "application/octet-stream"})
        with urllib.request.urlopen(req, timeout=TIMEOUT[group] + 240) as r:
            resp = json.loads(r.read().decode())
    except Exception as e:
        resp = {"status": "error:%s" % type(e).__name__}
    wall = time.monotonic() - t0
    c = resp.get("calib") or {}
    return {"status": resp.get("status"), "wall_s": wall, "cpu_s": cg.cpu() - cpu0,
            "peak_mem": ms.finish(), "ra": c.get("ra"), "dec": c.get("dec"),
            "scale": c.get("pixscale")}


def solve_twirl(group, job):
    cg = CGroup("twirl_solver")
    cache = 1 if group == "D" else 0
    url = ("http://localhost:8200/solve?ra=%.5f&dec=%.5f&fov=%.5f&cache=%d"
           % (job["ra"], job["dec"], job["fov_deg"], cache))
    cpu0, ms = cg.cpu(), MemSampler(cg)
    ms.start()
    t0 = time.monotonic()
    try:
        req = urllib.request.Request(url, data=open(os.path.join(IMG_DIR, job["file"]), "rb").read(),
                                     headers={"Content-Type": "application/octet-stream"})
        with urllib.request.urlopen(req, timeout=TIMEOUT[group] + 240) as r:
            resp = json.loads(r.read().decode())
    except Exception as e:
        resp = {"status": "error:%s" % type(e).__name__}
    wall = time.monotonic() - t0
    c = resp.get("calib") or {}
    extra = "detect=%.3f;gaia=%.3f;match=%.3f" % (resp.get("t_detect", -1), resp.get("t_gaia", -1),
                                                  resp.get("t_match", -1)) if "t_detect" in resp else ""
    return {"status": resp.get("status"), "wall_s": wall, "cpu_s": cg.cpu() - cpu0,
            "peak_mem": ms.finish(), "ra": c.get("ra"), "dec": c.get("dec"),
            "scale": c.get("pixscale"), "extra": extra}


def solve_windows_batch(solver, group, jobs):
    """Run a whole list of jobs in one PowerShell invocation (JSONL in/out)."""
    tag = "%s_%s" % (solver, group)
    jl = os.path.join(BASE, "results", "_jobs_%s.jsonl" % tag)
    ol = os.path.join(BASE, "results", "_out_%s.jsonl" % tag)
    win_jl = r"C:\astrometry\great\results\_jobs_%s.jsonl" % tag
    win_ol = r"C:\astrometry\great\results\_out_%s.jsonl" % tag
    for p in (jl, ol):
        if os.path.exists(p):
            os.remove(p)
    with open(jl, "w") as f:
        for j in jobs:
            f.write(json.dumps({"id": j["id"], "win_path": WIN_IMG_DIR + "\\" + j["file"],
                                "fov_deg": j["fov_deg"], "ra": j["ra"], "dec": j["dec"],
                                "pixscale": j["pixscale"], "timeout_s": TIMEOUT[group]}) + "\n")
    subprocess.run(["cp", jl, "/mnt/c/astrometry/great/results/"], check=False)
    subprocess.run([PS, "-NoProfile", "-ExecutionPolicy", "Bypass", "-File", PS1,
                    "-solver", solver, "-group", group, "-jobs", win_jl, "-outfile", win_ol],
                   capture_output=True, text=True, timeout=len(jobs) * (TIMEOUT[group] + 30) + 600)
    out = {}
    winout = "/mnt/c/astrometry/great/results/_out_%s.jsonl" % tag
    if os.path.exists(winout):
        for line in open(winout):
            line = line.strip()
            if line.startswith("{"):
                d = json.loads(line)
                out[d["id"]] = d
    return out


# --------------------------------------------------------------------- main
def load_done():
    done = set()
    if os.path.exists(CSV_PATH):
        with open(CSV_PATH) as f:
            for row in csv.DictReader(f):
                done.add((row["group"], row["candidate"], row["image_id"], row["rep"]))
    return done


def append_rows(rows):
    new = not os.path.exists(CSV_PATH)
    with open(CSV_PATH, "a", newline="") as f:
        w = csv.DictWriter(f, fieldnames=FIELDS)
        if new:
            w.writeheader()
        for r in rows:
            w.writerow(r)


def make_row(group, cand, job, rep, res):
    ra, dec = res.get("ra"), res.get("dec")
    err = sep_arcsec(job["ra"], job["dec"], ra, dec) if (ra is not None and dec is not None) else None
    sc = res.get("scale")
    scerr = (abs(sc - job["pixscale"]) / job["pixscale"] * 100.0) if sc else None
    status = res.get("status") or "error"
    # a "solve" that lands far from truth is a false match, not a success
    if status == "success" and err is not None and err > 0.10 * job["fov_deg"] * 3600.0:
        status = "false_match"
    return {"group": group, "candidate": cand, "image_id": job["id"], "file": job["file"],
            "fov_class": job["cls"], "fov_deg": job["fov_deg"], "true_ra": job["ra"],
            "true_dec": job["dec"], "true_pixscale": job["pixscale"], "rep": rep,
            "status": status, "wall_s": round(res.get("wall_s", -1), 4),
            "cpu_s": round(res.get("cpu_s", -1), 4),
            "peak_mem_bytes": int(res.get("peak_mem") or 0),
            "solved_ra": ra, "solved_dec": dec, "solved_scale": sc,
            "err_arcsec": round(err, 4) if err is not None else "",
            "scale_err_pct": round(scerr, 4) if scerr is not None else "",
            "extra": res.get("extra", ""),
            "utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}


def main():
    group = sys.argv[1]
    only = sys.argv[2:] if len(sys.argv) > 2 else None
    manifest = json.load(open(os.environ.get("MANIFEST", os.path.join(BASE, "manifest.json"))))
    os.makedirs(os.path.join(BASE, "results"), exist_ok=True)
    done = load_done()

    # group D solves each image twice back-to-back on half the set
    if group == "D":
        by_cls = {}
        for m in manifest:
            by_cls.setdefault(m["cls"], []).append(m)
        images = []
        for cls, lst in by_cls.items():
            images += lst[:len(lst) // 2]
        reps = [0, 1]
    else:
        images = list(manifest)
        reps = [0]

    for cand in GROUP_MEMBERS[group]:
        if only and cand not in only:
            continue
        caps = CLASS_CAP.get(cand, {})
        pool, per_cls = [], {}
        for j in images:
            c = j["cls"]
            per_cls[c] = per_cls.get(c, 0) + 1
            if c in caps and (caps[c] == 0 or per_cls[c] > caps[c]):
                continue
            pool.append(j)
        todo = [(j, r) for j in pool for r in reps
                if (group, cand, j["id"], str(r)) not in done]
        if not todo:
            print("== %s/%s already complete" % (group, cand), flush=True)
            continue
        print("== group %s | %s | %d solves" % (group, cand, len(todo)), flush=True)
        t_start = time.time()

        if cand in ("ansvr", "astap", "asps", "ps3"):
            # windows candidates run as one batch per (group, candidate)
            jobs = [j for j, r in todo]
            res = solve_windows_batch(cand, group, jobs)
            rows = []
            for j, r in todo:
                d = res.get(j["id"], {"status": "no_result"})
                rows.append(make_row(group, cand, j, r, d))
            append_rows(rows)
        else:
            rows = []
            for n, (j, r) in enumerate(todo):
                if cand == "faint_light":
                    # Group A restarts before EVERY solve: with no hints, a
                    # remembered pixel scale would be a real advantage, so the
                    # blind group must start cold every time. In B/C the hints
                    # already supply scale (and position), so a restart every
                    # 16 solves is enough to keep state from accumulating,
                    # and it keeps ~15 s of container prewarm from dominating
                    # the run. Group D deliberately never restarts.
                    if group == "A":
                        do_restart = True
                    elif group == "D":
                        do_restart = False
                    else:
                        do_restart = (n % 16 == 0)
                    res = solve_nova("localhost:8100", "faint_light", "x", group, j,
                                     restart=do_restart)
                elif cand == "anet_nova":
                    res = solve_nova("localhost:8000", CONTAINERS[cand], "1jcrmadfnxngxscd",
                                     group, j, restart=False)
                elif cand == "anet_direct":
                    res = solve_direct("localhost:8001", CONTAINERS[cand], group, j, restart=False)
                elif cand == "twirl":
                    res = solve_twirl(group, j)
                rows.append(make_row(group, cand, j, r, res))
                if len(rows) >= 5:
                    append_rows(rows)
                    rows = []
                if (n + 1) % 25 == 0:
                    print("   %s %d/%d (%.1f min elapsed)" % (cand, n + 1, len(todo),
                                                              (time.time() - t_start) / 60), flush=True)
            if rows:
                append_rows(rows)
        print("   %s done in %.1f min" % (cand, (time.time() - t_start) / 60), flush=True)
    print("GROUP %s COMPLETE" % group, flush=True)


if __name__ == "__main__":
    main()
