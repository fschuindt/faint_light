//! The solving machinery both APIs share: queueing, waiting, and turning a
//! solution into the files a client can ask for.

use std::sync::Arc;
use std::time::Duration;

use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::{json, Value};

use fl_solve::Solution;

use crate::state::AppState;
use crate::store::{Artifact, JobStatus};
use crate::worker::Task;

/// Queue a solve, creating its submission and job. `Err` is the response to
/// return to the client.
pub fn enqueue(
    state: &AppState,
    filename: String,
    bytes: Arc<Vec<u8>>,
    hints: fl_solve::SolveHints,
) -> Result<(u64, u64), Response> {
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
        return Err(err_json(
            StatusCode::SERVICE_UNAVAILABLE,
            "solve queue full",
        ));
    }
    Ok((sub_id, job_id))
}

/// Block until the worker finishes `job_id`. The budget is the per-job solve
/// timeout plus grace for time spent queued behind other jobs.
///
/// `failure` renders a solver failure in the caller's own error shape, since
/// the two APIs word errors differently.
pub async fn await_job(
    state: &AppState,
    job_id: u64,
    failure: impl Fn(u64, &str) -> Response,
) -> Result<Box<Solution>, Response> {
    let deadline = tokio::time::Instant::now() + state.solve_timeout + Duration::from_secs(30);
    loop {
        match state.store.job(job_id).map(|j| j.status) {
            Some(JobStatus::Success(sol)) => return Ok(sol),
            Some(JobStatus::Failure(msg)) => return Err(failure(job_id, &msg)),
            Some(JobStatus::Solving) => {
                if tokio::time::Instant::now() > deadline {
                    return Err(err_json(StatusCode::GATEWAY_TIMEOUT, "solve timed out"));
                }
                tokio::time::sleep(Duration::from_millis(100)).await;
            }
            None => return Err(err_json(StatusCode::INTERNAL_SERVER_ERROR, "job vanished")),
        }
    }
}

/// The six numbers nova reports, plus the two `*_arcsec` sizes.
pub fn calibration_json(sol: &Solution) -> Value {
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

/// The FITS answer for a solved upload: the original file re-headered when
/// it already was FITS, a fresh 32-bit float image otherwise.
pub fn solved_fits(original: &[u8], sol: &Solution) -> Result<Vec<u8>, String> {
    let mut cards = sol.wcs.fits_cards(sol.width, sol.height);
    // Commentary cards accumulate rather than replace, so date this one:
    // re-solving a file then reads as a history instead of a duplicate.
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs_f64();
    cards.push(fl_fits::write::Card::comment(&format!(
        "Plate solved by faint_light {} on {} UTC",
        crate::version(),
        fl_sky::altaz::format_utc(now),
    )));
    if original.starts_with(b"SIMPLE  =") {
        fl_fits::write::set_cards(original, &cards, fl_solve::tanwcs::is_wcs_key)
            .map_err(|e| e.to_string())
    } else {
        let img = fl_extract::decode(original).map_err(|e| e.to_string())?;
        Ok(fl_fits::write::image_f32(img.w, img.h, &img.data, &cards))
    }
}

/// Build the solved FITS and park it for retrieval.
pub fn store_fits(
    state: &AppState,
    job: u64,
    uploaded: &str,
    original: &[u8],
    sol: &Solution,
) -> Result<String, String> {
    let bytes = solved_fits(original, sol)?;
    let filename = derived_name(uploaded, "solved.fits");
    state.store.put_artifact(
        job,
        "fits",
        Artifact {
            content_type: "application/fits",
            filename: filename.clone(),
            bytes: Arc::new(bytes),
        },
    );
    Ok(filename)
}

/// A download name derived from what the client uploaded, reduced to
/// characters that survive a Content-Disposition header and every
/// filesystem a client might save it to.
pub fn derived_name(uploaded: &str, suffix: &str) -> String {
    let stem = std::path::Path::new(uploaded)
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("");
    let safe: String = stem
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || "-_.".contains(c) {
                c
            } else {
                '_'
            }
        })
        .take(64)
        .collect();
    let safe = safe.trim_matches(['.', '_']);
    format!("{}.{suffix}", if safe.is_empty() { "image" } else { safe })
}

/// nova's error shape: `{"status": "error", "errormessage": "..."}`.
pub fn err_json(code: StatusCode, msg: &str) -> Response {
    (code, Json(json!({"status": "error", "errormessage": msg}))).into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn derived_names_are_safe_and_keep_the_stem() {
        assert_eq!(
            derived_name("m42_light.jpg", "solved.fits"),
            "m42_light.solved.fits"
        );
        assert_eq!(
            derived_name("/tmp/sub dir/M 42.fit", "skyview.svg"),
            "M_42.skyview.svg"
        );
        // Nothing usable left, and nothing that could escape a directory.
        assert_eq!(derived_name("...", "solved.fits"), "image.solved.fits");
        assert_eq!(derived_name("", "solved.fits"), "image.solved.fits");
        assert!(!derived_name("../../etc/passwd", "solved.fits").contains(['/', '\\']));
    }
}
