//! Time and horizontal-coordinate transforms: Unix time → sidereal time,
//! equatorial (RA/Dec) → horizontal (Alt/Az) for a ground observer.
//!
//! Accuracy target is chart-drawing (arcminutes), so UT1≈UTC, no nutation,
//! no refraction, J2000 catalog positions used as-is.

/// Unix seconds of the J2000.0 epoch (2000-01-01 12:00:00 UTC).
const J2000_UNIX: f64 = 946_728_000.0;

/// Greenwich mean sidereal time in degrees, [0, 360).
pub fn gmst_deg(unix: f64) -> f64 {
    let d = (unix - J2000_UNIX) / 86_400.0;
    (280.460_618_37 + 360.985_647_366_29 * d).rem_euclid(360.0)
}

/// Local sidereal time in degrees for an east-positive longitude.
pub fn lst_deg(unix: f64, lon_east_deg: f64) -> f64 {
    (gmst_deg(unix) + lon_east_deg).rem_euclid(360.0)
}

/// RA/Dec (degrees) → (altitude, azimuth) in degrees for an observer at
/// `lat_deg` with local sidereal time `lst_deg`. Azimuth is measured from
/// North through East, [0, 360).
pub fn radec_to_altaz(ra_deg: f64, dec_deg: f64, lat_deg: f64, lst_deg: f64) -> (f64, f64) {
    let h = (lst_deg - ra_deg).to_radians();
    let dec = dec_deg.to_radians();
    let lat = lat_deg.to_radians();
    let sin_alt = lat.sin() * dec.sin() + lat.cos() * dec.cos() * h.cos();
    let alt = sin_alt.clamp(-1.0, 1.0).asin();
    let az = (-dec.cos() * h.sin())
        .atan2(dec.sin() * lat.cos() - dec.cos() * lat.sin() * h.cos())
        .to_degrees()
        .rem_euclid(360.0);
    (alt.to_degrees(), az)
}

/// Parse a picture timestamp: Unix seconds ("1783945845", fractional ok) or
/// UTC civil time "YYYY-MM-DDTHH:MM:SS[.fff][Z]" (a space also separates
/// date and time). Returns Unix seconds.
pub fn parse_timestamp(s: &str) -> Option<f64> {
    let s = s.trim();
    if let Ok(v) = s.parse::<f64>() {
        return v.is_finite().then_some(v);
    }
    let s = s.strip_suffix('Z').or_else(|| s.strip_suffix("UTC")).unwrap_or(s).trim();
    let (date, time) = s.split_once(['T', ' '])?;
    let mut d = date.split('-');
    let (y, m, day): (i64, i64, i64) = (
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
        d.next()?.parse().ok()?,
    );
    if d.next().is_some() || !(1..=12).contains(&m) || !(1..=31).contains(&day) {
        return None;
    }
    let mut t = time.split(':');
    let (hh, mm): (i64, i64) = (t.next()?.parse().ok()?, t.next()?.parse().ok()?);
    let ss: f64 = t.next().map_or(Some(0.0), |v| v.parse().ok())?;
    if t.next().is_some() || !(0..24).contains(&hh) || !(0..60).contains(&mm) || !(0.0..60.0).contains(&ss) {
        return None;
    }
    Some(days_from_civil(y, m, day) as f64 * 86_400.0 + (hh * 3600 + mm * 60) as f64 + ss)
}

/// Unix seconds → "YYYY-MM-DD HH:MM:SS" UTC (Howard Hinnant's
/// civil_from_days).
pub fn format_utc(unix: f64) -> String {
    let secs = unix.floor() as i64;
    let (days, rem) = (secs.div_euclid(86_400), secs.rem_euclid(86_400));
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = z.rem_euclid(146_097);
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let mm = if mp < 10 { mp + 3 } else { mp - 9 };
    let yy = if mm <= 2 { y + 1 } else { y };
    format!("{yy:04}-{mm:02}-{d:02} {h:02}:{m:02}:{s:02}")
}

/// Days since 1970-01-01 (Howard Hinnant's days_from_civil).
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = y.div_euclid(400);
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn gmst_at_j2000() {
        assert!((gmst_deg(J2000_UNIX) - 280.460_618_37).abs() < 1e-6);
    }

    #[test]
    fn polaris_altitude_tracks_latitude() {
        // Polaris (J2000 RA 37.95°, Dec +89.26°) sits within a degree of the
        // pole, so its altitude ≈ observer latitude at any time.
        for (lat, unix) in [(45.0, 1_783_945_845.0), (10.0, 1_600_000_000.0)] {
            let (alt, _) = radec_to_altaz(37.95, 89.26, lat, lst_deg(unix, -74.0));
            assert!((alt - lat).abs() < 1.0, "alt {alt} vs lat {lat}");
        }
    }

    #[test]
    fn zenith_and_meridian() {
        // A star at RA = LST, Dec = latitude is at the zenith.
        let (alt, _) = radec_to_altaz(100.0, 40.0, 40.0, 100.0);
        assert!(alt > 89.99);
        // On the meridian south of the zenith → azimuth 180.
        let (alt, az) = radec_to_altaz(100.0, 20.0, 40.0, 100.0);
        assert!((alt - 70.0).abs() < 1e-9);
        assert!((az - 180.0).abs() < 1e-9);
        // North of the zenith (over the pole) → azimuth 0.
        let (_, az) = radec_to_altaz(100.0, 80.0, 40.0, 100.0);
        assert!(az < 1e-9 || az > 360.0 - 1e-9);
    }

    #[test]
    fn timestamp_parsing() {
        assert_eq!(parse_timestamp("0"), Some(0.0));
        assert_eq!(parse_timestamp("1783945845"), Some(1_783_945_845.0));
        assert_eq!(parse_timestamp("1970-01-01T00:00:00Z"), Some(0.0));
        assert_eq!(parse_timestamp("2000-01-01T12:00:00Z"), Some(J2000_UNIX));
        assert_eq!(
            parse_timestamp("2026-07-13T12:30:45Z"),
            Some(1_783_945_845.0)
        );
        assert_eq!(
            parse_timestamp("2026-07-13 12:30:45.5"),
            Some(1_783_945_845.5)
        );
        assert_eq!(parse_timestamp("not a time"), None);
        assert_eq!(parse_timestamp("2026-13-01T00:00:00Z"), None);
    }

    #[test]
    fn utc_formatting_roundtrips_parse() {
        for unix in [0.0, 946_728_000.0, 1_783_945_845.0] {
            let text = format_utc(unix);
            assert_eq!(parse_timestamp(&text), Some(unix), "{text}");
        }
    }
}
