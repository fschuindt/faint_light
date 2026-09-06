//! SVG all-sky chart: azimuthal-equidistant projection with the zenith at
//! the center and the horizon as the outer circle. North is up, East is
//! left (the natural orientation when looking up at the sky).

use crate::altaz::{format_utc, lst_deg, radec_to_altaz};

const SIZE: f64 = 1000.0;
const CX: f64 = SIZE / 2.0;
const CY: f64 = SIZE / 2.0;
/// Horizon radius; the margin holds the cardinal letters.
const R: f64 = 430.0;

/// Solved field-of-view footprint to overlay, as the RA/Dec (degrees) of
/// the image corners in border order.
#[derive(Debug, Clone)]
pub struct Fov {
    pub corners: [(f64, f64); 4],
    /// Short annotation drawn next to the footprint, e.g. `2.1° × 1.4°`.
    pub label: String,
}

#[derive(Debug, Clone)]
pub struct ChartParams {
    /// Exposure time, Unix seconds (UTC).
    pub unix: f64,
    /// Observer latitude, degrees north-positive.
    pub lat_deg: f64,
    /// Observer longitude, degrees east-positive.
    pub lon_deg: f64,
    pub fov: Option<Fov>,
}

/// Project (alt, az) degrees onto the chart. r grows linearly with zenith
/// distance, so alt < 0 lands outside the horizon circle (clipped later).
fn project(alt: f64, az: f64) -> (f64, f64) {
    let r = R * (90.0 - alt) / 90.0;
    let a = az.to_radians();
    (CX - r * a.sin(), CY - r * a.cos())
}

/// Clip segment p1–p2 to the horizon disc. Returns the visible sub-segment.
fn clip_to_horizon(p1: (f64, f64), p2: (f64, f64)) -> Option<((f64, f64), (f64, f64))> {
    let (dx, dy) = (p2.0 - p1.0, p2.1 - p1.1);
    let (fx, fy) = (p1.0 - CX, p1.1 - CY);
    let a = dx * dx + dy * dy;
    let b = 2.0 * (fx * dx + fy * dy);
    let c = fx * fx + fy * fy - R * R;
    if a == 0.0 {
        return (c <= 0.0).then_some((p1, p2));
    }
    // Inside interval of the infinite line, intersected with [0, 1].
    let (mut t0, mut t1) = (0.0f64, 1.0f64);
    let disc = b * b - 4.0 * a * c;
    if c > 0.0 || (p2.0 - CX).powi(2) + (p2.1 - CY).powi(2) > R * R {
        if disc <= 0.0 {
            return (c <= 0.0).then_some((p1, p2));
        }
        let sq = disc.sqrt();
        t0 = t0.max((-b - sq) / (2.0 * a));
        t1 = t1.min((-b + sq) / (2.0 * a));
        if t0 >= t1 {
            return None;
        }
    }
    let at = |t: f64| (p1.0 + t * dx, p1.1 + t * dy);
    Some((at(t0), at(t1)))
}

fn esc(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;")
}

fn fmt(v: f64) -> String {
    format!("{v:.1}")
}

/// Dot radius (px) for a star of visual magnitude `mag`: brighter → bigger.
/// Linear in magnitude reads better on a stick-figure chart than true flux
/// scaling, which would make anything past mag 3 invisible.
fn star_radius(mag: f64) -> f64 {
    (3.4 - 0.42 * mag).clamp(1.0, 4.4)
}

/// Render the chart. Pure function of its parameters; returns an SVG
/// document string.
pub fn render_svg(p: &ChartParams) -> String {
    let lst = lst_deg(p.unix, p.lon_deg);
    let altaz = |ra: f64, dec: f64| radec_to_altaz(ra, dec, p.lat_deg, lst);
    let mut s = String::with_capacity(64 * 1024);
    s.push_str(&format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{SIZE}" height="{SIZE}" viewBox="0 0 {SIZE} {SIZE}" font-family="system-ui, sans-serif">
<rect width="{SIZE}" height="{SIZE}" fill="#070b14"/>
<circle cx="{CX}" cy="{CY}" r="{R}" fill="#0d1526"/>
"##
    ));

    // --- Alt/az grid (low opacity) ---
    s.push_str(r##"<g stroke="#7f9bc4" stroke-opacity="0.18" fill="none">"##);
    s.push('\n');
    for alt in [30.0, 60.0] {
        let r = R * (90.0 - alt) / 90.0;
        s.push_str(&format!(r#"<circle cx="{CX}" cy="{CY}" r="{}"/>"#, fmt(r)));
        s.push('\n');
    }
    for az in (0..360).step_by(30) {
        // Spokes stop short of the zenith to avoid a cluttered center.
        let (x1, y1) = project(0.0, az as f64);
        let (x2, y2) = project(80.0, az as f64);
        s.push_str(&format!(
            r#"<line x1="{}" y1="{}" x2="{}" y2="{}"/>"#,
            fmt(x1),
            fmt(y1),
            fmt(x2),
            fmt(y2)
        ));
        s.push('\n');
    }
    s.push_str("</g>\n");
    // Altitude ring labels along the NNE spoke.
    s.push_str(r##"<g fill="#7f9bc4" fill-opacity="0.45" font-size="11">"##);
    for alt in [30.0, 60.0] {
        let (x, y) = project(alt, 15.0);
        s.push_str(&format!(r#"<text x="{}" y="{}">{alt}&#176;</text>"#, fmt(x), fmt(y)));
    }
    s.push_str("</g>\n");
    // Horizon rim and zenith marker.
    s.push_str(&format!(
        r##"<circle cx="{CX}" cy="{CY}" r="{R}" fill="none" stroke="#a8c0e8" stroke-opacity="0.55" stroke-width="1.5"/>
<path d="M {x1} {CY} h 10 M {CX} {y1} v 10" stroke="#a8c0e8" stroke-opacity="0.5"/>
"##,
        x1 = CX - 5.0,
        y1 = CY - 5.0,
    ));
    // Cardinal letters (East is left when looking up).
    for (az, label) in [(0.0f64, "N"), (90.0, "E"), (180.0, "S"), (270.0, "W")] {
        let r = R + 22.0;
        let a = az.to_radians();
        let (x, y) = (CX - r * a.sin(), CY - r * a.cos());
        s.push_str(&format!(
            r##"<text x="{}" y="{}" fill="#c7d6ef" font-size="18" text-anchor="middle" dominant-baseline="middle">{label}</text>"##,
            fmt(x),
            fmt(y)
        ));
        s.push('\n');
    }

    // --- Constellations ---
    let mut lines = String::new();
    let mut stars = String::new();
    let mut labels = String::new();
    let mut label_count = 0usize;
    for con in crate::constellations() {
        let mut any_visible = false;
        for poly in &con.polylines {
            let pts: Vec<(f64, f64)> = poly.iter().map(|s| altaz(s.ra, s.dec)).collect();
            for w in pts.windows(2) {
                let (a1, _) = w[0];
                let (a2, _) = w[1];
                // Need one endpoint up; skip segments plunging far below the
                // horizon, where the projection stretches badly.
                if a1.max(a2) <= 0.0 || a1.min(a2) < -35.0 {
                    continue;
                }
                let (p1, p2) = (project(w[0].0, w[0].1), project(w[1].0, w[1].1));
                if let Some((q1, q2)) = clip_to_horizon(p1, p2) {
                    any_visible = true;
                    lines.push_str(&format!(
                        r#"<line x1="{}" y1="{}" x2="{}" y2="{}"/>"#,
                        fmt(q1.0),
                        fmt(q1.1),
                        fmt(q2.0),
                        fmt(q2.1)
                    ));
                    lines.push('\n');
                }
            }
            for (star, &(alt, az)) in poly.iter().zip(&pts) {
                if alt > 0.0 {
                    let (x, y) = project(alt, az);
                    stars.push_str(&format!(
                        r#"<circle cx="{}" cy="{}" r="{}"/>"#,
                        fmt(x),
                        fmt(y),
                        fmt(star_radius(star.mag))
                    ));
                    stars.push('\n');
                }
            }
        }
        let (lalt, laz) = altaz(con.label.0, con.label.1);
        if any_visible && lalt > 3.0 {
            let (x, y) = project(lalt, laz);
            labels.push_str(&format!(
                r#"<text x="{}" y="{}">{}</text>"#,
                fmt(x),
                fmt(y),
                esc(con.name)
            ));
            labels.push('\n');
            label_count += 1;
        }
    }
    let _ = label_count;
    s.push_str(&format!(
        r##"<g stroke="#5d86c9" stroke-opacity="0.75" stroke-width="1.1" stroke-linecap="round">
{lines}</g>
<g fill="#ffffff" fill-opacity="0.9">
{stars}</g>
<g fill="#93aed6" fill-opacity="0.85" font-size="13" text-anchor="middle">
{labels}</g>
"##
    ));

    // --- Field-of-view footprint ---
    if let Some(fov) = &p.fov {
        let pts: Vec<((f64, f64), (f64, f64))> = fov
            .corners
            .iter()
            .map(|&(ra, dec)| {
                let (alt, az) = altaz(ra, dec);
                ((alt, az), project(alt, az))
            })
            .collect();
        let mut fov_svg = String::new();
        let mut top: Option<(f64, f64)> = None;
        for i in 0..4 {
            let (aa1, p1) = pts[i];
            let (aa2, p2) = pts[(i + 1) % 4];
            if aa1.0.max(aa2.0) <= 0.0 {
                continue;
            }
            if let Some((q1, q2)) = clip_to_horizon(p1, p2) {
                fov_svg.push_str(&format!(
                    r#"<line x1="{}" y1="{}" x2="{}" y2="{}"/>"#,
                    fmt(q1.0),
                    fmt(q1.1),
                    fmt(q2.0),
                    fmt(q2.1)
                ));
                fov_svg.push('\n');
                for q in [q1, q2] {
                    if top.is_none_or(|t| q.1 < t.1) {
                        top = Some(q);
                    }
                }
            }
        }
        if !fov_svg.is_empty() {
            s.push_str(&format!(
                r##"<g class="fov" stroke="#ff4d4d" stroke-width="2" fill="none">
{fov_svg}</g>
"##
            ));
            if let Some((x, y)) = top {
                s.push_str(&format!(
                    r##"<text x="{}" y="{}" fill="#ff6b6b" font-size="13" text-anchor="middle">{}</text>"##,
                    fmt(x),
                    fmt(y - 10.0),
                    esc(&fov.label)
                ));
                s.push('\n');
            }
        }
    }

    // --- Caption ---
    s.push_str(&format!(
        r##"<text x="14" y="{}" fill="#6d82a8" font-size="13">{} UTC &#183; lat {:.4}&#176; lon {:.4}&#176; &#183; zenith-centered, horizon at rim</text>
</svg>
"##,
        SIZE - 14.0,
        format_utc(p.unix),
        p.lat_deg,
        p.lon_deg,
    ));
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_visible_sky() {
        // Mid-northern site: a good half of the constellations are up.
        let svg = render_svg(&ChartParams {
            unix: 1_783_945_845.0, // 2026-07-13 12:30:45 UTC
            lat_deg: 40.0,
            lon_deg: -74.0,
            fov: None,
        });
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("Ursa Major"), "circumpolar figure missing");
        let labels = svg.matches("<text").count();
        assert!(labels > 20, "only {labels} labels");
        assert!(!svg.contains("class=\"fov\""));
    }

    #[test]
    fn fov_at_zenith_is_drawn() {
        // Pick RA = LST, Dec = lat so the footprint sits at the zenith.
        let unix = 1_783_945_845.0;
        let (lat, lon) = (40.0, -74.0);
        let ra = crate::altaz::lst_deg(unix, lon);
        let quad = [
            ((ra - 1.0).rem_euclid(360.0), lat - 1.0),
            ((ra + 1.0).rem_euclid(360.0), lat - 1.0),
            ((ra + 1.0).rem_euclid(360.0), lat + 1.0),
            ((ra - 1.0).rem_euclid(360.0), lat + 1.0),
        ];
        let svg = render_svg(&ChartParams {
            unix,
            lat_deg: lat,
            lon_deg: lon,
            fov: Some(Fov {
                corners: quad,
                label: "2.0° × 2.0°".into(),
            }),
        });
        assert!(svg.contains("class=\"fov\""));
        assert!(svg.contains("2.0° × 2.0°"));
    }

    #[test]
    fn fov_below_horizon_is_omitted() {
        let unix = 1_783_945_845.0;
        let (lat, lon) = (40.0, -74.0);
        // Antipode of the zenith: definitely below the horizon.
        let ra = (crate::altaz::lst_deg(unix, lon) + 180.0).rem_euclid(360.0);
        let quad = [
            (ra - 1.0, -lat - 1.0),
            (ra + 1.0, -lat - 1.0),
            (ra + 1.0, -lat + 1.0),
            (ra - 1.0, -lat + 1.0),
        ];
        let svg = render_svg(&ChartParams {
            unix,
            lat_deg: lat,
            lon_deg: lon,
            fov: Some(Fov {
                corners: quad,
                label: "hidden".into(),
            }),
        });
        assert!(!svg.contains("class=\"fov\""));
        assert!(!svg.contains("hidden"));
    }

    #[test]
    fn star_radius_scales_with_brightness() {
        // Brighter (smaller mag) → bigger dot, with clamps at both ends.
        assert!(star_radius(-1.44) > star_radius(0.0));
        assert!(star_radius(0.0) > star_radius(2.0));
        assert!(star_radius(2.0) > star_radius(4.0));
        assert_eq!(star_radius(-9.0), 4.4);
        assert_eq!(star_radius(9.0), 1.0);
    }

    #[test]
    fn horizon_clipping() {
        // Fully inside.
        let inside = clip_to_horizon((CX - 10.0, CY), (CX + 10.0, CY)).unwrap();
        assert_eq!(inside, ((CX - 10.0, CY), (CX + 10.0, CY)));
        // Crossing the rim: clipped endpoint lands on the circle.
        let (_, q2) = clip_to_horizon((CX, CY), (CX + 2.0 * R, CY)).unwrap();
        assert!((q2.0 - (CX + R)).abs() < 1e-9);
        // Fully outside, pointing away.
        assert!(clip_to_horizon((CX + R + 10.0, CY), (CX + R + 20.0, CY)).is_none());
    }
}
