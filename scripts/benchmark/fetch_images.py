#!/usr/bin/env python3
"""Download benchmark images from CDS hips2fits (DSS2 red survey).

Generates:
  - 256 random-region images: 4 FOV classes x 64 images
  - per-class warm-start scenario sets: base, near1..near4 (overlapping
    chain of pointings), distant (large slew)

Writes images to IMG_DIR and a manifest.json describing every image.
Deterministic (seeded RNG) so the set is reproducible.
"""
import json
import math
import os
import random
import sys
import time
import urllib.parse
import urllib.request

IMG_DIR = os.path.expanduser(os.environ.get("BENCH_IMG_DIR", "~/bench/images"))
MANIFEST = os.path.join(IMG_DIR, "manifest.json")
HIPS = "CDS/P/DSS2/red"
SIZE = 1024
BASE_URL = "https://alasky.cds.unistra.fr/hips-image-services/hips2fits"

CLASSES = [
    # (key, fov_deg, n_random)
    ("s", 1.0, 64),    # small  ~3.5"/px
    ("m", 4.0, 64),    # medium ~14"/px
    ("l", 10.0, 64),   # large  ~35"/px
    ("xl", 20.0, 64),  # x-large ~70"/px
]

# Fixed base pointings for the warm-start scenarios (one per class).
SCENARIO_BASE = {
    "s": (132.825, 11.81),   # near M44 region
    "m": (83.82, -5.39),     # Orion
    "l": (201.0, -11.0),     # Virgo/Corvus border
    "xl": (310.0, 45.0),     # Cygnus
}

rng = random.Random(20260727)


def random_pointing():
    ra = rng.uniform(0.0, 360.0)
    # uniform on the sphere, keep away from poles
    while True:
        dec = math.degrees(math.asin(rng.uniform(-1.0, 1.0)))
        if -80.0 <= dec <= 80.0:
            return ra, dec


def fetch(ra, dec, fov, path, tries=5):
    params = {
        "hips": HIPS,
        "width": SIZE,
        "height": SIZE,
        "fov": fov,
        "projection": "TAN",
        "coordsys": "icrs",
        "ra": round(ra, 5),
        "dec": round(dec, 5),
        "format": "jpg",
    }
    url = BASE_URL + "?" + urllib.parse.urlencode(params)
    for attempt in range(tries):
        try:
            req = urllib.request.Request(url, headers={"User-Agent": "faint_light-benchmark/1.0"})
            with urllib.request.urlopen(req, timeout=120) as r:
                data = r.read()
            if len(data) < 5000:
                raise IOError("suspiciously small response (%d bytes)" % len(data))
            with open(path, "wb") as f:
                f.write(data)
            return True
        except Exception as e:
            print("  retry %d for %s: %s" % (attempt + 1, os.path.basename(path), e), flush=True)
            time.sleep(3.0 * (attempt + 1))
    return False


def main():
    os.makedirs(IMG_DIR, exist_ok=True)
    manifest = []
    jobs = []

    for key, fov, n in CLASSES:
        for i in range(n):
            ra, dec = random_pointing()
            jobs.append({
                "id": "rand_%s_%03d" % (key, i),
                "role": "random", "cls": key, "fov": fov,
                "ra": round(ra, 5), "dec": round(dec, 5),
            })

    for key, fov, _ in CLASSES:
        ra0, dec0 = SCENARIO_BASE[key]
        jobs.append({"id": "base_%s" % key, "role": "base", "cls": key,
                     "fov": fov, "ra": ra0, "dec": dec0})
        # chain of overlapping pointings, each ~0.35 FOV from the previous
        for j in range(1, 5):
            step = 0.35 * fov * j
            ra = ra0 + step / max(math.cos(math.radians(dec0)), 0.2)
            dec = dec0 + 0.1 * fov * j
            jobs.append({"id": "near%d_%s" % (j, key), "role": "near%d" % j,
                         "cls": key, "fov": fov,
                         "ra": round(ra % 360.0, 5), "dec": round(dec, 5)})
        # distant region: big slew
        jobs.append({"id": "dist_%s" % key, "role": "distant", "cls": key,
                     "fov": fov,
                     "ra": round((ra0 + 141.0) % 360.0, 5),
                     "dec": round(-dec0 * 0.8, 5)})

    total = len(jobs)
    print("fetching %d images to %s" % (total, IMG_DIR), flush=True)
    failed = []
    for n, job in enumerate(jobs):
        fname = job["id"] + ".jpg"
        path = os.path.join(IMG_DIR, fname)
        job["file"] = fname
        if os.path.exists(path) and os.path.getsize(path) > 5000:
            manifest.append(job)
            continue
        ok = fetch(job["ra"], job["dec"], job["fov"], path)
        if ok:
            manifest.append(job)
        else:
            failed.append(job["id"])
            print("FAILED: %s" % job["id"], flush=True)
        if (n + 1) % 10 == 0:
            print("  %d/%d done" % (n + 1, total), flush=True)
        time.sleep(0.25)

    with open(MANIFEST, "w") as f:
        json.dump(manifest, f, indent=1)
    print("done: %d ok, %d failed" % (len(manifest), len(failed)), flush=True)
    if failed:
        print("failed ids: %s" % ", ".join(failed), flush=True)
        sys.exit(1)


if __name__ == "__main__":
    main()
