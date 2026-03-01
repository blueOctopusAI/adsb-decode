//! Intelligence filters — military, emergency, anomaly detection, geofence.
//!
//! Each filter produces `FilterEvent` records. Dedup via `HashSet` prevents
//! duplicate alerts for the same aircraft + event type.

use std::collections::HashSet;

use crate::tracker::AircraftState;
use crate::types::{icao_to_string, Icao};

// ---------------------------------------------------------------------------
// Constants
// ---------------------------------------------------------------------------

pub const EVENT_MILITARY: &str = "military_detected";
pub const EVENT_EMERGENCY: &str = "emergency_squawk";
pub const EVENT_RAPID_DESCENT: &str = "rapid_descent";
pub const EVENT_LOW_ALTITUDE: &str = "low_altitude";
pub const EVENT_GEOFENCE: &str = "geofence_entry";
pub const EVENT_CIRCLING: &str = "circling";
pub const EVENT_HOLDING: &str = "holding_pattern";
pub const EVENT_PROXIMITY: &str = "proximity";
pub const EVENT_UNUSUAL_ALTITUDE: &str = "unusual_altitude";

const RAPID_DESCENT_THRESHOLD: i32 = -5000; // ft/min
const LOW_ALTITUDE_THRESHOLD: i32 = 500; // ft
const CIRCLING_WINDOW_SEC: f64 = 300.0; // 5 minutes
const CIRCLING_MIN_HEADING_CHANGE: f64 = 360.0; // degrees cumulative

// ---------------------------------------------------------------------------
// Types
// ---------------------------------------------------------------------------

/// A detected event/anomaly.
#[derive(Debug, Clone)]
pub struct FilterEvent {
    pub icao: Icao,
    pub event_type: &'static str,
    pub description: String,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub altitude_ft: Option<i32>,
    pub timestamp: f64,
}

/// Circular geofence zone.
#[derive(Debug, Clone)]
pub struct Geofence {
    pub name: String,
    pub lat: f64,
    pub lon: f64,
    pub radius_nm: f64,
}

/// Emergency squawk lookup.
pub fn emergency_squawk(squawk: &str) -> Option<&'static str> {
    match squawk {
        "7500" => Some("Hijack"),
        "7600" => Some("Radio failure"),
        "7700" => Some("Emergency"),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Haversine
// ---------------------------------------------------------------------------

const EARTH_RADIUS_NM: f64 = 3440.065;

/// Great-circle distance in nautical miles.
pub fn haversine_nm(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    EARTH_RADIUS_NM * 2.0 * a.sqrt().atan2((1.0 - a).sqrt())
}

// ---------------------------------------------------------------------------
// Filter Engine
// ---------------------------------------------------------------------------

/// Runs all filters against aircraft state and produces events.
///
/// Tracks which events have already been emitted per aircraft to avoid
/// duplicate alerts within a session.
pub struct FilterEngine {
    pub geofences: Vec<Geofence>,
    pub low_altitude_ft: i32,
    pub rapid_descent_fpm: i32,
    pub proximity_nm: f64,
    pub proximity_ft: i32,
    emitted: HashSet<(String, String)>,
}

impl FilterEngine {
    pub fn new() -> Self {
        FilterEngine {
            geofences: Vec::new(),
            low_altitude_ft: LOW_ALTITUDE_THRESHOLD,
            rapid_descent_fpm: RAPID_DESCENT_THRESHOLD,
            proximity_nm: 5.0,
            proximity_ft: 1000,
            emitted: HashSet::new(),
        }
    }

    /// Run all filters against a single aircraft.
    pub fn check(&mut self, ac: &AircraftState) -> Vec<FilterEvent> {
        let mut events = Vec::new();
        self.check_military(ac, &mut events);
        self.check_emergency(ac, &mut events);
        self.check_rapid_descent(ac, &mut events);
        self.check_low_altitude(ac, &mut events);
        self.check_circling(ac, &mut events);
        self.check_holding(ac, &mut events);
        self.check_geofences(ac, &mut events);
        events
    }

    /// Check all pairs for proximity alerts.
    pub fn check_proximity(&mut self, aircraft: &[&AircraftState]) -> Vec<FilterEvent> {
        let mut events = Vec::new();
        let positioned: Vec<&&AircraftState> =
            aircraft.iter().filter(|ac| ac.has_position()).collect();

        for i in 0..positioned.len() {
            for j in (i + 1)..positioned.len() {
                let a = positioned[i];
                let b = positioned[j];
                let dist = haversine_nm(
                    a.lat.unwrap(),
                    a.lon.unwrap(),
                    b.lat.unwrap(),
                    b.lon.unwrap(),
                );
                if dist > self.proximity_nm {
                    continue;
                }

                if let (Some(alt_a), Some(alt_b)) = (a.altitude_ft, b.altitude_ft) {
                    if (alt_a - alt_b).unsigned_abs() > self.proximity_ft as u32 {
                        continue;
                    }
                }

                let icao_a = icao_to_string(&a.icao);
                let icao_b = icao_to_string(&b.icao);
                let mut pair = [icao_a.clone(), icao_b.clone()];
                pair.sort();
                let key = (format!("{}:{}", pair[0], pair[1]), EVENT_PROXIMITY.to_string());
                if self.emitted.contains(&key) {
                    continue;
                }
                self.emitted.insert(key);

                let label_a = a
                    .callsign
                    .as_deref()
                    .unwrap_or(&icao_a);
                let label_b = b
                    .callsign
                    .as_deref()
                    .unwrap_or(&icao_b);

                events.push(FilterEvent {
                    icao: a.icao,
                    event_type: EVENT_PROXIMITY,
                    description: format!(
                        "Proximity alert: {} and {} within {:.1} nm",
                        label_a, label_b, dist
                    ),
                    lat: a.lat,
                    lon: a.lon,
                    altitude_ft: a.altitude_ft,
                    timestamp: a.last_seen,
                });
            }
        }
        events
    }

    /// Clear emitted events for a pruned aircraft.
    pub fn clear(&mut self, icao: &Icao) {
        let icao_str = icao_to_string(icao);
        self.emitted.retain(|k| !k.0.contains(&icao_str));
    }

    fn emit(&mut self, event: FilterEvent) -> Option<FilterEvent> {
        let key = (
            icao_to_string(&event.icao),
            event.event_type.to_string(),
        );
        if self.emitted.contains(&key) {
            return None;
        }
        self.emitted.insert(key);
        Some(event)
    }

    fn check_military(&mut self, ac: &AircraftState, events: &mut Vec<FilterEvent>) {
        if !ac.is_military {
            return;
        }
        let icao_str = icao_to_string(&ac.icao);
        let label = ac.callsign.as_deref().unwrap_or(&icao_str);
        if let Some(e) = self.emit(FilterEvent {
            icao: ac.icao,
            event_type: EVENT_MILITARY,
            description: format!("Military aircraft detected: {}", label),
            lat: ac.lat,
            lon: ac.lon,
            altitude_ft: ac.altitude_ft,
            timestamp: ac.last_seen,
        }) {
            events.push(e);
        }
    }

    fn check_emergency(&mut self, ac: &AircraftState, events: &mut Vec<FilterEvent>) {
        let squawk = match &ac.squawk {
            Some(s) => s.as_str(),
            None => return,
        };
        let desc = match emergency_squawk(squawk) {
            Some(d) => d,
            None => return,
        };
        let icao_str = icao_to_string(&ac.icao);
        let label = ac.callsign.as_deref().unwrap_or(&icao_str);
        if let Some(e) = self.emit(FilterEvent {
            icao: ac.icao,
            event_type: EVENT_EMERGENCY,
            description: format!("Squawk {}: {} - {}", squawk, desc, label),
            lat: ac.lat,
            lon: ac.lon,
            altitude_ft: ac.altitude_ft,
            timestamp: ac.last_seen,
        }) {
            events.push(e);
        }
    }

    fn check_rapid_descent(&mut self, ac: &AircraftState, events: &mut Vec<FilterEvent>) {
        let vr = match ac.vertical_rate_fpm {
            Some(v) => v,
            None => return,
        };
        if vr >= self.rapid_descent_fpm {
            return;
        }
        let icao_str = icao_to_string(&ac.icao);
        let label = ac.callsign.as_deref().unwrap_or(&icao_str);
        let alt_str = ac
            .altitude_ft
            .map(|a| a.to_string())
            .unwrap_or("?".into());
        if let Some(e) = self.emit(FilterEvent {
            icao: ac.icao,
            event_type: EVENT_RAPID_DESCENT,
            description: format!(
                "Rapid descent {} ft/min - {} at {} ft",
                vr, label, alt_str
            ),
            lat: ac.lat,
            lon: ac.lon,
            altitude_ft: ac.altitude_ft,
            timestamp: ac.last_seen,
        }) {
            events.push(e);
        }
    }

    fn check_low_altitude(&mut self, ac: &AircraftState, events: &mut Vec<FilterEvent>) {
        let alt = match ac.altitude_ft {
            Some(a) if a > 0 && a < self.low_altitude_ft => a,
            _ => return,
        };
        let icao_str = icao_to_string(&ac.icao);
        let label = ac.callsign.as_deref().unwrap_or(&icao_str);
        if let Some(e) = self.emit(FilterEvent {
            icao: ac.icao,
            event_type: EVENT_LOW_ALTITUDE,
            description: format!("Low altitude {} ft - {}", alt, label),
            lat: ac.lat,
            lon: ac.lon,
            altitude_ft: ac.altitude_ft,
            timestamp: ac.last_seen,
        }) {
            events.push(e);
        }
    }

    fn check_circling(&mut self, ac: &AircraftState, events: &mut Vec<FilterEvent>) {
        if ac.heading_history.len() < 4 {
            return;
        }

        let now = ac.last_seen;
        let cutoff = now - CIRCLING_WINDOW_SEC;
        let recent: Vec<&(f64, f64)> = ac
            .heading_history
            .iter()
            .filter(|(t, _)| *t >= cutoff)
            .collect();

        if recent.len() < 4 {
            return;
        }

        let mut total_change = 0.0f64;
        for i in 1..recent.len() {
            let mut delta = recent[i].1 - recent[i - 1].1;
            while delta > 180.0 {
                delta -= 360.0;
            }
            while delta < -180.0 {
                delta += 360.0;
            }
            total_change += delta.abs();
        }

        if total_change < CIRCLING_MIN_HEADING_CHANGE {
            return;
        }

        let icao_str = icao_to_string(&ac.icao);
        let label = ac.callsign.as_deref().unwrap_or(&icao_str);
        if let Some(e) = self.emit(FilterEvent {
            icao: ac.icao,
            event_type: EVENT_CIRCLING,
            description: format!(
                "Circling detected: {} - {:.0} deg heading change",
                label, total_change
            ),
            lat: ac.lat,
            lon: ac.lon,
            altitude_ft: ac.altitude_ft,
            timestamp: ac.last_seen,
        }) {
            events.push(e);
        }
    }

    fn check_holding(&mut self, ac: &AircraftState, events: &mut Vec<FilterEvent>) {
        if ac.position_history.len() < 8 || ac.heading_history.len() < 8 {
            return;
        }

        let now = ac.last_seen;
        let cutoff = now - 120.0; // 2 minutes

        // Altitude stability check
        let alts: Vec<i32> = ac
            .position_history
            .iter()
            .filter(|(t, _, _, _)| *t >= cutoff)
            .filter_map(|(_, _, _, alt)| *alt)
            .collect();
        if alts.len() < 4 {
            return;
        }
        let alt_range = alts.iter().max().unwrap() - alts.iter().min().unwrap();
        if alt_range > 500 {
            return;
        }

        // Heading reciprocal check
        let headings: Vec<f64> = ac
            .heading_history
            .iter()
            .filter(|(t, _)| *t >= cutoff)
            .map(|(_, h)| *h)
            .collect();
        if headings.len() < 8 {
            return;
        }

        let mut bins = [0u32; 36];
        for h in &headings {
            bins[((*h as usize) / 10) % 36] += 1;
        }

        let mut sorted: Vec<(usize, u32)> = bins.iter().enumerate().map(|(i, &c)| (i, c)).collect();
        sorted.sort_by(|a, b| b.1.cmp(&a.1));

        if sorted[0].1 < 2 || sorted[1].1 < 2 {
            return;
        }

        let mut sep = (sorted[0].0 as i32 - sorted[1].0 as i32).unsigned_abs();
        if sep > 18 {
            sep = 36 - sep;
        }
        if (sep as i32 - 18).unsigned_abs() > 3 {
            return;
        }

        let avg_alt = alts.iter().sum::<i32>() / alts.len() as i32;
        let icao_str = icao_to_string(&ac.icao);
        let label = ac.callsign.as_deref().unwrap_or(&icao_str);
        if let Some(e) = self.emit(FilterEvent {
            icao: ac.icao,
            event_type: EVENT_HOLDING,
            description: format!(
                "Holding pattern: {} - stable at {} ft, reciprocal headings",
                label, avg_alt
            ),
            lat: ac.lat,
            lon: ac.lon,
            altitude_ft: ac.altitude_ft,
            timestamp: ac.last_seen,
        }) {
            events.push(e);
        }
    }

    fn check_geofences(&mut self, ac: &AircraftState, events: &mut Vec<FilterEvent>) {
        if !ac.has_position() {
            return;
        }

        for fence in &self.geofences {
            let dist = haversine_nm(
                ac.lat.unwrap(),
                ac.lon.unwrap(),
                fence.lat,
                fence.lon,
            );
            if dist > fence.radius_nm {
                continue;
            }

            let fence_key = format!("{}:{}", icao_to_string(&ac.icao), fence.name);
            let key = (fence_key, EVENT_GEOFENCE.to_string());
            if self.emitted.contains(&key) {
                continue;
            }
            self.emitted.insert(key);

            let icao_str = icao_to_string(&ac.icao);
            let label = ac.callsign.as_deref().unwrap_or(&icao_str);
            events.push(FilterEvent {
                icao: ac.icao,
                event_type: EVENT_GEOFENCE,
                description: format!(
                    "Entered geofence '{}' - {} at {:.1} nm",
                    fence.name, label, dist
                ),
                lat: ac.lat,
                lon: ac.lon,
                altitude_ft: ac.altitude_ft,
                timestamp: ac.last_seen,
            });
        }
    }
}

impl Default for FilterEngine {
    fn default() -> Self {
        Self::new()
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tracker::AircraftState;

    fn make_ac(icao: Icao) -> AircraftState {
        AircraftState::new(icao, 1.0)
    }

    #[test]
    fn test_haversine_same_point() {
        let d = haversine_nm(35.0, -82.0, 35.0, -82.0);
        assert!(d < 0.01, "Same point should be ~0 nm");
    }

    #[test]
    fn test_haversine_known_distance() {
        // Asheville to Charlotte: ~96nm
        let d = haversine_nm(35.4362, -82.5418, 35.2140, -80.9431);
        assert!(d > 70.0 && d < 120.0, "AVL-CLT should be ~96nm, got {d}");
    }

    #[test]
    fn test_emergency_squawk() {
        assert_eq!(emergency_squawk("7500"), Some("Hijack"));
        assert_eq!(emergency_squawk("7600"), Some("Radio failure"));
        assert_eq!(emergency_squawk("7700"), Some("Emergency"));
        assert_eq!(emergency_squawk("1200"), None);
    }

    #[test]
    fn test_military_detection() {
        let mut engine = FilterEngine::new();
        let mut ac = make_ac([0xAD, 0xF7, 0xC8]); // US military range
        ac.is_military = true;
        ac.last_seen = 1.0;

        let events = engine.check(&ac);
        assert!(events.iter().any(|e| e.event_type == EVENT_MILITARY));
    }

    #[test]
    fn test_military_dedup() {
        let mut engine = FilterEngine::new();
        let mut ac = make_ac([0xAD, 0xF7, 0xC8]);
        ac.is_military = true;
        ac.last_seen = 1.0;

        let events1 = engine.check(&ac);
        let events2 = engine.check(&ac);
        assert_eq!(events1.len(), 1);
        assert!(events2.iter().all(|e| e.event_type != EVENT_MILITARY));
    }

    #[test]
    fn test_emergency_detection() {
        let mut engine = FilterEngine::new();
        let mut ac = make_ac([0x48, 0x40, 0xD6]);
        ac.squawk = Some("7700".to_string());
        ac.last_seen = 1.0;

        let events = engine.check(&ac);
        assert!(events.iter().any(|e| e.event_type == EVENT_EMERGENCY));
    }

    #[test]
    fn test_rapid_descent() {
        let mut engine = FilterEngine::new();
        let mut ac = make_ac([0x48, 0x40, 0xD6]);
        ac.vertical_rate_fpm = Some(-6000);
        ac.altitude_ft = Some(10000);
        ac.last_seen = 1.0;

        let events = engine.check(&ac);
        assert!(events.iter().any(|e| e.event_type == EVENT_RAPID_DESCENT));
    }

    #[test]
    fn test_no_rapid_descent_normal() {
        let mut engine = FilterEngine::new();
        let mut ac = make_ac([0x48, 0x40, 0xD6]);
        ac.vertical_rate_fpm = Some(-1000);
        ac.last_seen = 1.0;

        let events = engine.check(&ac);
        assert!(!events.iter().any(|e| e.event_type == EVENT_RAPID_DESCENT));
    }

    #[test]
    fn test_low_altitude() {
        let mut engine = FilterEngine::new();
        let mut ac = make_ac([0x48, 0x40, 0xD6]);
        ac.altitude_ft = Some(300);
        ac.last_seen = 1.0;

        let events = engine.check(&ac);
        assert!(events.iter().any(|e| e.event_type == EVENT_LOW_ALTITUDE));
    }

    #[test]
    fn test_no_low_altitude_ground() {
        let mut engine = FilterEngine::new();
        let mut ac = make_ac([0x48, 0x40, 0xD6]);
        ac.altitude_ft = Some(0);
        ac.last_seen = 1.0;

        let events = engine.check(&ac);
        assert!(!events.iter().any(|e| e.event_type == EVENT_LOW_ALTITUDE));
    }

    #[test]
    fn test_geofence() {
        let mut engine = FilterEngine::new();
        engine.geofences.push(Geofence {
            name: "test-zone".to_string(),
            lat: 35.0,
            lon: -82.0,
            radius_nm: 10.0,
        });

        let mut ac = make_ac([0x48, 0x40, 0xD6]);
        ac.lat = Some(35.01);
        ac.lon = Some(-82.01);
        ac.last_seen = 1.0;

        let events = engine.check(&ac);
        assert!(events.iter().any(|e| e.event_type == EVENT_GEOFENCE));
    }

    #[test]
    fn test_geofence_outside() {
        let mut engine = FilterEngine::new();
        engine.geofences.push(Geofence {
            name: "test-zone".to_string(),
            lat: 35.0,
            lon: -82.0,
            radius_nm: 1.0,
        });

        let mut ac = make_ac([0x48, 0x40, 0xD6]);
        ac.lat = Some(36.0); // ~60nm away
        ac.lon = Some(-82.0);
        ac.last_seen = 1.0;

        let events = engine.check(&ac);
        assert!(!events.iter().any(|e| e.event_type == EVENT_GEOFENCE));
    }

    #[test]
    fn test_circling_detection() {
        let mut engine = FilterEngine::new();
        let mut ac = make_ac([0x48, 0x40, 0xD6]);
        ac.last_seen = 300.0;

        // Simulate 360° of heading changes in 5 minutes
        for i in 0..40 {
            let t = 1.0 + i as f64 * 7.5; // 40 updates over 300s
            let h = (i as f64 * 10.0) % 360.0; // 10° per update
            ac.heading_history.push((t, h));
        }

        let events = engine.check(&ac);
        assert!(
            events.iter().any(|e| e.event_type == EVENT_CIRCLING),
            "Should detect circling"
        );
    }

    #[test]
    fn test_proximity_alert() {
        let mut engine = FilterEngine::new();

        let mut a = make_ac([0x01, 0x02, 0x03]);
        a.lat = Some(35.0);
        a.lon = Some(-82.0);
        a.altitude_ft = Some(10000);
        a.last_seen = 1.0;

        let mut b = make_ac([0x04, 0x05, 0x06]);
        b.lat = Some(35.01);
        b.lon = Some(-82.01);
        b.altitude_ft = Some(10200);
        b.last_seen = 1.0;

        let events = engine.check_proximity(&[&a, &b]);
        assert!(events.iter().any(|e| e.event_type == EVENT_PROXIMITY));
    }

    #[test]
    fn test_clear_emitted() {
        let mut engine = FilterEngine::new();
        let mut ac = make_ac([0xAD, 0xF7, 0xC8]);
        ac.is_military = true;
        ac.last_seen = 1.0;

        engine.check(&ac); // emits military event
        engine.clear(&ac.icao); // clear

        let events = engine.check(&ac); // should emit again
        assert!(events.iter().any(|e| e.event_type == EVENT_MILITARY));
    }
}
