#!/usr/bin/env python3
"""faint_light automated test/benchmark suites.

Runs fl-server (fresh per section, so warm-start state is controlled)
against the benchmark image sets and reports performance numbers for
every case. Python 3 stdlib only.

Two execution modes (FLT_MODE):
  container (benchmark host)  drives the deployed faint_light docker
      container: docker restart per section, CPU seconds from the
      container cgroup (cpu.stat), memory from memory.current/peak —
      the exact method behind the published benchmark. This is what the
      Makefile's default targets use, executed on the benchmark host.
  local (dev fallback)        spawns target/release/faint-light and
      accounts CPU/RSS via /proc. Numbers are NOT comparable to the
      reference host.

Suites:
  regression    3 small + 3 medium + 3 large random fields (must solve,
                calibration checked) + a nearby warm-start sequence.
                Images: testdata/bench/ (committed). Exit 1 on failure.
  known-failing The dense-field cases faint_light currently cannot
                solve (the dense-field failure mode of the published benchmark).
                Informational: prints status + cost per case, exits 0.
                A case that starts PASSING is highlighted.
  full          The complete 256-random-image benchmark + warm-start
                scenarios, reproducing the published benchmark's faint_light
                numbers (same method: grouped per FOV class, server
                restarted between classes, 100 ms API polling).
                Images: testdata/bench-full/ (make fetch-bench-images).
                Writes analyze.py-compatible JSON to bench-out/.

Environment:
  FLT_MODE       "container" or "local" (default local)
  FLT_CONTAINER  container mode: container name (default faint_light)
  FLT_HOST       container mode: host:port of its API (default localhost:8100)
  FLT_BIN        local mode: server binary (default target/release/faint-light)
  FLT_INDEX_DIR  index files (default ./indexes, needs 4107-4119)
  FLT_PORT       local mode: port for the test server (default 8377)
  FLT_TIMEOUT    solve-timeout budget in seconds (default 300). Local
                 mode passes it to the server; container mode applies it
                 as the client deadline and the per-section docker
                 restart aborts the abandoned solve. Lower it to iterate
                 faster on known-failing cases.
"""
import argparse
import io
import json
import math
import mimetypes
import os
import signal
import statistics as st
import subprocess
import sys
import threading
import time
import urllib.request
import uuid

ROOT = os.path.dirname(os.path.dirname(os.path.dirname(os.path.abspath(__file__))))
MODE = os.environ.get("FLT_MODE", "local")
CONTAINER = os.environ.get("FLT_CONTAINER", "faint_light")
HOST = os.environ.get("FLT_HOST", "localhost:8100")
BIN = os.environ.get("FLT_BIN", os.path.join(ROOT, "target/release/faint-light"))
INDEX_DIR = os.environ.get("FLT_INDEX_DIR", os.path.join(ROOT, "indexes"))
PORT = int(os.environ.get("FLT_PORT", "8377"))
SOLVE_TIMEOUT = os.environ.get("FLT_TIMEOUT", "300")
POLL = 0.1
# Client-side per-solve deadline. Local mode: the server enforces
# FLT_TIMEOUT itself, leave generous slack. Container mode: the deployed
# container keeps its own configured timeout, so FLT_TIMEOUT acts through
# this deadline (the per-section docker restart aborts abandoned solves).
DEADLINE = float(SOLVE_TIMEOUT) + (120.0 if os.environ.get("FLT_MODE", "local") != "container" else 30.0)
HZ = os.sysconf("SC_CLK_TCK")


def Server():
    """Fresh server instance for a test section, in the configured mode."""
    return ContainerServer() if MODE == "container" else LocalServer()


# ------------------------------------------------------------- http client
def post_multipart(url, fields, filepath=None, timeout=60):
    boundary = uuid.uuid4().hex
    body = io.BytesIO()
    for name, value in fields.items():
        body.write(b"--%s\r\n" % boundary.encode())
        body.write(b'Content-Disposition: form-data; name="%s"\r\n\r\n' % name.encode())
        body.write(value.encode() + b"\r\n")
    if filepath:
        fname = os.path.basename(filepath)
        ctype = mimetypes.guess_type(fname)[0] or "application/octet-stream"
        body.write(b"--%s\r\n" % boundary.encode())
        body.write(b'Content-Disposition: form-data; name="file"; filename="%s"\r\n' % fname.encode())
        body.write(b"Content-Type: %s\r\n\r\n" % ctype.encode())
        with open(filepath, "rb") as f:
            body.write(f.read())
        body.write(b"\r\n")
    body.write(b"--%s--\r\n" % boundary.encode())
    data = body.getvalue()
    req = urllib.request.Request(url, data=data, headers={
        "Content-Type": "multipart/form-data; boundary=%s" % boundary})
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


def get_json(url, timeout=30):
    with urllib.request.urlopen(url, timeout=timeout) as r:
        return json.loads(r.read().decode())


def ang_sep_deg(ra1, dec1, ra2, dec2):
    r1, d1, r2, d2 = map(math.radians, (ra1, dec1, ra2, dec2))
    c = (math.sin(d1) * math.sin(d2) +
         math.cos(d1) * math.cos(d2) * math.cos(r1 - r2))
    return math.degrees(math.acos(max(-1.0, min(1.0, c))))


# ------------------------------------------------------------- server proc
class BaseServer:
    """Shared nova-API client bits; subclasses manage lifecycle/accounting."""

    def wait_ready(self, deadline=120):
        t0 = time.monotonic()
        while time.monotonic() - t0 < deadline:
            try:
                r = post_multipart(self.base + "/api/login",
                                   {"request-json": json.dumps({"apikey": "x"})})
                if r.get("status") == "success":
                    self.session = r["session"]
                    return
            except Exception:
                pass
            self.check_alive()
            time.sleep(0.2)
        sys.exit("server did not become ready in %ds" % deadline)

    def check_alive(self):
        pass


class LocalServer(BaseServer):
    """A fresh local fl-server process with /proc-based CPU+RSS accounting."""

    def __init__(self):
        if not os.path.exists(BIN):
            sys.exit("server binary not found: %s (run `make build`)" % BIN)
        env = dict(os.environ,
                   FAINT_LIGHT_PORT=str(PORT),
                   FAINT_LIGHT_INDEX_DIR=INDEX_DIR,
                   FAINT_LIGHT_SOLVE_TIMEOUT=SOLVE_TIMEOUT,
                   RUST_LOG=os.environ.get("FLT_RUST_LOG", "warn"))
        self.proc = subprocess.Popen([BIN], env=env,
                                     stdout=subprocess.DEVNULL,
                                     stderr=subprocess.DEVNULL)
        self.base = "http://127.0.0.1:%d" % PORT
        self.wait_ready()

    def check_alive(self):
        if self.proc.poll() is not None:
            sys.exit("server exited on startup (rc=%s); check indexes in %s"
                     % (self.proc.returncode, INDEX_DIR))

    def stop(self):
        self.proc.send_signal(signal.SIGTERM)
        try:
            self.proc.wait(timeout=10)
        except subprocess.TimeoutExpired:
            self.proc.kill()

    def cpu_seconds(self):
        with open("/proc/%d/stat" % self.proc.pid) as f:
            parts = f.read().rsplit(")", 1)[1].split()
        return (int(parts[11]) + int(parts[12])) / HZ  # utime + stime

    def rss_bytes(self):
        with open("/proc/%d/status" % self.proc.pid) as f:
            for line in f:
                if line.startswith("VmRSS:"):
                    return int(line.split()[1]) * 1024
        return 0

    def rss_peak(self):
        with open("/proc/%d/status" % self.proc.pid) as f:
            for line in f:
                if line.startswith("VmHWM:"):
                    return int(line.split()[1]) * 1024
        return 0

class ContainerServer(BaseServer):
    """Restarts the deployed docker container; cgroup-v2 CPU+mem accounting
    (identical to the method used for the published benchmark)."""

    def __init__(self):
        subprocess.run(["docker", "restart", CONTAINER], check=True,
                       stdout=subprocess.DEVNULL)
        cid = subprocess.check_output(
            ["docker", "inspect", "-f", "{{.Id}}", CONTAINER], text=True).strip()
        for b in ("/sys/fs/cgroup/system.slice/docker-%s.scope" % cid,
                  "/sys/fs/cgroup/docker/%s" % cid,
                  "/sys/fs/cgroup/unified/docker/%s" % cid):
            if os.path.isfile(os.path.join(b, "cpu.stat")):
                self.cg = b
                break
        else:
            sys.exit("cannot find cgroup v2 dir for container %s" % CONTAINER)
        self.base = "http://%s" % HOST
        self.wait_ready()
        time.sleep(1.0)  # let index prefetch settle

    def stop(self):
        pass  # leave the deployed container running

    def cpu_seconds(self):
        with open(os.path.join(self.cg, "cpu.stat")) as f:
            for line in f:
                if line.startswith("usage_usec"):
                    return int(line.split()[1]) / 1e6
        return 0.0

    def rss_bytes(self):
        with open(os.path.join(self.cg, "memory.current")) as f:
            return int(f.read())

    def rss_peak(self):
        try:
            with open(os.path.join(self.cg, "memory.peak")) as f:
                return int(f.read())
        except Exception:
            return 0


def _server_solve(self, filepath):
        rec = {}
        t0 = time.monotonic()
        up = post_multipart(self.base + "/api/upload",
                            {"request-json": json.dumps({"session": self.session})},
                            filepath=filepath)
        t_up = time.monotonic()
        if up.get("status") != "success":
            return {"status": "upload_error", "error": repr(up)}
        subid = up["subid"]
        rec["wall_upload"] = t_up - t0
        jobid, t_job = None, None
        while time.monotonic() - t0 < DEADLINE:
            try:
                s = get_json("%s/api/submissions/%s" % (self.base, subid))
                jobs = [j for j in (s.get("jobs") or []) if j]
                if jobs:
                    jobid, t_job = jobs[0], time.monotonic()
                    break
            except Exception:
                pass
            time.sleep(POLL)
        if jobid is None:
            rec.update(status="timeout_nojob", wall_total=time.monotonic() - t0)
            return rec
        rec["wall_queue"] = t_job - t_up
        status = "solving"
        while time.monotonic() - t0 < DEADLINE:
            try:
                j = get_json("%s/api/jobs/%s" % (self.base, jobid))
                status = j.get("status", "solving")
                if status in ("success", "failure"):
                    break
            except Exception:
                pass
            time.sleep(POLL)
        t_end = time.monotonic()
        rec["status"] = status if status in ("success", "failure") else "timeout"
        rec["wall_solve"] = t_end - t_job
        rec["wall_total"] = t_end - t0
        if status == "success":
            cal = get_json("%s/api/jobs/%s/calibration" % (self.base, jobid))
            rec["calib"] = {k: cal.get(k) for k in
                            ("ra", "dec", "radius", "pixscale", "orientation", "parity")}
        return rec


BaseServer.solve = _server_solve


def run_case(server, img_dir, job, label=""):
    path = os.path.join(img_dir, job["file"])
    cpu0 = server.cpu_seconds()
    peak = [server.rss_bytes()]
    stop = threading.Event()

    def sample():
        while not stop.is_set():
            try:
                peak[0] = max(peak[0], server.rss_bytes())
            except Exception:
                pass
            stop.wait(0.1)

    t = threading.Thread(target=sample, daemon=True)
    t.start()
    rec = server.solve(path)
    stop.set()
    t.join(timeout=2)
    rec["cpu_sec"] = round(server.cpu_seconds() - cpu0, 3)
    rec["mem_peak_during"] = peak[0]
    rec.update({k: job[k] for k in ("id", "role", "cls", "fov", "ra", "dec", "file")})
    if label:
        rec["label"] = label
    if rec.get("status") == "success" and rec.get("calib", {}).get("ra") is not None:
        rec["err_center_deg"] = round(
            ang_sep_deg(job["ra"], job["dec"], rec["calib"]["ra"], rec["calib"]["dec"]), 5)
    return rec


def report_line(rec, expect=None, notes=""):
    err = ("err=%5.1f\"" % (rec["err_center_deg"] * 3600)) if "err_center_deg" in rec else "          "
    print("  %-14s %-8s wall %6.2fs  cpu %6.2fs  %s %s" % (
        rec.get("id", "?"), rec.get("status", "?"),
        rec.get("wall_total", -1), rec.get("cpu_sec", -1), err, notes), flush=True)


OUT_DIR = os.path.join(ROOT, "bench-out")


def write_csv(name, recs):
    """One row per solve, in execution order — for plotting (e.g. wall time
    per image file). Returns the path."""
    os.makedirs(OUT_DIR, exist_ok=True)
    path = os.path.join(OUT_DIR, name)

    def num(v, scale=1.0):
        return "" if v is None else "%.6g" % (v * scale)

    cols = ["seq", "label", "id", "file", "cls", "fov_deg", "role", "status",
            "wall_total_s", "wall_upload_s", "wall_queue_s", "wall_solve_s",
            "cpu_s", "mem_peak_bytes", "err_center_arcsec",
            "req_ra_deg", "req_dec_deg", "calib_ra_deg", "calib_dec_deg",
            "calib_pixscale_arcsec_px"]
    with open(path, "w") as f:
        f.write(",".join(cols) + "\n")
        for i, r in enumerate(recs):
            calib = r.get("calib") or {}
            row = [str(i), r.get("label", ""), r.get("id", ""), r.get("file", ""),
                   r.get("cls", ""), num(r.get("fov")), r.get("role", ""),
                   r.get("status", ""),
                   num(r.get("wall_total")), num(r.get("wall_upload")),
                   num(r.get("wall_queue")), num(r.get("wall_solve")),
                   num(r.get("cpu_sec")), str(r.get("mem_peak_during", "")),
                   num(r.get("err_center_deg"), 3600.0),
                   num(r.get("ra")), num(r.get("dec")),
                   num(calib.get("ra")), num(calib.get("dec")),
                   num(calib.get("pixscale"))]
            f.write(",".join(row) + "\n")
    return path


# ------------------------------------------------------------------ suites
def check_calibration(rec, job):
    """Sanity checks: right part of the sky, right pixel scale."""
    problems = []
    if rec.get("status") != "success":
        problems.append("did not solve (status=%s)" % rec.get("status"))
        return problems
    if rec["err_center_deg"] > 0.05 * job["fov"]:
        problems.append("center off by %.3f deg (max %.3f)" %
                        (rec["err_center_deg"], 0.05 * job["fov"]))
    expected_ps = job["fov"] * 3600.0 / 1024.0
    ps = rec["calib"]["pixscale"]
    if not (0.95 * expected_ps <= ps <= 1.05 * expected_ps):
        problems.append("pixscale %.2f outside 5%% of %.2f" % (ps, expected_ps))
    return problems


def suite_regression(manifest, img_dir):
    failures = []
    walls = []
    all_recs = []
    print("regression suite: 9 blind solves + nearby warm-start sequence")
    for cls in ("s", "m", "l"):
        jobs = [j for j in manifest if j["suite"] == "regression" and j["cls"] == cls]
        print("-- class %s (%.0f deg FOV), fresh server" % (cls, jobs[0]["fov"]))
        srv = Server()
        try:
            for job in jobs:
                rec = run_case(srv, img_dir, job)
                all_recs.append(rec)
                problems = check_calibration(rec, job)
                report_line(rec, notes="FAIL: " + "; ".join(problems) if problems else "ok")
                if problems:
                    failures.append((job["id"], problems))
                else:
                    walls.append(rec["wall_total"])
        finally:
            srv.stop()

    print("-- nearby warm-start (medium class), fresh server")
    srv = Server()
    try:
        setup = next(j for j in manifest if j["suite"] == "nearby_setup")
        rec = run_case(srv, img_dir, setup)
        all_recs.append(rec)
        problems = check_calibration(rec, setup)
        report_line(rec, notes="(setup) " + ("FAIL: " + "; ".join(problems) if problems else "ok"))
        if problems:
            failures.append((setup["id"], problems))
        for job in [j for j in manifest if j["suite"] == "nearby"]:
            rec = run_case(srv, img_dir, job)
            all_recs.append(rec)
            problems = check_calibration(rec, job)
            report_line(rec, notes="FAIL: " + "; ".join(problems) if problems else "ok")
            if problems:
                failures.append((job["id"], problems))
            else:
                walls.append(rec["wall_total"])
    finally:
        srv.stop()

    print()
    print("per-case values: %s" % write_csv("regression.csv", all_recs))
    if walls:
        print("performance: %d solves, median %.2fs, mean %.2fs, max %.2fs" %
              (len(walls), st.median(walls), st.mean(walls), max(walls)))
    if failures:
        print("RESULT: FAIL (%d case(s)):" % len(failures))
        for fid, probs in failures:
            print("  %s: %s" % (fid, "; ".join(probs)))
        return 1
    print("RESULT: PASS (%d/%d cases)" % (13 - len(failures), 13))
    return 0


def suite_known_failing(manifest, img_dir):
    jobs = [j for j in manifest if j["suite"] == "known_fail"]
    print("known-failing suite: %d dense-field cases faint_light cannot solve yet" % len(jobs))
    print("(server solve timeout: %ss — set FLT_TIMEOUT to iterate faster)" % SOLVE_TIMEOUT)
    fixed = []
    all_recs = []
    for job in jobs:
        srv = Server()  # fresh server per case: no warm state, isolated cost
        try:
            rec = run_case(srv, img_dir, job)
        finally:
            srv.stop()
        all_recs.append(rec)
        if rec.get("status") == "success":
            fixed.append(job["id"])
            report_line(rec, notes="*** NOW SOLVES (was a known failure) ***")
        else:
            report_line(rec, notes="still failing, as expected")
    print()
    print("per-case values: %s" % write_csv("known_failing.csv", all_recs))
    if fixed:
        print("RESULT: %d formerly-failing case(s) now solve: %s" % (len(fixed), ", ".join(fixed)))
        print("If intentional, move them out of the known_fail suite in testdata/bench/manifest.json")
    else:
        print("RESULT: all %d cases still failing (no regression baseline change)" % len(jobs))
    return 0


def suite_full(manifest, img_dir, limit):
    classes = []
    for j in manifest:
        if j["cls"] not in classes:
            classes.append(j["cls"])
    outdir = OUT_DIR
    os.makedirs(outdir, exist_ok=True)

    results, meta = [], {"server": "fl-local", "mode": "random",
                         "poll_interval": POLL, "deadline": DEADLINE,
                         "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    for cls in classes:
        jobs = [j for j in manifest if j["cls"] == cls and j["role"] == "random"]
        if limit:
            jobs = jobs[:limit]
        print("== class %s: %d images, fresh server" % (cls, len(jobs)), flush=True)
        srv = Server()
        try:
            meta.setdefault("baseline_mem", {})[cls] = srv.rss_bytes()
            for i, job in enumerate(jobs):
                rec = run_case(srv, img_dir, job, label="%s[%d]" % (cls, i))
                report_line(rec)
                results.append(rec)
        finally:
            srv.stop()
    meta["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    with open(os.path.join(outdir, "fl_random.json"), "w") as f:
        json.dump({"meta": meta, "results": results}, f, indent=1)
    write_csv("fl_random.csv", results)

    results, meta = [], {"server": "fl-local", "mode": "scenario",
                         "poll_interval": POLL, "deadline": DEADLINE,
                         "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())}
    for cls in classes:
        print("== scenario class %s, fresh server" % cls, flush=True)
        srv = Server()
        try:
            meta.setdefault("baseline_mem", {})[cls] = srv.rss_bytes()
            by_role = {j["role"]: j for j in manifest if j["cls"] == cls}
            seq = [("cold", by_role["base"])]
            seq += [("repeat%d" % k, by_role["base"]) for k in range(1, 6)]
            seq += [("near%d" % k, by_role["near%d" % k]) for k in range(1, 5)]
            seq += [("distant", by_role["distant"]), ("distant_repeat", by_role["distant"])]
            for label, job in seq:
                rec = run_case(srv, img_dir, job, label="%s_%s" % (cls, label))
                report_line(rec)
                results.append(rec)
        finally:
            srv.stop()
    meta["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    with open(os.path.join(outdir, "fl_scenario.json"), "w") as f:
        json.dump({"meta": meta, "results": results}, f, indent=1)
    write_csv("fl_scenario.csv", results)

    print("\n=== summary (published-benchmark tables, faint_light rows) ===\n", flush=True)
    sys.stdout.flush()
    subprocess.run([sys.executable,
                    os.path.join(ROOT, "scripts/benchmark/analyze.py"), outdir])
    print("\nraw results: %s/fl_random.{json,csv}, %s/fl_scenario.{json,csv}" % (outdir, outdir))
    print("CSV columns are per-solve, in execution order — plot e.g. wall_total_s")
    print("against file/seq directly.")
    print("note: wall/CPU/RSS depend on this machine; the reference numbers in")
    print("the published benchmark were measured on the dedicated benchmark host.")
    return 0


def main():
    ap = argparse.ArgumentParser(description=__doc__,
                                 formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("suite", choices=["regression", "known-failing", "full"])
    ap.add_argument("--limit", type=int, default=0,
                    help="full: cap random images per class (smoke test)")
    args = ap.parse_args()

    missing = [i for i in range(4107, 4120)
               if not os.path.exists(os.path.join(INDEX_DIR, "index-%d.fits" % i))]
    if missing:
        sys.exit("missing index files in %s: %s\nrun `make fetch-indexes` first"
                 % (INDEX_DIR, ", ".join("index-%d" % i for i in missing)))

    if args.suite == "full":
        img_dir = os.path.join(ROOT, "testdata/bench-full")
        mpath = os.path.join(img_dir, "manifest.json")
        if not os.path.exists(mpath):
            sys.exit("testdata/bench-full/ not populated; run `make fetch-bench-images`")
    else:
        img_dir = os.path.join(ROOT, "testdata/bench")
        mpath = os.path.join(img_dir, "manifest.json")
    with open(mpath) as f:
        manifest = json.load(f)

    if args.suite == "regression":
        sys.exit(suite_regression(manifest, img_dir))
    elif args.suite == "known-failing":
        sys.exit(suite_known_failing(manifest, img_dir))
    else:
        sys.exit(suite_full(manifest, img_dir, args.limit))


if __name__ == "__main__":
    main()
