//! Minimal FITS reader tailored to astrometry.net index files and camera images.
//!
//! Design goals:
//! - Zero-copy: HDUs are described by byte offsets into the caller's buffer
//!   (typically an mmap), never copied.
//! - Tolerant of astrometry.net/qfits quirks: blank-keyword continuation cards,
//!   native-endian binary table payloads (the caller decides byte order).

mod header;
mod hdu;
pub mod image;

pub use header::{Header, Value};
pub use hdu::{Fits, Hdu};

#[derive(Debug, thiserror::Error)]
pub enum FitsError {
    #[error("not a FITS file (missing SIMPLE/XTENSION)")]
    NotFits,
    #[error("truncated FITS file at offset {0}")]
    Truncated(usize),
    #[error("invalid header card: {0}")]
    BadCard(String),
    #[error("missing required card: {0}")]
    MissingCard(String),
    #[error("unsupported: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, FitsError>;

pub const BLOCK: usize = 2880;
pub const CARD: usize = 80;
