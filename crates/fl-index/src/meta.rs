use fl_fits::Fits;

use crate::{IndexError, Result};

/// Index-wide metadata gathered from the primary and code-tree headers.
#[derive(Debug, Clone)]
pub struct IndexMeta {
    pub index_id: i64,
    /// Healpix tile of this index (-1 for all-sky).
    pub healpix: i64,
    pub hpnside: i64,
    pub allsky: bool,
    /// Quad angular size bounds in radians.
    pub scale_lower_rad: f64,
    pub scale_upper_rad: f64,
    pub nquads: usize,
    pub nstars: usize,
    pub dimquads: usize,
    /// Codes obey cx <= dx (<= ex ...).
    pub cx_le_dx: bool,
    /// Codes obey mean(x) <= 1/2.
    pub meanx_le_half: bool,
    /// Stars C,D,... live in the unit circle centered (0.5, 0.5) through A,B.
    pub circle: bool,
}

impl IndexMeta {
    pub fn parse(fits: &Fits) -> Result<IndexMeta> {
        // Quad metadata: usually in the primary header; fall back to any HDU
        // that carries DIMQUADS (layout varies slightly across series).
        let qhdr = fits
            .hdus
            .iter()
            .map(|h| &h.header)
            .find(|h| h.get_i64("DIMQUADS").is_some())
            .ok_or_else(|| IndexError::Format("no HDU with DIMQUADS".into()))?;

        let chdr = &fits
            .find("kdtree_header_codes")
            .ok_or_else(|| IndexError::Format("missing code kdtree header".into()))?
            .header;

        Ok(IndexMeta {
            index_id: qhdr.get_i64("INDEXID").unwrap_or(-1),
            healpix: qhdr.get_i64("HEALPIX").unwrap_or(-1),
            hpnside: qhdr.get_i64("HPNSIDE").unwrap_or(0),
            allsky: qhdr.get_bool("ALLSKY").unwrap_or(false)
                || qhdr.get_i64("HEALPIX").unwrap_or(-1) < 0,
            scale_lower_rad: qhdr.req_f64("SCALE_L")?,
            scale_upper_rad: qhdr.req_f64("SCALE_U")?,
            nquads: qhdr.req_i64("NQUADS")? as usize,
            nstars: qhdr.req_i64("NSTARS")? as usize,
            dimquads: qhdr.req_i64("DIMQUADS")? as usize,
            cx_le_dx: chdr.get_bool("CXDX").unwrap_or(false),
            meanx_le_half: chdr.get_bool("CXDXLT1").unwrap_or(false),
            circle: chdr.get_bool("CIRCLE").unwrap_or(false),
        })
    }
}
