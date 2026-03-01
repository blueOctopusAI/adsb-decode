//! Per-aircraft state machine with CPR frame pairing.
//!
//! Pure logic — no I/O, no database. Produces `TrackEvent` outputs that
//! the caller (CLI/server) writes to a database.
//!
//! Tracks per-aircraft: position, velocity, callsign, squawk, CPR buffers,
//! heading history, and staleness.

use crate::cpr;
use crate::decode::decode;
use crate::frame::ModeFrame;
use crate::icao;
use crate::types::*;

/// Aircraft considered stale after this many seconds of silence.
pub const STALE_TIMEOUT: f64 = 60.0;

/// Maximum heading/position history entries per aircraft.
const MAX_HISTORY: usize = 120;

// ---------------------------------------------------------------------------
// Track events (output)
// ---------------------------------------------------------------------------

/// Events emitted by the tracker for the caller to persist.
#[derive(Debug, Clone)]
pub enum TrackEvent {
    /// First time seeing this ICAO address.
    NewAircraft {
        icao: Icao,
        country: Option<&'static str>,
        registration: Option<String>,
        is_military: bool,
        timestamp: f64,
    },
    /// Aircraft record should be updated (last_seen).
    AircraftUpdate { icao: Icao, timestamp: f64 },
    /// Sighting record should be updated.
    SightingUpdate {
        icao: Icao,
        capture_id: Option<i64>,
        callsign: Option<String>,
        squawk: Option<String>,
        altitude_ft: Option<i32>,
        timestamp: f64,
    },
    /// New position to store (after downsampling filter).
    PositionUpdate {
        icao: Icao,
        lat: f64,
        lon: f64,
        altitude_ft: Option<i32>,
        speed_kts: Option<f64>,
        heading_deg: Option<f64>,
        vertical_rate_fpm: Option<i32>,
        receiver_id: Option<i64>,
        timestamp: f64,
    },
}

// ---------------------------------------------------------------------------
// Aircraft state
// ---------------------------------------------------------------------------

/// Mutable state for a single tracked aircraft.
#[derive(Debug, Clone)]
pub struct AircraftState {
    pub icao: Icao,
    pub callsign: Option<String>,
    pub squawk: Option<String>,

    // Position
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub altitude_ft: Option<i32>,

    // Velocity
    pub speed_kts: Option<f64>,
    pub heading_deg: Option<f64>,
    pub vertical_rate_fpm: Option<i32>,

    // CPR buffer for global decode
    pub cpr_even_lat: Option<u32>,
    pub cpr_even_lon: Option<u32>,
    pub cpr_even_time: f64,
    pub cpr_odd_lat: Option<u32>,
    pub cpr_odd_lon: Option<u32>,
    pub cpr_odd_time: f64,

    // Metadata
    pub country: Option<&'static str>,
    pub registration: Option<String>,
    pub is_military: bool,
    pub first_seen: f64,
    pub last_seen: f64,
    pub message_count: u64,

    // History buffers for pattern detection
    pub heading_history: Vec<(f64, f64)>, // (timestamp, heading_deg)
    pub position_history: Vec<(f64, f64, f64, Option<i32>)>, // (ts, lat, lon, alt)
}

impl AircraftState {
    pub fn new(icao: Icao, timestamp: f64) -> Self {
        AircraftState {
            icao,
            callsign: None,
            squawk: None,
            lat: None,
            lon: None,
            altitude_ft: None,
            speed_kts: None,
            heading_deg: None,
            vertical_rate_fpm: None,
            cpr_even_lat: None,
            cpr_even_lon: None,
            cpr_even_time: 0.0,
            cpr_odd_lat: None,
            cpr_odd_lon: None,
            cpr_odd_time: 0.0,
            country: icao::lookup_country(&icao),
            registration: icao::icao_to_n_number(&icao),
            is_military: icao::is_military(&icao, None),
            first_seen: timestamp,
            last_seen: timestamp,
            message_count: 0,
            heading_history: Vec::new(),
            position_history: Vec::new(),
        }
    }

    pub fn has_position(&self) -> bool {
        self.lat.is_some() && self.lon.is_some()
    }

    pub fn age(&self, now: f64) -> f64 {
        now - self.last_seen
    }

    pub fn is_stale(&self, now: f64) -> bool {
        self.age(now) > STALE_TIMEOUT
    }
}

// ---------------------------------------------------------------------------
// Tracker
// ---------------------------------------------------------------------------

/// Track multiple aircraft from decoded messages.
///
/// Pure state machine: call `update()` with frames, get back decoded messages
/// and `TrackEvent` outputs. The caller decides what to do with events
/// (write to DB, send to network, etc.).
pub struct Tracker {
    pub aircraft: std::collections::HashMap<Icao, AircraftState>,
    pub receiver_id: Option<i64>,
    pub capture_id: Option<i64>,
    pub ref_lat: Option<f64>,
    pub ref_lon: Option<f64>,
    pub min_position_interval: f64,

    // Last stored position timestamp per ICAO (for downsampling)
    last_stored: std::collections::HashMap<Icao, f64>,

    // Counters
    pub total_frames: u64,
    pub valid_frames: u64,
    pub position_decodes: u64,
    pub positions_skipped: u64,
}

impl Tracker {
    pub fn new(
        receiver_id: Option<i64>,
        capture_id: Option<i64>,
        ref_lat: Option<f64>,
        ref_lon: Option<f64>,
        min_position_interval: f64,
    ) -> Self {
        Tracker {
            aircraft: std::collections::HashMap::new(),
            receiver_id,
            capture_id,
            ref_lat,
            ref_lon,
            min_position_interval,
            last_stored: std::collections::HashMap::new(),
            total_frames: 0,
            valid_frames: 0,
            position_decodes: 0,
            positions_skipped: 0,
        }
    }

    /// Process a single parsed frame. Returns decoded message and events to persist.
    pub fn update(&mut self, frame: &ModeFrame) -> (Option<DecodedMsg>, Vec<TrackEvent>) {
        self.total_frames += 1;
        let mut events = Vec::new();

        let msg = match decode(frame) {
            Some(m) => m,
            None => return (None, events),
        };

        self.valid_frames += 1;
        let icao = *msg.icao();
        let timestamp = msg.timestamp();

        // Get or create aircraft state
        let is_new = !self.aircraft.contains_key(&icao);
        if is_new {
            let ac = AircraftState::new(icao, timestamp);
            events.push(TrackEvent::NewAircraft {
                icao,
                country: ac.country,
                registration: ac.registration.clone(),
                is_military: ac.is_military,
                timestamp,
            });
            self.aircraft.insert(icao, ac);
        }

        let ac = self.aircraft.get_mut(&icao).unwrap();
        ac.last_seen = timestamp;
        ac.message_count += 1;

        // Process message type
        match &msg {
            DecodedMsg::Identification(m) => {
                let cs = m.callsign.trim().to_string();
                if !cs.is_empty() {
                    // Re-check military status with callsign
                    if !ac.is_military {
                        ac.is_military = icao::is_military(&icao, Some(&cs));
                    }
                    ac.callsign = Some(cs);
                }
            }
            DecodedMsg::Position(m) => {
                if let Some(alt) = m.altitude_ft {
                    ac.altitude_ft = Some(alt);
                }

                // Store CPR frame
                if m.cpr_odd {
                    ac.cpr_odd_lat = Some(m.cpr_lat);
                    ac.cpr_odd_lon = Some(m.cpr_lon);
                    ac.cpr_odd_time = m.timestamp;
                } else {
                    ac.cpr_even_lat = Some(m.cpr_lat);
                    ac.cpr_even_lon = Some(m.cpr_lon);
                    ac.cpr_even_time = m.timestamp;
                }

                // Attempt position decode
                if let Some((lat, lon)) = try_cpr_decode(ac, self.ref_lat, self.ref_lon) {
                    ac.lat = Some(lat);
                    ac.lon = Some(lon);
                    self.position_decodes += 1;

                    // Record for pattern detection (always)
                    ac.position_history
                        .push((timestamp, lat, lon, ac.altitude_ft));
                    if ac.position_history.len() > MAX_HISTORY {
                        let start = ac.position_history.len() - MAX_HISTORY;
                        ac.position_history = ac.position_history[start..].to_vec();
                    }

                    // Downsample: only emit position event if enough time passed
                    let last = self.last_stored.get(&icao).copied();
                    if last.is_none() || timestamp - last.unwrap() >= self.min_position_interval {
                        events.push(TrackEvent::PositionUpdate {
                            icao,
                            lat,
                            lon,
                            altitude_ft: ac.altitude_ft,
                            speed_kts: ac.speed_kts,
                            heading_deg: ac.heading_deg,
                            vertical_rate_fpm: ac.vertical_rate_fpm,
                            receiver_id: self.receiver_id,
                            timestamp,
                        });
                        self.last_stored.insert(icao, timestamp);
                    } else {
                        self.positions_skipped += 1;
                    }
                }
            }
            DecodedMsg::Velocity(m) => {
                if let Some(spd) = m.speed_kts {
                    ac.speed_kts = Some(spd);
                }
                if let Some(hdg) = m.heading_deg {
                    ac.heading_deg = Some(hdg);
                    ac.heading_history.push((timestamp, hdg));
                    if ac.heading_history.len() > MAX_HISTORY {
                        let start = ac.heading_history.len() - MAX_HISTORY;
                        ac.heading_history = ac.heading_history[start..].to_vec();
                    }
                }
                if let Some(vr) = m.vertical_rate_fpm {
                    ac.vertical_rate_fpm = Some(vr);
                }
            }
            DecodedMsg::Altitude(m) => {
                if let Some(alt) = m.altitude_ft {
                    ac.altitude_ft = Some(alt);
                }
            }
            DecodedMsg::Squawk(m) => {
                ac.squawk = Some(m.squawk.clone());
            }
        }

        // Emit aircraft update + sighting
        events.push(TrackEvent::AircraftUpdate { icao, timestamp });
        events.push(TrackEvent::SightingUpdate {
            icao,
            capture_id: self.capture_id,
            callsign: ac.callsign.clone(),
            squawk: ac.squawk.clone(),
            altitude_ft: ac.altitude_ft,
            timestamp,
        });

        (Some(msg), events)
    }

    /// Return all non-stale aircraft, sorted by last seen (most recent first).
    pub fn get_active(&self, now: f64) -> Vec<&AircraftState> {
        let mut active: Vec<_> = self
            .aircraft
            .values()
            .filter(|ac| !ac.is_stale(now))
            .collect();
        active.sort_by(|a, b| b.last_seen.partial_cmp(&a.last_seen).unwrap());
        active
    }

    /// Remove stale aircraft from tracking. Returns count removed.
    pub fn prune_stale(&mut self, now: f64) -> usize {
        let stale: Vec<Icao> = self
            .aircraft
            .iter()
            .filter(|(_, ac)| ac.is_stale(now))
            .map(|(k, _)| *k)
            .collect();
        let count = stale.len();
        for k in stale {
            self.aircraft.remove(&k);
        }
        count
    }
}

/// Try to decode position from CPR frames (free function to avoid borrow conflicts).
fn try_cpr_decode(
    ac: &AircraftState,
    tracker_ref_lat: Option<f64>,
    tracker_ref_lon: Option<f64>,
) -> Option<(f64, f64)> {
    // Try global decode if we have both even and odd
    if ac.cpr_even_lat.is_some() && ac.cpr_odd_lat.is_some() {
        let result = cpr::global_decode(
            ac.cpr_even_lat.unwrap(),
            ac.cpr_even_lon.unwrap(),
            ac.cpr_odd_lat.unwrap(),
            ac.cpr_odd_lon.unwrap(),
            ac.cpr_even_time,
            ac.cpr_odd_time,
        );
        if result.is_some() {
            return result;
        }
    }

    // Try local decode with reference position
    let (ref_lat, ref_lon) = match (tracker_ref_lat, tracker_ref_lon) {
        (Some(lat), Some(lon)) => (lat, lon),
        _ => {
            // Fall back to last known position
            match (ac.lat, ac.lon) {
                (Some(lat), Some(lon)) => (lat, lon),
                _ => return None,
            }
        }
    };

    // Use the most recent CPR frame
    if ac.cpr_odd_time >= ac.cpr_even_time {
        if let Some(lat) = ac.cpr_odd_lat {
            return Some(cpr::local_decode(
                lat,
                ac.cpr_odd_lon.unwrap(),
                true,
                ref_lat,
                ref_lon,
            ));
        }
    } else if let Some(lat) = ac.cpr_even_lat {
        return Some(cpr::local_decode(
            lat,
            ac.cpr_even_lon.unwrap(),
            false,
            ref_lat,
            ref_lon,
        ));
    }

    None
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use crate::frame::parse_frame_uncached;

    fn make_tracker() -> Tracker {
        Tracker::new(None, None, None, None, 2.0)
    }

    fn parse(hex: &str, ts: f64) -> ModeFrame {
        parse_frame_uncached(hex, ts, None).expect("valid frame")
    }

    #[test]
    fn test_new_aircraft_event() {
        let mut tracker = make_tracker();
        let frame = parse("8D4840D6202CC371C32CE0576098", 1.0);
        let (msg, events) = tracker.update(&frame);

        assert!(msg.is_some());
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TrackEvent::NewAircraft { .. })),
            "Should emit NewAircraft event"
        );
    }

    #[test]
    fn test_aircraft_state_created() {
        let mut tracker = make_tracker();
        let frame = parse("8D4840D6202CC371C32CE0576098", 1.0);
        tracker.update(&frame);

        let icao = [0x48, 0x40, 0xD6];
        assert!(tracker.aircraft.contains_key(&icao));

        let ac = &tracker.aircraft[&icao];
        assert_eq!(ac.callsign.as_deref(), Some("KLM1023"));
        assert_eq!(ac.country, Some("Netherlands"));
        assert_eq!(ac.message_count, 1);
    }

    #[test]
    fn test_callsign_update() {
        let mut tracker = make_tracker();
        let frame = parse("8D4840D6202CC371C32CE0576098", 1.0);
        tracker.update(&frame);

        let icao = [0x48, 0x40, 0xD6];
        assert_eq!(tracker.aircraft[&icao].callsign.as_deref(), Some("KLM1023"));
    }

    #[test]
    fn test_position_cpr_pairing() {
        let mut tracker = make_tracker();

        // Even frame
        let frame = parse("8D40621D58C382D690C8AC2863A7", 1.0);
        tracker.update(&frame);

        let icao = [0x40, 0x62, 0x1D];
        let ac = &tracker.aircraft[&icao];
        assert!(ac.cpr_even_lat.is_some());
        assert!(!ac.has_position()); // Need both even+odd

        // Odd frame (within 10s)
        let frame = parse("8D40621D58C386435CC412692AD6", 2.0);
        let (_, events) = tracker.update(&frame);

        let ac = &tracker.aircraft[&icao];
        assert!(ac.has_position(), "Should have position after CPR pair");
        assert_eq!(ac.altitude_ft, Some(38000));

        // Should have emitted a PositionUpdate
        assert!(
            events
                .iter()
                .any(|e| matches!(e, TrackEvent::PositionUpdate { .. })),
            "Should emit PositionUpdate"
        );
    }

    #[test]
    fn test_velocity_update() {
        let mut tracker = make_tracker();
        let frame = parse("8D485020994409940838175B284F", 1.0);
        tracker.update(&frame);

        let icao = [0x48, 0x50, 0x20];
        let ac = &tracker.aircraft[&icao];
        assert!(ac.speed_kts.is_some());
        assert!(ac.heading_deg.is_some());
        assert_eq!(ac.vertical_rate_fpm, Some(-832));
    }

    #[test]
    fn test_heading_history() {
        let mut tracker = make_tracker();
        let frame = parse("8D485020994409940838175B284F", 1.0);
        tracker.update(&frame);

        let icao = [0x48, 0x50, 0x20];
        let ac = &tracker.aircraft[&icao];
        assert_eq!(ac.heading_history.len(), 1);
    }

    #[test]
    fn test_stale_detection() {
        let ac = AircraftState::new([0x01, 0x02, 0x03], 1.0);
        assert!(!ac.is_stale(2.0));
        assert!(ac.is_stale(62.0));
    }

    #[test]
    fn test_prune_stale() {
        let mut tracker = make_tracker();

        let frame = parse("8D4840D6202CC371C32CE0576098", 1.0);
        tracker.update(&frame);

        assert_eq!(tracker.aircraft.len(), 1);
        assert_eq!(tracker.prune_stale(2.0), 0);
        assert_eq!(tracker.prune_stale(62.0), 1);
        assert_eq!(tracker.aircraft.len(), 0);
    }

    #[test]
    fn test_get_active() {
        let mut tracker = make_tracker();

        let frame = parse("8D4840D6202CC371C32CE0576098", 1.0);
        tracker.update(&frame);

        assert_eq!(tracker.get_active(2.0).len(), 1);
        assert_eq!(tracker.get_active(62.0).len(), 0);
    }

    #[test]
    fn test_position_downsampling() {
        let mut tracker = Tracker::new(None, None, None, None, 5.0);

        // First position pair — odd frame at t=2 triggers global decode
        let frame = parse("8D40621D58C382D690C8AC2863A7", 1.0); // even
        tracker.update(&frame);
        let frame = parse("8D40621D58C386435CC412692AD6", 2.0); // odd
        tracker.update(&frame);

        assert_eq!(tracker.position_decodes, 1);
        assert_eq!(tracker.positions_skipped, 0);

        // Second pair too soon — after first pair, each frame triggers a decode
        // (pairs with the complement from previous). Both within 5s → skipped.
        let frame = parse("8D40621D58C382D690C8AC2863A7", 3.0); // even pairs with odd@2
        tracker.update(&frame);
        let frame = parse("8D40621D58C386435CC412692AD6", 4.0); // odd pairs with even@3
        tracker.update(&frame);

        assert_eq!(tracker.position_decodes, 3); // 1 + 2 new decodes
        assert_eq!(tracker.positions_skipped, 2); // both skipped

        // Third pair after interval — even@7 is 5s after stored@2 → stored
        // odd@8 is 1s after stored@7 → skipped
        let frame = parse("8D40621D58C382D690C8AC2863A7", 7.0); // even, 7-2=5 >= 5 → stored
        tracker.update(&frame);
        let frame = parse("8D40621D58C386435CC412692AD6", 8.0); // odd, 8-7=1 < 5 → skipped
        tracker.update(&frame);

        assert_eq!(tracker.position_decodes, 5); // 3 + 2 new decodes
        assert_eq!(tracker.positions_skipped, 3); // 2 + 1 new skip
    }

    #[test]
    fn test_counters() {
        let mut tracker = make_tracker();

        let frame = parse("8D4840D6202CC371C32CE0576098", 1.0);
        tracker.update(&frame);

        assert_eq!(tracker.total_frames, 1);
        assert_eq!(tracker.valid_frames, 1);
    }

    #[test]
    fn test_sighting_event_emitted() {
        let mut tracker = make_tracker();
        let frame = parse("8D4840D6202CC371C32CE0576098", 1.0);
        let (_, events) = tracker.update(&frame);

        assert!(
            events
                .iter()
                .any(|e| matches!(e, TrackEvent::SightingUpdate { .. })),
            "Should emit SightingUpdate"
        );
    }

    #[test]
    fn test_second_message_not_new_aircraft() {
        let mut tracker = make_tracker();

        let frame = parse("8D4840D6202CC371C32CE0576098", 1.0);
        tracker.update(&frame);

        let frame = parse("8D4840D6202CC371C32CE0576098", 2.0);
        let (_, events) = tracker.update(&frame);

        let new_count = events
            .iter()
            .filter(|e| matches!(e, TrackEvent::NewAircraft { .. }))
            .count();
        assert_eq!(new_count, 0, "Second message should NOT emit NewAircraft");
    }

    #[test]
    fn test_multiple_aircraft() {
        let mut tracker = make_tracker();

        tracker.update(&parse("8D4840D6202CC371C32CE0576098", 1.0));
        tracker.update(&parse("8D406B902015A678D4D220AA4BDA", 2.0));

        assert_eq!(tracker.aircraft.len(), 2);
    }

    #[test]
    fn test_military_callsign_detection() {
        let mut tracker = make_tracker();

        // This frame won't be military by ICAO alone, but if we had a RCH callsign...
        let frame = parse("8D4840D6202CC371C32CE0576098", 1.0);
        tracker.update(&frame);

        let icao = [0x48, 0x40, 0xD6];
        // Netherlands address is not military
        assert!(!tracker.aircraft[&icao].is_military);
    }
}
