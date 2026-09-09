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

/// Longest edge of a preview, pixels. Enough for a browser column on a
/// high-density screen; a 60-megapixel frame would otherwise arrive as a
/// PNG the size of the original.
pub const PREVIEW_MAX_EDGE: usize = 2048;

/// A viewable rendering of the upload: greyscale, auto-stretched the way an
/// imaging program's screen transfer function would, and reduced by an
/// integer factor so it fits `PREVIEW_MAX_EDGE`. Returns the PNG and the
/// factor, which a client needs to map preview pixels back to the WCS.
pub fn preview_png(original: &[u8]) -> Result<(Vec<u8>, usize), String> {
    let img = fl_extract::decode(original).map_err(|e| e.to_string())?;
    if img.w == 0 || img.h == 0 {
        return Err("empty image".into());
    }
    let factor = img.w.max(img.h).div_ceil(PREVIEW_MAX_EDGE).max(1);
    let small = img.downsample(factor);
    let pixels = stretch(&small.data);
    let mut png = Vec::new();
    image::write_buffer_with_format(
        &mut std::io::Cursor::new(&mut png),
        &pixels,
        small.w as u32,
        small.h as u32,
        image::ExtendedColorType::L8,
        image::ImageFormat::Png,
    )
    .map_err(|e| e.to_string())?;
    Ok((png, factor))
}

/// Auto-stretch to 8 bits: the shadows are clipped a few deviations below
/// the median and a midtones curve lifts the median to a quarter grey, so
/// faint nebulosity shows without the stars saturating into blobs. This is
/// the usual "auto STF" recipe; the noise estimate is the median absolute
/// deviation, which a handful of bright stars cannot skew.
pub fn stretch(data: &[f32]) -> Vec<u8> {
    // Statistics over at most ~1M samples; the sort dominates otherwise.
    let step = (data.len() / 1_000_000).max(1);
    let mut sample: Vec<f32> = data
        .iter()
        .step_by(step)
        .copied()
        .filter(|v| v.is_finite())
        .collect();
    if sample.is_empty() {
        return vec![0; data.len()];
    }
    sample.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let median = sample[sample.len() / 2];
    let mut dev: Vec<f32> = sample.iter().map(|v| (v - median).abs()).collect();
    dev.sort_by(|a, b| a.partial_cmp(b).unwrap());
    let mad = dev[dev.len() / 2];
    let (lo, hi) = (sample[0], sample[sample.len() - 1]);
    if hi <= lo {
        return vec![0; data.len()];
    }
    // A flat image (MAD 0) falls back to a plain linear stretch.
    let sigma = if mad > 0.0 {
        mad * 1.4826
    } else {
        (hi - lo) / 6.0
    };
    let black = (median - 2.8 * sigma).max(lo);
    let white = hi;
    let range = white - black;
    // Midtones balance so that the median lands at `target`.
    let target = 0.25f64;
    let x0 = ((median - black) / range).clamp(1e-6, 1.0 - 1e-6) as f64;
    let m = x0 * (target - 1.0) / ((2.0 * x0 - 1.0) * target - x0);
    let mtf = |x: f64| -> f64 {
        if x <= 0.0 {
            0.0
        } else if x >= 1.0 {
            1.0
        } else {
            ((m - 1.0) * x) / ((2.0 * m - 1.0) * x - m)
        }
    };
    data.iter()
        .map(|&v| {
            if !v.is_finite() {
                return 0;
            }
            let x = ((v - black) / range) as f64;
            (mtf(x) * 255.0).round().clamp(0.0, 255.0) as u8
        })
        .collect()
}

/// Render the preview and park it for retrieval. Returns the preview's
/// dimensions and reduction factor.
pub fn store_preview(
    state: &AppState,
    job: u64,
    uploaded: &str,
    original: &[u8],
) -> Result<(usize, usize, usize), String> {
    let (png, factor) = preview_png(original)?;
    let (w, h) = png_dimensions(&png).ok_or("preview encoder produced no header")?;
    state.store.put_artifact(
        job,
        "preview",
        Artifact {
            content_type: "image/png",
            filename: derived_name(uploaded, "preview.png"),
            bytes: Arc::new(png),
        },
    );
    Ok((w, h, factor))
}

/// Width and height from a PNG's IHDR chunk.
fn png_dimensions(png: &[u8]) -> Option<(usize, usize)> {
    if png.len() < 24 || &png[12..16] != b"IHDR" {
        return None;
    }
    let be = |b: &[u8]| u32::from_be_bytes([b[0], b[1], b[2], b[3]]) as usize;
    Some((be(&png[16..20]), be(&png[20..24])))
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
    fn the_stretch_lifts_the_median_and_keeps_stars_bright() {
        // A dark sky with faint noise and a few bright stars.
        let mut data = vec![100.0f32; 10_000];
        for (i, v) in data.iter_mut().enumerate() {
            *v += ((i * 7919) % 13) as f32 - 6.0;
        }
        data[5_000] = 60_000.0;
        data[123] = 40_000.0;
        data[9_999] = f32::NAN;
        let out = stretch(&data);
        assert_eq!(out.len(), data.len());
        let median = {
            let mut s = out.clone();
            s.sort_unstable();
            s[s.len() / 2]
        };
        assert!((40..=90).contains(&median), "median mapped to {median}");
        assert_eq!(out[5_000], 255);
        assert!(out[123] > 200);
        assert_eq!(out[9_999], 0, "non-finite pixels go black");
        // A constant image must not divide by zero.
        assert!(stretch(&[5.0; 16]).iter().all(|&v| v == 0));
        assert!(stretch(&[]).is_empty());
    }

    #[test]
    fn previews_are_png_and_reduced_to_the_size_budget() {
        // 3000x1000 8-bit gradient as a PNG upload.
        let (w, h) = (3000usize, 1000usize);
        let pixels: Vec<u8> = (0..w * h).map(|i| (i % w * 255 / w) as u8).collect();
        let mut upload = Vec::new();
        image::write_buffer_with_format(
            &mut std::io::Cursor::new(&mut upload),
            &pixels,
            w as u32,
            h as u32,
            image::ExtendedColorType::L8,
            image::ImageFormat::Png,
        )
        .unwrap();
        let (png, factor) = preview_png(&upload).unwrap();
        assert_eq!(factor, 2);
        assert_eq!(png_dimensions(&png), Some((1500, 500)));
        assert!(png.starts_with(b"\x89PNG"));
        // Small images are passed through at full size.
        let (png, factor) = preview_png(&testdata_jpeg()).unwrap();
        assert_eq!(factor, 1);
        assert!(png_dimensions(&png).is_some());
        assert!(preview_png(b"not an image").is_err());
    }

    fn testdata_jpeg() -> Vec<u8> {
        std::fs::read(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/../../testdata/bench/base_m.jpg"
        ))
        .unwrap()
    }

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
