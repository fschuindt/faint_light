use crate::{FitsError, Header, Result, BLOCK};

/// One HDU: parsed header plus the byte range of its data section.
#[derive(Debug)]
pub struct Hdu {
    pub header: Header,
    pub data_offset: usize,
    /// Unpadded data length in bytes (NAXIS product + PCOUNT heap).
    pub data_len: usize,
}

impl Hdu {
    /// Byte offset of this HDU's first header card.
    pub fn header_offset(&self) -> usize {
        self.data_offset - self.header.nblocks * BLOCK
    }

    /// For BINTABLE HDUs: (row_bytes, nrows).
    pub fn table_shape(&self) -> (usize, usize) {
        let w = self.header.get_i64("NAXIS1").unwrap_or(0).max(0) as usize;
        let h = self.header.get_i64("NAXIS2").unwrap_or(0).max(0) as usize;
        (w, h)
    }

    /// Name identifying this HDU: EXTNAME if present, else TTYPE1 (the
    /// convention astrometry.net index files use for their single-column
    /// tables).
    pub fn name(&self) -> Option<&str> {
        self.header
            .get_str("EXTNAME")
            .or_else(|| self.header.get_str("TTYPE1"))
    }
}

/// All HDUs of a FITS file, described by offsets into the caller's buffer.
#[derive(Debug)]
pub struct Fits {
    pub hdus: Vec<Hdu>,
}

impl Fits {
    pub fn parse(buf: &[u8]) -> Result<Fits> {
        if buf.len() < BLOCK || !buf.starts_with(b"SIMPLE  =") {
            return Err(FitsError::NotFits);
        }
        let mut hdus = Vec::new();
        let mut off = 0usize;
        while off < buf.len() {
            let (header, data_offset) = Header::parse(buf, off)?;
            let bitpix = header.get_i64("BITPIX").unwrap_or(8).unsigned_abs() as usize;
            let naxis = header.get_i64("NAXIS").unwrap_or(0).max(0) as usize;
            let mut nelem = if naxis > 0 { 1usize } else { 0 };
            for i in 1..=naxis {
                let d = header.get_i64(&format!("NAXIS{i}")).unwrap_or(0).max(0) as usize;
                nelem = nelem.saturating_mul(d);
            }
            let pcount = header.get_i64("PCOUNT").unwrap_or(0).max(0) as usize;
            let gcount = header.get_i64("GCOUNT").unwrap_or(1).max(1) as usize;
            let data_len = (bitpix / 8) * gcount * (nelem + pcount);
            let padded = data_len.div_ceil(BLOCK) * BLOCK;
            if data_offset + data_len > buf.len() {
                return Err(FitsError::Truncated(data_offset));
            }
            hdus.push(Hdu {
                header,
                data_offset,
                data_len,
            });
            off = data_offset + padded;
        }
        Ok(Fits { hdus })
    }

    /// Find an HDU by EXTNAME/TTYPE1.
    pub fn find(&self, name: &str) -> Option<&Hdu> {
        self.hdus.iter().find(|h| h.name() == Some(name))
    }
}
