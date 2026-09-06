#!/usr/bin/env python3
"""Synchronous plate-solving endpoint for twirl (lgrcia/twirl).

twirl is NOT a blind solver: it needs an approximate centre and field of
view, and it fetches its reference stars from Gaia over the network. So the
endpoint takes those as query parameters and reports the phases separately:

  t_detect  star detection in the image (twirl.find_peaks)
  t_gaia    Gaia catalogue query           <- network, cacheable
  t_match   asterism matching + WCS fit    (twirl.compute_wcs)

POST /solve?ra=<deg>&dec=<deg>&fov=<deg>[&nstars=12][&cache=1][&limit=1000]

`limit` caps how many Gaia sources the TAP query returns. twirl's default of
10000 makes the archive time out (HTTP 408) for the 7-14 deg cones our medium
and large fields need, and only the brightest `nstars` are matched anyway.
body = raw FITS bytes
"""
import io
import json
import time
import urllib.parse
from http.server import BaseHTTPRequestHandler, HTTPServer

import numpy as np
from astropy import units as u
from astropy.coordinates import SkyCoord
from astropy.io import fits
from astropy.wcs import WCS
import twirl

PORT = 8000
_gaia_cache = {}


def solve(data, ra, dec, fov_deg, nstars, use_cache, limit):
    center = SkyCoord(ra, dec, unit=["deg", "deg"])
    fov = fov_deg * u.deg

    t0 = time.monotonic()
    pixel_coords = twirl.find_peaks(data)[0:nstars]
    t1 = time.monotonic()

    key = (round(ra, 4), round(dec, 4), round(fov_deg, 5), nstars, limit)
    if use_cache and key in _gaia_cache:
        sky_coords = _gaia_cache[key]
        cached = True
    else:
        sky_coords = twirl.gaia_radecs(center, 1.2 * fov, limit=limit)[0:nstars]
        _gaia_cache[key] = sky_coords
        cached = False
    t2 = time.monotonic()

    wcs = twirl.compute_wcs(pixel_coords, sky_coords)
    t3 = time.monotonic()

    h, w = data.shape
    sky = wcs.pixel_to_world(w / 2.0, h / 2.0)
    scale = float(np.abs(wcs.pixel_scale_matrix[0, 0]) * 3600.0)
    return {
        "status": "success",
        "calib": {"ra": float(sky.ra.deg), "dec": float(sky.dec.deg), "pixscale": scale},
        "t_detect": t1 - t0,
        "t_gaia": t2 - t1,
        "t_match": t3 - t2,
        "solve_wall": t3 - t0,
        "solve_wall_no_gaia": (t1 - t0) + (t3 - t2),
        "gaia_cached": cached,
        "gaia_limit": limit,
        "n_stars": int(len(pixel_coords)),
    }


class Handler(BaseHTTPRequestHandler):
    protocol_version = "HTTP/1.1"

    def log_message(self, *a):
        pass

    def _reply(self, code, obj):
        b = json.dumps(obj).encode()
        self.send_response(code)
        self.send_header("Content-Type", "application/json")
        self.send_header("Content-Length", str(len(b)))
        self.end_headers()
        self.wfile.write(b)

    def do_GET(self):
        self._reply(200, {"status": "ok"})

    def do_POST(self):
        parsed = urllib.parse.urlparse(self.path)
        if parsed.path.rstrip("/") != "/solve":
            self._reply(404, {"status": "error", "error": "POST /solve"})
            return
        q = urllib.parse.parse_qs(parsed.query)
        try:
            ra = float(q["ra"][0])
            dec = float(q["dec"][0])
            fov = float(q["fov"][0])
        except Exception:
            self._reply(400, {"status": "error",
                              "error": "twirl needs ra, dec and fov (degrees)"})
            return
        nstars = int(q.get("nstars", ["12"])[0])
        use_cache = q.get("cache", ["0"])[0] == "1"
        limit = int(q.get("limit", ["1000"])[0])
        body = self.rfile.read(int(self.headers.get("Content-Length", 0)))
        t0 = time.monotonic()
        try:
            hdul = fits.open(io.BytesIO(body))
            data = hdul[0].data
            if data is None:
                for h in hdul:
                    if getattr(h, "data", None) is not None and h.data.ndim == 2:
                        data = h.data
                        break
            data = np.asarray(data, dtype=float)
            res = solve(data, ra, dec, fov, nstars, use_cache, limit)
            res["total_wall"] = time.monotonic() - t0
            self._reply(200, res)
        except Exception as e:
            self._reply(200, {"status": "failure", "error": "%s: %s" % (type(e).__name__, e),
                              "total_wall": time.monotonic() - t0})


if __name__ == "__main__":
    print("twirl solver listening on :%d" % PORT, flush=True)
    HTTPServer(("0.0.0.0", PORT), Handler).serve_forever()
