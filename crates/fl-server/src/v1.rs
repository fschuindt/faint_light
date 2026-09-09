//! The Faint Light API, served at the root under `/api/v1`.
//!
//! One endpoint solves; two flags decide what else it produces. Both extras
//! are returned as URLs rather than inline, so a single response can offer
//! both, a browser can render the chart straight from an `<img>`, and a
//! caller that only wants the numbers pays nothing for the rest.

use std::sync::Arc;
use std::time::Instant;

use axum::extract::{Multipart, Path, State};
use axum::http::{header, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post};
use axum::{Json, Router};
use serde_json::{json, Value};

use fl_solve::{Parity, SolveHints};

use crate::form::Form;
use crate::solve::{await_job, calibration_json, derived_name, enqueue, store_fits, store_preview};
use crate::state::AppState;
use crate::store::Artifact;

pub fn router() -> Router<AppState> {
    Router::new()
        .route("/api/v1/solve", post(solve))
        .route("/api/v1/jobs/{id}/fits", get(fits))
        .route("/api/v1/jobs/{id}/skyview.svg", get(skyview_svg))
        .route("/api/v1/jobs/{id}/preview.png", get(preview_png))
}

/// POST /api/v1/solve — solve an image synchronously.
///
/// Multipart fields:
/// - `file` (required): FITS, JPEG, PNG or TIFF.
/// - `fits`: build the solved image as FITS and report `fits_url`.
/// - `skyview`: render the all-sky chart and report `skyview.chart_url`;
///   needs `timestamp`, `latitude` and `longitude`.
/// - `preview`: render the upload as a viewable PNG and report
///   `preview_url` (what the web UI draws its coordinate readout over).
/// - hints (all optional): `scale_low`/`scale_high` in arcsec/px,
///   `center_ra`/`center_dec`/`radius` in degrees, `downsample`, and
///   `parity` (`normal`, `flip` or `both`).
async fn solve(State(state): State<AppState>, multipart: Multipart) -> Response {
    let started = Instant::now();
    let mut form = match Form::read(multipart).await {
        Ok(f) => f,
        Err(r) => return r,
    };
    let want_fits = form.flag("fits");
    let want_skyview = form.flag("skyview");
    let want_preview = form.flag("preview");

    // Validate the observer before spending a solve on it.
    let observer = if want_skyview {
        match Observer::from_form(&form) {
            Ok(o) => Some(o),
            Err(msg) => return error(StatusCode::BAD_REQUEST, &msg),
        }
    } else {
        None
    };

    let hints = match hints_from(&form) {
        Ok(h) => h,
        Err(msg) => return error(StatusCode::BAD_REQUEST, &msg),
    };
    let (filename, bytes) = match form.take_file() {
        Ok(f) => f,
        Err(r) => return r,
    };
    // The worker and the FITS writer both need the upload, so share it.
    let bytes = Arc::new(bytes);
    let (_, job) = match enqueue(&state, filename.clone(), bytes.clone(), hints) {
        Ok(ids) => ids,
        Err(r) => return r,
    };
    tracing::info!(
        job,
        fits = want_fits,
        skyview = want_skyview,
        preview = want_preview,
        "solve accepted"
    );

    let sol = match await_job(&state, job, |job, msg| {
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"status": "error", "job": job, "error": msg})),
        )
            .into_response()
    })
    .await
    {
        Ok(s) => s,
        Err(r) => return r,
    };

    let mut body = json!({
        "status": "success",
        "job": job,
        "solved_in_ms": started.elapsed().as_millis() as u64,
        "image": {"width": sol.width, "height": sol.height},
        "calibration": calibration_json(&sol),
        "wcs": {
            "crval": sol.wcs.crval,
            // 1-based, as written to a FITS header.
            "crpix": [sol.wcs.crpix[0] + 1.0, sol.wcs.crpix[1] + 1.0],
            "cd": sol.wcs.cd,
        },
        "match": {
            "logodds": sol.logodds,
            "nmatch": sol.nmatch,
            "index_id": sol.index_id,
        },
    });

    if want_fits {
        match store_fits(&state, job, &filename, &bytes, &sol) {
            Ok(name) => {
                body["fits_url"] = json!(format!("/api/v1/jobs/{job}/fits"));
                body["fits_filename"] = json!(name);
            }
            Err(e) => {
                tracing::warn!(job, "fits output failed: {e}");
                return error(
                    StatusCode::UNPROCESSABLE_ENTITY,
                    &format!("could not write FITS output: {e}"),
                );
            }
        }
    }

    if let Some(obs) = observer {
        body["skyview"] = skyview(&state, job, &filename, &sol, &obs);
    }

    if want_preview {
        // A preview that fails is not worth failing a solve that succeeded;
        // the caller still has the numbers.
        match store_preview(&state, job, &filename, &bytes) {
            Ok((w, h, factor)) => {
                body["preview_url"] = json!(format!("/api/v1/jobs/{job}/preview.png"));
                body["preview"] = json!({"width": w, "height": h, "factor": factor});
            }
            Err(e) => tracing::warn!(job, "preview failed: {e}"),
        }
    }

    tracing::info!(
        job,
        ms = started.elapsed().as_millis() as u64,
        ra = sol.calibration.ra,
        dec = sol.calibration.dec,
        "solve answered"
    );
    Json(body).into_response()
}

/// GET /api/v1/jobs/{id}/fits
async fn fits(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    artifact_response(&state, id, "fits", "attachment")
}

/// GET /api/v1/jobs/{id}/skyview.svg
async fn skyview_svg(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    artifact_response(&state, id, "skyview", "attachment")
}

/// GET /api/v1/jobs/{id}/preview.png — inline, so an `<img>` can show it.
async fn preview_png(State(state): State<AppState>, Path(id): Path<u64>) -> Response {
    artifact_response(&state, id, "preview", "inline")
}

/// Serve a stored artifact, as a download or for display.
fn artifact_response(state: &AppState, job: u64, kind: &str, disposition: &str) -> Response {
    let Some(art) = state.store.artifact(job, kind) else {
        return error(
            StatusCode::NOT_FOUND,
            "no such file for this job (it may have been evicted; solve again)",
        );
    };
    (
        [
            (header::CONTENT_TYPE, art.content_type.to_string()),
            (
                header::CONTENT_DISPOSITION,
                format!("{disposition}; filename=\"{}\"", art.filename),
            ),
        ],
        art.bytes.as_ref().clone(),
    )
        .into_response()
}

/// Where and when the exposure was taken.
struct Observer {
    unix: f64,
    lat: f64,
    lon: f64,
}

impl Observer {
    fn from_form(form: &Form) -> Result<Observer, String> {
        let unix = form
            .field("timestamp")
            .and_then(fl_sky::altaz::parse_timestamp)
            .ok_or(
                "skyview needs a timestamp (Unix seconds or YYYY-MM-DDTHH:MM:SSZ UTC)".to_string(),
            )?;
        let (Some(lat), Some(lon)) = (form.number("latitude"), form.number("longitude")) else {
            return Err("skyview needs latitude and longitude in degrees".into());
        };
        if !(-90.0..=90.0).contains(&lat) || !(-180.0..=360.0).contains(&lon) {
            return Err("latitude/longitude out of range".into());
        }
        Ok(Observer { unix, lat, lon })
    }
}

/// Place the solved field in the observer's sky, render the chart, and
/// describe both.
fn skyview(
    state: &AppState,
    job: u64,
    filename: &str,
    sol: &fl_solve::Solution,
    obs: &Observer,
) -> Value {
    // Image footprint on the sky, in border order.
    let (w, h) = (sol.width, sol.height);
    let corners =
        [(0.0, 0.0), (w, 0.0), (w, h), (0.0, h)].map(|(x, y)| sol.wcs.pixel_to_radec(x, y));
    let c = &sol.calibration;
    let lst = fl_sky::altaz::lst_deg(obs.unix, obs.lon);
    let (alt, az) = fl_sky::altaz::radec_to_altaz(c.ra, c.dec, obs.lat, lst);
    let width_deg = w * c.pixscale / 3600.0;
    let height_deg = h * c.pixscale / 3600.0;

    let svg = fl_sky::chart::render_svg(&fl_sky::chart::ChartParams {
        unix: obs.unix,
        lat_deg: obs.lat,
        lon_deg: obs.lon,
        fov: Some(fl_sky::chart::Fov {
            corners,
            label: format!("{width_deg:.2}\u{b0} \u{d7} {height_deg:.2}\u{b0}"),
        }),
    });
    state.store.put_artifact(
        job,
        "skyview",
        Artifact {
            content_type: "image/svg+xml; charset=utf-8",
            filename: derived_name(filename, "skyview.svg"),
            bytes: Arc::new(svg.into_bytes()),
        },
    );

    if alt <= 0.0 {
        tracing::warn!(
            job,
            alt,
            "solved field is below the horizon - check timestamp/coordinates"
        );
    }
    json!({
        "chart_url": format!("/api/v1/jobs/{job}/skyview.svg"),
        "alt": alt,
        "az": az,
        "above_horizon": alt > 0.0,
        "width_deg": width_deg,
        "height_deg": height_deg,
        "corners_radec": corners.iter().map(|&(r, d)| json!([r, d])).collect::<Vec<_>>(),
        "observer": {
            "timestamp_utc": fl_sky::altaz::format_utc(obs.unix),
            "unix": obs.unix,
            "latitude": obs.lat,
            "longitude": obs.lon,
            "lst_deg": lst,
        },
    })
}

/// Solve hints as plain form fields — no `request-json` envelope; that is
/// nova's convention, not ours.
fn hints_from(form: &Form) -> Result<SolveHints, String> {
    let (scale_lo, scale_hi) = (form.number("scale_low"), form.number("scale_high"));
    if let (Some(lo), Some(hi)) = (scale_lo, scale_hi) {
        if lo <= 0.0 || hi < lo {
            return Err("scale_low must be positive and no greater than scale_high".into());
        }
    }
    let parity = match form.field("parity").map(str::trim) {
        None | Some("") | Some("both") => Parity::Both,
        Some("normal") => Parity::Normal,
        Some("flip") => Parity::Flip,
        Some(other) => {
            return Err(format!(
                "parity must be normal, flip or both (got {other:?})"
            ))
        }
    };
    Ok(SolveHints {
        scale_lo,
        scale_hi,
        center_ra: form.number("center_ra"),
        center_dec: form.number("center_dec"),
        radius: form.number("radius"),
        downsample: form.number("downsample").map(|d| d.max(1.0) as usize),
        parity,
        ..SolveHints::default()
    })
}

/// This API's error shape: `{"status": "error", "error": "..."}`.
fn error(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({"status": "error", "error": msg}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn form(fields: &[(&str, &str)]) -> Form {
        Form {
            file: None,
            fields: fields
                .iter()
                .map(|(k, v)| (k.to_string(), v.to_string()))
                .collect(),
        }
    }

    #[test]
    fn hints_come_from_plain_fields() {
        let h = hints_from(&form(&[
            ("scale_low", "1.5"),
            ("scale_high", "3.0"),
            ("center_ra", "83.0"),
            ("center_dec", "-5.0"),
            ("radius", "10"),
            ("downsample", "2"),
            ("parity", "flip"),
        ]))
        .unwrap();
        assert_eq!((h.scale_lo, h.scale_hi), (Some(1.5), Some(3.0)));
        assert_eq!(h.center_ra, Some(83.0));
        assert_eq!(h.downsample, Some(2));
        assert!(matches!(h.parity, Parity::Flip));
    }

    #[test]
    fn nonsense_hints_are_rejected_rather_than_ignored() {
        assert!(hints_from(&form(&[("scale_low", "3"), ("scale_high", "1")])).is_err());
        assert!(hints_from(&form(&[("parity", "sideways")])).is_err());
        // No hints at all is the normal case.
        assert!(hints_from(&form(&[])).is_ok());
    }

    #[test]
    fn the_observer_is_validated_before_solving() {
        let ok = Observer::from_form(&form(&[
            ("timestamp", "2026-01-15T02:30:00Z"),
            ("latitude", "40.7128"),
            ("longitude", "-74.0060"),
        ]))
        .unwrap();
        assert!((ok.lat - 40.7128).abs() < 1e-9);
        assert!(Observer::from_form(&form(&[("latitude", "40"), ("longitude", "-74")])).is_err());
        assert!(Observer::from_form(&form(&[
            ("timestamp", "2026-01-15T02:30:00Z"),
            ("latitude", "99"),
            ("longitude", "0")
        ]))
        .is_err());
    }
}
