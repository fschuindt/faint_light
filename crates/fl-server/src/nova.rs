//! The nova.astrometry.net-compatible API, served under `/nova`.
//!
//! Contract notes (verified against NINA source and nova docs):
//! - NINA: POST /api/login/ (urlencoded `request-json`), POST /api/upload
//!   (multipart `request-json` + `file`), GET /api/submissions/{id},
//!   GET /api/jobs/{id}, GET /api/jobs/{id}/calibration/.
//! - test_submission.sh / client.py additionally send login as multipart and
//!   hit no-trailing-slash paths — both forms are accepted everywhere.
//! - Authentication is intentionally absent: any (or no) apikey succeeds.
//!
//! Clients point at `http://host:port/nova`; everything below is relative to
//! that. The server's own API lives at the root instead.

use std::sync::Arc;

use axum::extract::{Multipart, Path, Request, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, RequestExt, Router};
use serde_json::{json, Value};

use fl_solve::{Parity, SolveHints};

use crate::form::Form;
use crate::solve::{calibration_json, enqueue, err_json};
use crate::state::AppState;
use crate::store::{format_time, JobStatus};

pub fn router() -> Router<AppState> {
    let mut r = Router::new();
    // Register each route with and without a trailing slash.
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
    r = both(r, "/wcs_file/{id}", get(wcs_file));
    r = both(r, "/new_fits_file/{id}", get(not_available));
    r = both(r, "/annotated_display/{id}", get(not_available));
    r.route("/", get(index))
}

async fn index() -> &'static str {
    "faint_light: nova.astrometry.net-compatible API. Point your client here.\n"
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
async fn upload(State(state): State<AppState>, multipart: Multipart) -> Response {
    let mut form = match Form::read(multipart).await {
        Ok(f) => f,
        Err(r) => return r,
    };
    let hints = request_json_hints(&form);
    let (filename, bytes) = match form.take_file() {
        Ok(f) => f,
        Err(r) => return r,
    };

    let mut sha = sha1_smol::Sha1::new();
    sha.update(&bytes);
    let hash = sha.digest().to_string();

    let (sub_id, job_id) = match enqueue(&state, filename, Arc::new(bytes), hints) {
        Ok(ids) => ids,
        Err(r) => return r,
    };
    tracing::info!(sub = sub_id, job = job_id, "upload accepted");
    Json(json!({"status": "success", "subid": sub_id, "hash": hash})).into_response()
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
    let buf = fl_fits::write::header_only(&sol.wcs.fits_cards(sol.width, sol.height));
    ([(header::CONTENT_TYPE, "application/fits")], buf).into_response()
}

// ---------------------------------------------------------------------------

/// Solve hints from the `request-json` field, or the defaults when it is
/// absent or unparseable (NINA sends no hints at all).
fn request_json_hints(form: &Form) -> SolveHints {
    form.field("request-json")
        .and_then(|t| serde_json::from_str::<Value>(t).ok())
        .map(|v| parse_hints(&v))
        .unwrap_or_default()
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
