//! Aircraft type enrichment — classify from observed ADS-B data.
//!
//! Infers aircraft category from speed, altitude, callsign, and ICAO address.
//! No external database required — works purely from observed data.

use crate::filter::haversine_nm;

// ---------------------------------------------------------------------------
// Aircraft categories
// ---------------------------------------------------------------------------

pub const CAT_JET: &str = "jet";
pub const CAT_PROP: &str = "prop";
pub const CAT_TURBOPROP: &str = "turboprop";
pub const CAT_HELICOPTER: &str = "helicopter";
pub const CAT_MILITARY: &str = "military";
pub const CAT_CARGO: &str = "cargo";
pub const CAT_UNKNOWN: &str = "unknown";

// ---------------------------------------------------------------------------
// Airline lookup
// ---------------------------------------------------------------------------

/// Airline ICAO prefixes → operator name.
const AIRLINE_PREFIXES: &[(&str, &str)] = &[
    ("AAL", "American Airlines"),
    ("DAL", "Delta Air Lines"),
    ("UAL", "United Airlines"),
    ("SWA", "Southwest Airlines"),
    ("JBU", "JetBlue Airways"),
    ("NKS", "Spirit Airlines"),
    ("FFT", "Frontier Airlines"),
    ("ASA", "Alaska Airlines"),
    ("HAL", "Hawaiian Airlines"),
    ("SKW", "SkyWest Airlines"),
    ("RPA", "Republic Airways"),
    ("ENY", "Envoy Air"),
    ("ASH", "Mesa Airlines"),
    ("PDT", "Piedmont Airlines"),
    ("JIA", "PSA Airlines"),
    ("UPS", "UPS"),
    ("FDX", "FedEx"),
    ("GTI", "Atlas Air"),
    ("ABX", "ABX Air"),
    ("ACA", "Air Canada"),
    ("WJA", "WestJet"),
    ("BAW", "British Airways"),
    ("DLH", "Lufthansa"),
    ("AFR", "Air France"),
    ("EZY", "easyJet"),
    ("RYR", "Ryanair"),
];

const CARGO_PREFIXES: &[&str] = &["UPS", "FDX", "GTI", "ABX", "CLX", "GEC", "CKS", "BOX"];

/// Look up operator name from callsign prefix.
pub fn lookup_operator(callsign: &str) -> Option<&'static str> {
    if callsign.len() < 3 {
        return None;
    }
    let prefix = &callsign[..3].to_ascii_uppercase();
    AIRLINE_PREFIXES
        .iter()
        .find(|(p, _)| *p == prefix.as_str())
        .map(|(_, name)| *name)
}

/// Classify aircraft category from observed flight profile.
pub fn classify_from_profile(
    speed_kts: Option<f64>,
    altitude_ft: Option<i32>,
    is_military: bool,
    callsign: Option<&str>,
) -> &'static str {
    if is_military {
        return CAT_MILITARY;
    }

    // Check callsign for cargo operators
    if let Some(cs) = callsign {
        if cs.len() >= 3 {
            let prefix = cs[..3].to_ascii_uppercase();
            if CARGO_PREFIXES.contains(&prefix.as_str()) {
                return CAT_CARGO;
            }
        }
    }

    // Speed-based classification
    if let Some(speed) = speed_kts {
        if speed > 250.0 {
            return CAT_JET;
        }
        if speed < 80.0 {
            if let Some(alt) = altitude_ft {
                if alt < 3000 {
                    return CAT_HELICOPTER;
                }
            }
        }
        if (80.0..=180.0).contains(&speed) {
            if let Some(alt) = altitude_ft {
                if alt > 15000 {
                    return CAT_TURBOPROP;
                }
            }
            return CAT_PROP;
        }
        if speed > 180.0 && speed <= 250.0 {
            return CAT_TURBOPROP;
        }
    }

    // Altitude-only fallback
    if let Some(alt) = altitude_ft {
        if alt > 30000 {
            return CAT_JET;
        }
        if alt < 5000 {
            return CAT_PROP;
        }
    }

    CAT_UNKNOWN
}

// ---------------------------------------------------------------------------
// Airport database
// ---------------------------------------------------------------------------

/// A known airport.
#[derive(Debug, Clone)]
pub struct Airport {
    pub icao: &'static str,
    pub name: &'static str,
    pub lat: f64,
    pub lon: f64,
    pub elevation_ft: i32,
}

/// Minimal built-in airports (used when CSV not available).
const BUILTIN_AIRPORTS: &[Airport] = &[
    Airport {
        icao: "KATL",
        name: "Atlanta Hartsfield-Jackson",
        lat: 33.6367,
        lon: -84.4281,
        elevation_ft: 1026,
    },
    Airport {
        icao: "KCLT",
        name: "Charlotte Douglas",
        lat: 35.2140,
        lon: -80.9431,
        elevation_ft: 748,
    },
    Airport {
        icao: "KAVL",
        name: "Asheville Regional",
        lat: 35.4362,
        lon: -82.5418,
        elevation_ft: 2165,
    },
    Airport {
        icao: "KTYS",
        name: "Knoxville McGhee Tyson",
        lat: 35.8110,
        lon: -83.9940,
        elevation_ft: 981,
    },
];

/// Find nearest airport within max_nm nautical miles.
///
/// Returns (icao, name, distance_nm) or None.
pub fn nearest_airport(
    lat: f64,
    lon: f64,
    max_nm: f64,
) -> Option<(&'static str, &'static str, f64)> {
    let mut best: Option<(&str, &str, f64)> = None;

    for apt in BUILTIN_AIRPORTS {
        let dist = haversine_nm(lat, lon, apt.lat, apt.lon);
        if dist < max_nm && (best.is_none() || dist < best.unwrap().2) {
            best = Some((apt.icao, apt.name, dist));
        }
    }

    best
}

/// Classify flight phase relative to nearest airport.
pub fn classify_flight_phase(
    lat: f64,
    lon: f64,
    altitude_ft: Option<i32>,
    vertical_rate_fpm: Option<i32>,
    max_airport_nm: f64,
) -> Option<String> {
    let (code, _name, dist) = nearest_airport(lat, lon, max_airport_nm)?;

    if let (Some(alt), Some(vr)) = (altitude_ft, vertical_rate_fpm) {
        if dist < 15.0 && vr < -200 && alt < 10000 {
            return Some(format!("Approaching {} ({:.0}nm)", code, dist));
        }
        if dist < 15.0 && vr > 200 && alt < 10000 {
            return Some(format!("Departing {} ({:.0}nm)", code, dist));
        }
    }

    if dist < 5.0 {
        return Some(format!("Near {} ({:.1}nm)", code, dist));
    }

    Some(format!("Overflying {} ({:.0}nm)", code, dist))
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_classify_jet() {
        assert_eq!(
            classify_from_profile(Some(300.0), Some(35000), false, None),
            CAT_JET
        );
    }

    #[test]
    fn test_classify_prop() {
        assert_eq!(
            classify_from_profile(Some(120.0), Some(5000), false, None),
            CAT_PROP
        );
    }

    #[test]
    fn test_classify_turboprop() {
        assert_eq!(
            classify_from_profile(Some(120.0), Some(20000), false, None),
            CAT_TURBOPROP
        );
    }

    #[test]
    fn test_classify_helicopter() {
        assert_eq!(
            classify_from_profile(Some(60.0), Some(1500), false, None),
            CAT_HELICOPTER
        );
    }

    #[test]
    fn test_classify_military() {
        assert_eq!(
            classify_from_profile(Some(300.0), Some(35000), true, None),
            CAT_MILITARY
        );
    }

    #[test]
    fn test_classify_cargo() {
        assert_eq!(
            classify_from_profile(Some(300.0), Some(35000), false, Some("FDX123")),
            CAT_CARGO
        );
    }

    #[test]
    fn test_classify_altitude_only_jet() {
        assert_eq!(
            classify_from_profile(None, Some(35000), false, None),
            CAT_JET
        );
    }

    #[test]
    fn test_classify_altitude_only_prop() {
        assert_eq!(
            classify_from_profile(None, Some(3000), false, None),
            CAT_PROP
        );
    }

    #[test]
    fn test_classify_unknown() {
        assert_eq!(classify_from_profile(None, None, false, None), CAT_UNKNOWN);
    }

    #[test]
    fn test_lookup_operator_known() {
        assert_eq!(lookup_operator("AAL123"), Some("American Airlines"));
        assert_eq!(lookup_operator("DAL456"), Some("Delta Air Lines"));
        assert_eq!(lookup_operator("SWA789"), Some("Southwest Airlines"));
    }

    #[test]
    fn test_lookup_operator_unknown() {
        assert_eq!(lookup_operator("XYZ999"), None);
    }

    #[test]
    fn test_lookup_operator_too_short() {
        assert_eq!(lookup_operator("AA"), None);
    }

    #[test]
    fn test_nearest_airport_asheville() {
        let result = nearest_airport(35.4, -82.5, 50.0);
        assert!(result.is_some());
        let (code, _, dist) = result.unwrap();
        assert_eq!(code, "KAVL");
        assert!(dist < 5.0);
    }

    #[test]
    fn test_nearest_airport_none() {
        // Middle of ocean
        let result = nearest_airport(0.0, 0.0, 50.0);
        assert!(result.is_none());
    }

    #[test]
    fn test_flight_phase_approaching() {
        let phase = classify_flight_phase(35.45, -82.55, Some(5000), Some(-500), 30.0);
        assert!(phase.is_some());
        assert!(phase.unwrap().contains("Approaching KAVL"));
    }

    #[test]
    fn test_flight_phase_departing() {
        let phase = classify_flight_phase(35.45, -82.55, Some(3000), Some(1000), 30.0);
        assert!(phase.is_some());
        assert!(phase.unwrap().contains("Departing KAVL"));
    }

    #[test]
    fn test_classify_turboprop_speed_range() {
        assert_eq!(
            classify_from_profile(Some(200.0), Some(15000), false, None),
            CAT_TURBOPROP
        );
    }
}
