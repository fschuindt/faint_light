//! Field-quad geometric hash codes, matching astrometry.net's conventions
//! exactly (solver.c try_all_codes / try_all_codes_2 / try_permutations).
//!
//! Stars A,B define a local frame mapping A→(0,0), B→(1,1). Remaining stars
//! map to (cx,cy),(dx,dy),... — the code. Index invariants (all published
//! series): stars C,D,… lie in the circle centered (0.5,0.5) with radius
//! 1/√2; codes are stored with cx≤dx≤… and mean(x)≤1/2 (enforced by
//! swapping C/D order and A/B labels respectively). At solve time we
//! enumerate the orderings that could satisfy the invariants within a
//! tolerance margin, for both parities (a parity flip swaps x↔y).

pub const DQMAX: usize = 5;
pub const DCMAX: usize = 2 * (DQMAX - 2);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Parity {
    Normal,
    Flip,
    Both,
}

impl Parity {
    pub fn from_nova(v: i64) -> Parity {
        match v {
            0 => Parity::Normal,
            1 => Parity::Flip,
            _ => Parity::Both,
        }
    }
}

/// The A,B "backbone" of a candidate quad: the code-space transform.
#[derive(Debug, Clone)]
pub struct PQuad {
    pub a: usize,
    pub b: usize,
    /// |AB|^2 in pixels^2.
    pub scale2: f64,
    ax: f64,
    ay: f64,
    costheta: f64,
    sintheta: f64,
    /// Field star indices accepted as C/D members (inside the circle).
    pub inbox: Vec<usize>,
    /// Code-space coordinates of the inbox members (parallel array).
    pub inbox_xy: Vec<(f64, f64)>,
}

impl PQuad {
    pub fn new(a: usize, b: usize, axy: (f64, f64), bxy: (f64, f64)) -> PQuad {
        let dx = bxy.0 - axy.0;
        let dy = bxy.1 - axy.1;
        let scale2 = dx * dx + dy * dy;
        PQuad {
            a,
            b,
            scale2,
            ax: axy.0,
            ay: axy.1,
            costheta: (dy + dx) / scale2,
            sintheta: (dy - dx) / scale2,
            inbox: Vec::new(),
            inbox_xy: Vec::new(),
        }
    }

    /// Transform a field pixel position into code space.
    #[inline]
    pub fn code_xy(&self, xy: (f64, f64)) -> (f64, f64) {
        let cx = xy.0 - self.ax;
        let cy = xy.1 - self.ay;
        (
            cx * self.costheta + cy * self.sintheta,
            -cx * self.sintheta + cy * self.costheta,
        )
    }

    /// Circle membership test with codetol fudge (solver.c check_inbox):
    /// (x-1/2)^2 + (y-1/2)^2 <= (1/sqrt(2) + tol)^2.
    #[inline]
    pub fn in_circle(cxy: (f64, f64), codetol: f64) -> bool {
        let (x, y) = cxy;
        let r = (x * x - x) + (y * y - y);
        r <= codetol * (std::f64::consts::SQRT_2 + codetol)
    }

    /// Try to add field star `idx` at pixel `xy` as a C/D member.
    pub fn add_member(&mut self, idx: usize, xy: (f64, f64), codetol: f64) -> bool {
        let c = self.code_xy(xy);
        if Self::in_circle(c, codetol) {
            self.inbox.push(idx);
            self.inbox_xy.push(c);
            true
        } else {
            false
        }
    }
}

/// Invariant flags of the index being searched.
#[derive(Debug, Clone, Copy)]
pub struct CodeInvariants {
    pub cx_le_dx: bool,
    pub meanx_le_half: bool,
}

/// Enumerate all candidate (code, star ordering) pairs for one member combo,
/// mirroring try_all_codes: both parities (unless pinned), both A/B
/// orderings, and all member permutations surviving the invariant margins.
///
/// `members`: the dimquad-2 chosen field star indices; `mxy` their code
/// coords under `pq`'s transform. The callback receives the full star
/// ordering [A,B,C,D,...] (field indices), the code, and the parity flag of
/// the candidate; returning `true` stops enumeration.
#[allow(clippy::too_many_arguments)]
pub fn for_each_code(
    pq: &PQuad,
    members: &[usize],
    mxy: &[(f64, f64)],
    inv: CodeInvariants,
    codetol: f64,
    parity: Parity,
    cb: &mut impl FnMut(&[usize], &[f64], bool) -> bool,
) -> bool {
    let nm = members.len();
    debug_assert!((1..=DQMAX - 2).contains(&nm));
    let margin = 1.5 * codetol;

    let mut origcode = [0f64; DCMAX];
    for parity_flip in [false, true] {
        match parity {
            Parity::Normal if parity_flip => continue,
            Parity::Flip if !parity_flip => continue,
            _ => {}
        }
        for (i, &(x, y)) in mxy.iter().enumerate().take(nm) {
            if parity_flip {
                origcode[2 * i] = y;
                origcode[2 * i + 1] = x;
            } else {
                origcode[2 * i] = x;
                origcode[2 * i + 1] = y;
            }
        }
        // A,B and B,A (complemented code) orderings.
        for ab_swap in [false, true] {
            let mut code = [0f64; DCMAX];
            for i in 0..2 * nm {
                code[i] = if ab_swap {
                    1.0 - origcode[i]
                } else {
                    origcode[i]
                };
            }
            let (sa, sb) = if ab_swap { (pq.b, pq.a) } else { (pq.a, pq.b) };
            let mut stars = [0usize; DQMAX];
            stars[0] = sa;
            stars[1] = sb;
            let mut placed = [false; DQMAX];
            let mut out = [0f64; DCMAX];
            if permute(
                members,
                &code[..2 * nm],
                inv,
                margin,
                &mut stars,
                &mut out,
                0,
                nm,
                &mut placed,
                parity_flip,
                cb,
            ) {
                return true;
            }
        }
    }
    false
}

#[allow(clippy::too_many_arguments)]
fn permute(
    members: &[usize],
    origcode: &[f64],
    inv: CodeInvariants,
    margin: f64,
    stars: &mut [usize; DQMAX],
    code: &mut [f64; DCMAX],
    slot: usize,
    nm: usize,
    placed: &mut [bool; DQMAX],
    parity_flip: bool,
    cb: &mut impl FnMut(&[usize], &[f64], bool) -> bool,
) -> bool {
    for i in 0..nm {
        if placed[i] {
            continue;
        }
        // cx <= dx invariant between consecutive slots.
        if slot > 0 && inv.cx_le_dx && code[2 * (slot - 1)] > origcode[2 * i] + margin {
            continue;
        }
        stars[slot + 2] = members[i];
        code[2 * slot] = origcode[2 * i];
        code[2 * slot + 1] = origcode[2 * i + 1];

        // mean(x) <= 1/2 invariant over filled slots.
        if inv.cx_le_dx && inv.meanx_le_half {
            let meanx: f64 =
                (0..=slot).map(|j| code[2 * j]).sum::<f64>() / (slot + 1) as f64;
            if meanx > 0.5 + margin {
                continue;
            }
        }

        if slot + 1 < nm {
            placed[i] = true;
            if permute(
                members, origcode, inv, margin, stars, code, slot + 1, nm, placed,
                parity_flip, cb,
            ) {
                placed[i] = false;
                return true;
            }
            placed[i] = false;
        } else if cb(&stars[..nm + 2], &code[..2 * nm], parity_flip) {
            return true;
        }
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ab_maps_to_unit() {
        let pq = PQuad::new(0, 1, (10.0, 20.0), (110.0, 90.0));
        let a = pq.code_xy((10.0, 20.0));
        let b = pq.code_xy((110.0, 90.0));
        assert!(a.0.abs() < 1e-12 && a.1.abs() < 1e-12);
        assert!((b.0 - 1.0).abs() < 1e-12 && (b.1 - 1.0).abs() < 1e-12);
    }

    #[test]
    fn enumeration_covers_canonical_form() {
        // A random C,D pair: exactly one (parity, ab, perm) candidate must
        // satisfy strict invariants; enumeration must produce it among its
        // candidates.
        let pq = PQuad::new(0, 1, (0.0, 0.0), (100.0, 100.0));
        let c = (30.0, 60.0);
        let d = (70.0, 20.0);
        let inv = CodeInvariants { cx_le_dx: true, meanx_le_half: true };
        let mut found_canonical = 0;
        let mut cands = 0;
        for_each_code(
            &pq,
            &[2, 3],
            &[pq.code_xy(c), pq.code_xy(d)],
            inv,
            0.01,
            Parity::Both,
            &mut |_stars, code, _flip| {
                cands += 1;
                let meanx = (code[0] + code[2]) / 2.0;
                if code[0] <= code[2] && meanx <= 0.5 {
                    found_canonical += 1;
                }
                false
            },
        );
        assert!(cands >= 2, "expected multiple candidates, got {cands}");
        assert!(found_canonical >= 2, "canonical form missing (one per parity)");
    }
}
