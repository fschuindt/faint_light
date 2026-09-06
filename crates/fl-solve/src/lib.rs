//! Plate-solving engine with the nova.astrometry.net-compatible surface the
//! server needs: bytes in, calibration out.

pub mod codes;
pub mod engine;
pub mod tanwcs;
pub mod verify;

pub use codes::Parity;
pub use engine::{Engine, EngineConfig, Solution};

/// The six numbers NINA (and the nova API) consume.
#[derive(Debug, Clone, Copy)]
pub struct Calibration {
    /// Image-center RA, degrees.
    pub ra: f64,
    /// Image-center Dec, degrees.
    pub dec: f64,
    /// Center-to-corner angular radius, degrees.
    pub radius: f64,
    /// arcsec / pixel.
    pub pixscale: f64,
    /// Position angle of image up, degrees E of N.
    pub orientation: f64,
    /// +1 / -1 (sign of det CD).
    pub parity: f64,
}

/// Optional client hints parsed from the upload request-json. NINA sends
/// none of these; astrometry's client.py and astroquery send several.
#[derive(Debug, Clone)]
pub struct SolveHints {
    /// arcsec/px bounds (already normalized by the server from
    /// scale_units/scale_type/scale_est/scale_err/scale_lower/scale_upper).
    pub scale_lo: Option<f64>,
    pub scale_hi: Option<f64>,
    /// Field-width bounds in degrees (scale_units degwidth/arcminwidth);
    /// resolved into scale_lo/hi once the image width is known.
    pub width_deg: Option<(f64, f64)>,
    /// Position hint, degrees.
    pub center_ra: Option<f64>,
    pub center_dec: Option<f64>,
    /// Search radius around the position hint, degrees.
    pub radius: Option<f64>,
    pub downsample: Option<usize>,
    pub parity: Parity,
}

impl Default for SolveHints {
    fn default() -> Self {
        SolveHints {
            scale_lo: None,
            scale_hi: None,
            width_deg: None,
            center_ra: None,
            center_dec: None,
            radius: None,
            downsample: None,
            parity: Parity::Both,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum SolveError {
    #[error("engine setup: {0}")]
    Setup(String),
    #[error("image decode failed: {0}")]
    Decode(String),
    #[error("no solution found")]
    NoSolution,
    #[error("solve timed out")]
    Timeout,
}
