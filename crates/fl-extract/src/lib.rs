//! Image decoding and star extraction (a compact port of astrometry.net's
//! simplexy pipeline: background median-smooth, sigma estimation, Gaussian
//! smoothing, thresholded connected components, 3x3 sub-pixel centroids).

mod background;
mod decode;
mod simplexy;

pub use decode::{decode, GrayImage};
pub use simplexy::{extract_stars, ExtractParams, Star};

#[derive(Debug, thiserror::Error)]
pub enum ExtractError {
    #[error("image decode: {0}")]
    Image(#[from] image::ImageError),
    #[error("fits decode: {0}")]
    Fits(#[from] fl_fits::FitsError),
    #[error("unsupported image: {0}")]
    Unsupported(String),
}

pub type Result<T> = std::result::Result<T, ExtractError>;
