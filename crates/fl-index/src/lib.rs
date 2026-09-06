//! Reader for stock astrometry.net index files (4100/4200/5000 series).
//!
//! An index file is a multi-HDU FITS file containing:
//! - a `quads` table: for each quad, the IDs of its member stars,
//! - a code kd-tree (`codes`): geometric hash codes of the quads,
//! - a star kd-tree (`stars`): unit-sphere xyz positions,
//! - auxiliary `sweep` / tag-along tables (unused here).
//!
//! The kd-trees are serialized by libkd as one FITS extension per internal
//! array, in the *producer's native byte order* (declared by the `ENDIAN`
//! header card) — not FITS big-endian.

mod index_file;
mod kdtree;
mod meta;
pub mod cache;

pub use index_file::{IndexFile, DQMAX};
pub use kdtree::{KdTree, KdView, PackedType};
pub use meta::IndexMeta;

#[derive(Debug, thiserror::Error)]
pub enum IndexError {
    #[error("fits: {0}")]
    Fits(#[from] fl_fits::FitsError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("bad index file: {0}")]
    Format(String),
}

pub type Result<T> = std::result::Result<T, IndexError>;
