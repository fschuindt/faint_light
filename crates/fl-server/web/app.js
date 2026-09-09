/* Faint Light web UI. Plain browser JavaScript, no build step: the page is
   served from the binary and talks to /api/v1 with fetch, the same calls a
   script would make. */
(function () {
  "use strict";

  var $ = function (id) { return document.getElementById(id); };
  var D2R = Math.PI / 180, R2D = 180 / Math.PI;

  /* ---- theme (same as the website) ---------------------------------- */

  (function () {
    var b = document.querySelector(".theme-toggle");
    if (!b) return;
    b.addEventListener("click", function () {
      var root = document.documentElement;
      var dark = root.getAttribute("data-theme") === "dark";
      if (dark) root.removeAttribute("data-theme"); else root.setAttribute("data-theme", "dark");
      try { localStorage.setItem("theme", dark ? "light" : "dark"); } catch (e) {}
      viewer.draw();
    });
  })();

  /* ---- TAN WCS, as the server fits it ------------------------------- */

  function radecToXyz(ra, dec) {
    var r = ra * D2R, d = dec * D2R;
    return [Math.cos(d) * Math.cos(r), Math.cos(d) * Math.sin(r), Math.sin(d)];
  }
  function xyzToRadec(v) {
    var ra = Math.atan2(v[1], v[0]) * R2D;
    ra = ((ra % 360) + 360) % 360;
    var dec = Math.asin(Math.max(-1, Math.min(1, v[2]))) * R2D;
    return [ra, dec];
  }
  function normalize(v) {
    var n = Math.sqrt(v[0] * v[0] + v[1] * v[1] + v[2] * v[2]);
    return [v[0] / n, v[1] / n, v[2] / n];
  }
  function eastNorth(r) {
    var en = Math.sqrt(r[0] * r[0] + r[1] * r[1]);
    var e = [-r[1] / en, r[0] / en, 0];
    var n = [-r[2] * e[1], r[2] * e[0], r[0] * e[1] - r[1] * e[0]];
    return [e, n];
  }
  /* Tangent plane (x east, y north, radians) around reference r -> xyz. */
  function tanUnproject(x, y, r) {
    if (Math.abs(r[2]) >= 1) {
      var sx = r[2] > 0 ? x : -x;
      return normalize([sx, y, r[2] > 0 ? 1 : -1]);
    }
    var en = eastNorth(r), e = en[0], n = en[1];
    return normalize([r[0] + x * e[0] + y * n[0], r[1] + x * e[1] + y * n[1], r[2] + x * e[2] + y * n[2]]);
  }
  /* xyz -> tangent plane around r, or null on the far hemisphere. */
  function tanProject(s, r) {
    var sdotr = s[0] * r[0] + s[1] * r[1] + s[2] * r[2];
    if (sdotr <= 0) return null;
    if (Math.abs(r[2]) >= 1) {
      var inv0 = 1 / s[2];
      return r[2] > 0 ? [s[0] * inv0, s[1] * inv0] : [-s[0] * inv0, s[1] * inv0];
    }
    var en = eastNorth(r), e = en[0], n = en[1];
    var inv = 1 / sdotr;
    return [(s[0] * e[0] + s[1] * e[1] + s[2] * e[2]) * inv, (s[0] * n[0] + s[1] * n[1] + s[2] * n[2]) * inv];
  }

  function Wcs(w) {
    this.crval = w.crval;
    // The API reports CRPIX 1-based (FITS); pixel centres are 0-based here.
    this.crpix = [w.crpix[0] - 1, w.crpix[1] - 1];
    this.cd = w.cd;
    this.ref = radecToXyz(w.crval[0], w.crval[1]);
    this.det = w.cd[0][0] * w.cd[1][1] - w.cd[0][1] * w.cd[1][0];
  }
  Wcs.prototype.pixToRadec = function (px, py) {
    var u = px - this.crpix[0], v = py - this.crpix[1], cd = this.cd;
    var x = (cd[0][0] * u + cd[0][1] * v) * D2R;
    var y = (cd[1][0] * u + cd[1][1] * v) * D2R;
    return xyzToRadec(tanUnproject(x, y, this.ref));
  };
  Wcs.prototype.radecToPix = function (ra, dec) {
    var p = tanProject(radecToXyz(ra, dec), this.ref);
    if (!p || this.det === 0) return null;
    var xd = p[0] * R2D, yd = p[1] * R2D, cd = this.cd, inv = 1 / this.det;
    var u = inv * (cd[1][1] * xd - cd[0][1] * yd);
    var v = inv * (-cd[1][0] * xd + cd[0][0] * yd);
    return [this.crpix[0] + u, this.crpix[1] + v];
  };

  /* ---- formatting --------------------------------------------------- */

  function pad(n, w) { var s = String(n); while (s.length < w) s = "0" + s; return s; }
  function fmtRa(deg, secDigits) {
    if (secDigits === undefined) secDigits = 1;
    var h = ((deg % 360) + 360) % 360 / 15;
    var hh = Math.floor(h), m = (h - hh) * 60, mm = Math.floor(m), s = (m - mm) * 60;
    var ss = s.toFixed(secDigits);
    if (parseFloat(ss) >= 60) { ss = (0).toFixed(secDigits); mm += 1; }
    if (mm >= 60) { mm = 0; hh = (hh + 1) % 24; }
    return pad(hh, 2) + "h " + pad(mm, 2) + "m " + (s < 10 && parseFloat(ss) < 10 ? "0" : "") + ss + "s";
  }
  function fmtDec(deg, secDigits) {
    if (secDigits === undefined) secDigits = 0;
    var sign = deg < 0 ? "−" : "+", a = Math.abs(deg);
    var dd = Math.floor(a), m = (a - dd) * 60, mm = Math.floor(m), s = (m - mm) * 60;
    var ss = s.toFixed(secDigits);
    if (parseFloat(ss) >= 60) { ss = (0).toFixed(secDigits); mm += 1; }
    if (mm >= 60) { mm = 0; dd += 1; }
    return sign + pad(dd, 2) + "° " + pad(mm, 2) + "′ " + (parseFloat(ss) < 10 ? "0" : "") + ss + "″";
  }
  function fmtDeg(x, d) { return x.toFixed(d === undefined ? 4 : d) + "°"; }
  function fmtAngle(deg) {
    // A field size or a grid step: whichever unit reads best.
    if (deg >= 1) return (Math.round(deg * 100) / 100) + "°";
    var m = deg * 60;
    if (m >= 1) return (Math.round(m * 10) / 10) + "′";
    return Math.round(m * 60) + "″";
  }
  function fmtMs(ms) { return ms < 1000 ? ms + " ms" : (ms / 1000).toFixed(ms < 10000 ? 2 : 1) + " s"; }
  function fmtBytes(b) {
    if (b < 1024) return b + " B";
    if (b < 1048576) return (b / 1024).toFixed(0) + " KB";
    return (b / 1048576).toFixed(1) + " MB";
  }
  function isoUtc(d) { return d.toISOString().replace(/\.\d{3}Z$/, "Z"); }
  function esc(s) {
    return String(s).replace(/[&<>"]/g, function (c) {
      return { "&": "&amp;", "<": "&lt;", ">": "&gt;", '"': "&quot;" }[c];
    });
  }

  /* ---- the upload --------------------------------------------------- */

  var file = null;
  var dropzone = $("dropzone"), fileInput = $("file"), dropFile = $("drop-file"), solveBtn = $("solve-btn");

  function setFile(f) {
    if (!f) return;
    file = f;
    dropFile.innerHTML = esc(f.name) + ' <span class="size">' + fmtBytes(f.size) + "</span>";
    dropFile.hidden = false;
    solveBtn.disabled = false;
    hideError();
    fitsDateObs(f).then(function (date) {
      var note = $("timestamp-note");
      if (date) {
        $("timestamp").value = date;
        note.textContent = "From the file's DATE-OBS.";
      } else if (f.lastModified) {
        $("timestamp").value = isoUtc(new Date(f.lastModified));
        note.textContent = "From the file's modification time; edit if the exposure was earlier.";
      }
    });
  }

  /* DATE-OBS from a FITS primary header, read in the browser: the first few
     2880-byte blocks are enough for any real header. */
  function fitsDateObs(f) {
    return f.slice(0, 2880 * 12).arrayBuffer().then(function (buf) {
      var bytes = new Uint8Array(buf);
      if (bytes.length < 80 || String.fromCharCode.apply(null, bytes.subarray(0, 9)) !== "SIMPLE  =") return null;
      var text = "";
      for (var i = 0; i < bytes.length; i++) text += String.fromCharCode(bytes[i]);
      var m = text.match(/DATE-OBS= *'([^']+)'/);
      if (!m) return null;
      var v = m[1].trim();
      // FITS: YYYY-MM-DD[THH:MM:SS[.fff]], UTC by convention.
      if (!/^\d{4}-\d{2}-\d{2}(T\d{2}:\d{2}:\d{2}(\.\d+)?)?$/.test(v)) return null;
      if (v.length === 10) v += "T00:00:00";
      var d = new Date(v + "Z");
      return isNaN(d.getTime()) ? null : isoUtc(d);
    }).catch(function () { return null; });
  }

  dropzone.addEventListener("click", function () { fileInput.click(); });
  dropzone.addEventListener("keydown", function (e) {
    if (e.key === "Enter" || e.key === " ") { e.preventDefault(); fileInput.click(); }
  });
  fileInput.addEventListener("change", function () { setFile(fileInput.files[0]); });
  ["dragenter", "dragover"].forEach(function (ev) {
    dropzone.addEventListener(ev, function (e) { e.preventDefault(); dropzone.classList.add("over"); });
  });
  ["dragleave", "drop"].forEach(function (ev) {
    dropzone.addEventListener(ev, function (e) { e.preventDefault(); dropzone.classList.remove("over"); });
  });
  dropzone.addEventListener("drop", function (e) {
    if (e.dataTransfer && e.dataTransfer.files.length) setFile(e.dataTransfer.files[0]);
  });
  // Dropping anywhere else must not navigate the tab to the image.
  window.addEventListener("dragover", function (e) { e.preventDefault(); });
  window.addEventListener("drop", function (e) { e.preventDefault(); });
  document.addEventListener("paste", function (e) {
    var files = e.clipboardData && e.clipboardData.files;
    if (files && files.length) setFile(files[0]);
  });

  /* ---- observer ----------------------------------------------------- */

  var optFits = $("opt-fits"), optSky = $("opt-skyview"), observer = $("observer");

  function loadObserver() {
    try {
      var o = JSON.parse(localStorage.getItem("fl.observer") || "{}");
      if (o.latitude !== undefined) $("latitude").value = o.latitude;
      if (o.longitude !== undefined) $("longitude").value = o.longitude;
      if (o.fits === false) optFits.checked = false;
      if (o.skyview === true) optSky.checked = true;
    } catch (e) {}
  }
  function saveObserver() {
    try {
      localStorage.setItem("fl.observer", JSON.stringify({
        latitude: $("latitude").value.trim(),
        longitude: $("longitude").value.trim(),
        fits: optFits.checked,
        skyview: optSky.checked
      }));
    } catch (e) {}
  }
  function syncOptions() { observer.disabled = !optSky.checked; observer.style.opacity = optSky.checked ? "" : ".5"; }
  optSky.addEventListener("change", function () { syncOptions(); saveObserver(); });
  optFits.addEventListener("change", saveObserver);
  $("latitude").addEventListener("change", saveObserver);
  $("longitude").addEventListener("change", saveObserver);
  loadObserver();
  syncOptions();
  if (!$("timestamp").value) $("timestamp").value = isoUtc(new Date());

  $("btn-now").addEventListener("click", function () {
    $("timestamp").value = isoUtc(new Date());
    $("timestamp-note").textContent = "";
  });
  $("btn-locate").addEventListener("click", function () {
    var btn = $("btn-locate");
    if (!navigator.geolocation) { showError("This browser offers no geolocation."); return; }
    btn.disabled = true;
    navigator.geolocation.getCurrentPosition(function (pos) {
      $("latitude").value = pos.coords.latitude.toFixed(4);
      $("longitude").value = pos.coords.longitude.toFixed(4);
      saveObserver();
      btn.disabled = false;
    }, function (err) {
      btn.disabled = false;
      showError("Could not read the position: " + err.message +
        (location.protocol === "http:" && location.hostname !== "localhost" && location.hostname !== "127.0.0.1"
          ? " Browsers only allow geolocation on localhost or https; type the coordinates instead." : ""));
    }, { timeout: 15000 });
  });

  /* ---- solving ------------------------------------------------------ */

  var errorBox = $("error"), statusEl = $("status");
  function showError(msg) { errorBox.innerHTML = "<strong>Error:</strong> " + esc(msg); errorBox.hidden = false; }
  function hideError() { errorBox.hidden = true; }

  function hintFields() {
    var out = [];
    ["scale_low", "scale_high", "center_ra", "center_dec", "radius", "downsample"].forEach(function (k) {
      var v = $(k).value.trim();
      if (v) out.push([k, v]);
    });
    if ($("parity").value !== "both") out.push(["parity", $("parity").value]);
    return out;
  }

  function requestFields() {
    var fields = [];
    if (optFits.checked) { fields.push(["fits", "1"]); fields.push(["preview", "1"]); }
    if (optSky.checked) {
      fields.push(["skyview", "1"]);
      fields.push(["timestamp", $("timestamp").value.trim()]);
      fields.push(["latitude", $("latitude").value.trim()]);
      fields.push(["longitude", $("longitude").value.trim()]);
    }
    return fields.concat(hintFields());
  }

  function curlFor(fields) {
    var s = "curl -X POST " + location.origin + "/api/v1/solve \\\n  -F file=@" + JSON.stringify(file.name);
    fields.forEach(function (kv) { s += " \\\n  -F " + kv[0] + "=" + kv[1]; });
    return s;
  }

  var timer = null;
  function startTimer() {
    var t0 = Date.now();
    statusEl.className = "status busy";
    statusEl.textContent = "Solving…";
    timer = setInterval(function () {
      statusEl.textContent = "Solving… " + ((Date.now() - t0) / 1000).toFixed(1) + " s";
    }, 100);
  }
  function stopTimer(text) {
    clearInterval(timer);
    statusEl.className = "status";
    statusEl.textContent = text || "";
  }

  $("solve-form").addEventListener("submit", function (e) {
    e.preventDefault();
    if (!file) { showError("Choose an image first."); return; }
    hideError();
    if (optSky.checked) {
      var lat = parseFloat($("latitude").value), lon = parseFloat($("longitude").value);
      if (!$("timestamp").value.trim()) { showError("The sky view needs the exposure time."); return; }
      if (isNaN(lat) || isNaN(lon)) { showError("The sky view needs the site's latitude and longitude, or untick it."); return; }
    }
    var fields = requestFields();
    var fd = new FormData();
    fd.append("file", file, file.name);
    fields.forEach(function (kv) { fd.append(kv[0], kv[1]); });

    solveBtn.disabled = true;
    startTimer();
    var t0 = Date.now();
    fetch("/api/v1/solve", { method: "POST", body: fd }).then(function (res) {
      return res.text().then(function (text) {
        var body;
        try { body = JSON.parse(text); } catch (err) { body = { status: "error", error: text || (res.status + " " + res.statusText) }; }
        return { ok: res.ok, body: body };
      });
    }).then(function (r) {
      solveBtn.disabled = false;
      if (!r.ok || r.body.status !== "success") {
        stopTimer("No solution, after " + fmtMs(Date.now() - t0) + ".");
        showError(r.body.error || r.body.errormessage || "the server returned an error");
        $("result").hidden = true;
        return;
      }
      stopTimer("Solved in " + fmtMs(r.body.solved_in_ms) + ".");
      render(r.body, fields);
    }).catch(function (err) {
      solveBtn.disabled = false;
      stopTimer();
      showError("Could not reach the server: " + err.message);
    });
  });

  /* ---- results ------------------------------------------------------ */

  var result = null;

  function stat(v, k) { return '<div class="stat"><div class="v">' + v + '</div><div class="k">' + k + "</div></div>"; }
  function row(k, v) { return "<tr><td>" + k + "</td><td>" + v + "</td></tr>"; }
  function head(t) { return '<tr><th colspan="2">' + t + "</th></tr>"; }
  function num(x, d) {
    if (typeof x !== "number") return esc(x);
    var s = x.toFixed(d === undefined ? 4 : d);
    return /^-0(\.0*)?$/.test(s) ? s.slice(1) : s;
  }
  /* The solver reports orientation in [0, 360); a rotation reads better
     as a signed angle, and 359.995 as -0.005. */
  function wrap180(deg) { var w = ((deg + 180) % 360 + 360) % 360 - 180; return Math.abs(w) < 5e-4 ? 0 : w; }

  function render(body, fields) {
    result = body;
    var c = body.calibration, w = body.image;
    var wdeg = c.width_arcsec / 3600, hdeg = c.height_arcsec / 3600;

    $("stats").innerHTML =
      stat(fmtRa(c.ra, 0), "centre RA") +
      stat(fmtDec(c.dec), "centre Dec") +
      stat(num(c.pixscale, c.pixscale < 1 ? 3 : 2) + "<small>″/px</small>", "pixel scale") +
      stat(fmtAngle(wdeg) + " × " + fmtAngle(hdeg), "field") +
      stat(num(wrap180(c.orientation), 1) + "°", "rotation, E of N") +
      stat(fmtMs(body.solved_in_ms), "solve time");

    var html = head("Solution") +
      row("Job", body.job) +
      row("File", esc(file.name)) +
      row("Image", w.width + " × " + w.height + " px") +
      row("Centre", fmtDeg(c.ra) + ", " + fmtDeg(c.dec) + " (J2000)") +
      row("Field", num(wdeg, 3) + "° × " + num(hdeg, 3) + "°, radius " + num(c.radius, 3) + "°") +
      row("Pixel scale", num(c.pixscale, 4) + " ″/px") +
      row("Orientation", num(wrap180(c.orientation), 3) + "° east of north (" + num(c.orientation, 3) + "°)") +
      row("Parity", c.parity > 0 ? "positive (+1)" : "negative (−1)") +
      head("Match") +
      row("Index", body.match.index_id) +
      row("Stars matched", body.match.nmatch) +
      row("Log-odds", num(body.match.logodds, 1)) +
      head("WCS (TAN)") +
      row("CRVAL", num(body.wcs.crval[0], 6) + ", " + num(body.wcs.crval[1], 6)) +
      row("CRPIX", num(body.wcs.crpix[0], 2) + ", " + num(body.wcs.crpix[1], 2)) +
      row("CD", "[" + body.wcs.cd[0].map(function (x) { return x.toExponential(4); }).join(", ") + "]<br>[" +
        body.wcs.cd[1].map(function (x) { return x.toExponential(4); }).join(", ") + "]");
    if (body.skyview) {
      var s = body.skyview;
      html += head("Sky view") +
        row("Altitude", num(s.alt, 2) + "°" + (s.above_horizon ? "" : " (below the horizon)")) +
        row("Azimuth", num(s.az, 2) + "°") +
        row("Time", esc(s.observer.timestamp_utc) + " UTC") +
        row("Site", num(s.observer.latitude, 4) + "°, " + num(s.observer.longitude, 4) + "°") +
        row("Local sidereal time", fmtRa(s.observer.lst_deg, 0));
    }
    $("details").innerHTML = html;
    $("raw").textContent = JSON.stringify(body, null, 2);
    $("curl").textContent = curlFor(fields);

    // The image, with the coordinate readout.
    var imagePanel = $("image-panel");
    if (body.fits_url || body.preview_url) {
      imagePanel.hidden = false;
      var dl = $("fits-dl");
      if (body.fits_url) {
        dl.hidden = false;
        dl.href = body.fits_url;
        dl.download = body.fits_filename || "solved.fits";
        dl.querySelector("span").textContent = "Download " + (body.fits_filename || "solved FITS");
      } else {
        dl.hidden = true;
      }
      if (body.preview_url) {
        viewer.load(body);
      } else {
        viewer.clear();
      }
    } else {
      imagePanel.hidden = true;
      viewer.clear();
    }

    // The sky chart, inlined so it scales with the column.
    var skyPanel = $("sky-panel");
    if (body.skyview) {
      skyPanel.hidden = false;
      $("sky-dl").href = body.skyview.chart_url;
      $("sky-dl").download = (file.name.replace(/\.[^.]+$/, "") || "image") + ".skyview.svg";
      $("sky-note").textContent = "Centre at altitude " + body.skyview.alt.toFixed(1) + "°, azimuth " +
        body.skyview.az.toFixed(1) + "°" + (body.skyview.above_horizon ? "." : " — below the horizon; check the time and site.");
      $("sky-chart").innerHTML = "";
      fetch(body.skyview.chart_url).then(function (r) { return r.text(); }).then(function (svg) {
        $("sky-chart").innerHTML = svg;
      }).catch(function () {
        $("sky-chart").innerHTML = '<img src="' + esc(body.skyview.chart_url) + '" alt="Sky chart">';
      });
    } else {
      skyPanel.hidden = true;
    }

    $("result").hidden = false;
    $("result").scrollIntoView({ behavior: "smooth", block: "start" });
  }

  /* ---- the image viewer --------------------------------------------- */

  var viewer = (function () {
    var box = $("viewer"), img = $("preview"), canvas = $("overlay"), readout = $("readout");
    var wcs = null, meta = null, hover = null, pin = null, grid = $("opt-grid");

    function cssVar(name) { return getComputedStyle(document.documentElement).getPropertyValue(name).trim(); }

    /* Displayed position (CSS px within the image) <-> solver pixel
       coordinates (0-based pixel centres, like the WCS). */
    function toPixel(x, y) {
      var f = meta.preview.factor;
      return [x / img.clientWidth * meta.preview.width * f - 0.5, y / img.clientHeight * meta.preview.height * f - 0.5];
    }
    function toScreen(px, py) {
      var f = meta.preview.factor;
      return [(px + 0.5) / (meta.preview.width * f) * img.clientWidth, (py + 0.5) / (meta.preview.height * f) * img.clientHeight];
    }

    function load(body) {
      wcs = new Wcs(body.wcs);
      meta = body;
      hover = null; pin = null;
      $("btn-copy").hidden = true;
      readout.classList.remove("pinned");
      img.onload = draw;
      img.src = body.preview_url;
      showPoint(null);
    }
    function clear() { wcs = null; meta = null; img.removeAttribute("src"); }

    function showPoint(p) {
      if (!p) {
        ["ro-ra", "ro-dec", "ro-deg", "ro-px"].forEach(function (id) { $(id).textContent = "—"; });
        return;
      }
      var rd = wcs.pixToRadec(p[0], p[1]);
      $("ro-ra").textContent = fmtRa(rd[0], 2);
      $("ro-dec").textContent = fmtDec(rd[1], 1);
      $("ro-deg").textContent = rd[0].toFixed(5) + ", " + rd[1].toFixed(5);
      $("ro-px").textContent = Math.floor(p[0] + 0.5) + 1 + ", " + (Math.floor(p[1] + 0.5) + 1);
      return rd;
    }

    /* Grid steps: the largest "nice" spacing that still gives a few lines
       across the field. RA steps are in time units, as on any chart. */
    var DEC_STEPS = [45, 30, 15, 10, 5, 2, 1, 0.5, 1 / 3, 1 / 6, 1 / 12, 1 / 20, 1 / 30, 1 / 60, 1 / 120, 1 / 300];
    var RA_STEPS = [90, 45, 30, 15, 7.5, 5, 2.5, 1.25, 0.5, 0.25, 0.125, 0.0625, 0.025, 0.0125, 0.00625, 0.0025];
    function pickStep(steps, span, min) {
      for (var i = 0; i < steps.length; i++) if (span / steps[i] >= min) return steps[i];
      return steps[steps.length - 1];
    }

    function drawGrid(ctx, W, H) {
      var f = meta.preview.factor, iw = meta.preview.width * f, ih = meta.preview.height * f;
      // Sky extent from the frame's corners and edge midpoints.
      var samples = [], nx = 4, ny = 4;
      for (var i = 0; i <= nx; i++) for (var j = 0; j <= ny; j++) {
        samples.push(wcs.pixToRadec(i / nx * iw - 0.5, j / ny * ih - 0.5));
      }
      var centre = wcs.pixToRadec(iw / 2 - 0.5, ih / 2 - 0.5);
      var decMin = 90, decMax = -90, dRaMin = 0, dRaMax = 0;
      samples.forEach(function (rd) {
        decMin = Math.min(decMin, rd[1]); decMax = Math.max(decMax, rd[1]);
        var d = ((rd[0] - centre[0] + 540) % 360) - 180;
        dRaMin = Math.min(dRaMin, d); dRaMax = Math.max(dRaMax, d);
      });
      var poleIn = [90, -90].some(function (dec) {
        var p = wcs.radecToPix(0, dec);
        return p && p[0] >= -0.5 && p[0] <= iw - 0.5 && p[1] >= -0.5 && p[1] <= ih - 0.5;
      });
      if (poleIn) {
        if (wcs.radecToPix(0, 90)) decMax = 90; else decMin = -90;
        dRaMin = -180; dRaMax = 180;
      }
      var decSpan = decMax - decMin, raSpan = dRaMax - dRaMin;
      var decStep = pickStep(DEC_STEPS, decSpan, 3);
      var cosd = Math.max(Math.cos(Math.max(Math.abs(decMin), Math.abs(decMax)) * D2R), 0.02);
      var raStep = poleIn ? 15 : pickStep(RA_STEPS, raSpan * cosd, 3);

      ctx.save();
      ctx.lineWidth = 1;
      ctx.strokeStyle = "rgba(127, 155, 196, 0.55)";
      ctx.fillStyle = "rgba(199, 214, 239, 0.9)";
      ctx.font = "11px " + cssVar("--sans");
      ctx.shadowColor = "rgba(0,0,0,0.9)";
      ctx.shadowBlur = 3;

      function polyline(points, label) {
        var open = false, first = null;
        ctx.beginPath();
        points.forEach(function (rd) {
          var p = rd && wcs.radecToPix(rd[0], rd[1]);
          if (!p) { open = false; return; }
          var s = toScreen(p[0], p[1]);
          if (!open) { ctx.moveTo(s[0], s[1]); open = true; } else { ctx.lineTo(s[0], s[1]); }
          if (!first && s[0] >= 0 && s[0] <= W && s[1] >= 0 && s[1] <= H) first = s;
        });
        ctx.stroke();
        if (first && label) ctx.fillText(label, Math.min(first[0] + 4, W - 60), Math.max(first[1] - 4, 12));
      }
      // Lines of constant Dec.
      var N = 120;
      var d0 = Math.ceil((decMin - decStep) / decStep) * decStep;
      for (var dec = d0; dec <= decMax + decStep; dec += decStep) {
        if (dec <= -90 || dec >= 90) continue;
        var pts = [];
        for (var k = 0; k <= N; k++) pts.push([centre[0] + dRaMin - raStep + (raSpan + 2 * raStep) * k / N, dec]);
        polyline(pts, fmtDec(dec, 0).replace(/ 00″$/, "").replace(/ 00′$/, ""));
      }
      // Lines of constant RA.
      var lo = Math.ceil((centre[0] + dRaMin - raStep) / raStep) * raStep;
      var hi = centre[0] + dRaMax + raStep;
      for (var ra = lo; ra <= hi; ra += raStep) {
        var pts2 = [];
        var a = Math.max(decMin - decStep, -89.999), b = Math.min(decMax + decStep, 89.999);
        for (var k2 = 0; k2 <= N; k2++) pts2.push([ra, a + (b - a) * k2 / N]);
        polyline(pts2, fmtRa(ra, 0).replace(/ 00s$/, ""));
      }
      ctx.restore();
    }

    function draw() {
      if (!wcs || !img.clientWidth) return;
      var dpr = window.devicePixelRatio || 1, W = img.clientWidth, H = img.clientHeight;
      canvas.width = Math.round(W * dpr); canvas.height = Math.round(H * dpr);
      var ctx = canvas.getContext("2d");
      ctx.setTransform(dpr, 0, 0, dpr, 0, 0);
      ctx.clearRect(0, 0, W, H);
      if (grid.checked) drawGrid(ctx, W, H);

      var accent = cssVar("--link") || "#7f112b";
      if (pin) {
        var s = toScreen(pin[0], pin[1]);
        ctx.save();
        ctx.strokeStyle = accent; ctx.lineWidth = 1.5;
        ctx.beginPath(); ctx.arc(s[0], s[1], 7, 0, 2 * Math.PI); ctx.stroke();
        ctx.beginPath();
        ctx.moveTo(s[0] - 14, s[1]); ctx.lineTo(s[0] - 7, s[1]); ctx.moveTo(s[0] + 7, s[1]); ctx.lineTo(s[0] + 14, s[1]);
        ctx.moveTo(s[0], s[1] - 14); ctx.lineTo(s[0], s[1] - 7); ctx.moveTo(s[0], s[1] + 7); ctx.lineTo(s[0], s[1] + 14);
        ctx.stroke();
        ctx.restore();
      }
      if (hover) {
        var h = toScreen(hover[0], hover[1]);
        ctx.save();
        ctx.strokeStyle = "rgba(255,255,255,0.45)"; ctx.lineWidth = 1;
        ctx.beginPath();
        ctx.moveTo(0, h[1] + 0.5); ctx.lineTo(W, h[1] + 0.5);
        ctx.moveTo(h[0] + 0.5, 0); ctx.lineTo(h[0] + 0.5, H);
        ctx.stroke();
        ctx.restore();
      }
    }

    function eventPixel(e) {
      var r = img.getBoundingClientRect();
      var x = Math.min(Math.max(e.clientX - r.left, 0), r.width), y = Math.min(Math.max(e.clientY - r.top, 0), r.height);
      return toPixel(x, y);
    }
    box.addEventListener("pointermove", function (e) {
      if (!wcs) return;
      hover = eventPixel(e);
      if (!pin) showPoint(hover);
      draw();
    });
    box.addEventListener("pointerleave", function () {
      hover = null;
      if (!pin) showPoint(null);
      draw();
    });
    box.addEventListener("click", function (e) {
      if (!wcs) return;
      var p = eventPixel(e);
      if (pin && Math.abs(p[0] - pin[0]) < 6 * meta.preview.factor && Math.abs(p[1] - pin[1]) < 6 * meta.preview.factor) {
        pin = null;
      } else {
        pin = p;
      }
      readout.classList.toggle("pinned", !!pin);
      $("btn-copy").hidden = !pin;
      showPoint(pin || hover);
      draw();
    });
    grid.addEventListener("change", draw);
    window.addEventListener("resize", draw);
    if (window.ResizeObserver) new ResizeObserver(draw).observe(img);

    $("btn-copy").addEventListener("click", function () {
      if (!pin) return;
      var rd = wcs.pixToRadec(pin[0], pin[1]);
      var text = "RA " + fmtRa(rd[0], 2) + "  Dec " + fmtDec(rd[1], 1) + "  (" + rd[0].toFixed(5) + ", " + rd[1].toFixed(5) + ")";
      var btn = $("btn-copy");
      var done = function () { btn.textContent = "Copied"; setTimeout(function () { btn.textContent = "Copy pinned coordinates"; }, 1200); };
      if (navigator.clipboard && navigator.clipboard.writeText) navigator.clipboard.writeText(text).then(done, done);
      else done();
    });

    return { load: load, clear: clear, draw: draw };
  })();
})();
