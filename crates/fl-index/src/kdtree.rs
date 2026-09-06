//! libkd kd-tree reading and range search.
//!
//! Serialized layout (per tree `<name>`):
//! - `kdtree_header_<name>`: empty table whose header carries
//!   KDT_NDAT / KDT_NDIM / KDT_NNOD / KDT_INT / KDT_DATA / KDT_LINL / ENDIAN.
//! - `kdtree_lr_<name>`: u32 per leaf — index of the rightmost data point
//!   owned by that leaf (absent when KDT_LINL=T: leaf ranges are implicit).
//! - `kdtree_split_<name>`: split plane per interior node. When there is no
//!   `kdtree_splitdim_<name>` array, the split dimension is packed into the
//!   low `ceil(log2(ndim))` bits of the split value.
//! - `kdtree_range_<name>`: f64 mins[ndim], maxs[ndim], scale — the packing
//!   transform: packed = (x - min[dim]) * scale.
//! - `kdtree_data_<name>`: ndata x ndim packed points.
//!
//! The tree is an implicit complete binary tree: children of node n are
//! 2n+1 / 2n+2; the last `nbottom` nodes are leaves; data is stored permuted
//! in tree order so each leaf owns a contiguous range.

use fl_fits::Fits;

use crate::{IndexError, Result};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackedType {
    U16,
    U32,
    F32,
    F64,
}

impl PackedType {
    pub fn size(self) -> usize {
        match self {
            PackedType::U16 => 2,
            PackedType::U32 | PackedType::F32 => 4,
            PackedType::F64 => 8,
        }
    }
    fn from_kdt(s: &str) -> Result<Self> {
        match s {
            "u16" => Ok(PackedType::U16),
            "u32" => Ok(PackedType::U32),
            "float" => Ok(PackedType::F32),
            "double" => Ok(PackedType::F64),
            other => Err(IndexError::Format(format!("unsupported kd type {other:?}"))),
        }
    }
    fn is_int(self) -> bool {
        matches!(self, PackedType::U16 | PackedType::U32)
    }
}

/// Reference to a typed array inside the mmap.
#[derive(Debug, Clone, Copy)]
struct ArrRef {
    off: usize,
    ty: PackedType,
}

impl ArrRef {
    #[inline(always)]
    fn get(&self, base: &[u8], idx: usize, swap: bool) -> f64 {
        let o = self.off + idx * self.ty.size();
        match self.ty {
            PackedType::U16 => {
                let b: [u8; 2] = base[o..o + 2].try_into().unwrap();
                (if swap { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) }) as f64
            }
            PackedType::U32 => {
                let b: [u8; 4] = base[o..o + 4].try_into().unwrap();
                (if swap { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) }) as f64
            }
            PackedType::F32 => {
                let b: [u8; 4] = base[o..o + 4].try_into().unwrap();
                (if swap { f32::from_be_bytes(b) } else { f32::from_le_bytes(b) }) as f64
            }
            PackedType::F64 => {
                let b: [u8; 8] = base[o..o + 8].try_into().unwrap();
                if swap { f64::from_be_bytes(b) } else { f64::from_le_bytes(b) }
            }
        }
    }

    /// Raw integer value (for split-dim masking and lr entries).
    #[inline(always)]
    fn get_int(&self, base: &[u8], idx: usize, swap: bool) -> u64 {
        let o = self.off + idx * self.ty.size();
        match self.ty {
            PackedType::U16 => {
                let b: [u8; 2] = base[o..o + 2].try_into().unwrap();
                (if swap { u16::from_be_bytes(b) } else { u16::from_le_bytes(b) }) as u64
            }
            PackedType::U32 => {
                let b: [u8; 4] = base[o..o + 4].try_into().unwrap();
                (if swap { u32::from_be_bytes(b) } else { u32::from_le_bytes(b) }) as u64
            }
            _ => panic!("get_int on float array"),
        }
    }
}

#[derive(Debug)]
pub struct KdTree {
    pub name: String,
    pub ndim: usize,
    pub ndata: usize,
    pub nnodes: usize,
    pub nbottom: usize,
    pub ninterior: usize,
    data: ArrRef,
    split: Option<ArrRef>,
    splitdim: Option<ArrRef>, // u8 per interior node
    lr: Option<ArrRef>,       // u32 per leaf
    perm: Option<ArrRef>,     // u32 per data point
    linear_lr: bool,
    pub minval: Vec<f64>,
    pub maxval: Vec<f64>,
    pub scale: f64,
    pub invscale: f64,
    /// Conservative slack (in external units) added when pruning subtrees, to
    /// absorb split-value quantization and dim-bit masking.
    split_eps: f64,
    dim_mask: u64,
    swap: bool,
}

impl KdTree {
    /// Parse tree `<name>` from an already-parsed FITS file over `base`.
    pub fn parse(fits: &Fits, base: &[u8], name: &str) -> Result<KdTree> {
        let hname = format!("kdtree_header_{name}");
        let hdr = &fits
            .find(&hname)
            .ok_or_else(|| IndexError::Format(format!("missing HDU {hname}")))?
            .header;

        let ndata = hdr.req_i64("KDT_NDAT")? as usize;
        let ndim = hdr.req_i64("KDT_NDIM")? as usize;
        let nnodes = hdr.req_i64("KDT_NNOD")? as usize;
        let int_ty = PackedType::from_kdt(hdr.get_str("KDT_INT").unwrap_or("u32"))?;
        let data_ty = PackedType::from_kdt(hdr.get_str("KDT_DATA").unwrap_or("u32"))?;
        let linear_lr = hdr.get_bool("KDT_LINL").unwrap_or(false);
        let swap = endian_swap(hdr.get_str("ENDIAN"));

        let nbottom = nnodes.div_ceil(2);
        let ninterior = nnodes - nbottom;
        if !nbottom.is_power_of_two() || nnodes != 2 * nbottom - 1 {
            return Err(IndexError::Format(format!(
                "tree {name}: unsupported shape nnodes={nnodes}"
            )));
        }
        if ndim == 0 || ndim > 8 {
            return Err(IndexError::Format(format!("tree {name}: ndim={ndim}")));
        }

        let table = |suffix: &str| -> Option<(usize, usize, usize)> {
            let h = fits.find(&format!("kdtree_{suffix}_{name}"))?;
            let (w, n) = h.table_shape();
            Some((h.data_offset, w, n))
        };

        // Data array (required).
        let (doff, dw, dn) = table("data")
            .ok_or_else(|| IndexError::Format(format!("tree {name}: missing data table")))?;
        if dn != ndata || dw != ndim * data_ty.size() {
            return Err(IndexError::Format(format!(
                "tree {name}: data table {dw}x{dn}, expected {}x{ndata}",
                ndim * data_ty.size()
            )));
        }
        let data = ArrRef { off: doff, ty: data_ty };

        // Range array: mins[ndim], maxs[ndim], scale (only for packed data).
        let (mut minval, mut maxval) = (vec![0.0; ndim], vec![0.0; ndim]);
        let (mut scale, mut invscale) = (1.0f64, 1.0f64);
        if let Some((roff, rw, rn)) = table("range") {
            if rw != 8 || rn < 2 * ndim + 1 {
                return Err(IndexError::Format(format!(
                    "tree {name}: bad range table {rw}x{rn}"
                )));
            }
            let r = ArrRef { off: roff, ty: PackedType::F64 };
            for d in 0..ndim {
                minval[d] = r.get(base, d, swap);
                maxval[d] = r.get(base, ndim + d, swap);
            }
            scale = r.get(base, 2 * ndim, swap);
            invscale = 1.0 / scale;
        } else if data_ty.is_int() {
            return Err(IndexError::Format(format!(
                "tree {name}: packed data but no range table"
            )));
        }

        // Split array (interior nodes).
        let split = match table("split") {
            Some((soff, sw, sn)) => {
                if sn != ninterior || sw != int_ty.size() {
                    return Err(IndexError::Format(format!(
                        "tree {name}: split table {sw}x{sn}, expected {}x{ninterior}",
                        int_ty.size()
                    )));
                }
                Some(ArrRef { off: soff, ty: int_ty })
            }
            None => None,
        };

        // Optional split-dimension array (u8 per interior node).
        let splitdim = table("splitdim").map(|(off, _w, _n)| ArrRef {
            off,
            ty: PackedType::U16, // never read via get(); raw bytes below
        });

        // lr array: u32 per leaf.
        let lr = match table("lr") {
            Some((loff, lw, ln)) => {
                if ln != nbottom || lw != 4 {
                    return Err(IndexError::Format(format!(
                        "tree {name}: lr table {lw}x{ln}, expected 4x{nbottom}"
                    )));
                }
                Some(ArrRef { off: loff, ty: PackedType::U32 })
            }
            None => None,
        };
        if lr.is_none() && !linear_lr {
            return Err(IndexError::Format(format!(
                "tree {name}: no lr table and not linear_lr"
            )));
        }

        let perm = match table("perm") {
            Some((poff, pw, pn)) => {
                if pn != ndata || pw != 4 {
                    return Err(IndexError::Format(format!("tree {name}: bad perm table")));
                }
                Some(ArrRef { off: poff, ty: PackedType::U32 })
            }
            None => None,
        };

        // Split-dim packing: low ceil(log2(ndim)) bits of the split value,
        // unless a separate splitdim array exists.
        let dim_bits = if splitdim.is_some() {
            0
        } else {
            (usize::BITS - (ndim - 1).leading_zeros()) as u64
        };
        let dim_mask = (1u64 << dim_bits) - 1;
        // Quantization slack: masked dim bits (+1 for split rounding).
        let split_eps = if int_ty.is_int() {
            (dim_mask as f64 + 2.0) * invscale
        } else {
            0.0
        };

        Ok(KdTree {
            name: name.to_string(),
            ndim,
            ndata,
            nnodes,
            nbottom,
            ninterior,
            data,
            split,
            splitdim,
            lr,
            perm,
            linear_lr,
            minval,
            maxval,
            scale,
            invscale,
            split_eps,
            dim_mask,
            swap,
        })
    }

    #[inline(always)]
    fn unpack(&self, packed: f64, dim: usize) -> f64 {
        packed * self.invscale + self.minval[dim]
    }

    /// Unpack data point `i` into `out[..ndim]`.
    #[inline]
    pub fn point(&self, base: &[u8], i: usize, out: &mut [f64]) {
        for d in 0..self.ndim {
            let raw = self.data.get(base, i * self.ndim + d, self.swap);
            out[d] = if self.data.ty.is_int() {
                self.unpack(raw, d)
            } else {
                raw
            };
        }
    }

    #[inline]
    fn leaf_range(&self, base: &[u8], node: usize) -> (usize, usize) {
        let leafid = node - self.ninterior;
        if self.linear_lr {
            let left = leafid * self.ndata / self.nbottom;
            let right_excl = (leafid + 1) * self.ndata / self.nbottom;
            (left, right_excl)
        } else {
            let lr = self.lr.as_ref().unwrap();
            let right = lr.get_int(base, leafid, self.swap) as usize;
            let left = if leafid == 0 {
                0
            } else {
                lr.get_int(base, leafid - 1, self.swap) as usize + 1
            };
            (left, (right + 1).min(self.ndata))
        }
    }

    #[inline]
    fn split_plane(&self, base: &[u8], node: usize) -> (usize, f64) {
        let split = self.split.as_ref().expect("interior node without splits");
        if let Some(sd) = &self.splitdim {
            let dim = base[sd.off + node] as usize;
            let raw = split.get(base, node, self.swap);
            let val = if split.ty.is_int() {
                self.unpack(raw, dim)
            } else {
                raw
            };
            (dim, val)
        } else if split.ty.is_int() {
            let raw = split.get_int(base, node, self.swap);
            let dim = (raw & self.dim_mask) as usize;
            let val = self.unpack((raw & !self.dim_mask) as f64, dim);
            (dim, val)
        } else {
            panic!("float splits require a splitdim array");
        }
    }

    /// All data points within Euclidean distance sqrt(r2) of `query`,
    /// appended to `out` as (data_index, dist2). Indices are mapped through
    /// the permutation array when present.
    pub fn range_search(
        &self,
        base: &[u8],
        query: &[f64],
        r2: f64,
        out: &mut Vec<(u32, f64)>,
    ) {
        debug_assert_eq!(query.len(), self.ndim);
        let r = r2.sqrt();
        let mut stack: Vec<u32> = Vec::with_capacity(64);
        stack.push(0);
        let mut pt = [0f64; 8];
        while let Some(node) = stack.pop() {
            let node = node as usize;
            if node >= self.ninterior {
                let (left, right_excl) = self.leaf_range(base, node);
                for i in left..right_excl {
                    self.point(base, i, &mut pt[..self.ndim]);
                    let mut d2 = 0.0;
                    for d in 0..self.ndim {
                        let diff = pt[d] - query[d];
                        d2 += diff * diff;
                        if d2 > r2 {
                            break;
                        }
                    }
                    if d2 <= r2 {
                        let idx = match &self.perm {
                            Some(p) => p.get_int(base, i, self.swap) as u32,
                            None => i as u32,
                        };
                        out.push((idx, d2));
                    }
                }
            } else {
                let (dim, val) = self.split_plane(base, node);
                let qd = query[dim];
                if qd - r <= val + self.split_eps {
                    stack.push((2 * node + 1) as u32);
                }
                if qd + r >= val - self.split_eps {
                    stack.push((2 * node + 2) as u32);
                }
            }
        }
    }

    /// Estimated resident bytes of this tree's arrays (for cache budgeting).
    pub fn byte_size(&self) -> usize {
        let mut sz = self.ndata * self.ndim * self.data.ty.size();
        if let Some(s) = &self.split {
            sz += self.ninterior * s.ty.size();
        }
        if self.lr.is_some() {
            sz += self.nbottom * 4;
        }
        if self.perm.is_some() {
            sz += self.ndata * 4;
        }
        if self.splitdim.is_some() {
            sz += self.ninterior;
        }
        sz
    }
}

/// A kd-tree coupled with the byte buffer it lives in.
#[derive(Clone, Copy)]
pub struct KdView<'a> {
    pub tree: &'a KdTree,
    pub base: &'a [u8],
}

impl<'a> KdView<'a> {
    #[inline]
    pub fn point(&self, i: usize, out: &mut [f64]) {
        self.tree.point(self.base, i, out)
    }
    pub fn range_search(&self, query: &[f64], r2: f64, out: &mut Vec<(u32, f64)>) {
        self.tree.range_search(self.base, query, r2, out)
    }
    pub fn ndata(&self) -> usize {
        self.tree.ndata
    }
    pub fn ndim(&self) -> usize {
        self.tree.ndim
    }
}

/// Whether the file's byte order differs from the host's.
/// The ENDIAN card stores the u32 0x01020304 in file byte order:
/// "04:03:02:01" = little-endian file.
pub(crate) fn endian_swap(card: Option<&str>) -> bool {
    let file_le = match card {
        Some("04:03:02:01") => true,
        Some("01:02:03:04") => false,
        // Missing/unknown: assume little-endian (every index published by
        // astrometry.net was written on x86).
        _ => true,
    };
    file_le != cfg!(target_endian = "little")
}
