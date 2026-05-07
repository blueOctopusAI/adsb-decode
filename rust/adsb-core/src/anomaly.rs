//! Per-position anomaly scoring.
//!
//! Returns a non-negative `f64` score where 0.0 is "looks normal" and higher
//! values mean "the physics, the aircraft type, or the sequencing of the
//! tracks doesn't add up." This is the foundation: today the rules are
//! hand-tuned thresholds; the column the score writes to is what later ML
//! work will replace, not the persistence format.
//!
//! The scorer is a pure function — same inputs, same output, no I/O. Callers
//! supply the current position context and (optionally) the previous one.
//! When `previous` is None — first sighting, or the previous position was
//! pruned — only single-point rules fire.
//!
//! Scoring is additive across rules. Each rule produces a score component
//! and a stable text flag. Flags let consumers see *why* a position scored
//! the way it did without having to re-derive it from raw fields.

/// Single position context for scoring. Field types match the tracker's
/// `AircraftState` so both the live path and historical re-scoring can build
/// these without conversion.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PositionContext {
    pub timestamp: f64,
    pub lat: f64,
    pub lon: f64,
    pub altitude_ft: Option<i32>,
    pub speed_kts: Option<f64>,
    pub vertical_rate_fpm: Option<i32>,
}

/// Anomaly score for a position. `score` is non-negative; 0.0 means normal.
/// `flags` lists which rules contributed, in the order they fired.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct AnomalyScore {
    pub score: f64,
    pub flags: Vec<&'static str>,
}

impl AnomalyScore {
    pub fn is_normal(&self) -> bool {
        self.score == 0.0
    }
}

// ---- Thresholds -----------------------------------------------------------
//
// Hand-tuned for civilian + general aviation. Military aircraft can exceed
// some of these (e.g. supersonic intercepts). When ML replaces these rules,
// per-aircraft-class thresholds become learned, not hand-tuned.

const MAX_PLAUSIBLE_SPEED_KTS: f64 = 750.0;
const MAX_PLAUSIBLE_VERTICAL_RATE_FPM: i32 = 12000;
const MAX_PLAUSIBLE_TELEPORT_KTS: f64 = 1500.0;
const MAX_PLAUSIBLE_ALT_RATE_FPM: f64 = 20000.0;
const STUCK_DISTANCE_NM: f64 = 0.001;
const STUCK_DURATION_S: f64 = 30.0;
const STUCK_MIN_REPORTED_SPEED_KTS: f64 = 100.0;

const EARTH_RADIUS_NM: f64 = 3440.065;

/// Great-circle distance in nautical miles. Pure trig, no allocation.
fn haversine_nm(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let to_rad = std::f64::consts::PI / 180.0;
    let phi1 = lat1 * to_rad;
    let phi2 = lat2 * to_rad;
    let dphi = (lat2 - lat1) * to_rad;
    let dlam = (lon2 - lon1) * to_rad;
    let a = (dphi / 2.0).sin().powi(2) + phi1.cos() * phi2.cos() * (dlam / 2.0).sin().powi(2);
    2.0 * EARTH_RADIUS_NM * a.sqrt().asin()
}

/// Score a position. Higher = more anomalous. See module doc for semantics.
pub fn score_position(
    current: &PositionContext,
    previous: Option<&PositionContext>,
) -> AnomalyScore {
    let mut out = AnomalyScore::default();

    if let Some(speed) = current.speed_kts {
        if speed > MAX_PLAUSIBLE_SPEED_KTS {
            out.score += 1.0;
            out.flags.push("extreme_speed");
        }
    }

    if let Some(vr) = current.vertical_rate_fpm {
        if vr.abs() > MAX_PLAUSIBLE_VERTICAL_RATE_FPM {
            out.score += 1.0;
            out.flags.push("extreme_vertical_rate");
        }
    }

    let Some(prev) = previous else {
        return out;
    };

    let dt = current.timestamp - prev.timestamp;
    // Out-of-order timestamps: don't score, but flag.
    if dt <= 0.0 {
        out.score += 0.5;
        out.flags.push("nonmonotonic_time");
        return out;
    }

    let dist_nm = haversine_nm(prev.lat, prev.lon, current.lat, current.lon);
    let implied_speed_kts = dist_nm / (dt / 3600.0);

    if implied_speed_kts > MAX_PLAUSIBLE_TELEPORT_KTS {
        out.score += 2.0;
        out.flags.push("position_teleport");
    }

    if let (Some(a1), Some(a0)) = (current.altitude_ft, prev.altitude_ft) {
        let alt_rate_fpm = ((a1 - a0) as f64 / dt) * 60.0;
        if alt_rate_fpm.abs() > MAX_PLAUSIBLE_ALT_RATE_FPM {
            out.score += 1.0;
            out.flags.push("altitude_jump");
        }
    }

    let reported_speed = current.speed_kts.unwrap_or(0.0);
    if dist_nm < STUCK_DISTANCE_NM
        && dt > STUCK_DURATION_S
        && reported_speed > STUCK_MIN_REPORTED_SPEED_KTS
    {
        out.score += 0.5;
        out.flags.push("stuck_position");
    }

    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ctx(ts: f64, lat: f64, lon: f64) -> PositionContext {
        PositionContext {
            timestamp: ts,
            lat,
            lon,
            altitude_ft: None,
            speed_kts: None,
            vertical_rate_fpm: None,
        }
    }

    #[test]
    fn normal_flight_scores_zero() {
        // KAVL → moves ~5nm in 1 minute = 300 kt. Normal.
        let prev = PositionContext {
            altitude_ft: Some(8000),
            speed_kts: Some(300.0),
            vertical_rate_fpm: Some(500),
            ..ctx(1000.0, 35.43, -82.54)
        };
        let curr = PositionContext {
            altitude_ft: Some(8050),
            speed_kts: Some(305.0),
            vertical_rate_fpm: Some(500),
            ..ctx(1060.0, 35.50, -82.50)
        };
        let s = score_position(&curr, Some(&prev));
        assert!(s.is_normal(), "expected normal, got {s:?}");
    }

    #[test]
    fn first_sighting_with_no_previous_can_still_flag_extreme_speed() {
        let curr = PositionContext {
            speed_kts: Some(900.0),
            ..ctx(1000.0, 35.0, -82.0)
        };
        let s = score_position(&curr, None);
        assert!(s.score > 0.0);
        assert!(s.flags.contains(&"extreme_speed"));
    }

    #[test]
    fn extreme_vertical_rate_fires() {
        let curr = PositionContext {
            vertical_rate_fpm: Some(15000),
            ..ctx(1000.0, 35.0, -82.0)
        };
        let s = score_position(&curr, None);
        assert!(s.flags.contains(&"extreme_vertical_rate"));
    }

    #[test]
    fn position_teleport_flagged() {
        // 100 nm in 1 second = 360,000 kt. Definitely a teleport.
        let prev = ctx(1000.0, 35.0, -82.0);
        let curr = ctx(1001.0, 36.5, -82.0); // ~90 nm north
        let s = score_position(&curr, Some(&prev));
        assert!(
            s.flags.contains(&"position_teleport"),
            "flags={:?}",
            s.flags
        );
        assert!(s.score >= 2.0);
    }

    #[test]
    fn altitude_jump_flagged_independent_of_position() {
        let prev = PositionContext {
            altitude_ft: Some(10_000),
            ..ctx(1000.0, 35.0, -82.0)
        };
        // 30,000 ft change in 10 seconds = 180,000 fpm. Way over.
        let curr = PositionContext {
            altitude_ft: Some(40_000),
            ..ctx(1010.0, 35.001, -82.0)
        };
        let s = score_position(&curr, Some(&prev));
        assert!(s.flags.contains(&"altitude_jump"), "flags={:?}", s.flags);
    }

    #[test]
    fn stuck_position_flagged_when_reporting_speed() {
        let prev = PositionContext {
            speed_kts: Some(250.0),
            ..ctx(1000.0, 35.0, -82.0)
        };
        // 60 seconds later, exactly the same lat/lon, still claiming 250 kt.
        let curr = PositionContext {
            speed_kts: Some(250.0),
            ..ctx(1060.0, 35.0, -82.0)
        };
        let s = score_position(&curr, Some(&prev));
        assert!(s.flags.contains(&"stuck_position"));
    }

    #[test]
    fn stuck_position_not_flagged_when_actually_stationary() {
        // On the ground, speed near zero — staying still is fine.
        let prev = PositionContext {
            speed_kts: Some(0.0),
            ..ctx(1000.0, 35.0, -82.0)
        };
        let curr = PositionContext {
            speed_kts: Some(0.0),
            ..ctx(1060.0, 35.0, -82.0)
        };
        let s = score_position(&curr, Some(&prev));
        assert!(
            s.is_normal(),
            "stationary aircraft should not score, got {s:?}"
        );
    }

    #[test]
    fn nonmonotonic_time_flagged() {
        let prev = ctx(1000.0, 35.0, -82.0);
        let curr = ctx(900.0, 35.0, -82.0); // earlier than prev
        let s = score_position(&curr, Some(&prev));
        assert!(s.flags.contains(&"nonmonotonic_time"));
    }

    #[test]
    fn flags_accumulate_for_compound_anomalies() {
        // Extreme speed AND a teleport AND extreme vertical rate.
        let prev = PositionContext {
            altitude_ft: Some(10_000),
            ..ctx(1000.0, 35.0, -82.0)
        };
        let curr = PositionContext {
            altitude_ft: Some(40_000),
            speed_kts: Some(2000.0),
            vertical_rate_fpm: Some(20_000),
            ..ctx(1001.0, 36.5, -82.0)
        };
        let s = score_position(&curr, Some(&prev));
        assert!(s.flags.contains(&"extreme_speed"));
        assert!(s.flags.contains(&"extreme_vertical_rate"));
        assert!(s.flags.contains(&"position_teleport"));
        assert!(s.flags.contains(&"altitude_jump"));
        assert!(s.score >= 5.0);
    }

    #[test]
    fn haversine_self_distance_is_zero() {
        let d = haversine_nm(35.0, -82.0, 35.0, -82.0);
        assert!(d.abs() < 1e-9);
    }

    #[test]
    fn haversine_one_degree_lat_is_about_60nm() {
        let d = haversine_nm(35.0, -82.0, 36.0, -82.0);
        assert!((d - 60.0).abs() < 0.5, "got {d}nm");
    }
}
