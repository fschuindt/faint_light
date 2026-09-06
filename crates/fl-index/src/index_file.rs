use std::path::{Path, PathBuf};

use fl_fits::Fits;
use memmap2::Mmap;

use crate::kdtree::{endian_swap, KdTree, KdView};
use crate::{IndexError, IndexMeta, Result};

/// Maximum stars per quad across all published index series.
pub const DQMAX: usize = 5;

/// A memory-mapped astrometry.net index file.
pub struct IndexFile {
    pub path: PathBuf,
    mmap: Mmap,
    pub meta: IndexMeta,
    code_kd: KdTree,
    star_kd: KdTree,
    quads_off: usize,
    quads_row: usize,
    quads_swap: bool,
}

impl IndexFile {
    pub fn open(path: &Path) -> Result<IndexFile> {
        let file = std::fs::File::open(path)?;
        // Safety: the file is opened read-only; we treat any concurrent
        // truncation as a bug of the operator (indexes are static data).
        let mmap = unsafe { Mmap::map(&file)? };
        let fits = Fits::parse(&mmap)?;
        let meta = IndexMeta::parse(&fits)?;

        let code_kd = KdTree::parse(&fits, &mmap, "codes")?;
        let star_kd = KdTree::parse(&fits, &mmap, "stars")?;

        if code_kd.ndim != 2 * (meta.dimquads - 2) {
            return Err(IndexError::Format(format!(
                "code dim {} inconsistent with dimquads {}",
                code_kd.ndim, meta.dimquads
            )));
        }
        if star_kd.ndim != 3 {
            return Err(IndexError::Format("star tree is not 3-D".into()));
        }
        if code_kd.ndata != meta.nquads || star_kd.ndata != meta.nstars {
            return Err(IndexError::Format("tree sizes inconsistent with meta".into()));
        }
        if meta.dimquads < 3 || meta.dimquads > DQMAX {
            return Err(IndexError::Format(format!("dimquads {}", meta.dimquads)));
        }

        let quads = fits
            .find("quads")
            .ok_or_else(|| IndexError::Format("missing quads table".into()))?;
        let (qw, qn) = quads.table_shape();
        if qn != meta.nquads || qw != 4 * meta.dimquads {
            return Err(IndexError::Format(format!(
                "quads table {qw}x{qn}, expected {}x{}",
                4 * meta.dimquads,
                meta.nquads
            )));
        }
        // Quads are written native-endian, declared in the primary header.
        let quads_swap = endian_swap(fits.hdus[0].header.get_str("ENDIAN"));

        Ok(IndexFile {
            path: path.to_path_buf(),
            quads_off: quads.data_offset,
            quads_row: qw,
            quads_swap,
            mmap,
            meta,
            code_kd,
            star_kd,
        })
    }

    pub fn codes(&self) -> KdView<'_> {
        KdView { tree: &self.code_kd, base: &self.mmap }
    }

    pub fn stars(&self) -> KdView<'_> {
        KdView { tree: &self.star_kd, base: &self.mmap }
    }

    /// Star IDs of quad `q`. Returns (ids, dimquads).
    #[inline]
    pub fn quad_stars(&self, q: usize) -> ([u32; DQMAX], usize) {
        let dq = self.meta.dimquads;
        let off = self.quads_off + q * self.quads_row;
        let mut out = [0u32; DQMAX];
        for (i, slot) in out.iter_mut().enumerate().take(dq) {
            let b: [u8; 4] = self.mmap[off + 4 * i..off + 4 * i + 4].try_into().unwrap();
            *slot = if self.quads_swap {
                u32::from_be_bytes(b)
            } else {
                u32::from_le_bytes(b)
            };
        }
        (out, dq)
    }

    /// Unit-sphere position of star `i`.
    #[inline]
    pub fn star_xyz(&self, i: u32) -> [f64; 3] {
        let mut out = [0f64; 3];
        self.star_kd.point(&self.mmap, i as usize, &mut out);
        out
    }

    /// File size in bytes (cache budgeting).
    pub fn byte_size(&self) -> usize {
        self.mmap.len()
    }

    /// Ask the OS to prefetch the whole mapping into the page cache.
    pub fn prewarm(&self) {
        #[cfg(unix)]
        let _ = self.mmap.advise(memmap2::Advice::WillNeed);
    }
}

impl std::fmt::Debug for IndexFile {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IndexFile")
            .field("path", &self.path)
            .field("meta", &self.meta)
            .finish()
    }
}
