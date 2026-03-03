//! Demo mode — synthetic aircraft simulation for portfolio demos.
//!
//! Generates realistic aircraft trajectories around a region and continuously
//! updates a shared Tracker so the live dashboard shows moving aircraft without
//! requiring an RTL-SDR dongle.

use std::sync::{Arc, RwLock};

use adsb_core::icao;
use adsb_core::tracker::{AircraftState, Tracker};
use adsb_core::types::{icao_from_hex, Icao};

use crate::db;

// ---------------------------------------------------------------------------
// Flight definitions — realistic routes around Asheville/SE US
// ---------------------------------------------------------------------------

struct DemoFlight {
    icao_hex: &'static str,
    callsign: &'static str,
    /// Starting position
    lat: f64,
    lon: f64,
    altitude_ft: i32,
    speed_kts: f64,
    heading_deg: f64,
    is_military: bool,
    squawk: &'static str,
}

const DEMO_FLIGHTS: &[DemoFlight] = &[
    // Commercial — east-bound across SE US
    DemoFlight {
        icao_hex: "A12345",
        callsign: "DAL1842",
        lat: 35.60,
        lon: -83.80,
        altitude_ft: 36000,
        speed_kts: 460.0,
        heading_deg: 75.0,
        is_military: false,
        squawk: "3412",
    },
    // Commercial — west-bound
    DemoFlight {
        icao_hex: "A23456",
        callsign: "UAL952",
        lat: 35.20,
        lon: -82.40,
        altitude_ft: 38000,
        speed_kts: 480.0,
        heading_deg: 255.0,
        is_military: false,
        squawk: "5241",
    },
    // Regional — climbing out of AVL
    DemoFlight {
        icao_hex: "A34567",
        callsign: "RPA4521",
        lat: 35.44,
        lon: -82.54,
        altitude_ft: 12000,
        speed_kts: 280.0,
        heading_deg: 45.0,
        is_military: false,
        squawk: "1200",
    },
    // GA — low altitude pattern
    DemoFlight {
        icao_hex: "A45678",
        callsign: "N172SP",
        lat: 35.38,
        lon: -82.55,
        altitude_ft: 4500,
        speed_kts: 110.0,
        heading_deg: 180.0,
        is_military: false,
        squawk: "1200",
    },
    // Military — C-17 from Pope AAF
    DemoFlight {
        icao_hex: "AE1234",
        callsign: "RCH405",
        lat: 35.15,
        lon: -83.10,
        altitude_ft: 24000,
        speed_kts: 400.0,
        heading_deg: 310.0,
        is_military: true,
        squawk: "4567",
    },
    // Commercial — south-bound to ATL
    DemoFlight {
        icao_hex: "A56789",
        callsign: "SWA2233",
        lat: 35.50,
        lon: -83.20,
        altitude_ft: 32000,
        speed_kts: 440.0,
        heading_deg: 195.0,
        is_military: false,
        squawk: "6130",
    },
    // Cargo — FedEx from MEM to CLT
    DemoFlight {
        icao_hex: "A67890",
        callsign: "FDX812",
        lat: 35.70,
        lon: -83.60,
        altitude_ft: 34000,
        speed_kts: 470.0,
        heading_deg: 95.0,
        is_military: false,
        squawk: "2344",
    },
    // Military — F-16 fast mover
    DemoFlight {
        icao_hex: "AE5678",
        callsign: "VIPER21",
        lat: 35.30,
        lon: -82.80,
        altitude_ft: 28000,
        speed_kts: 520.0,
        heading_deg: 135.0,
        is_military: true,
        squawk: "7777",
    },
    // Helicopter — low/slow
    DemoFlight {
        icao_hex: "A78901",
        callsign: "N412MH",
        lat: 35.59,
        lon: -82.56,
        altitude_ft: 2500,
        speed_kts: 90.0,
        heading_deg: 270.0,
        is_military: false,
        squawk: "1200",
    },
    // Commercial — north-bound to DCA
    DemoFlight {
        icao_hex: "A89012",
        callsign: "AAL1187",
        lat: 34.90,
        lon: -82.70,
        altitude_ft: 35000,
        speed_kts: 450.0,
        heading_deg: 25.0,
        is_military: false,
        squawk: "4510",
    },
    // Private jet — business aviation
    DemoFlight {
        icao_hex: "A9ABCD",
        callsign: "EJA441",
        lat: 35.45,
        lon: -83.00,
        altitude_ft: 41000,
        speed_kts: 490.0,
        heading_deg: 60.0,
        is_military: false,
        squawk: "3320",
    },
    // Military tanker — orbiting pattern
    DemoFlight {
        icao_hex: "AE9012",
        callsign: "TEAL71",
        lat: 35.00,
        lon: -83.50,
        altitude_ft: 26000,
        speed_kts: 350.0,
        heading_deg: 90.0,
        is_military: true,
        squawk: "4456",
    },
];

// ---------------------------------------------------------------------------
// Demo vessel definitions — realistic maritime traffic near SE US coast
// ---------------------------------------------------------------------------

struct DemoVessel {
    mmsi: &'static str,
    name: &'static str,
    vessel_type: &'static str,
    flag: &'static str,
    lat: f64,
    lon: f64,
    speed_kts: f64,
    course_deg: f64,
}

const DEMO_VESSELS: &[DemoVessel] = &[
    // Container ship — heading to Charleston
    DemoVessel {
        mmsi: "367000001",
        name: "ATLANTIC PIONEER",
        vessel_type: "Cargo",
        flag: "US",
        lat: 32.60,
        lon: -79.20,
        speed_kts: 18.5,
        course_deg: 315.0,
    },
    // Tanker — heading south along coast
    DemoVessel {
        mmsi: "367000002",
        name: "GULF SPIRIT",
        vessel_type: "Tanker",
        flag: "US",
        lat: 33.20,
        lon: -78.80,
        speed_kts: 12.0,
        course_deg: 195.0,
    },
    // Fishing vessel — offshore
    DemoVessel {
        mmsi: "367000003",
        name: "MISS CAROLINA",
        vessel_type: "Fishing",
        flag: "US",
        lat: 33.50,
        lon: -77.80,
        speed_kts: 6.0,
        course_deg: 45.0,
    },
    // Passenger cruise — leaving Savannah
    DemoVessel {
        mmsi: "311000004",
        name: "CARNIVAL DREAM",
        vessel_type: "Passenger",
        flag: "BS",
        lat: 31.90,
        lon: -80.50,
        speed_kts: 20.0,
        course_deg: 135.0,
    },
    // Military vessel — patrol
    DemoVessel {
        mmsi: "369970001",
        name: "USCG FORWARD",
        vessel_type: "Military",
        flag: "US",
        lat: 32.80,
        lon: -79.60,
        speed_kts: 15.0,
        course_deg: 270.0,
    },
    // Tug — near port
    DemoVessel {
        mmsi: "367000005",
        name: "PALMETTO TUG",
        vessel_type: "Tug",
        flag: "US",
        lat: 32.78,
        lon: -79.92,
        speed_kts: 8.0,
        course_deg: 180.0,
    },
    // Bulk carrier — inbound
    DemoVessel {
        mmsi: "538000006",
        name: "IRON VOYAGER",
        vessel_type: "Cargo",
        flag: "MH",
        lat: 33.10,
        lon: -78.30,
        speed_kts: 14.0,
        course_deg: 290.0,
    },
    // Sailing vessel
    DemoVessel {
        mmsi: "367000007",
        name: "WIND DANCER",
        vessel_type: "Sailing",
        flag: "US",
        lat: 33.30,
        lon: -78.60,
        speed_kts: 7.0,
        course_deg: 120.0,
    },
];

// ---------------------------------------------------------------------------
// State for running simulation
// ---------------------------------------------------------------------------

struct SimVessel {
    mmsi: String,
    lat: f64,
    lon: f64,
    speed_kts: f64,
    course_deg: f64,
    course_drift: f64,
    drift_counter: u32,
}

struct SimAircraft {
    icao: Icao,
    callsign: String,
    lat: f64,
    lon: f64,
    altitude_ft: i32,
    speed_kts: f64,
    heading_deg: f64,
    is_military: bool,
    squawk: String,
    /// Heading drift per tick (adds gentle turns)
    heading_drift: f64,
    /// Altitude drift per tick (climb/descend)
    alt_drift: i32,
    /// Ticks until heading drift reverses
    drift_counter: u32,
}

/// Seed the database with initial demo positions and start the simulation loop.
pub async fn start_demo(
    database: Arc<std::sync::Mutex<db::Database>>,
    tracker: Arc<RwLock<Tracker>>,
) {
    let now = now_ts();

    // Build simulation state
    let mut sim_aircraft: Vec<SimAircraft> = Vec::new();

    for (i, flight) in DEMO_FLIGHTS.iter().enumerate() {
        let icao = icao_from_hex(flight.icao_hex).unwrap();

        // Seed aircraft in DB
        {
            let mut db = database.lock().unwrap();
            db.upsert_aircraft(
                &icao,
                icao::lookup_country(&icao),
                None,
                flight.is_military,
                now,
            );
            // Seed a few historical positions for trails
            for j in (1..=10).rev() {
                let t = now - (j as f64) * 5.0;
                let (trail_lat, trail_lon) = advance_position(
                    flight.lat,
                    flight.lon,
                    flight.heading_deg,
                    flight.speed_kts,
                    -(j as f64) * 5.0,
                );
                db.add_position(
                    &icao,
                    trail_lat,
                    trail_lon,
                    Some(flight.altitude_ft),
                    Some(flight.speed_kts),
                    Some(flight.heading_deg),
                    None,
                    None,
                    t,
                );
            }
            db.flush();
        }

        // Deterministic "random" drift based on index
        let heading_drift = match i % 4 {
            0 => 0.3,
            1 => -0.2,
            2 => 0.15,
            _ => -0.1,
        };
        let alt_drift = match i % 5 {
            0 => 0,
            1 => 50,
            2 => -25,
            3 => 0,
            _ => -50,
        };

        sim_aircraft.push(SimAircraft {
            icao,
            callsign: flight.callsign.to_string(),
            lat: flight.lat,
            lon: flight.lon,
            altitude_ft: flight.altitude_ft,
            speed_kts: flight.speed_kts,
            heading_deg: flight.heading_deg,
            is_military: flight.is_military,
            squawk: flight.squawk.to_string(),
            heading_drift,
            alt_drift,
            drift_counter: 0,
        });
    }

    // ----- Seed demo vessels -----
    let mut sim_vessels: Vec<SimVessel> = Vec::new();

    for (i, vessel) in DEMO_VESSELS.iter().enumerate() {
        {
            let mut db = database.lock().unwrap();
            db.upsert_vessel(
                vessel.mmsi,
                Some(vessel.name),
                Some(vessel.vessel_type),
                Some(vessel.flag),
                now,
            );
            // Seed historical positions for vessel trails
            for j in (1..=8).rev() {
                let t = now - (j as f64) * 30.0;
                let (trail_lat, trail_lon) = advance_position(
                    vessel.lat,
                    vessel.lon,
                    vessel.course_deg,
                    vessel.speed_kts,
                    -(j as f64) * 30.0,
                );
                db.add_vessel_position(
                    vessel.mmsi,
                    trail_lat,
                    trail_lon,
                    Some(vessel.speed_kts),
                    Some(vessel.course_deg),
                    Some(vessel.course_deg),
                    t,
                );
            }
            db.flush();
        }

        let course_drift = match i % 3 {
            0 => 0.1,
            1 => -0.08,
            _ => 0.05,
        };

        sim_vessels.push(SimVessel {
            mmsi: vessel.mmsi.to_string(),
            lat: vessel.lat,
            lon: vessel.lon,
            speed_kts: vessel.speed_kts,
            course_deg: vessel.course_deg,
            course_drift,
            drift_counter: 0,
        });
    }

    // Seed tracker with initial state
    update_tracker(&tracker, &sim_aircraft, now);

    eprintln!(
        "  [demo] {} synthetic aircraft + {} vessels active",
        sim_aircraft.len(),
        sim_vessels.len()
    );

    // Simulation loop — update positions every 2 seconds
    let mut tick_count: u32 = 0;
    let mut interval = tokio::time::interval(std::time::Duration::from_secs(2));
    loop {
        interval.tick().await;
        let now = now_ts();
        tick_count += 1;

        // Update aircraft
        for ac in &mut sim_aircraft {
            let (new_lat, new_lon) =
                advance_position(ac.lat, ac.lon, ac.heading_deg, ac.speed_kts, 2.0);
            ac.lat = new_lat;
            ac.lon = new_lon;

            ac.heading_deg = (ac.heading_deg + ac.heading_drift).rem_euclid(360.0);
            ac.drift_counter += 1;
            if ac.drift_counter > 60 {
                ac.heading_drift = -ac.heading_drift;
                ac.drift_counter = 0;
            }

            ac.altitude_ft += ac.alt_drift;
            ac.altitude_ft = ac.altitude_ft.clamp(1000, 45000);
            if ac.altitude_ft <= 1000 || ac.altitude_ft >= 45000 {
                ac.alt_drift = -ac.alt_drift;
            }

            if ac.lon < -90.0 || ac.lon > -75.0 {
                ac.heading_deg = (360.0 - ac.heading_deg).rem_euclid(360.0);
            }
            if ac.lat < 33.0 || ac.lat > 37.0 {
                ac.heading_deg = (180.0 - ac.heading_deg).rem_euclid(360.0);
            }

            {
                let mut db = database.lock().unwrap();
                db.add_position(
                    &ac.icao,
                    ac.lat,
                    ac.lon,
                    Some(ac.altitude_ft),
                    Some(ac.speed_kts),
                    Some(ac.heading_deg),
                    None,
                    None,
                    now,
                );
            }
        }

        // Update vessels (ships move slower, update every 5 ticks = 10 seconds)
        if tick_count.is_multiple_of(5) {
            for v in &mut sim_vessels {
                let (new_lat, new_lon) =
                    advance_position(v.lat, v.lon, v.course_deg, v.speed_kts, 10.0);
                v.lat = new_lat;
                v.lon = new_lon;

                v.course_deg = (v.course_deg + v.course_drift).rem_euclid(360.0);
                v.drift_counter += 1;
                if v.drift_counter > 120 {
                    v.course_drift = -v.course_drift;
                    v.drift_counter = 0;
                }

                // Keep vessels in coastal waters
                if v.lon < -82.0 || v.lon > -76.0 {
                    v.course_deg = (360.0 - v.course_deg).rem_euclid(360.0);
                }
                if v.lat < 30.0 || v.lat > 35.0 {
                    v.course_deg = (180.0 - v.course_deg).rem_euclid(360.0);
                }

                {
                    let mut db = database.lock().unwrap();
                    db.add_vessel_position(
                        &v.mmsi,
                        v.lat,
                        v.lon,
                        Some(v.speed_kts),
                        Some(v.course_deg),
                        Some(v.course_deg),
                        now,
                    );
                }
            }
        }

        // Flush DB periodically
        if tick_count.is_multiple_of(10) {
            let mut db = database.lock().unwrap();
            db.flush();
        }

        // Update tracker state for live aircraft API
        update_tracker(&tracker, &sim_aircraft, now);
    }
}

/// Update the shared Tracker with current simulation state.
fn update_tracker(tracker: &Arc<RwLock<Tracker>>, aircraft: &[SimAircraft], now: f64) {
    let mut t = tracker.write().unwrap();
    for ac in aircraft {
        let state = t.aircraft.entry(ac.icao).or_insert_with(|| AircraftState {
            icao: ac.icao,
            callsign: Some(ac.callsign.clone()),
            squawk: Some(ac.squawk.clone()),
            lat: Some(ac.lat),
            lon: Some(ac.lon),
            altitude_ft: Some(ac.altitude_ft),
            speed_kts: Some(ac.speed_kts),
            heading_deg: Some(ac.heading_deg),
            vertical_rate_fpm: None,
            cpr_even_lat: None,
            cpr_even_lon: None,
            cpr_even_time: 0.0,
            cpr_odd_lat: None,
            cpr_odd_lon: None,
            cpr_odd_time: 0.0,
            country: icao::lookup_country(&ac.icao),
            registration: None,
            is_military: ac.is_military,
            first_seen: now,
            last_seen: now,
            message_count: 0,
            heading_history: Vec::new(),
            position_history: Vec::new(),
        });
        state.lat = Some(ac.lat);
        state.lon = Some(ac.lon);
        state.altitude_ft = Some(ac.altitude_ft);
        state.speed_kts = Some(ac.speed_kts);
        state.heading_deg = Some(ac.heading_deg);
        state.last_seen = now;
        state.message_count += 1;
    }
}

/// Advance a lat/lon by heading and speed over dt seconds.
fn advance_position(
    lat: f64,
    lon: f64,
    heading_deg: f64,
    speed_kts: f64,
    dt_secs: f64,
) -> (f64, f64) {
    let nm_per_sec = speed_kts / 3600.0;
    let distance_nm = nm_per_sec * dt_secs;
    let heading_rad = heading_deg.to_radians();

    // 1 degree of latitude ≈ 60 nm
    let dlat = distance_nm * heading_rad.cos() / 60.0;
    // 1 degree of longitude ≈ 60 * cos(lat) nm
    let dlon = distance_nm * heading_rad.sin() / (60.0 * lat.to_radians().cos());

    (lat + dlat, lon + dlon)
}

fn now_ts() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs_f64()
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_advance_position_northbound() {
        let (lat, lon) = advance_position(35.0, -82.0, 0.0, 360.0, 3600.0);
        // 360 kts * 1 hour = 360 nm north = 6 degrees latitude
        assert!((lat - 41.0).abs() < 0.1);
        assert!((lon - (-82.0)).abs() < 0.01);
    }

    #[test]
    fn test_advance_position_eastbound() {
        let (lat, lon) = advance_position(35.0, -82.0, 90.0, 360.0, 3600.0);
        // Should move east, latitude roughly unchanged
        assert!((lat - 35.0).abs() < 0.1);
        assert!(lon > -82.0); // moved east
    }

    #[test]
    fn test_advance_position_zero_speed() {
        let (lat, lon) = advance_position(35.0, -82.0, 45.0, 0.0, 100.0);
        assert!((lat - 35.0).abs() < 1e-10);
        assert!((lon - (-82.0)).abs() < 1e-10);
    }

    #[test]
    fn test_advance_position_negative_dt() {
        // Negative dt should go backwards (used for trail seeding)
        let (lat, _lon) = advance_position(35.0, -82.0, 0.0, 360.0, -3600.0);
        assert!(lat < 35.0); // moved south (reverse)
    }

    #[test]
    fn test_demo_flights_valid_icao() {
        for flight in DEMO_FLIGHTS {
            let icao = icao_from_hex(flight.icao_hex);
            assert!(icao.is_some(), "Invalid ICAO hex: {}", flight.icao_hex);
        }
    }

    #[test]
    fn test_demo_flights_reasonable_values() {
        for flight in DEMO_FLIGHTS {
            assert!(flight.altitude_ft >= 1000 && flight.altitude_ft <= 45000);
            assert!(flight.speed_kts >= 50.0 && flight.speed_kts <= 600.0);
            assert!(flight.heading_deg >= 0.0 && flight.heading_deg < 360.0);
            assert!(flight.lat >= 30.0 && flight.lat <= 40.0);
            assert!(flight.lon >= -90.0 && flight.lon <= -75.0);
        }
    }

    #[test]
    fn test_update_tracker() {
        let tracker = Arc::new(RwLock::new(Tracker::new(None, None, None, None, 2.0)));
        let now = 1000.0;

        let sim = vec![SimAircraft {
            icao: icao_from_hex("A12345").unwrap(),
            callsign: "TEST01".to_string(),
            lat: 35.0,
            lon: -82.0,
            altitude_ft: 30000,
            speed_kts: 400.0,
            heading_deg: 90.0,
            is_military: false,
            squawk: "1200".to_string(),
            heading_drift: 0.0,
            alt_drift: 0,
            drift_counter: 0,
        }];

        update_tracker(&tracker, &sim, now);

        let t = tracker.read().unwrap();
        assert_eq!(t.aircraft.len(), 1);
        let ac = t.aircraft.values().next().unwrap();
        assert_eq!(ac.callsign.as_deref(), Some("TEST01"));
        assert_eq!(ac.lat, Some(35.0));
        assert_eq!(ac.altitude_ft, Some(30000));
        assert_eq!(ac.last_seen, now);
    }

    #[test]
    fn test_demo_vessels_valid_mmsi() {
        for vessel in DEMO_VESSELS {
            assert!(
                vessel.mmsi.len() == 9,
                "MMSI should be 9 digits: {}",
                vessel.mmsi
            );
            assert!(
                vessel.mmsi.chars().all(|c| c.is_ascii_digit()),
                "MMSI should be all digits: {}",
                vessel.mmsi
            );
        }
    }

    #[test]
    fn test_demo_vessels_reasonable_values() {
        for vessel in DEMO_VESSELS {
            assert!(
                vessel.speed_kts >= 0.0 && vessel.speed_kts <= 30.0,
                "Vessel speed out of range: {} ({} kts)",
                vessel.name,
                vessel.speed_kts
            );
            assert!(
                vessel.course_deg >= 0.0 && vessel.course_deg < 360.0,
                "Vessel course out of range: {} ({} deg)",
                vessel.name,
                vessel.course_deg
            );
            assert!(
                vessel.lat >= 25.0 && vessel.lat <= 40.0,
                "Vessel lat out of range: {} ({})",
                vessel.name,
                vessel.lat
            );
            assert!(
                vessel.lon >= -90.0 && vessel.lon <= -70.0,
                "Vessel lon out of range: {} ({})",
                vessel.name,
                vessel.lon
            );
            assert!(!vessel.name.is_empty(), "Vessel name should not be empty");
            assert!(
                !vessel.vessel_type.is_empty(),
                "Vessel type should not be empty"
            );
        }
    }

    #[test]
    fn test_demo_vessels_unique_mmsi() {
        let mut seen = std::collections::HashSet::new();
        for vessel in DEMO_VESSELS {
            assert!(seen.insert(vessel.mmsi), "Duplicate MMSI: {}", vessel.mmsi);
        }
    }
}
