#!/usr/bin/env python3
"""Benchmark a nova.astrometry.net-compatible plate-solving server.

Measures, per solve:
  - wall time end-to-end through the HTTP API (upload -> terminal status)
  - upload time, queue time (upload done -> job created), solve time
  - container CPU seconds consumed (cgroup cpu.stat delta)
  - container memory: value before solve, peak sampled during solve
  - calibration returned, and its angular error vs the requested center

Modes:
  random   - the 256 random-region images, grouped by FOV class,
             container restarted before each class
  scenario - per FOV class: restart, solve base, repeat base x5,
             nearby chain near1..near4, then distant slew

Usage:
  bench.py --name fl --host localhost:8100 --container faint_light \
           --manifest ~/bench/images/manifest.json --mode random \
           --out ~/bench/results/fl_random.json
"""
import argparse
import io
import json
import mimetypes
import os
import subprocess
import threading
import time
import urllib.parse
import urllib.request
import uuid
import math
import sys

APIKEY = "1jcrmadfnxngxscd"
POLL = 0.1
DEADLINE = 420.0  # per-solve wall clock cap, seconds


# ---------------------------------------------------------------- cgroups
class CGroup:
    """Read CPU usage and memory of a docker container from host cgroups."""

    def __init__(self, container):
        self.container = container
        self.refresh()

    def refresh(self):
        cid = subprocess.check_output(
            ["docker", "inspect", "-f", "{{.Id}}", self.container],
            text=True).strip()
        base_candidates = [
            "/sys/fs/cgroup/system.slice/docker-%s.scope" % cid,
            "/sys/fs/cgroup/docker/%s" % cid,
            "/sys/fs/cgroup/unified/docker/%s" % cid,
        ]
        self.v2 = None
        for b in base_candidates:
            if os.path.isfile(os.path.join(b, "cpu.stat")):
                self.v2 = b
                return
        # cgroup v1 fallback
        self.cpu1 = "/sys/fs/cgroup/cpu,cpuacct/docker/%s/cpuacct.usage" % cid
        self.mem1 = "/sys/fs/cgroup/memory/docker/%s/memory.usage_in_bytes" % cid
        if not os.path.isfile(self.cpu1):
            raise RuntimeError("cannot find cgroup for container %s" % cid)

    def _read(self, path):
        with open(path) as f:
            return f.read()

    def cpu_seconds(self):
        if self.v2:
            for line in self._read(os.path.join(self.v2, "cpu.stat")).splitlines():
                if line.startswith("usage_usec"):
                    return int(line.split()[1]) / 1e6
            return 0.0
        return int(self._read(self.cpu1)) / 1e9

    def mem_current(self):
        if self.v2:
            return int(self._read(os.path.join(self.v2, "memory.current")))
        return int(self._read(self.mem1))

    def mem_peak(self):
        try:
            if self.v2:
                return int(self._read(os.path.join(self.v2, "memory.peak")))
            return int(self._read(self.mem1.replace("usage_in_bytes",
                                                    "max_usage_in_bytes")))
        except Exception:
            return None


class MemSampler(threading.Thread):
    """Samples memory.current at 100 ms while a solve is in flight."""

    def __init__(self, cg):
        super().__init__(daemon=True)
        self.cg = cg
        self.peak = 0
        self.stop_flag = threading.Event()

    def run(self):
        while not self.stop_flag.is_set():
            try:
                v = self.cg.mem_current()
                if v > self.peak:
                    self.peak = v
            except Exception:
                pass
            self.stop_flag.wait(0.1)

    def stop(self):
        self.stop_flag.set()
        self.join(timeout=2)
        return self.peak


# ---------------------------------------------------------------- http api
def http_post_multipart(url, fields, filepath=None, timeout=120):
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
        "Content-Type": "multipart/form-data; boundary=%s" % boundary,
        "Content-Length": str(len(data)),
    })
    with urllib.request.urlopen(req, timeout=timeout) as r:
        return json.loads(r.read().decode())


def http_get_json(url, timeout=30):
    with urllib.request.urlopen(url, timeout=timeout) as r:
        return json.loads(r.read().decode())


class DirectClient:
    """Client for the synchronous /solve endpoint (no job system)."""

    def __init__(self, host):
        self.base = "http://%s" % host

    def login(self):
        with urllib.request.urlopen(self.base + "/", timeout=10) as r:
            r.read()

    def solve(self, filepath):
        rec = {}
        with open(filepath, "rb") as f:
            data = f.read()
        req = urllib.request.Request(self.base + "/solve", data=data, headers={
            "Content-Type": "application/octet-stream",
            "Content-Length": str(len(data)),
        })
        t0 = time.monotonic()
        try:
            with urllib.request.urlopen(req, timeout=DEADLINE) as r:
                resp = json.loads(r.read().decode())
        except Exception as e:
            rec["status"] = "error"
            rec["error"] = str(e)
            rec["wall_total"] = time.monotonic() - t0
            return rec
        rec["wall_total"] = time.monotonic() - t0
        rec["status"] = resp.get("status", "error")
        rec["wall_solve"] = resp.get("solve_wall")
        if "calib" in resp:
            rec["calib"] = resp["calib"]
        return rec


class NovaClient:
    def __init__(self, host):
        self.base = "http://%s" % host
        self.session = None

    def login(self):
        resp = http_post_multipart(self.base + "/api/login",
                                   {"request-json": json.dumps({"apikey": APIKEY})})
        if resp.get("status") != "success":
            raise RuntimeError("login failed: %r" % resp)
        self.session = resp["session"]

    def solve(self, filepath):
        """Returns a record dict; blocks until terminal status or deadline."""
        rec = {}
        t0 = time.monotonic()
        up = http_post_multipart(self.base + "/api/upload",
                                 {"request-json": json.dumps({"session": self.session})},
                                 filepath=filepath)
        t_up = time.monotonic()
        if up.get("status") != "success":
            # session may have expired (server restart) -> relogin once
            self.login()
            up = http_post_multipart(self.base + "/api/upload",
                                     {"request-json": json.dumps({"session": self.session})},
                                     filepath=filepath)
            t_up = time.monotonic()
            if up.get("status") != "success":
                rec["status"] = "upload_error"
                rec["error"] = repr(up)
                return rec
        subid = up["subid"]
        rec["subid"] = subid
        rec["wall_upload"] = t_up - t0

        jobid = None
        t_job = None
        while time.monotonic() - t0 < DEADLINE:
            try:
                sub = http_get_json("%s/api/submissions/%s" % (self.base, subid))
                jobs = [j for j in (sub.get("jobs") or []) if j]
                if jobs:
                    jobid = jobs[0]
                    t_job = time.monotonic()
                    break
            except Exception:
                pass
            time.sleep(POLL)
        if jobid is None:
            rec["status"] = "timeout_nojob"
            rec["wall_total"] = time.monotonic() - t0
            return rec
        rec["jobid"] = jobid
        rec["wall_queue"] = t_job - t_up

        status = "solving"
        while time.monotonic() - t0 < DEADLINE:
            try:
                j = http_get_json("%s/api/jobs/%s" % (self.base, jobid))
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
            try:
                cal = http_get_json("%s/api/jobs/%s/calibration" % (self.base, jobid))
                rec["calib"] = {k: cal.get(k) for k in
                                ("ra", "dec", "radius", "pixscale", "orientation", "parity")}
            except Exception as e:
                rec["calib_error"] = str(e)
        return rec


# ---------------------------------------------------------------- helpers
def ang_sep_deg(ra1, dec1, ra2, dec2):
    r1, d1, r2, d2 = map(math.radians, (ra1, dec1, ra2, dec2))
    c = (math.sin(d1) * math.sin(d2) +
         math.cos(d1) * math.cos(d2) * math.cos(r1 - r2))
    return math.degrees(math.acos(max(-1.0, min(1.0, c))))


def restart_container(container, host, api="nova"):
    print("  restarting %s ..." % container, flush=True)
    subprocess.run(["docker", "restart", container], check=True,
                   stdout=subprocess.DEVNULL)
    # wait for the API to come back
    c = DirectClient(host) if api == "direct" else NovaClient(host)
    t0 = time.monotonic()
    while time.monotonic() - t0 < 180:
        try:
            c.login()
            time.sleep(3)  # let job processors finish starting
            return
        except Exception:
            time.sleep(1)
    raise RuntimeError("server did not come back after restart")


def run_one(client, cg, img_dir, job, results, label=""):
    path = os.path.join(img_dir, job["file"])
    cpu0 = cg.cpu_seconds()
    mem0 = cg.mem_current()
    sampler = MemSampler(cg)
    sampler.start()
    rec = client.solve(path)
    peak = sampler.stop()
    rec["cpu_sec"] = round(cg.cpu_seconds() - cpu0, 3)
    rec["mem_before"] = mem0
    rec["mem_peak_during"] = max(peak, mem0)
    rec.update({k: job[k] for k in ("id", "role", "cls", "fov", "ra", "dec", "file")})
    if label:
        rec["label"] = label
    if rec.get("status") == "success" and rec.get("calib", {}).get("ra") is not None:
        rec["err_center_deg"] = round(
            ang_sep_deg(job["ra"], job["dec"],
                        rec["calib"]["ra"], rec["calib"]["dec"]), 5)
    results.append(rec)
    print("  %-14s %-8s %6.2fs cpu=%6.2fs %s" % (
        rec.get("label", rec["id"]), rec.get("status"),
        rec.get("wall_total", -1), rec.get("cpu_sec", -1),
        ("err=%.4fdeg" % rec["err_center_deg"]) if "err_center_deg" in rec else ""),
        flush=True)


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--name", required=True)
    ap.add_argument("--host", required=True)
    ap.add_argument("--container", required=True)
    ap.add_argument("--manifest", required=True)
    ap.add_argument("--mode", choices=["random", "scenario", "smoke"], required=True)
    ap.add_argument("--api", choices=["nova", "direct"], default="nova")
    ap.add_argument("--out", required=True)
    ap.add_argument("--limit", type=int, default=0,
                    help="random mode: cap images per class (0 = all)")
    args = ap.parse_args()

    with open(os.path.expanduser(args.manifest)) as f:
        manifest = json.load(f)
    img_dir = os.path.dirname(os.path.expanduser(args.manifest))

    classes = []
    for j in manifest:
        if j["cls"] not in classes:
            classes.append(j["cls"])

    results = []
    meta = {
        "server": args.name, "host": args.host, "container": args.container,
        "mode": args.mode, "poll_interval": POLL, "deadline": DEADLINE,
        "started_utc": time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime()),
    }

    cg = CGroup(args.container)
    client = DirectClient(args.host) if args.api == "direct" else NovaClient(args.host)
    meta["api"] = args.api

    if args.mode == "smoke":
        client.login()
        for cls in classes:
            job = next(j for j in manifest if j["cls"] == cls and j["role"] == "base")
            run_one(client, cg, img_dir, job, results, label="smoke_%s" % cls)
    elif args.mode == "random":
        for cls in classes:
            jobs = [j for j in manifest if j["cls"] == cls and j["role"] == "random"]
            if args.limit:
                jobs = jobs[:args.limit]
            print("== class %s: %d images" % (cls, len(jobs)), flush=True)
            restart_container(args.container, args.host, args.api)
            cg.refresh()
            client.login()
            meta.setdefault("baseline_mem", {})[cls] = cg.mem_current()
            for i, job in enumerate(jobs):
                run_one(client, cg, img_dir, job, results,
                        label="%s[%d]" % (cls, i))
    else:  # scenario
        for cls in classes:
            print("== scenario class %s" % cls, flush=True)
            restart_container(args.container, args.host, args.api)
            cg.refresh()
            client.login()
            meta.setdefault("baseline_mem", {})[cls] = cg.mem_current()
            by_role = {j["role"]: j for j in manifest if j["cls"] == cls}
            seq = [("cold", by_role["base"])]
            seq += [("repeat%d" % k, by_role["base"]) for k in range(1, 6)]
            seq += [("near%d" % k, by_role["near%d" % k]) for k in range(1, 5)]
            seq += [("distant", by_role["distant"])]
            # a second distant-region hit to see re-warming after a slew
            seq += [("distant_repeat", by_role["distant"])]
            for label, job in seq:
                run_one(client, cg, img_dir, job, results,
                        label="%s_%s" % (cls, label))

    meta["finished_utc"] = time.strftime("%Y-%m-%dT%H:%M:%SZ", time.gmtime())
    out = os.path.expanduser(args.out)
    os.makedirs(os.path.dirname(out), exist_ok=True)
    with open(out, "w") as f:
        json.dump({"meta": meta, "results": results}, f, indent=1)
    ok = sum(1 for r in results if r.get("status") == "success")
    print("wrote %s: %d/%d success" % (out, ok, len(results)), flush=True)


if __name__ == "__main__":
    main()
