//! Compact Position Reporting — CPR decode for ADS-B positions.
//!
//! Two decode modes:
//! - Global: requires even+odd frame pair within 10 seconds. No reference needed.
//! - Local: single frame + reference position within ~180nm.
//!
//! Key constants:
//! - NZ = 15 (latitude zones per hemisphere for even frames)
//! - Nb = 17 (bits per coordinate)
//! - Dlat_even = 360 / (4 * NZ) = 6.0 degrees
//! - Dlat_odd = 360 / (4 * NZ - 1) ≈ 6.1017 degrees

/// Number of latitude zones per hemisphere.
const NZ: f64 = 15.0;

/// Bits per CPR coordinate.
const NB: u32 = 17;

/// Maximum CPR value (2^17 = 131072).
const CPR_MAX: f64 = (1u32 << NB) as f64;

/// Maximum time between even/odd frames for global decode (seconds).
pub const MAX_PAIR_AGE: f64 = 10.0;

/// Number of longitude zones at a given latitude (NL function).
///
/// Returns the number of CPR longitude zones for the latitude.
/// Ranges from 1 near poles to 59 at equator.
pub fn nl(lat: f64) -> i32 {
    if lat.abs() >= 87.0 {
        return 1;
    }

    let a = 1.0 - (std::f64::consts::PI / (2.0 * NZ)).cos();
    let b = (std::f64::consts::PI / 180.0 * lat.abs()).cos().powi(2);
    let nl_val = (2.0 * std::f64::consts::PI / (1.0 - a / b).acos()).floor() as i32;
    nl_val.max(1)
}

/// Modulo that always returns a non-negative result.
fn modulo(x: f64, y: f64) -> f64 {
    x - y * (x / y).floor()
}

/// Global CPR decode from an even/odd frame pair.
///
/// Returns `(latitude, longitude)` in degrees, or `None` if decode fails
/// (e.g., zone boundary crossing or pair too old).
pub fn global_decode(
    lat_even: u32,
    lon_even: u32,
    lat_odd: u32,
    lon_odd: u32,
    t_even: f64,
    t_odd: f64,
) -> Option<(f64, f64)> {
    // Check time difference
    if (t_even - t_odd).abs() > MAX_PAIR_AGE {
        return None;
    }

    let dlat_even = 360.0 / (4.0 * NZ); // 6.0
    let dlat_odd = 360.0 / (4.0 * NZ - 1.0); // ~6.1017

    let lat_even_cpr = lat_even as f64 / CPR_MAX;
    let lon_even_cpr = lon_even as f64 / CPR_MAX;
    let lat_odd_cpr = lat_odd as f64 / CPR_MAX;
    let lon_odd_cpr = lon_odd as f64 / CPR_MAX;

    // Compute latitude zone index j
    let j = (59.0 * lat_even_cpr - 60.0 * lat_odd_cpr + 0.5).floor();

    // Compute candidate latitudes
    let mut lat_e = dlat_even * (modulo(j, 60.0) + lat_even_cpr);
    let mut lat_o = dlat_odd * (modulo(j, 59.0) + lat_odd_cpr);

    // Normalize to [-90, 90]
    if lat_e >= 270.0 {
        lat_e -= 360.0;
    }
    if lat_o >= 270.0 {
        lat_o -= 360.0;
    }

    // Check that both latitudes give the same NL value
    if nl(lat_e) != nl(lat_o) {
        return None; // Zone boundary crossing
    }

    let (lat, lon) = if t_even >= t_odd {
        // Use even frame
        let nl_val = nl(lat_e);
        let n_lon = nl_val.max(1);
        let dlon = 360.0 / n_lon as f64;
        let m = (lon_even_cpr * (nl_val - 1) as f64 - lon_odd_cpr * nl_val as f64 + 0.5).floor();
        let lon = dlon * (modulo(m, n_lon as f64) + lon_even_cpr);
        (lat_e, lon)
    } else {
        // Use odd frame
        let nl_val = nl(lat_o);
        let n_lon = (nl_val - 1).max(1);
        let dlon = 360.0 / n_lon as f64;
        let m = (lon_even_cpr * (nl_val - 1) as f64 - lon_odd_cpr * nl_val as f64 + 0.5).floor();
        let lon = dlon * (modulo(m, n_lon as f64) + lon_odd_cpr);
        (lat_o, lon)
    };

    // Normalize longitude to [-180, 180]
    let lon = if lon >= 180.0 { lon - 360.0 } else { lon };

    Some((round6(lat), round6(lon)))
}

/// Local CPR decode using a reference position.
///
/// Valid when the aircraft is within ~180nm of the reference.
pub fn local_decode(
    cpr_lat: u32,
    cpr_lon: u32,
    cpr_odd: bool,
    ref_lat: f64,
    ref_lon: f64,
) -> (f64, f64) {
    let i = if cpr_odd { 1.0 } else { 0.0 };
    let dlat = 360.0 / (4.0 * NZ - i);

    let cpr_lat_norm = cpr_lat as f64 / CPR_MAX;
    let cpr_lon_norm = cpr_lon as f64 / CPR_MAX;

    // Compute latitude zone index from reference
    let j = (ref_lat / dlat).floor()
        + (modulo(ref_lat, dlat) / dlat - cpr_lat_norm + 0.5).floor();
    let lat = dlat * (j + cpr_lat_norm);

    // Compute longitude zone size at this latitude
    let nl_val = nl(lat);
    let n_lon = (nl_val - i as i32).max(1);
    let dlon = 360.0 / n_lon as f64;

    // Compute longitude zone index from reference
    let m = (ref_lon / dlon).floor()
        + (modulo(ref_lon, dlon) / dlon - cpr_lon_norm + 0.5).floor();
    let mut lon = dlon * (m + cpr_lon_norm);

    // Normalize
    let mut lat = lat;
    if lat > 90.0 {
        lat -= 360.0;
    }
    if lon >= 180.0 {
        lon -= 360.0;
    }

    (round6(lat), round6(lon))
}

/// Round to 6 decimal places (matching Python's behavior).
fn round6(val: f64) -> f64 {
    (val * 1_000_000.0).round() / 1_000_000.0
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_nl_equator() {
        assert_eq!(nl(0.0), 59);
    }

    #[test]
    fn test_nl_poles() {
        assert_eq!(nl(87.0), 1);
        assert_eq!(nl(-87.0), 1);
        assert_eq!(nl(90.0), 1);
    }

    #[test]
    fn test_nl_mid_latitude() {
        // ~52° N (London area) should give NL around 36
        let n = nl(52.0);
        assert!(n > 30 && n < 40, "NL at 52° should be ~36, got {n}");
    }

    #[test]
    fn test_global_decode_known_pair() {
        // Test vectors from "The 1090MHz Riddle"
        // Even frame: cpr_lat=93000, cpr_lon=51372
        // Odd frame: cpr_lat=74158, cpr_lon=50194
        // Expected: lat≈52.2572, lon≈3.9194
        let result = global_decode(93000, 51372, 74158, 50194, 1.0, 0.0);
        assert!(result.is_some(), "Global decode should succeed");

        let (lat, lon) = result.unwrap();
        assert!(
            (lat - 52.2572).abs() < 0.01,
            "Latitude should be ~52.2572, got {lat}"
        );
        assert!(
            (lon - 3.9194).abs() < 0.01,
            "Longitude should be ~3.9194, got {lon}"
        );
    }

    #[test]
    fn test_global_decode_pair_too_old() {
        // Pair older than 10 seconds should fail
        let result = global_decode(93000, 51372, 74158, 50194, 11.0, 0.0);
        assert!(result.is_none());
    }

    #[test]
    fn test_local_decode() {
        // Use decoded position as reference, decode even frame locally
        let (lat, lon) = local_decode(93000, 51372, false, 52.25, 3.92);
        assert!(
            (lat - 52.2572).abs() < 0.01,
            "Local lat should be ~52.2572, got {lat}"
        );
        assert!(
            (lon - 3.9194).abs() < 0.01,
            "Local lon should be ~3.9194, got {lon}"
        );
    }

    #[test]
    fn test_local_decode_odd() {
        // Local decode accuracy depends on reference proximity.
        // With ref (52.25, 3.92), odd frame should decode near the actual position.
        let (lat, lon) = local_decode(74158, 50194, true, 52.25, 3.92);
        assert!(
            (lat - 52.2572).abs() < 0.05,
            "Local odd lat should be ~52.2572, got {lat}"
        );
        assert!(
            (lon - 3.92).abs() < 0.05,
            "Local odd lon should be ~3.92, got {lon}"
        );
    }

    #[test]
    fn test_modulo_positive() {
        assert!((modulo(7.0, 3.0) - 1.0).abs() < 1e-10);
    }

    #[test]
    fn test_modulo_negative() {
        // modulo(-1, 60) should return 59
        assert!((modulo(-1.0, 60.0) - 59.0).abs() < 1e-10);
    }
}
