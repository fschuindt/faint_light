//! The nova.astrometry.net-compatible HTTP API.
//!
//! Contract notes (verified against NINA source and nova docs):
//! - NINA: POST /api/login/ (urlencoded `request-json`), POST /api/upload
//!   (multipart `request-json` + `file`), GET /api/submissions/{id},
//!   GET /api/jobs/{id}, GET /api/jobs/{id}/calibration/.
//! - test_submission.sh / client.py additionally send login as multipart and
//!   hit no-trailing-slash paths — both forms are accepted everywhere.
//! - Authentication is intentionally absent: any (or no) apikey succeeds.

use std::sync::mpsc::SyncSender;
use std::sync::Arc;
use std::time::Duration;

use axum::extract::{DefaultBodyLimit, Multipart, Path, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, RequestExt, Router};
use serde_json::{json, Value};

use fl_solve::{Parity, SolveHints};

use crate::store::{format_time, JobStatus, Store};
use crate::worker::Task;

#[derive(Clone)]
pub struct AppState {
    pub store: Arc<Store>,
    pub tx: SyncSender<Task>,
    /// Per-job solve budget; the synchronous skyview endpoint waits this
    /// long (plus queueing grace) before giving up.
    pub solve_timeout: Duration,
}

pub fn router(state: AppState) -> Router {
    let mut r = Router::new();
    // Register each API route with and without trailing slash.
    let both = |mut r: Router<AppState>, path: &str, m: axum::routing::MethodRouter<AppState>| {
        r = r.route(path, m.clone());
        r.route(&format!("{path}/"), m)
    };
    r = both(r, "/api/login", post(login));
    r = both(r, "/api/upload", post(upload));
    r = both(r, "/api/url_upload", post(url_upload));
    r = both(r, "/api/submissions/{id}", get(submission_status));
    r = both(r, "/api/jobs/{id}", get(job_status));
    r = both(r, "/api/jobs/{id}/calibration", get(job_calibration));
    r = both(r, "/api/jobs/{id}/info", get(job_info));
    r = both(r, "/api/jobs/{id}/objects_in_field", get(objects_in_field));
    r = both(r, "/api/jobs/{id}/machine_tags", get(machine_tags));
    r = both(r, "/api/jobs/{id}/annotations", get(annotations));
    r = both(r, "/api/jobs/{id}/tags", get(tags));
    r = both(r, "/api/skyview", post(skyview));
    r = both(r, "/wcs_file/{id}", get(wcs_file));
    r = r.route("/skyview/{id}/chart.svg", get(skyview_chart));
    r = both(r, "/new_fits_file/{id}", get(not_available));
    r = both(r, "/annotated_display/{id}", get(not_available));
    r.route("/", get(index))
        .layer(DefaultBodyLimit::max(256 << 20))
        .with_state(state)
}

async fn index() -> &'static str {
    concat!(
        "faint_light ",
        env!("CARGO_PKG_VERSION"),
        " — nova.astrometry.net-compatible plate solving server\n"
    )
}

/// POST /api/login — accepts urlencoded or multipart `request-json`;
/// any or no apikey authenticates.
async fn login(req: Request) -> Json<Value> {
    let session = format!("{:032x}", fastrand_session());
    // Best-effort parse purely for a friendly message; failures still log in.
    let _ = read_request_json(req).await;
    Json(json!({
        "status": "success",
        "message": "authenticated user: anonymous",
        "authenticated": true,
        "session": session,
    }))
}

/// POST /api/upload — multipart with `request-json` and `file`.
async fn upload(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    let mut hints = SolveHints::default();
    let mut file: Option<(String, Vec<u8>)> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "request-json" => {
                if let Ok(text) = field.text().await {
                    if let Ok(v) = serde_json::from_str::<Value>(&text) {
                        hints = parse_hints(&v);
                    }
                }
            }
            "file" => {
                let name = field.file_name().unwrap_or("upload").to_string();
                match field.bytes().await {
                    Ok(b) => file = Some((name, b.to_vec())),
                    Err(e) => {
                        return err_json(StatusCode::BAD_REQUEST, &format!("file read: {e}"))
                    }
                }
            }
            _ => {}
        }
    }
    let Some((filename, bytes)) = file else {
        return err_json(StatusCode::BAD_REQUEST, "no file field in upload");
    };
    if bytes.is_empty() {
        return err_json(StatusCode::BAD_REQUEST, "empty upload");
    }

    let mut sha = sha1_smol::Sha1::new();
    sha.update(&bytes);
    let hash = sha.digest().to_string();

    let (sub_id, job_id) = state.store.create(filename);
    if state
        .tx
        .try_send(Task {
            job: job_id,
            bytes,
            hints,
        })
        .is_err()
    {
        state
            .store
            .finish(job_id, JobStatus::Failure("solve queue full".into()));
        return err_json(StatusCode::SERVICE_UNAVAILABLE, "solve queue full");
    }
    tracing::info!(sub = sub_id, job = job_id, "upload accepted");
    Json(json!({"status": "success", "subid": sub_id, "hash": hash})).into_response()
}

/// POST /api/skyview — synchronous plate solve plus an all-sky chart.
///
/// Multipart fields:
/// - `file` (required): the image, same formats as /api/upload
/// - `timestamp` (required): exposure time, Unix seconds or
///   `YYYY-MM-DDTHH:MM:SS[.fff]Z` UTC
/// - `latitude` / `longitude` (required): observing site, degrees
///   (north- and east-positive)
/// - `request-json` (optional): same solve hints as /api/upload
///
/// Blocks until the solve finishes, then responds with the calibration,
/// the field's alt/az placement, and `sky_chart_url` — an SVG all-sky view
/// (zenith-centered) with constellation figures, an alt/az grid, and the
/// image footprint outlined in red.
async fn skyview(State(state): State<AppState>, mut multipart: Multipart) -> Response {
    let mut hints = SolveHints::default();
    let mut file: Option<(String, Vec<u8>)> = None;
    let mut timestamp: Option<String> = None;
    let mut latitude: Option<String> = None;
    let mut longitude: Option<String> = None;
    while let Ok(Some(field)) = multipart.next_field().await {
        match field.name().unwrap_or("") {
            "request-json" => {
                if let Ok(text) = field.text().await {
                    if let Ok(v) = serde_json::from_str::<Value>(&text) {
                        hints = parse_hints(&v);
                    }
                }
            }
            "file" => {
                let name = field.file_name().unwrap_or("upload").to_string();
                match field.bytes().await {
                    Ok(b) => file = Some((name, b.to_vec())),
                    Err(e) => {
                        return err_json(StatusCode::BAD_REQUEST, &format!("file read: {e}"))
                    }
                }
            }
            "timestamp" => timestamp = field.text().await.ok(),
            "latitude" | "lat" => latitude = field.text().await.ok(),
            "longitude" | "lon" => longitude = field.text().await.ok(),
            _ => {}
        }
    }
    let Some((filename, bytes)) = file else {
        return err_json(StatusCode::BAD_REQUEST, "no file field in upload");
    };
    if bytes.is_empty() {
        return err_json(StatusCode::BAD_REQUEST, "empty upload");
    }
    let Some(unix) = timestamp
        .as_deref()
        .and_then(fl_sky::altaz::parse_timestamp)
    else {
        return err_json(
            StatusCode::BAD_REQUEST,
            "timestamp field required (Unix seconds or YYYY-MM-DDTHH:MM:SSZ UTC)",
        );
    };
    let (Some(Ok(lat)), Some(Ok(lon))) = (
        latitude.as_deref().map(str::parse::<f64>),
        longitude.as_deref().map(str::parse::<f64>),
    ) else {
        return err_json(
            StatusCode::BAD_REQUEST,
            "latitude and longitude fields required (degrees)",
        );
    };
    if !(-90.0..=90.0).contains(&lat) || !(-180.0..=360.0).contains(&lon) {
        return err_json(StatusCode::BAD_REQUEST, "latitude/longitude out of range");
    }

    let (sub_id, job_id) = state.store.create(filename);
    if state
        .tx
        .try_send(Task {
            job: job_id,
            bytes,
            hints,
        })
        .is_err()
    {
        state
            .store
            .finish(job_id, JobStatus::Failure("solve queue full".into()));
        return err_json(StatusCode::SERVICE_UNAVAILABLE, "solve queue full");
    }
    tracing::info!(sub = sub_id, job = job_id, lat, lon, unix, "skyview accepted");

    // Wait for the worker (extra grace on top of the per-job solve budget
    // covers time spent queued behind other jobs).
    let deadline = tokio::time::Instant::now() + state.solve_timeout + Duration::from_secs(30);
    let sol = loop {
        match state.store.job(job_id).map(|j| j.status) {
            Some(JobStatus::Success(sol)) => break sol,
            Some(JobStatus::Failure(msg)) => {
                return (
                    StatusCode::UNPROCESSABLE_ENTITY,
                    Json(json!({"status": "failure", "job": job_id, "errormessage": msg})),
                )
                    .into_response();
            }
            Some(JobStatus::Solving) => {
                if tokio::time::Instant::now() > deadline {
                    return err_json(StatusCode::GATEWAY_TIMEOUT, "solve timed out");
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            None => return err_json(StatusCode::INTERNAL_SERVER_ERROR, "job vanished"),
        }
    };

    // Image footprint on the sky, in border order.
    let (w, h) = (sol.width, sol.height);
    let corners = [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)]
        .map(|(x, y)| sol.wcs.pixel_to_radec(x, y));
    let c = &sol.calibration;
    let lst = fl_sky::altaz::lst_deg(unix, lon);
    let (alt, az) = fl_sky::altaz::radec_to_altaz(c.ra, c.dec, lat, lst);
    let width_deg = w * c.pixscale / 3600.0;
    let height_deg = h * c.pixscale / 3600.0;

    let svg = fl_sky::chart::render_svg(&fl_sky::chart::ChartParams {
        unix,
        lat_deg: lat,
        lon_deg: lon,
        fov: Some(fl_sky::chart::Fov {
            corners,
            label: format!("{width_deg:.2}\u{b0} \u{d7} {height_deg:.2}\u{b0}"),
        }),
    });
    state.store.put_chart(job_id, svg);

    if alt <= 0.0 {
        tracing::warn!(
            job = job_id,
            alt,
            "solved field is below the horizon — check timestamp/coordinates"
        );
    }
    Json(json!({
        "status": "success",
        "subid": sub_id,
        "job": job_id,
        "calibration": calibration_json(&sol),
        "observer": {
            "timestamp_utc": fl_sky::altaz::format_utc(unix),
            "unix": unix,
            "latitude": lat,
            "longitude": lon,
            "lst_deg": lst,
        },
        "field": {
            "alt": alt,
            "az": az,
            "above_horizon": alt > 0.0,
            "width_deg": width_deg,
            "height_deg": height_deg,
            "orientation": c.orientation,
            "parity": c.parity,
            "corners_radec": corners.iter().map(|&(r, d)| json!([r, d])).collect::<Vec<_>>(),
        },
        "sky_chart_url": format!("/skyview/{job_id}/chart.svg"),
    }))
    .into_response()
}

/// GET /skyview/{id}/chart.svg — the chart rendered by POST /api/skyview.
async fn skyview_chart(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    match state.store.chart(id) {
        Some(svg) => (
            [(header::CONTENT_TYPE, "image/svg+xml; charset=utf-8")],
            svg.as_str().to_owned(),
        )
            .into_response(),
        None => err_json(StatusCode::NOT_FOUND, "no sky chart for this job"),
    }
}

async fn url_upload() -> Response {
    err_json(
        StatusCode::NOT_IMPLEMENTED,
        "url_upload is not supported by faint_light",
    )
}

async fn submission_status(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    let Some(sub) = state.store.submission(id) else {
        return err_json(StatusCode::NOT_FOUND, "no such submission");
    };
    let job = state.store.job(sub.job);
    let (finished, calibrations) = match &job {
        Some(j) => (
            j.finished.map(format_time),
            match &j.status {
                JobStatus::Success(_) => json!([[sub.job, sub.job]]),
                _ => json!([]),
            },
        ),
        None => (None, json!([])),
    };
    Json(json!({
        "processing_started": format_time(sub.received),
        "processing_finished": finished,
        "user": 1,
        "user_images": [sub.job],
        "images": [sub.job],
        "jobs": [sub.job],
        "job_calibrations": calibrations,
    }))
    .into_response()
}

fn job_status_str(status: &JobStatus) -> &'static str {
    match status {
        JobStatus::Solving => "solving",
        JobStatus::Success(_) => "success",
        JobStatus::Failure(_) => "failure",
    }
}

async fn job_status(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    let Some(job) = state.store.job(id) else {
        return err_json(StatusCode::NOT_FOUND, "no such job");
    };
    Json(json!({"status": job_status_str(&job.status)})).into_response()
}

fn calibration_json(sol: &fl_solve::Solution) -> Value {
    let c = &sol.calibration;
    json!({
        "ra": c.ra,
        "dec": c.dec,
        "width_arcsec": sol.width * c.pixscale,
        "height_arcsec": sol.height * c.pixscale,
        "radius": c.radius,
        "pixscale": c.pixscale,
        "orientation": c.orientation,
        "parity": c.parity,
    })
}

async fn job_calibration(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    match state.store.job(id).map(|j| j.status) {
        Some(JobStatus::Success(sol)) => Json(calibration_json(&sol)).into_response(),
        Some(_) => err_json(StatusCode::NOT_FOUND, "job is not calibrated"),
        None => err_json(StatusCode::NOT_FOUND, "no such job"),
    }
}

async fn job_info(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    let Some(job) = state.store.job(id) else {
        return err_json(StatusCode::NOT_FOUND, "no such job");
    };
    let mut body = json!({
        "status": job_status_str(&job.status),
        "original_filename": job.filename,
        "started": format_time(job.started),
        "objects_in_field": [],
        "machine_tags": [],
        "tags": [],
    });
    match &job.status {
        JobStatus::Success(sol) => body["calibration"] = calibration_json(sol),
        JobStatus::Failure(msg) => body["error_message"] = json!(msg),
        JobStatus::Solving => {}
    }
    Json(body).into_response()
}

async fn objects_in_field() -> Json<Value> {
    Json(json!({"objects_in_field": []}))
}
async fn machine_tags() -> Json<Value> {
    Json(json!({"tags": []}))
}
async fn annotations() -> Json<Value> {
    Json(json!({"annotations": []}))
}
async fn tags() -> Json<Value> {
    Json(json!({"tags": []}))
}

async fn not_available() -> Response {
    err_json(StatusCode::NOT_FOUND, "not available")
}

/// Minimal single-HDU FITS file carrying the TAN WCS solution
/// (what solve-field writes as *.wcs).
async fn wcs_file(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    let Some(JobStatus::Success(sol)) = state.store.job(id).map(|j| j.status) else {
        return err_json(StatusCode::NOT_FOUND, "no WCS for this job");
    };
    let w = &sol.wcs;
    let mut cards: Vec<String> = Vec::new();
    fn push_card(cards: &mut Vec<String>, k: &str, v: String, c: &str) {
        cards.push(format!("{k:<8}= {v:>20} / {c}"));
    }
    macro_rules! card {
        ($k:expr, $v:expr, $c:expr) => {
            push_card(&mut cards, $k, $v, $c)
        };
    }
    cards.push(format!("{:<8}=                    T / faint_light WCS", "SIMPLE"));
    card!("BITPIX", "8".into(), "8-bit");
    card!("NAXIS", "0".into(), "no data");
    card!("EXTEND", "T".into(), "");
    card!("WCSAXES", "2".into(), "");
    card!("EQUINOX", "2000.0".into(), "");
    card!("LONPOLE", "180.0".into(), "");
    card!("LATPOLE", "0.0".into(), "");
    cards.push(format!("{:<8}= 'RA---TAN'           / TAN projection", "CTYPE1"));
    cards.push(format!("{:<8}= 'DEC--TAN'           / TAN projection", "CTYPE2"));
    cards.push(format!("{:<8}= 'deg     '           / degrees", "CUNIT1"));
    cards.push(format!("{:<8}= 'deg     '           / degrees", "CUNIT2"));
    card!("CRVAL1", format!("{:.12E}", w.crval[0]), "RA of reference point");
    card!("CRVAL2", format!("{:.12E}", w.crval[1]), "DEC of reference point");
    // FITS pixels are 1-based.
    card!("CRPIX1", format!("{:.12E}", w.crpix[0] + 1.0), "X reference pixel");
    card!("CRPIX2", format!("{:.12E}", w.crpix[1] + 1.0), "Y reference pixel");
    card!("CD1_1", format!("{:.12E}", w.cd[0][0]), "Transformation matrix");
    card!("CD1_2", format!("{:.12E}", w.cd[0][1]), "");
    card!("CD2_1", format!("{:.12E}", w.cd[1][0]), "");
    card!("CD2_2", format!("{:.12E}", w.cd[1][1]), "");
    card!("IMAGEW", format!("{:.1}", sol.width), "Image width in pixels");
    card!("IMAGEH", format!("{:.1}", sol.height), "Image height in pixels");
    cards.push("END".to_string());

    let mut buf = Vec::with_capacity(2880);
    for c in &cards {
        let mut b = c.clone().into_bytes();
        b.resize(80, b' ');
        buf.extend_from_slice(&b);
    }
    buf.resize(buf.len().div_ceil(2880) * 2880, b' ');
    (
        [(header::CONTENT_TYPE, "application/fits")],
        buf,
    )
        .into_response()
}

// ---------------------------------------------------------------------------

fn err_json(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({"status": "error", "errormessage": msg}))).into_response()
}

/// Extract the `request-json` payload from either an urlencoded body (NINA)
/// or a multipart form (curl -F / client.py).
async fn read_request_json(req: Request) -> Option<Value> {
    let content_type = req
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if content_type.starts_with("multipart/form-data") {
        let mut mp: Multipart = req.extract().await.ok()?;
        while let Ok(Some(field)) = mp.next_field().await {
            if field.name() == Some("request-json") {
                let text = field.text().await.ok()?;
                return serde_json::from_str(&text).ok();
            }
        }
        None
    } else {
        let body = axum::body::to_bytes(req.into_body(), 1 << 20).await.ok()?;
        let body = std::str::from_utf8(&body).ok()?;
        for pair in body.split('&') {
            let mut it = pair.splitn(2, '=');
            if it.next() == Some("request-json") {
                let raw = percent_decode(it.next().unwrap_or(""));
                return serde_json::from_str(&raw).ok();
            }
        }
        // Bare JSON body fallback.
        serde_json::from_str(body).ok()
    }
}

fn percent_decode(s: &str) -> String {
    let bytes = s.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'+' => out.push(b' '),
            b'%' if i + 2 < bytes.len() => {
                if let Ok(v) =
                    u8::from_str_radix(std::str::from_utf8(&bytes[i + 1..i + 3]).unwrap_or(""), 16)
                {
                    out.push(v);
                    i += 2;
                } else {
                    out.push(b'%');
                }
            }
            b => out.push(b),
        }
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn num(v: &Value, key: &str) -> Option<f64> {
    match v.get(key)? {
        Value::Number(n) => n.as_f64(),
        Value::String(s) => s.parse().ok(),
        _ => None,
    }
}

/// Translate a nova upload request-json into SolveHints.
fn parse_hints(v: &Value) -> SolveHints {
    let mut h = SolveHints::default();
    let units = v.get("scale_units").and_then(|u| u.as_str()).unwrap_or("");
    let (lo, hi) = match v.get("scale_type").and_then(|t| t.as_str()) {
        Some("ev") => {
            let est = num(v, "scale_est");
            let err = num(v, "scale_err").unwrap_or(20.0);
            (
                est.map(|e| e * (1.0 - err / 100.0)),
                est.map(|e| e * (1.0 + err / 100.0)),
            )
        }
        _ => (num(v, "scale_lower"), num(v, "scale_upper")),
    };
    if let (Some(lo), Some(hi)) = (lo, hi) {
        match units {
            "arcsecperpix" | "app" => {
                h.scale_lo = Some(lo);
                h.scale_hi = Some(hi);
            }
            "arcminwidth" => h.width_deg = Some((lo / 60.0, hi / 60.0)),
            "degwidth" | "" => h.width_deg = Some((lo, hi)),
            _ => {}
        }
    }
    h.center_ra = num(v, "center_ra");
    h.center_dec = num(v, "center_dec");
    h.radius = num(v, "radius");
    h.downsample = num(v, "downsample_factor").map(|d| d as usize);
    if let Some(p) = num(v, "parity") {
        h.parity = Parity::from_nova(p as i64);
    }
    h
}

/// Session tokens need no cryptographic strength (there is no auth);
/// hash the clock for uniqueness.
fn fastrand_session() -> u128 {
    use std::time::{SystemTime, UNIX_EPOCH};
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let x = now.as_nanos() ^ (std::process::id() as u128) << 64;
    // splitmix-ish scramble
    let mut z = x.wrapping_mul(0x9E3779B97F4A7C15_9E3779B97F4A7C15);
    z ^= z >> 61;
    z.wrapping_mul(0xBF58476D1CE4E5B9_BF58476D1CE4E5B9)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hint_parsing_arcsecperpix() {
        let v: Value = serde_json::from_str(
            r#"{"scale_units":"arcsecperpix","scale_type":"ul","scale_lower":1.5,"scale_upper":3.0,
                "center_ra":83.0,"center_dec":-5.0,"radius":10.0,"downsample_factor":2,"parity":2}"#,
        )
        .unwrap();
        let h = parse_hints(&v);
        assert_eq!(h.scale_lo, Some(1.5));
        assert_eq!(h.scale_hi, Some(3.0));
        assert_eq!(h.center_ra, Some(83.0));
        assert_eq!(h.radius, Some(10.0));
        assert_eq!(h.downsample, Some(2));
    }

    #[test]
    fn hint_parsing_ev_degwidth() {
        let v: Value = serde_json::from_str(
            r#"{"scale_units":"degwidth","scale_type":"ev","scale_est":2.0,"scale_err":10}"#,
        )
        .unwrap();
        let h = parse_hints(&v);
        assert_eq!(h.width_deg, Some((1.8, 2.2)));
        assert!(h.scale_lo.is_none());
    }

    #[test]
    fn percent_decoding() {
        assert_eq!(
            percent_decode("%7B%22apikey%22%3A+%22abc%22%7D"),
            r#"{"apikey": "abc"}"#
        );
    }
}
